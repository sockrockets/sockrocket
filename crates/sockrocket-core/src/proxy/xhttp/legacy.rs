//! XHTTP stream-one over HTTP/2, following Xray's unframed streaming POST.
//! Each tunnel owns its driver; dropping the tunnel cancels all network work.
use crate::config::model::XhttpConfig;
use crate::proxy::connector::BoxProxyStream;
use bytes::{Buf, Bytes};
use std::{
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

fn failure() -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionAborted,
        "XHTTP HTTP/2 stream failed",
    )
}

struct Driver(tokio::task::JoinHandle<()>);
impl Drop for Driver {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) async fn connect(
    io: BoxProxyStream,
    host: &str,
    secure: bool,
    config: &XhttpConfig,
) -> anyhow::Result<BoxProxyStream> {
    config.validate()?;
    let mut builder = h2::client::Builder::new();
    builder
        .enable_push(false)
        .initial_window_size(65535)
        .initial_connection_window_size(65535)
        .max_header_list_size(16 * 1024)
        .max_send_buffer_size(16 * 1024);
    let (client, connection) =
        tokio::time::timeout(Duration::from_secs(15), builder.handshake::<_, Bytes>(io))
            .await
            .map_err(|_| anyhow::anyhow!("XHTTP HTTP/2 handshake timed out"))?
            .map_err(|_| anyhow::anyhow!("XHTTP HTTP/2 handshake failed"))?;
    let driver = Driver(tokio::spawn(async move {
        let _ = connection.await;
    }));
    let mut client = tokio::time::timeout(Duration::from_secs(15), client.ready())
        .await
        .map_err(|_| anyhow::anyhow!("XHTTP HTTP/2 readiness timed out"))?
        .map_err(|_| anyhow::anyhow!("XHTTP HTTP/2 connection unavailable"))?;
    let path = config.path.as_deref().unwrap_or("/");
    let (path, query) = path.split_once('?').map_or((path, ""), |(p, q)| (p, q));
    let mut path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    if !path.ends_with('/') {
        path.push('/');
    }
    if !query.is_empty() {
        path.push('?');
        path.push_str(query);
    }
    let authority = config
        .host
        .as_deref()
        .filter(|h| !h.is_empty())
        .unwrap_or(host);
    let uri = http::Uri::builder()
        .scheme(if secure { "https" } else { "http" })
        .authority(authority)
        .path_and_query(path)
        .build()
        .map_err(|_| anyhow::anyhow!("Invalid XHTTP request target"))?;
    // Legacy Xray padding: a Referer query named x_padding, 100..1000 bytes.
    let padding = "X".repeat(100 + (rand::random::<u16>() as usize % 901));
    let referer = format!(
        "{}{}x_padding={padding}",
        uri,
        if uri.query().is_some() { "&" } else { "?" }
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(uri)
        .version(http::Version::HTTP_2)
        .header("content-type", "application/grpc")
        .header("referer", referer)
        .body(())
        .map_err(|_| anyhow::anyhow!("Invalid XHTTP request"))?;
    let (response, send) = client
        .send_request(request, false)
        .map_err(|_| anyhow::anyhow!("XHTTP request failed"))?;
    // Do not wait for response headers: servers may wait for the VLESS request
    // body before replying. Reads obtain the response once application writes.
    Ok(Box::new(XhttpStream {
        send,
        response: Some(response),
        recv: None,
        pending: Bytes::new(),
        driver,
        write_closed: false,
        read_closed: false,
        terminal: false,
        response_deadline: None,
    }))
}
struct XhttpStream {
    send: h2::SendStream<Bytes>,
    response: Option<h2::client::ResponseFuture>,
    recv: Option<h2::RecvStream>,
    pending: Bytes,
    driver: Driver,
    write_closed: bool,
    read_closed: bool,
    terminal: bool,
    response_deadline: Option<Pin<Box<tokio::time::Sleep>>>,
}
impl XhttpStream {
    fn fail(&mut self) -> io::Error {
        self.terminal = true;
        self.send.send_reset(h2::Reason::CANCEL);
        self.driver.0.abort();
        failure()
    }
}
impl AsyncRead for XhttpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        dst: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.terminal {
            return Poll::Ready(Err(failure()));
        }
        if dst.remaining() == 0 || this.read_closed {
            return Poll::Ready(Ok(()));
        }
        if let Some(response) = this.response.as_mut() {
            let deadline = this
                .response_deadline
                .get_or_insert_with(|| Box::pin(tokio::time::sleep(Duration::from_secs(15))));
            if deadline.as_mut().poll(cx).is_ready() {
                return Poll::Ready(Err(this.fail()));
            }
            let response = match Pin::new(response).poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(r)) => r,
                Poll::Ready(Err(_)) => return Poll::Ready(Err(this.fail())),
            };
            if response.status() != http::StatusCode::OK {
                return Poll::Ready(Err(this.fail()));
            }
            this.response = None;
            this.response_deadline = None;
            this.recv = Some(response.into_body());
        }
        for _ in 0..32 {
            if !this.pending.is_empty() {
                let n = dst.remaining().min(this.pending.len());
                dst.put_slice(&this.pending[..n]);
                this.pending.advance(n);
                if this
                    .recv
                    .as_mut()
                    .unwrap()
                    .flow_control()
                    .release_capacity(n)
                    .is_err()
                {
                    return Poll::Ready(Err(this.fail()));
                }
                return Poll::Ready(Ok(()));
            }
            match this.recv.as_mut().unwrap().poll_data(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(data))) => this.pending = data,
                Poll::Ready(Some(Err(_))) => return Poll::Ready(Err(this.fail())),
                Poll::Ready(None) => {
                    this.read_closed = true;
                    return Poll::Ready(Ok(()));
                }
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}
impl AsyncWrite for XhttpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if this.terminal {
            return Poll::Ready(Err(failure()));
        }
        if this.write_closed {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let wanted = data.len().min(16 * 1024);
        this.send.reserve_capacity(wanted);
        let n = if this.send.capacity() > 0 {
            wanted.min(this.send.capacity())
        } else {
            match this.send.poll_capacity(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(0))) => {
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                Poll::Ready(Some(Ok(n))) => wanted.min(n),
                _ => return Poll::Ready(Err(this.fail())),
            }
        };
        if this
            .send
            .send_data(Bytes::copy_from_slice(&data[..n]), false)
            .is_err()
        {
            return Poll::Ready(Err(this.fail()));
        }
        Poll::Ready(Ok(n))
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(if self.terminal {
            Err(failure())
        } else {
            Ok(())
        })
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.terminal {
            return Poll::Ready(Err(failure()));
        }
        if !this.write_closed {
            if this.send.send_data(Bytes::new(), true).is_err() {
                return Poll::Ready(Err(this.fail()));
            }
            this.write_closed = true;
        }
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn send_all(send: &mut h2::SendStream<Bytes>, mut bytes: Bytes) {
        while !bytes.is_empty() {
            send.reserve_capacity(bytes.len().min(16384));
            let n = std::future::poll_fn(|cx| send.poll_capacity(cx))
                .await
                .unwrap()
                .unwrap();
            if n == 0 {
                continue;
            }
            let n = n.min(bytes.len());
            send.send_data(bytes.split_to(n), false).unwrap();
        }
    }
    #[tokio::test]
    async fn stream_one_is_duplex_backpressured_and_half_close_preserves_reply() {
        let (client, server) = tokio::io::duplex(4096);
        let (done_tx, done) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let mut conn = h2::server::handshake(server).await.unwrap();
            let (request, mut respond) = conn.accept().await.unwrap().unwrap();
            assert_eq!(request.method(), "POST");
            assert_eq!(request.uri().path(), "/tunnel/");
            assert_eq!(request.uri().query(), Some("test=1"));
            assert_eq!(request.uri().host(), Some("example.invalid"));
            assert_eq!(request.headers()["content-type"], "application/grpc");
            let referer = request.headers()["referer"].to_str().unwrap();
            assert!(referer.contains("&x_padding="));
            let driver = tokio::spawn(async move { while conn.accept().await.is_some() {} });
            let mut recv = request.into_body();
            // Reply only once the body starts: connect must not await headers.
            let first = recv.data().await.unwrap().unwrap();
            recv.flow_control().release_capacity(first.len()).unwrap();
            let mut send = respond
                .send_response(http::Response::new(()), false)
                .unwrap();
            send_all(&mut send, first).await;
            while let Some(chunk) = recv.data().await {
                let chunk = chunk.unwrap();
                recv.flow_control().release_capacity(chunk.len()).unwrap();
                send_all(&mut send, chunk).await;
            }
            send_all(&mut send, Bytes::from_static(b"after-eof")).await;
            send.send_data(Bytes::new(), true).unwrap();
            let _ = done_tx.send(());
            driver.await.unwrap();
        });
        let conf = XhttpConfig {
            options: Default::default(),
            path: Some("tunnel?test=1".into()),
            host: Some("example.invalid".into()),
            mode: "stream-one".into(),
        };
        let stream = tokio::time::timeout(
            Duration::from_secs(2),
            connect(Box::new(client), "fallback.invalid", false, &conf),
        )
        .await
        .unwrap()
        .unwrap();
        let (mut read, mut write) = tokio::io::split(stream);
        let payload = vec![42; 1024 * 1024];
        let expected = payload.clone();
        let exchange = async {
            let upload = async {
                write.write_all(&payload).await.unwrap();
                write.shutdown().await.unwrap();
            };
            let download = async {
                let mut result = Vec::new();
                read.read_to_end(&mut result).await.unwrap();
                result
            };
            let (_, result) = tokio::join!(upload, download);
            assert_eq!(&result[..expected.len()], expected);
            assert_eq!(&result[expected.len()..], b"after-eof");
        };
        tokio::time::timeout(Duration::from_secs(10), exchange)
            .await
            .unwrap();
        done.await.unwrap();
        drop(read);
        drop(write);
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
    }
    #[tokio::test]
    async fn dropping_idle_tunnel_stops_driver_and_closes_socket() {
        let (client, server) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            let mut conn = h2::server::handshake(server).await.unwrap();
            while let Some(result) = conn.accept().await {
                if result.is_err() {
                    break;
                }
            }
        });
        let stream = connect(
            Box::new(client),
            "example.invalid",
            false,
            &XhttpConfig::default(),
        )
        .await
        .unwrap();
        drop(stream);
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    }
    #[tokio::test]
    async fn rejected_request_is_an_error_without_forwarding_error_body() {
        let (client, server) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            let mut conn = h2::server::handshake(server).await.unwrap();
            let (_request, mut response) = conn.accept().await.unwrap().unwrap();
            response
                .send_response(
                    http::Response::builder().status(403).body(()).unwrap(),
                    true,
                )
                .unwrap();
            while conn.accept().await.is_some() {}
        });
        let mut stream = connect(
            Box::new(client),
            "example.invalid",
            false,
            &XhttpConfig::default(),
        )
        .await
        .unwrap();
        let mut b = [0; 16];
        let error = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut b))
            .await
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
        assert!(stream.write_all(b"no retry").await.is_err());
        drop(stream);
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn missing_response_headers_time_out_and_cancel_tunnel() {
        let (client, server) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            let mut conn = h2::server::handshake(server).await.unwrap();
            let (_request, _response) = conn.accept().await.unwrap().unwrap();
            while conn.accept().await.is_some() {}
        });
        let mut stream = connect(
            Box::new(client),
            "example.invalid",
            false,
            &XhttpConfig::default(),
        )
        .await
        .unwrap();
        let mut buffer = [0; 1];
        let result = stream.read(&mut buffer).await;
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
        assert!(stream.write_all(b"closed").await.is_err());
        drop(stream);
        server.await.unwrap();
    }
    #[tokio::test]
    async fn blocked_peer_does_not_allow_unbounded_upload_buffering() {
        let (client, server) = tokio::io::duplex(4096);
        let server = tokio::spawn(async move {
            let mut conn = h2::server::handshake(server).await.unwrap();
            let (request, mut respond) = conn.accept().await.unwrap().unwrap();
            let _body = request.into_body();
            let _response = respond
                .send_response(http::Response::new(()), false)
                .unwrap();
            while conn.accept().await.is_some() {}
        });
        let mut stream = connect(
            Box::new(client),
            "example.invalid",
            false,
            &XhttpConfig::default(),
        )
        .await
        .unwrap();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(100),
                stream.write_all(&vec![0; 1024 * 1024])
            )
            .await
            .is_err()
        );
        drop(stream);
        server.abort();
    }
}
