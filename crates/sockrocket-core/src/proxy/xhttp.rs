//! Native XHTTP client, pinned to the Xray v26.10.31 wire format.
mod http;
#[cfg(test)]
mod legacy;
#[cfg(test)]
mod reference_tests;
mod request;
#[cfg(test)]
mod tests;
use self::{
    http::{Body, Client, Task, Version},
    settings::Settings,
};
#[cfg(test)]
use crate::config::model::XhttpConfig;
use crate::config::model::{TlsConfig, TransportConfig};
use crate::config::xhttp as settings;
use crate::proxy::{connector::BoxProxyStream, transport::TlsConnFactory};
use anyhow::{Result, ensure};
use std::{
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};

struct Slot {
    client: Arc<Client>,
    active: AtomicUsize,
    uses: AtomicUsize,
    requests: AtomicUsize,
    created: Instant,
    max_uses: usize,
    max_requests: usize,
    max_secs: u64,
    concurrency: usize,
    last_idle: Mutex<Instant>,
}
struct Lease {
    slot: Arc<Slot>,
    notify: Arc<tokio::sync::Notify>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if self.slot.active.fetch_sub(1, Ordering::AcqRel) == 1 {
            *self.slot.last_idle.lock().unwrap() = Instant::now();
        }
        self.notify.notify_one();
    }
}
struct Pool {
    slots: tokio::sync::Mutex<Vec<Arc<Slot>>>,
    notify: Arc<tokio::sync::Notify>,
    target: usize,
}
pub struct Factory {
    raw: Arc<TlsConnFactory>,
    server: String,
    port: u16,
    tls: Option<TlsConfig>,
    host: String,
    secure: bool,
    reality: bool,
    version: Version,
    settings: Settings,
    pool: Pool,
    download: Option<Arc<Factory>>,
    cleanup: Mutex<Option<Task>>,
}
struct Exchange {
    response: http::Response,
    _lease: Arc<Lease>,
}
impl Factory {
    pub fn new(
        raw: Arc<TlsConnFactory>,
        server: &str,
        port: u16,
        transport: &TransportConfig,
    ) -> Result<Arc<Self>> {
        Self::build(raw, server, port, transport, false)
    }
    fn build(
        raw: Arc<TlsConnFactory>,
        server: &str,
        port: u16,
        transport: &TransportConfig,
        download_only: bool,
    ) -> Result<Arc<Self>> {
        let config = transport.xhttp.clone().unwrap_or_default();
        let settings = Settings::parse(&config)?;
        ensure!(transport.ws.is_none(), "XHTTP cannot also use WebSocket");
        let reality = transport.reality.is_some();
        let secure = transport.tls.is_some() || reality;
        let tls = transport.tls.clone();
        let version = if reality {
            Version::H2
        } else if let Some(tls) = &tls {
            match tls.alpn.as_deref() {
                Some([one]) if one == "h3" => Version::H3,
                Some([one]) if one == "http/1.1" => Version::H1,
                _ => Version::H2,
            }
        } else {
            Version::H1
        };
        ensure!(
            download_only || settings.mode(reality) != "stream-one" || version != Version::H1,
            "XHTTP stream-one requires HTTP/2 or HTTP/3"
        );
        ensure!(
            settings.mode(reality) != "stream-one" || settings.options.download_settings.is_none(),
            "XHTTP stream-one cannot split download transport"
        );
        let host = tls
            .as_ref()
            .and_then(|t| t.sni.clone())
            .or_else(|| transport.reality.as_ref().and_then(|r| r.sni.clone()))
            .unwrap_or_else(|| server.to_string());
        let download = if let Some(d) = &settings.options.download_settings {
            let map = d
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("Invalid XHTTP download settings"))?;
            ensure!(
                map.get("network")
                    .and_then(|v| v.as_str())
                    .unwrap_or("xhttp")
                    == "xhttp",
                "XHTTP download transport must be xhttp"
            );
            let address = map
                .get("address")
                .and_then(|v| v.as_str())
                .unwrap_or(server);
            let port = map
                .get("port")
                .and_then(|v| v.as_u64())
                .map(u16::try_from)
                .transpose()?
                .unwrap_or(port);
            let xhttp = map
                .get("xhttpSettings")
                .or_else(|| map.get("splithttpSettings"));
            if let Some(x) = xhttp {
                ensure!(
                    !contains_download(x),
                    "Nested XHTTP download settings are not allowed"
                );
            }
            let mut stream = d.clone();
            stream
                .as_object_mut()
                .unwrap()
                .entry("network")
                .or_insert("xhttp".into());
            let json = serde_json::json!({"outbounds":[{"protocol":"vless","settings":{"vnext":[{"address":address,"port":port,"users":[{"id":"00000000-0000-0000-0000-000000000001"}]}]},"streamSettings":stream}]});
            let nodes = crate::config::v2ray::parse_v2ray_config(&json.to_string())?;
            let node = nodes
                .first()
                .ok_or_else(|| anyhow::anyhow!("Invalid XHTTP download transport"))?;
            let transport = node
                .transport
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Missing XHTTP download transport"))?;
            let raw = Arc::new(
                TlsConnFactory::from_transport(address, port, Some(transport), false)
                    .without_xhttp(),
            );
            Some(Self::build(raw, address, port, transport, true)?)
        } else {
            None
        };
        let target = settings
            .options
            .xmux
            .max_connections
            .map(|r| r.sample() as usize)
            .unwrap_or(if settings.options.xmux.max_concurrency.is_some() {
                32
            } else {
                3
            });
        let factory = Arc::new(Self {
            raw,
            server: server.into(),
            port,
            tls,
            host,
            secure,
            reality,
            version,
            settings,
            pool: Pool {
                slots: tokio::sync::Mutex::new(Vec::new()),
                notify: Arc::new(tokio::sync::Notify::new()),
                target,
            },
            download,
            cleanup: Mutex::new(None),
        });
        factory.ensure_cleanup();
        Ok(factory)
    }
    fn ensure_cleanup(self: &Arc<Self>) {
        let mut slot = self.cleanup.lock().unwrap();
        if slot.is_some() {
            return;
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let weak = Arc::downgrade(self);
            let task = Task(runtime.spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    let Some(factory) = weak.upgrade() else { break };
                    let mut slots = factory.pool.slots.lock().await;
                    slots.retain(|slot| {
                        slot.active.load(Ordering::Acquire) > 0
                            || (slot.client.live()
                                && slot.created.elapsed().as_secs() < slot.max_secs
                                && slot.last_idle.lock().unwrap().elapsed()
                                    < Duration::from_secs(90))
                    });
                    factory.pool.notify.notify_waiters();
                }
            }));
            *slot = Some(task);
        }
    }
    async fn acquire(&self) -> Result<Lease> {
        loop {
            let notified = self.pool.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let mut slots = self.pool.slots.lock().await;
            slots.retain(|s| {
                s.client.live()
                    && s.uses.load(Ordering::Relaxed) < s.max_uses
                    && s.requests.load(Ordering::Relaxed) < s.max_requests
                    && s.created.elapsed().as_secs() < s.max_secs
            });
            let candidate = slots
                .iter()
                .filter(|s| {
                    s.client.live()
                        && s.uses.load(Ordering::Relaxed) < s.max_uses
                        && s.requests.load(Ordering::Relaxed) < s.max_requests
                        && s.created.elapsed().as_secs() < s.max_secs
                        && s.active.load(Ordering::Acquire) < s.concurrency
                })
                .min_by_key(|s| s.active.load(Ordering::Relaxed))
                .cloned();
            let spread = slots.len() < self.pool.target
                && self.settings.options.xmux.max_concurrency.is_none();
            let chosen = if candidate.is_some() && !spread {
                candidate
            } else if slots.len() < self.pool.target {
                let result = tokio::time::timeout(
                    Duration::from_secs(60),
                    Client::new(
                        self.raw.clone(),
                        self.version,
                        &self.server,
                        self.port,
                        self.tls.as_ref(),
                        self.settings.options.xmux.h_keep_alive_period.unwrap_or(0),
                    ),
                )
                .await
                .map_err(|_| anyhow::anyhow!("XHTTP connection setup timed out"))
                .and_then(|result| result);
                let client = match result {
                    Ok(client) => client,
                    Err(error) => {
                        if let Some(slot) = candidate {
                            slot.active.fetch_add(1, Ordering::AcqRel);
                            slot.uses.fetch_add(1, Ordering::Relaxed);
                            return Ok(Lease {
                                slot,
                                notify: self.pool.notify.clone(),
                            });
                        }
                        return Err(error);
                    }
                };
                let x = &self.settings.options.xmux;
                let s = Arc::new(Slot {
                    client,
                    active: AtomicUsize::new(0),
                    uses: AtomicUsize::new(0),
                    requests: AtomicUsize::new(0),
                    created: Instant::now(),
                    max_uses: x
                        .c_max_reuse_times
                        .filter(|r| r.to > 0)
                        .map(|r| r.sample() as usize)
                        .unwrap_or(usize::MAX),
                    max_requests: x
                        .h_max_request_times
                        .filter(|r| r.to > 0)
                        .map(|r| r.sample() as usize)
                        .unwrap_or(usize::MAX),
                    max_secs: x
                        .h_max_reusable_secs
                        .filter(|r| r.to > 0)
                        .map(|r| r.sample() as u64)
                        .unwrap_or(u64::MAX),
                    last_idle: Mutex::new(Instant::now()),
                    concurrency: x
                        .max_concurrency
                        .map(|r| r.sample() as usize)
                        .unwrap_or(256),
                });
                slots.push(s.clone());
                Some(s)
            } else {
                candidate
            };
            if let Some(slot) = chosen {
                slot.active.fetch_add(1, Ordering::AcqRel);
                slot.uses.fetch_add(1, Ordering::Relaxed);
                return Ok(Lease {
                    slot,
                    notify: self.pool.notify.clone(),
                });
            }
            drop(slots);
            notified.await;
        }
    }
    async fn request(
        &self,
        request: ::http::Request<()>,
        body: Body,
        preferred: Arc<Lease>,
    ) -> Result<Exchange> {
        let lease = self.request_lease(preferred).await?;
        lease.slot.requests.fetch_add(1, Ordering::Relaxed);
        let response = tokio::time::timeout(
            http::TIMEOUT,
            http::success(&lease.slot.client, request, body),
        )
        .await
        .map_err(|_| anyhow::anyhow!("XHTTP request timed out"))??;
        Ok(Exchange {
            response,
            _lease: lease,
        })
    }
    async fn request_lease(&self, preferred: Arc<Lease>) -> Result<Arc<Lease>> {
        let s = &preferred.slot;
        if s.client.live()
            && s.requests.load(Ordering::Relaxed) < s.max_requests
            && s.created.elapsed().as_secs() < s.max_secs
        {
            Ok(preferred)
        } else {
            Ok(Arc::new(self.acquire().await?))
        }
    }
    pub async fn open(self: &Arc<Self>) -> Result<BoxProxyStream> {
        self.ensure_cleanup();
        // Verify connectivity before returning a stream; failed node probes must
        // not mistake a locally allocated duplex pipe for a live server.
        let lease = Arc::new(self.acquire().await?);
        let (user, worker) = tokio::io::duplex(16384);
        let error = Arc::new(http::ErrorState::new());
        let failed = error.clone();
        let factory = self.clone();
        let task = Task(tokio::spawn(async move {
            let result = factory
                .run(
                    Worker {
                        io: worker,
                        error: failed.clone(),
                        success: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    },
                    lease,
                )
                .await;
            if result.is_err() {
                failed.fail();
            }
        }));
        Ok(Box::new(Session {
            io: user,
            error,
            _task: task,
        }))
    }
    async fn run(self: Arc<Self>, io: Worker, lease: Arc<Lease>) -> Result<()> {
        let mode = self.settings.mode(self.reality).to_string();
        let session = if mode == "stream-one" {
            String::new()
        } else {
            self.settings.session()
        };
        let success = io.success.clone();
        let (reader, mut writer) = tokio::io::split(io);
        if mode == "stream-one" {
            let (request, _) =
                self.settings
                    .request(&self.host, self.secure, "", None, None, false)?;
            let mut exchange = self
                .request(request, Body::Read(Box::new(reader)), lease.clone())
                .await?;
            tokio::io::copy(&mut exchange.response.body, &mut writer).await?;
            success.store(true, Ordering::Release);
            writer.shutdown().await?;
            return Ok(());
        }
        let download = self.download.clone().unwrap_or(self.clone());
        let (request, _) = download.settings.request(
            &download.host,
            download.secure,
            &session,
            None,
            None,
            true,
        )?;
        let download_lease = if self.download.is_some() {
            Arc::new(download.acquire().await?)
        } else {
            lease.clone()
        };
        let download_job = async {
            let mut exchange = download
                .request(request, Body::Empty, download_lease)
                .await?;
            tokio::io::copy(&mut exchange.response.body, &mut writer).await?;
            success.store(true, Ordering::Release);
            writer.shutdown().await?;
            Ok::<_, anyhow::Error>(())
        };
        let upload_job = async {
            if mode == "stream-up" {
                let (request, _) =
                    self.settings
                        .request(&self.host, self.secure, &session, None, None, false)?;
                let mut exchange = self
                    .request(request, Body::Read(Box::new(reader)), lease.clone())
                    .await?;
                tokio::io::copy(&mut exchange.response.body, &mut tokio::io::sink()).await?;
            } else {
                self.packet_upload(reader, &session, lease.clone()).await?;
            }
            Ok::<_, anyhow::Error>(())
        };
        tokio::pin!(download_job, upload_job);
        tokio::select! {result=&mut download_job=>result,result=&mut upload_job=>{result?;download_job.await}}
    }
    async fn packet_upload<R: AsyncRead + Unpin>(
        &self,
        mut reader: R,
        session: &str,
        mut preferred: Arc<Lease>,
    ) -> Result<()> {
        let o = &self.settings.options;
        let max = o
            .sc_max_each_post_bytes
            .unwrap_or(settings::Range::fixed(1_000_000));
        let mut sequence = 0u64;
        let mut pending = tokio::task::JoinSet::new();
        let limit = o.sc_max_buffered_posts.unwrap_or(30).clamp(1, 32) as usize;
        let mut last = None;
        loop {
            while pending.len() >= limit {
                if let Some(result) = pending.join_next().await {
                    result??;
                }
            }
            let size = max.sample() as usize;
            static BUDGET: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> =
                std::sync::OnceLock::new();
            let (data, credit) = {
                let receive = async {
                    let credit = BUDGET
                        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(8 * 1024 * 1024)))
                        .clone()
                        .acquire_many_owned(size as u32)
                        .await?;
                    let mut data = vec![0; size];
                    let n = reader.read(&mut data).await?;
                    data.truncate(n);
                    Ok::<_, anyhow::Error>((data, credit))
                };
                tokio::pin!(receive);
                loop {
                    tokio::select! {
                        result=&mut receive=>break result?,
                        Some(result)=pending.join_next(),if !pending.is_empty()=>{result??;}
                    }
                }
            };
            if data.is_empty() {
                break;
            }
            if let Some(sent) = last {
                let interval = o
                    .sc_min_posts_interval_ms
                    .unwrap_or(settings::Range::fixed(30))
                    .sample() as u64;
                let delay = tokio::time::sleep_until(sent + Duration::from_millis(interval));
                tokio::pin!(delay);
                loop {
                    tokio::select! {_= &mut delay=>break,Some(result)=pending.join_next(),if !pending.is_empty()=>{result??;}}
                }
            }
            last = Some(tokio::time::Instant::now());
            let (request, body) = self.settings.request(
                &self.host,
                self.secure,
                session,
                Some(sequence),
                Some(bytes::Bytes::from(data)),
                false,
            )?;
            sequence = sequence
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("XHTTP sequence exhausted"))?;
            // Owned factory state is passed by the session task; request futures
            // stay in this bounded set and are cancelled together on failure.
            let lease = self.request_lease(preferred).await?;
            preferred = lease.clone();
            lease.slot.requests.fetch_add(1, Ordering::Relaxed);
            pending.spawn(async move {
                let mut response = tokio::time::timeout(
                    http::TIMEOUT,
                    http::success(
                        &lease.slot.client,
                        request,
                        body.map(Body::Data).unwrap_or(Body::Empty),
                    ),
                )
                .await
                .map_err(|_| anyhow::anyhow!("XHTTP packet upload timed out"))??;
                tokio::time::timeout(
                    http::TIMEOUT,
                    tokio::io::copy(&mut response.body, &mut tokio::io::sink()),
                )
                .await
                .map_err(|_| anyhow::anyhow!("XHTTP packet response timed out"))??;
                drop(lease);
                drop(credit);
                Ok::<_, anyhow::Error>(())
            });
        }
        while let Some(result) = pending.join_next().await {
            result??;
        }
        Ok(())
    }
}
fn contains_download(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(m) => m
            .iter()
            .any(|(k, v)| k == "downloadSettings" || contains_download(v)),
        serde_json::Value::Array(a) => a.iter().any(contains_download),
        _ => false,
    }
}
struct Worker {
    io: tokio::io::DuplexStream,
    error: Arc<http::ErrorState>,
    success: Arc<std::sync::atomic::AtomicBool>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        if !self.success.load(Ordering::Acquire) {
            self.error.fail();
        }
    }
}
impl AsyncRead for Worker {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        b: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, b)
    }
}
impl AsyncWrite for Worker {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        b: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, b)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}
struct Session {
    io: tokio::io::DuplexStream,
    error: Arc<http::ErrorState>,
    _task: Task,
}
impl AsyncRead for Session {
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
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "XHTTP session failed",
            )))
        } else {
            result
        }
    }
}
impl AsyncWrite for Session {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        b: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.error.failed() {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "XHTTP session failed",
            )));
        }
        Pin::new(&mut self.io).poll_write(cx, b)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}
