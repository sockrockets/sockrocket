//! Flow-controlled HTTP/2 request stream with lazy response headers and half-close.
use super::connector::BoxProxyStream;
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
    io::Error::new(io::ErrorKind::ConnectionAborted, "HTTP/2 stream failed")
}

struct Driver(tokio::task::JoinHandle<()>);
impl Drop for Driver {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) async fn open(
    io: BoxProxyStream,
    request: http::Request<()>,
) -> anyhow::Result<BoxProxyStream> {
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
            .map_err(|_| anyhow::anyhow!("HTTP/2 handshake timed out"))?
            .map_err(|_| anyhow::anyhow!("HTTP/2 handshake failed"))?;
    let driver = Driver(tokio::spawn(async move {
        let _ = connection.await;
    }));
    let mut client = tokio::time::timeout(Duration::from_secs(15), client.ready())
        .await
        .map_err(|_| anyhow::anyhow!("HTTP/2 readiness timed out"))?
        .map_err(|_| anyhow::anyhow!("HTTP/2 connection unavailable"))?;
    let grpc = request.headers().contains_key("te");
    let (response, send) = client
        .send_request(request, false)
        .map_err(|_| anyhow::anyhow!("HTTP/2 request failed"))?;
    // Do not wait for response headers: servers may wait for the VLESS request
    // body before replying. Reads obtain the response once application writes.
    Ok(Box::new(H2Stream {
        grpc,
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
struct H2Stream {
    grpc: bool,
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
impl H2Stream {
    fn fail(&mut self) -> io::Error {
        self.terminal = true;
        self.send.send_reset(h2::Reason::CANCEL);
        self.driver.0.abort();
        failure()
    }
}
impl AsyncRead for H2Stream {
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
            if response.status() != http::StatusCode::OK
                || (this.grpc
                    && (response
                        .headers()
                        .get("content-type")
                        .and_then(|v| v.to_str().ok())
                        .is_none_or(|s| !s.starts_with("application/grpc"))
                        || response
                            .headers()
                            .get("grpc-status")
                            .is_some_and(|s| s != "0")))
            {
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
                    if this.grpc {
                        match this.recv.as_mut().unwrap().poll_trailers(cx) {
                            Poll::Pending => return Poll::Pending,
                            Poll::Ready(Err(_)) => return Poll::Ready(Err(this.fail())),
                            Poll::Ready(Ok(Some(t)))
                                if t.get("grpc-status").is_some_and(|s| s != "0") =>
                            {
                                return Poll::Ready(Err(this.fail()));
                            }
                            _ => {}
                        }
                    }
                    this.read_closed = true;
                    return Poll::Ready(Ok(()));
                }
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}
impl AsyncWrite for H2Stream {
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
