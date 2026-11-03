//! Bounded HTTP request adapters. A response owns every request-specific task;
//! connection drivers are shared only while a pool lease still owns them.
use crate::config::model::TlsConfig;
use crate::proxy::{connector::BoxProxyStream, pool::ConnFactory, transport::TlsConnFactory};
use anyhow::{Result, ensure};
use bytes::{Buf, Bytes};
use http_body_util::BodyExt;
use std::{
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt, ReadBuf};
pub(super) struct ErrorState {
    failed: std::sync::atomic::AtomicBool,
    waker: Mutex<Option<std::task::Waker>>,
}
impl ErrorState {
    pub fn new() -> Self {
        Self {
            failed: std::sync::atomic::AtomicBool::new(false),
            waker: Mutex::new(None),
        }
    }
    pub fn fail(&self) {
        self.failed
            .store(true, std::sync::atomic::Ordering::Release);
        if let Some(w) = self.waker.lock().unwrap().take() {
            w.wake();
        }
    }
    pub fn failed(&self) -> bool {
        self.failed.load(std::sync::atomic::Ordering::Acquire)
    }
    pub fn register(&self, cx: &Context<'_>) {
        *self.waker.lock().unwrap() = Some(cx.waker().clone());
    }
}
pub type Reader = Box<dyn AsyncRead + Unpin + Send>;
pub const TIMEOUT: Duration = Duration::from_secs(15);
fn failure() -> io::Error {
    io::Error::new(io::ErrorKind::ConnectionAborted, "XHTTP request failed")
}
pub struct Task(pub tokio::task::JoinHandle<()>);
impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Version {
    H1,
    H2,
    H3,
}
pub enum Body {
    Empty,
    Data(Bytes),
    Read(Reader),
}
impl Body {
    async fn next(&mut self) -> io::Result<Option<Bytes>> {
        match self {
            Self::Empty => Ok(None),
            Self::Data(bytes) => {
                let bytes = std::mem::take(bytes);
                if bytes.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(bytes))
                }
            }
            Self::Read(reader) => {
                let mut data = vec![0; 16384];
                let n = reader.read(&mut data).await?;
                data.truncate(n);
                if n == 0 {
                    Ok(None)
                } else {
                    Ok(Some(Bytes::from(data)))
                }
            }
        }
    }
}
struct HttpBody {
    body: Body,
}
impl hyper::body::Body for HttpBody {
    type Data = Bytes;
    type Error = io::Error;
    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<io::Result<hyper::body::Frame<Bytes>>>> {
        let body = &mut self.body;
        match body {
            Body::Empty => Poll::Ready(None),
            Body::Data(bytes) => {
                if bytes.is_empty() {
                    Poll::Ready(None)
                } else {
                    Poll::Ready(Some(Ok(hyper::body::Frame::data(std::mem::take(bytes)))))
                }
            }
            Body::Read(reader) => {
                let mut data = [0; 16384];
                let mut buf = ReadBuf::new(&mut data);
                match Pin::new(reader).poll_read(cx, &mut buf) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(Err(e)) => Poll::Ready(Some(Err(e))),
                    Poll::Ready(Ok(())) => {
                        if buf.filled().is_empty() {
                            Poll::Ready(None)
                        } else {
                            Poll::Ready(Some(Ok(hyper::body::Frame::data(Bytes::copy_from_slice(
                                buf.filled(),
                            )))))
                        }
                    }
                }
            }
        }
    }
    fn is_end_stream(&self) -> bool {
        matches!(&self.body, Body::Empty) || matches!(&self.body,Body::Data(b) if b.is_empty())
    }
    fn size_hint(&self) -> hyper::body::SizeHint {
        let mut h = hyper::body::SizeHint::default();
        match &self.body {
            Body::Empty => h.set_exact(0),
            Body::Data(b) => h.set_exact(b.len() as u64),
            _ => {}
        }
        h
    }
}
struct H3Owner {
    _credit: tokio::sync::OwnedSemaphorePermit,
    _endpoint: quinn::Endpoint,
    connection: quinn::Connection,
    _driver: Task,
}
impl Drop for H3Owner {
    fn drop(&mut self) {
        self.connection
            .close(0u32.into(), b"XHTTP connection released");
    }
}
enum Shared {
    H1 {
        raw: Arc<TlsConnFactory>,
        initial: tokio::sync::Mutex<Option<BoxProxyStream>>,
    },
    H2 {
        sender: h2::client::SendRequest<Bytes>,
        _driver: Task,
    },
    H3 {
        sender: h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>,
        _owner: H3Owner,
    },
}
pub struct Client {
    shared: Shared,
}
pub struct Response {
    pub status: http::StatusCode,
    pub body: Pipe,
}
pub struct Pipe {
    io: tokio::io::DuplexStream,
    error: Arc<ErrorState>,
    _tasks: Vec<Task>,
    _client: Arc<Client>,
}
impl AsyncRead for Pipe {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        b: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if b.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        self.error.register(cx);
        let before = b.filled().len();
        let result = Pin::new(&mut self.io).poll_read(cx, b);
        if b.filled().len() == before && self.error.failed() {
            Poll::Ready(Err(failure()))
        } else {
            result
        }
    }
}
impl Client {
    pub async fn new(
        raw: Arc<TlsConnFactory>,
        version: Version,
        server: &str,
        port: u16,
        tls: Option<&TlsConfig>,
        keepalive: i64,
    ) -> Result<Arc<Self>> {
        let shared = match version {
            Version::H1 => Shared::H1 {
                initial: tokio::sync::Mutex::new(Some(raw.create().await?)),
                raw,
            },
            Version::H2 => {
                let io = raw.create().await?;
                let mut b = h2::client::Builder::new();
                b.enable_push(false)
                    .initial_window_size(65535)
                    .initial_connection_window_size(65535)
                    .max_header_list_size(128 * 1024)
                    .max_send_buffer_size(16384);
                let (sender, mut connection) = tokio::time::timeout(TIMEOUT, b.handshake(io))
                    .await
                    .map_err(|_| failure())?
                    .map_err(|_| failure())?;
                let ping = connection.ping_pong();
                let driver = Task(tokio::spawn(async move {
                    if keepalive < 0 {
                        let _ = connection.await;
                        return;
                    }
                    let interval =
                        Duration::from_secs(if keepalive == 0 { 30 } else { keepalive as u64 });
                    let monitor = async move {
                        if let Some(mut ping) = ping {
                            loop {
                                tokio::time::sleep(interval).await;
                                if !matches!(
                                    tokio::time::timeout(TIMEOUT, ping.ping(h2::Ping::opaque()))
                                        .await,
                                    Ok(Ok(_))
                                ) {
                                    break;
                                }
                            }
                        } else {
                            std::future::pending::<()>().await
                        }
                    };
                    tokio::select! {_=connection=>{},_=monitor=>{}}
                }));
                Shared::H2 {
                    sender,
                    _driver: driver,
                }
            }
            Version::H3 => {
                crate::proxy::quic_conn::ensure_quic_crypto_provider();
                let tls = tls.ok_or_else(|| anyhow::anyhow!("HTTP/3 requires TLS"))?;
                let chrome = tls
                    .fingerprint
                    .as_deref()
                    .is_some_and(crate::proxy::quic_fingerprint::is_chrome);
                let config = if chrome {
                    crate::proxy::quic_fingerprint::client_config(
                        tls.skip_cert_verify,
                        &["h3".into()],
                        None,
                    )?
                } else {
                    crate::proxy::quic_conn::build_quic_client_config(
                        tls.skip_cert_verify,
                        &["h3".into()],
                    )?
                };
                static QUIC_BUDGET: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> =
                    std::sync::OnceLock::new();
                let reservation = if chrome { 17 * 1024 * 1024 } else { 256 * 1024 };
                let credit = QUIC_BUDGET
                    .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(96 * 1024 * 1024)))
                    .clone()
                    .try_acquire_many_owned(reservation)
                    .map_err(|_| anyhow::anyhow!("XHTTP QUIC receive budget exhausted"))?;
                let addresses = crate::proxy::quic_conn::resolve_server_addrs(server, port).await?;
                let mut connected = None;
                for address in addresses.into_iter().take(4) {
                    let bind = if address.is_ipv4() {
                        "0.0.0.0:0".parse()?
                    } else {
                        "[::]:0".parse()?
                    };
                    let mut endpoint = crate::proxy::quic_conn::client_endpoint_with_config(
                        bind,
                        if chrome {
                            crate::proxy::quic_fingerprint::endpoint_config()
                        } else {
                            quinn::EndpointConfig::default()
                        },
                    )?;
                    let mut cfg = config.clone();
                    let mut transport = quinn::TransportConfig::default();
                    if chrome {
                        crate::proxy::quic_fingerprint::transport_config(&mut transport);
                    } else {
                        transport
                            .receive_window(262144u32.into())
                            .stream_receive_window(65535u32.into())
                            .max_idle_timeout(Some(Duration::from_secs(90).try_into()?));
                    }
                    transport.send_window(262144);
                    if keepalive >= 0 {
                        transport.keep_alive_interval(Some(Duration::from_secs(
                            if keepalive == 0 { 30 } else { keepalive as u64 },
                        )));
                    }
                    cfg.transport_config(Arc::new(transport));
                    endpoint.set_default_client_config(cfg);
                    if let Ok(Ok(connection)) = tokio::time::timeout(
                        TIMEOUT,
                        endpoint.connect(address, tls.sni.as_deref().unwrap_or(server))?,
                    )
                    .await
                    {
                        connected = Some((endpoint, connection));
                        break;
                    }
                }
                let (endpoint, connection) =
                    connected.ok_or_else(|| anyhow::anyhow!("XHTTP HTTP/3 connection failed"))?;
                let (mut driver, sender) = h3::client::builder()
                    .max_field_section_size(128 * 1024)
                    .build(h3_quinn::Connection::new(connection.clone()))
                    .await
                    .map_err(|_| failure())?;
                let task = Task(tokio::spawn(async move {
                    let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
                }));
                Shared::H3 {
                    sender,
                    _owner: H3Owner {
                        _credit: credit,
                        _endpoint: endpoint,
                        connection,
                        _driver: task,
                    },
                }
            }
        };
        Ok(Arc::new(Self { shared }))
    }
    pub fn live(&self) -> bool {
        match &self.shared {
            Shared::H1 { .. } => true,
            Shared::H2 { _driver, .. } => !_driver.0.is_finished(),
            Shared::H3 { _owner, .. } => _owner.connection.close_reason().is_none(),
        }
    }
    pub async fn request(
        self: &Arc<Self>,
        mut request: http::Request<()>,
        mut body: Body,
    ) -> Result<Response> {
        let (error, owner) = (Arc::new(ErrorState::new()), self.clone());
        let (reader, mut writer) = tokio::io::duplex(16384);
        let mut tasks = Vec::new();
        let status = match &self.shared {
            Shared::H1 { raw, initial } => {
                let io = match initial.lock().await.take() {
                    Some(io) => io,
                    None => raw.create().await?,
                };
                let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
                    .max_buf_size(128 * 1024)
                    .handshake(hyper_util::rt::TokioIo::new(io))
                    .await
                    .map_err(|_| failure())?;
                tasks.push(Task(tokio::spawn(async move {
                    let _ = connection.await;
                })));
                let uri = request.uri().clone();
                let host = uri.authority().ok_or_else(failure)?.as_str();
                request
                    .headers_mut()
                    .insert("host", host.parse().map_err(|_| failure())?);
                *request.uri_mut() = uri
                    .path_and_query()
                    .map(|p| p.as_str())
                    .unwrap_or("/")
                    .parse()
                    .map_err(|_| failure())?;
                // Match Xray's non-keepalive H1 client. In Go's HTTP/1
                // server this also permits an early streaming response
                // without draining the entire chunked upload first.
                request
                    .headers_mut()
                    .insert("connection", http::HeaderValue::from_static("close"));
                *request.version_mut() = http::Version::HTTP_11;
                let request = request.map(|_| HttpBody { body });
                let mut response = tokio::time::timeout(TIMEOUT, sender.send_request(request))
                    .await
                    .map_err(|_| failure())?
                    .map_err(|_| failure())?;
                let status = response.status();
                let failed = error.clone();
                tasks.push(Task(tokio::spawn(async move {
                    let result = async {
                        while let Some(frame) = response.body_mut().frame().await {
                            let frame = frame.map_err(|_| failure())?;
                            if let Ok(data) = frame.into_data() {
                                writer.write_all(&data).await?;
                            }
                        }
                        Ok::<_, io::Error>(())
                    }
                    .await;
                    if result.is_err() {
                        failed.fail();
                    }
                    let _ = writer.shutdown().await;
                })));
                status
            }
            Shared::H2 { sender, .. } => {
                *request.version_mut() = http::Version::HTTP_2;
                let mut sender = sender.clone().ready().await.map_err(|_| failure())?;
                let empty = matches!(body, Body::Empty);
                let (response, mut send) =
                    sender.send_request(request, empty).map_err(|_| failure())?;
                if !empty {
                    let failed = error.clone();
                    tasks.push(Task(tokio::spawn(async move {
                        let result = async {
                            while let Some(mut data) = body.next().await? {
                                while !data.is_empty() {
                                    send.reserve_capacity(data.len().min(16384));
                                    let n = std::future::poll_fn(|cx| send.poll_capacity(cx))
                                        .await
                                        .ok_or_else(failure)?
                                        .map_err(|_| failure())?;
                                    if n == 0 {
                                        continue;
                                    }
                                    let n = n.min(data.len());
                                    send.send_data(data.split_to(n), false)
                                        .map_err(|_| failure())?;
                                }
                            }
                            send.send_data(Bytes::new(), true).map_err(|_| failure())?;
                            Ok::<_, io::Error>(())
                        }
                        .await;
                        if result.is_err() {
                            failed.fail();
                            send.send_reset(h2::Reason::CANCEL);
                        }
                    })));
                }
                let response = tokio::time::timeout(TIMEOUT, response)
                    .await
                    .map_err(|_| failure())?
                    .map_err(|_| failure())?;
                let status = response.status();
                let mut recv = response.into_body();
                let failed = error.clone();
                tasks.push(Task(tokio::spawn(async move {
                    let result = async {
                        while let Some(data) = recv.data().await {
                            let data = data.map_err(|_| failure())?;
                            writer.write_all(&data).await?;
                            recv.flow_control()
                                .release_capacity(data.len())
                                .map_err(|_| failure())?;
                        }
                        Ok::<_, io::Error>(())
                    }
                    .await;
                    if result.is_err() {
                        failed.fail();
                    }
                    let _ = writer.shutdown().await;
                })));
                status
            }
            Shared::H3 { sender, .. } => {
                *request.version_mut() = http::Version::HTTP_3;
                let mut sender = sender.clone();
                let stream = sender.send_request(request).await.map_err(|_| failure())?;
                let (mut send, mut recv) = stream.split();
                let failed = error.clone();
                tasks.push(Task(tokio::spawn(async move {
                    let result = async {
                        while let Some(data) = body.next().await? {
                            send.send_data(data).await.map_err(|_| failure())?;
                        }
                        send.finish().await.map_err(|_| failure())?;
                        Ok::<_, io::Error>(())
                    }
                    .await;
                    if result.is_err() {
                        failed.fail();
                        send.stop_stream(h3::error::Code::H3_REQUEST_CANCELLED);
                    }
                })));
                let response = tokio::time::timeout(TIMEOUT, recv.recv_response())
                    .await
                    .map_err(|_| failure())?
                    .map_err(|_| failure())?;
                let status = response.status();
                let failed = error.clone();
                tasks.push(Task(tokio::spawn(async move {
                    let result = async {
                        while let Some(mut data) = recv.recv_data().await.map_err(|_| failure())? {
                            while data.has_remaining() {
                                let bytes = data.chunk();
                                writer.write_all(bytes).await?;
                                let n = bytes.len();
                                data.advance(n);
                            }
                        }
                        Ok::<_, io::Error>(())
                    }
                    .await;
                    if result.is_err() {
                        failed.fail();
                    }
                    let _ = writer.shutdown().await;
                })));
                status
            }
        };
        Ok(Response {
            status,
            body: Pipe {
                io: reader,
                error,
                _tasks: tasks,
                _client: owner,
            },
        })
    }
}
pub async fn success(
    client: &Arc<Client>,
    request: http::Request<()>,
    body: Body,
) -> Result<Response> {
    let response = client.request(request, body).await?;
    ensure!(
        response.status == http::StatusCode::OK,
        "XHTTP server rejected request"
    );
    Ok(response)
}
