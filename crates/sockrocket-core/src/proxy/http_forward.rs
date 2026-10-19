//! Streaming HTTP/1 forwarding: every request has its own authority and framing.
use super::{
    ProxyStats,
    connector::{SharedOutbound, connect_upstream},
    relay::relay_bidirectional,
};
use anyhow::{Result, ensure};
use bytes::Bytes;
use http::{HeaderMap, Request, Response, StatusCode};
use http_body_util::{BodyExt, Full, combinators::UnsyncBoxBody};
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use std::{
    convert::Infallible,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
type Body = UnsyncBoxBody<Bytes, hyper::Error>;
type Tasks = Arc<Mutex<tokio::task::JoinSet<()>>>;
fn spawn(tasks: &Tasks, future: impl std::future::Future<Output = ()> + Send + 'static) {
    let mut tasks = tasks.lock().unwrap();
    while tasks.try_join_next().is_some() {}
    tasks.spawn(future);
}
fn empty(status: StatusCode) -> Response<Body> {
    Response::builder()
        .status(status)
        .body(
            Full::new(Bytes::new())
                .map_err(|never: Infallible| match never {})
                .boxed_unsync(),
        )
        .unwrap()
}
fn clean(headers: &mut HeaderMap, upgrade: bool) {
    let nominated: Vec<_> = headers
        .get_all("connection")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|v| v.trim().to_owned())
        .collect();
    for name in nominated {
        if !upgrade || !name.eq_ignore_ascii_case("upgrade") {
            headers.remove(name);
        }
    }
    for name in [
        "proxy-authorization",
        "proxy-authenticate",
        "proxy-connection",
        "connection",
        "keep-alive",
        "te",
        "trailer",
        "transfer-encoding",
    ] {
        headers.remove(name);
    }
    if upgrade {
        headers.insert("connection", http::HeaderValue::from_static("upgrade"));
    } else {
        headers.remove("upgrade");
    }
}
async fn forward(
    mut request: Request<Incoming>,
    outbound: SharedOutbound,
    tasks: Tasks,
) -> Result<Response<Body>> {
    ensure!(
        request.method() != http::Method::CONNECT,
        "CONNECT requires a dedicated proxy connection"
    );
    ensure!(
        request.uri().scheme_str() == Some("http"),
        "Only absolute HTTP URLs are accepted"
    );
    let authority = request
        .uri()
        .authority()
        .ok_or_else(|| anyhow::anyhow!("Missing authority"))?
        .clone();
    ensure!(
        !authority.as_str().contains('@'),
        "User information is not accepted in proxy URLs"
    );
    let host = authority
        .host()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let port = authority.port_u16().unwrap_or(80);
    let wants_upgrade = request
        .headers()
        .get_all("connection")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .any(|v| {
            v.split(',')
                .any(|v| v.trim().eq_ignore_ascii_case("upgrade"))
        })
        && request.headers().contains_key("upgrade");
    let client_upgrade = wants_upgrade.then(|| hyper::upgrade::on(&mut request));
    clean(request.headers_mut(), wants_upgrade);
    request
        .headers_mut()
        .insert(http::header::HOST, authority.as_str().parse()?);
    *request.uri_mut() = request
        .uri()
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/")
        .parse()?;
    let remote = connect_upstream(&outbound, &host, port).await?;
    let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
        .max_buf_size(64 * 1024)
        .handshake::<_, Incoming>(TokioIo::new(remote))
        .await?;
    spawn(&tasks, async move {
        let _ = connection.with_upgrades().await;
    });
    let mut response = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        sender.send_request(request),
    )
    .await
    .map_err(|_| anyhow::anyhow!("Origin response headers timed out"))??;
    let upgraded = response.status() == StatusCode::SWITCHING_PROTOCOLS;
    ensure!(!upgraded || wants_upgrade, "Unsolicited HTTP upgrade");
    if upgraded {
        let upstream = hyper::upgrade::on(&mut response);
        let client = client_upgrade.unwrap();
        spawn(&tasks, async move {
            if let Ok(Ok((client, upstream))) =
                tokio::time::timeout(std::time::Duration::from_secs(10), async {
                    Ok::<_, hyper::Error>((client.await?, upstream.await?))
                })
                .await
            {
                let _ = relay_bidirectional(&mut TokioIo::new(client), &mut TokioIo::new(upstream))
                    .await;
            }
        });
    }
    clean(response.headers_mut(), upgraded);
    Ok(response.map(|body| body.boxed_unsync()))
}
struct Prefixed {
    stream: tokio::net::TcpStream,
    prefix: std::io::Cursor<Vec<u8>>,
}
impl AsyncRead for Prefixed {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let pos = self.prefix.position() as usize;
        if pos < self.prefix.get_ref().len() {
            let n = buf.remaining().min(self.prefix.get_ref().len() - pos);
            buf.put_slice(&self.prefix.get_ref()[pos..pos + n]);
            self.prefix.set_position((pos + n) as u64);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}
impl AsyncWrite for Prefixed {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
pub(super) async fn serve(
    client: tokio::net::TcpStream,
    header: &[u8],
    remaining: &[u8],
    outbound: SharedOutbound,
    stats: ProxyStats,
) -> Result<()> {
    let mut prefix = header.to_vec();
    prefix.extend_from_slice(remaining);
    let client = Prefixed {
        stream: client,
        prefix: std::io::Cursor::new(prefix),
    };
    let record = stats.track("HTTP requests".into(), "HTTP");
    let tasks = Arc::new(Mutex::new(tokio::task::JoinSet::new()));
    let owned = tasks.clone();
    let service = hyper::service::service_fn(move |request| {
        let outbound = outbound.clone();
        let tasks = owned.clone();
        async move {
            Ok::<_, Infallible>(match forward(request, outbound, tasks).await {
                Ok(response) => response,
                Err(_) => empty(StatusCode::BAD_GATEWAY),
            })
        }
    });
    let result = hyper::server::conn::http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(super::PROXY_HANDSHAKE_TIMEOUT)
        .max_buf_size(64 * 1024)
        .half_close(true)
        .serve_connection(TokioIo::new(record.observe_owned(client)), service)
        .with_upgrades()
        .await;
    let mut tasks = std::mem::take(&mut *tasks.lock().unwrap());
    if result.is_err() {
        tasks.abort_all();
    }
    while tasks.join_next().await.is_some() {}
    result?;
    record.finish(0, 0);
    Ok(())
}
