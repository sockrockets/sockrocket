//! Native AnyTLS v2. One active stream per TLS session; completed sessions are
//! reused newest-first with increasing stream IDs, bounded slots and idle expiry.
//! Reference: https://github.com/anytls/anytls-go/blob/main/docs/protocol.md
mod padding;
use super::{
    connector::{BoxProxyStream, Outbound},
    transport,
};
use crate::config::model::TlsConfig;
use anyhow::{Result, ensure};
use padding::Padding;
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, Mutex, OnceLock},
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf},
    sync::{OwnedSemaphorePermit, Semaphore},
};
const IDLE_LIMIT: usize = 4;
const IDLE_TTL: Duration = Duration::from_secs(30);
const MAX_PAYLOAD: usize = 16 * 1024;
struct Client {
    server: String,
    port: u16,
    tls: TlsConfig,
    connector: craft_tls::TlsConnector,
    password: [u8; 32],
    padding: Arc<Mutex<Padding>>,
    idle: Mutex<Vec<Session>>,
    slots: Arc<Semaphore>,
    keep_idle: bool,
    reaper: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl Drop for Client {
    fn drop(&mut self) {
        if let Some(task) = self.reaper.get_mut().unwrap().take() {
            task.abort();
        }
    }
}
pub struct AnyTlsOutbound {
    client: Arc<Client>,
}
impl AnyTlsOutbound {
    pub fn new(
        server: &str,
        port: u16,
        password: &str,
        tls: Option<&TlsConfig>,
        keep_idle: bool,
    ) -> Result<Self> {
        Self::new_pinned(server, port, password, tls, keep_idle, None)
    }
    pub(crate) fn new_pinned(
        server: &str,
        port: u16,
        password: &str,
        tls: Option<&TlsConfig>,
        keep_idle: bool,
        pin: Option<[u8; 32]>,
    ) -> Result<Self> {
        ensure!(
            !server.is_empty() && port != 0 && !password.is_empty(),
            "AnyTLS requires an endpoint and password"
        );
        let tls = tls.cloned().unwrap_or(TlsConfig {
            sni: None,
            skip_cert_verify: false,
            alpn: None,
            fingerprint: None,
        });
        Ok(Self {
            client: Arc::new(Client {
                server: server.into(),
                port,
                connector: transport::build_tls_connector_pinned(Some(&tls), pin),
                tls,
                password: Sha256::digest(password.as_bytes()).into(),
                padding: Arc::new(Mutex::new(Padding::default())),
                idle: Mutex::new(Vec::new()),
                slots: Arc::new(Semaphore::new(64)),
                keep_idle,
                reaper: Mutex::new(None),
            }),
        })
    }
    async fn open(&self, host: &str, port: u16) -> Result<AnyTlsStream> {
        self.open_initial(host, port, &[]).await
    }
    async fn open_initial(&self, host: &str, port: u16, initial: &[u8]) -> Result<AnyTlsStream> {
        let mut address = socks_address(host, port)?;
        address.extend_from_slice(initial);
        let client = self.client.clone();
        client.ensure_reaper();
        tokio::time::timeout(super::connector::UPSTREAM_CONNECT_TIMEOUT, async {
            // A stale idle TLS session is retried once with a fresh connection.
            for attempt in 0..2 {
                let reused = if attempt == 0 {
                    client.take_idle()
                } else {
                    None
                };
                let mut session = match reused {
                    Some(s) => s,
                    None => client.create_session().await?,
                };
                if session.next_id == u32::MAX {
                    continue;
                }
                session.id = session.next_id;
                session.next_id += 1;
                session.acked = false;
                session.peer_finished = false;
                session.local_finished = false;
                session.content.clear();
                session.content_pos = 0;
                let mut request = Vec::new();
                if session.id == 1 {
                    let settings = format!(
                        "v=2\nclient=sockrocket/{}\npadding-md5={}\n",
                        env!("CARGO_PKG_VERSION"),
                        client.padding.lock().unwrap().md5
                    );
                    frame(&mut request, 4, 0, settings.as_bytes());
                }
                frame(&mut request, 1, session.id, &[]);
                frame(&mut request, 2, session.id, &address);
                session.queue(request);
                let ready = async {
                    std::future::poll_fn(|cx| session.poll_flush_wire(cx)).await?;
                    std::future::poll_fn(|cx| session.poll_ready(cx)).await
                }
                .await;
                match ready {
                    Ok(()) => {
                        return Ok(AnyTlsStream {
                            session: Some(session),
                            client: client.clone(),
                        });
                    }
                    Err(error) if attempt == 1 => return Err(error.into()),
                    Err(_) => {}
                }
            }
            anyhow::bail!("AnyTLS session could not be opened")
        })
        .await
        .map_err(|_| anyhow::anyhow!("AnyTLS handshake timed out"))?
    }
}
impl Client {
    fn ensure_reaper(self: &Arc<Self>) {
        if !self.keep_idle {
            return;
        }
        let mut reaper = self.reaper.lock().unwrap();
        if reaper.is_some() {
            return;
        }
        let weak = Arc::downgrade(self);
        *reaper = Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let Some(client) = weak.upgrade() else {
                    break;
                };
                client
                    .idle
                    .lock()
                    .unwrap()
                    .retain(|s| s.idle_since.elapsed() < IDLE_TTL);
            }
        }));
    }
    fn take_idle(&self) -> Option<Session> {
        let mut idle = self.idle.lock().unwrap();
        while let Some(session) = idle.pop() {
            if session.idle_since.elapsed() < IDLE_TTL {
                return Some(session);
            }
        }
        None
    }
    async fn create_session(&self) -> Result<Session> {
        static GLOBAL: OnceLock<Arc<Semaphore>> = OnceLock::new();
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| anyhow::anyhow!("AnyTLS session capacity reached"))?;
        let global = GLOBAL
            .get_or_init(|| Arc::new(Semaphore::new(128)))
            .clone()
            .try_acquire_owned()
            .map_err(|_| anyhow::anyhow!("AnyTLS global capacity reached"))?;
        let mut raw = transport::connect_with_connector(
            &self.server,
            self.port,
            &self.connector,
            self.tls.sni.as_deref().unwrap_or(&self.server),
            true,
            Duration::from_secs(15),
        )
        .await?;
        let pad = self
            .padding
            .lock()
            .unwrap()
            .sizes(0)
            .into_iter()
            .flatten()
            .next()
            .unwrap_or(0);
        let mut auth = Vec::with_capacity(34 + pad);
        auth.extend(self.password);
        auth.extend((pad as u16).to_be_bytes());
        auth.resize(34 + pad, 0);
        use rand::Rng;
        rand::rng().fill(&mut auth[34..]);
        raw.write_all(&auth).await?;
        raw.flush().await?;
        Ok(Session {
            raw,
            _slot: slot,
            _global: global,
            padding: self.padding.clone(),
            id: 0,
            next_id: 1,
            packet: 1,
            idle_since: Instant::now(),
            wire: VecDeque::new(),
            wire_pos: 0,
            header: [0; 7],
            header_pos: 0,
            body: Vec::new(),
            body_pos: 0,
            content: Vec::new(),
            content_pos: 0,
            acked: false,
            server_settings: false,
            peer_finished: false,
            local_finished: false,
            healthy: true,
        })
    }
    fn recycle(&self, mut session: Session) {
        if !self.keep_idle || !session.healthy {
            return;
        }
        session.idle_since = Instant::now();
        let mut idle = self.idle.lock().unwrap();
        if idle.len() >= IDLE_LIMIT {
            idle.remove(0);
        }
        idle.push(session);
    }
}
fn frame(out: &mut Vec<u8>, command: u8, id: u32, data: &[u8]) {
    out.push(command);
    out.extend(id.to_be_bytes());
    out.extend((data.len() as u16).to_be_bytes());
    out.extend(data);
}
pub(super) fn socks_address(host: &str, port: u16) -> Result<Vec<u8>> {
    ensure!(
        !host.is_empty() && !host.chars().any(|c| c.is_control() || c.is_whitespace()),
        "Invalid proxy destination"
    );
    let mut result = Vec::new();
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            result.push(1);
            result.extend(ip.octets());
        }
        Ok(std::net::IpAddr::V6(ip)) => {
            result.push(4);
            result.extend(ip.octets());
        }
        Err(_) => {
            ensure!(host.len() <= 255, "Destination name too long");
            result.extend([3, host.len() as u8]);
            result.extend(host.as_bytes());
        }
    }
    result.extend(port.to_be_bytes());
    Ok(result)
}
struct Session {
    raw: BoxProxyStream,
    _slot: OwnedSemaphorePermit,
    _global: OwnedSemaphorePermit,
    padding: Arc<Mutex<Padding>>,
    id: u32,
    next_id: u32,
    packet: u32,
    idle_since: Instant,
    wire: VecDeque<Vec<u8>>,
    wire_pos: usize,
    header: [u8; 7],
    header_pos: usize,
    body: Vec<u8>,
    body_pos: usize,
    content: Vec<u8>,
    content_pos: usize,
    acked: bool,
    server_settings: bool,
    peer_finished: bool,
    local_finished: bool,
    healthy: bool,
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
impl Session {
    fn queue(&mut self, data: Vec<u8>) {
        let sizes = self.padding.lock().unwrap().sizes(self.packet);
        self.packet = self.packet.saturating_add(1);
        let mut pos = 0;
        for size in sizes {
            let Some(size) = size else {
                if pos == data.len() {
                    break;
                }
                continue;
            };
            let n = size.min(data.len() - pos);
            let mut record = data[pos..pos + n].to_vec();
            pos += n;
            if size - n >= 7 {
                let gap = size - n - 7;
                frame(&mut record, 0, 0, &vec![0; gap]);
            }
            if !record.is_empty() {
                self.wire.push_back(record);
            }
        }
        if pos < data.len() {
            self.wire.push_back(data[pos..].to_vec());
        }
        if self.wire.iter().map(Vec::len).sum::<usize>() > 128 * 1024 {
            self.healthy = false;
            self.wire.clear();
        }
    }
    fn poll_flush_wire(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if !self.healthy {
            return Poll::Ready(Err(invalid("AnyTLS session is no longer usable")));
        }
        while let Some(record) = self.wire.front() {
            while self.wire_pos < record.len() {
                match Pin::new(&mut *self.raw).poll_write(cx, &record[self.wire_pos..]) {
                    Poll::Ready(Ok(0)) => {
                        self.healthy = false;
                        return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
                    }
                    Poll::Ready(Ok(n)) => self.wire_pos += n,
                    Poll::Ready(Err(e)) => {
                        self.healthy = false;
                        return Poll::Ready(Err(e));
                    }
                    Poll::Pending => return Poll::Pending,
                }
            }
            match Pin::new(&mut *self.raw).poll_flush(cx) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(e)) => {
                    self.healthy = false;
                    return Poll::Ready(Err(e));
                }
                Poll::Pending => return Poll::Pending,
            }
            self.wire.pop_front();
            self.wire_pos = 0;
        }
        Poll::Ready(Ok(()))
    }
    fn poll_frame(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<(u8, u32, Vec<u8>)>> {
        while self.header_pos < 7 {
            let mut buf = ReadBuf::new(&mut self.header[self.header_pos..]);
            match Pin::new(&mut *self.raw).poll_read(cx, &mut buf) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(())) => {
                    let n = buf.filled().len();
                    if n == 0 {
                        return Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()));
                    }
                    self.header_pos += n;
                }
            }
        }
        let len = u16::from_be_bytes([self.header[5], self.header[6]]) as usize;
        if self.body.len() != len {
            self.body.resize(len, 0);
        }
        while self.body_pos < len {
            let mut buf = ReadBuf::new(&mut self.body[self.body_pos..]);
            match Pin::new(&mut *self.raw).poll_read(cx, &mut buf) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(())) => {
                    let n = buf.filled().len();
                    if n == 0 {
                        return Poll::Ready(Err(io::ErrorKind::UnexpectedEof.into()));
                    }
                    self.body_pos += n;
                }
            }
        }
        let command = self.header[0];
        let id = u32::from_be_bytes(self.header[1..5].try_into().unwrap());
        self.header_pos = 0;
        self.body_pos = 0;
        Poll::Ready(Ok((command, id, std::mem::take(&mut self.body))))
    }
    fn apply(&mut self, command: u8, id: u32, data: Vec<u8>) -> io::Result<()> {
        match command {
            0 => {}
            2 if id == self.id => {
                if self.content.len().saturating_sub(self.content_pos) + data.len() > 65535 {
                    return Err(invalid("AnyTLS early data limit exceeded"));
                }
                self.content.drain(..self.content_pos);
                self.content_pos = 0;
                self.content.extend(data);
            }
            3 if id == self.id && data.is_empty() => self.peer_finished = true,
            7 if id == self.id => {
                if !data.is_empty() {
                    return Err(io::Error::new(
                        io::ErrorKind::ConnectionRefused,
                        "AnyTLS destination rejected",
                    ));
                }
                self.acked = true;
            }
            2 | 3 | 7 if id > 0 && id < self.id => {} // Late frames from an explicitly closed prior stream.
            5 => {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "AnyTLS server rejected the session",
                ));
            }
            6 if id == 0 => {
                let policy =
                    Padding::parse(&data).map_err(|_| invalid("Invalid AnyTLS padding update"))?;
                *self.padding.lock().unwrap() = policy;
            }
            8 if data.is_empty() => {
                let mut reply = Vec::new();
                frame(&mut reply, 9, id, &[]);
                if self.wire.len() >= 16 {
                    return Err(invalid("AnyTLS control backlog exceeded"));
                }
                self.queue(reply);
            }
            9 if data.is_empty() => {}
            10 if id == 0 => {
                let text = std::str::from_utf8(&data)
                    .map_err(|_| invalid("Invalid AnyTLS server settings"))?;
                if !text.lines().any(|line| line == "v=2") {
                    return Err(invalid("Unsupported AnyTLS server version"));
                }
                self.server_settings = true;
            }
            _ => return Err(invalid("Unexpected AnyTLS session frame")),
        }
        Ok(())
    }
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        for _ in 0..16 {
            if let Poll::Ready(Err(e)) = self.poll_flush_wire(cx) {
                return Poll::Ready(Err(e));
            }
            if self.acked && self.server_settings {
                return Poll::Ready(Ok(()));
            }
            let (cmd, id, data) = match self.poll_frame(cx) {
                Poll::Ready(Ok(frame)) => frame,
                Poll::Ready(Err(e)) => {
                    self.healthy = false;
                    return Poll::Ready(Err(e));
                }
                Poll::Pending => return Poll::Pending,
            };
            if let Err(e) = self.apply(cmd, id, data) {
                self.healthy = false;
                return Poll::Ready(Err(e));
            }
            if self.peer_finished {
                self.healthy = false;
                return Poll::Ready(Err(invalid("AnyTLS stream closed before acknowledgement")));
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}
struct AnyTlsStream {
    session: Option<Session>,
    client: Arc<Client>,
}
impl AsyncRead for AnyTlsStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let s = self.session.as_mut().unwrap();
        for _ in 0..16 {
            if s.content_pos < s.content.len() {
                let n = buf.remaining().min(s.content.len() - s.content_pos);
                buf.put_slice(&s.content[s.content_pos..s.content_pos + n]);
                s.content_pos += n;
                return Poll::Ready(Ok(()));
            }
            s.content.clear();
            s.content_pos = 0;
            if s.peer_finished {
                return Poll::Ready(Ok(()));
            }
            if let Poll::Ready(Err(e)) = s.poll_flush_wire(cx) {
                return Poll::Ready(Err(e));
            }
            let (cmd, id, data) = match s.poll_frame(cx) {
                Poll::Ready(Ok(f)) => f,
                Poll::Ready(Err(e)) => {
                    s.healthy = false;
                    return Poll::Ready(Err(e));
                }
                Poll::Pending => return Poll::Pending,
            };
            if let Err(e) = s.apply(cmd, id, data) {
                s.healthy = false;
                return Poll::Ready(Err(e));
            }
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}
impl AsyncWrite for AnyTlsStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let s = self.session.as_mut().unwrap();
        if s.local_finished || s.peer_finished {
            return Poll::Ready(Err(io::ErrorKind::BrokenPipe.into()));
        }
        match s.poll_flush_wire(cx) {
            Poll::Ready(Ok(())) => {}
            other => return other.map_ok(|_| 0),
        }
        let n = data.len().min(MAX_PAYLOAD);
        if n == 0 {
            return Poll::Ready(Ok(0));
        }
        let mut bytes = Vec::with_capacity(n + 7);
        frame(&mut bytes, 2, s.id, &data[..n]);
        s.queue(bytes);
        Poll::Ready(Ok(n))
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.session.as_mut().unwrap().poll_flush_wire(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let s = self.session.as_mut().unwrap();
        if !s.local_finished {
            let mut fin = Vec::new();
            frame(&mut fin, 3, s.id, &[]);
            s.queue(fin);
            s.local_finished = true;
        }
        s.poll_flush_wire(cx)
    }
}
impl Drop for AnyTlsStream {
    fn drop(&mut self) {
        let Some(mut s) = self.session.take() else {
            return;
        };
        if !s.healthy || !self.client.keep_idle {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let client = self.client.clone();
        // The bounded finalizer owns the session; cancellation closes its TLS
        // socket and releases permits. No detached reader survives stream drop.
        runtime.spawn(async move {
            if !s.local_finished && !s.peer_finished {
                let mut fin = Vec::new();
                frame(&mut fin, 3, s.id, &[]);
                s.queue(fin);
                s.local_finished = true;
            }
            if matches!(
                tokio::time::timeout(
                    Duration::from_secs(1),
                    std::future::poll_fn(|cx| s.poll_flush_wire(cx))
                )
                .await,
                Ok(Ok(()))
            ) {
                client.recycle(s);
            }
        });
    }
}
impl Outbound for AnyTlsOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let host = host.to_owned();
        Box::pin(async move { Ok(Box::new(self.open(&host, port).await?) as BoxProxyStream) })
    }
    fn connect_datagram(
        &self,
        host: &str,
        port: u16,
    ) -> super::datagram::DatagramFuture<'_, super::datagram::BoxDatagram> {
        let host = host.to_owned();
        Box::pin(async move {
            let mut request = vec![1];
            request.extend(socks_address(&host, port)?);
            let stream = self
                .open_initial("sp.v2.udp-over-tcp.arpa", 0, &request)
                .await?;
            Ok(super::stream_datagram::StreamDatagram::boxed(
                Box::new(stream),
                super::stream_datagram::Framing::Length16,
            ))
        })
    }
    fn name(&self) -> &str {
        "anytls"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;
    fn session(raw: BoxProxyStream) -> Session {
        Session {
            raw,
            _slot: Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap(),
            _global: Arc::new(Semaphore::new(1)).try_acquire_owned().unwrap(),
            padding: Arc::new(Mutex::new(Padding::default())),
            id: 1,
            next_id: 2,
            packet: 1,
            idle_since: Instant::now(),
            wire: VecDeque::new(),
            wire_pos: 0,
            header: [0; 7],
            header_pos: 0,
            body: vec![],
            body_pos: 0,
            content: vec![],
            content_pos: 0,
            acked: false,
            server_settings: false,
            peer_finished: false,
            local_finished: false,
            healthy: true,
        }
    }
    #[tokio::test]
    async fn partial_frame_reads_survive_cancellation_and_keep_payload_boundaries() {
        let (a, mut b) = tokio::io::duplex(128);
        let mut s = session(Box::new(a));
        let mut bytes = vec![];
        frame(&mut bytes, 2, 1, b"preserved");
        b.write_all(&bytes[..3]).await.unwrap();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(20),
                std::future::poll_fn(|cx| s.poll_frame(cx))
            )
            .await
            .is_err()
        );
        b.write_all(&bytes[3..]).await.unwrap();
        let (cmd, id, body) = std::future::poll_fn(|cx| s.poll_frame(cx)).await.unwrap();
        assert_eq!((cmd, id, body), (2, 1, b"preserved".to_vec()));
    }
    #[test]
    fn errors_and_old_streams_never_become_new_stream_payload() {
        let (a, _) = tokio::io::duplex(128);
        let mut s = session(Box::new(a));
        s.id = 2;
        s.apply(2, 1, b"old stream".to_vec()).unwrap();
        assert!(s.content.is_empty());
        s.apply(2, 2, b"current".to_vec()).unwrap();
        assert_eq!(s.content, b"current");
        assert!(s.apply(2, 3, vec![]).is_err());
        assert!(
            !s.apply(7, 2, b"private server response".to_vec())
                .unwrap_err()
                .to_string()
                .contains("private")
        );
        let before = s.padding.lock().unwrap().md5.clone();
        assert!(s.apply(6, 0, b"stop=999999999".to_vec()).is_err());
        assert_eq!(s.padding.lock().unwrap().md5, before);
    }
    #[tokio::test]
    async fn authentication_version_ack_and_session_reuse_use_increasing_ids() {
        let (a, mut b) = tokio::io::duplex(2048);
        let mut s = session(Box::new(a));
        let mut input = vec![];
        frame(&mut input, 10, 0, b"v=2\n");
        frame(&mut input, 7, 1, b"");
        frame(&mut input, 2, 1, b"hello");
        b.write_all(&input).await.unwrap();
        std::future::poll_fn(|cx| s.poll_ready(cx)).await.unwrap();
        assert!(s.acked && s.server_settings);
        let c = AnyTlsOutbound::new("localhost", 443, "synthetic", None, false).unwrap();
        let mut stream = AnyTlsStream {
            session: Some(s),
            client: c.client,
        };
        let mut data = [0; 5];
        stream.read_exact(&mut data).await.unwrap();
        assert_eq!(&data, b"hello");
        stream.write_all(b"reply").await.unwrap();
        stream.flush().await.unwrap();
        let mut header = [0; 7];
        b.read_exact(&mut header).await.unwrap();
        assert_eq!(header[0], 2);
        assert_eq!(u32::from_be_bytes(header[1..5].try_into().unwrap()), 1);
        let n = u16::from_be_bytes(header[5..].try_into().unwrap()) as usize;
        let mut body = vec![0; n];
        b.read_exact(&mut body).await.unwrap();
        assert_eq!(body, b"reply");
    }
    #[test]
    fn server_padding_is_per_client_and_idle_pool_has_a_hard_cap() {
        let a = AnyTlsOutbound::new("localhost", 443, "synthetic", None, true).unwrap();
        let b = AnyTlsOutbound::new("localhost", 443, "synthetic", None, true).unwrap();
        *a.client.padding.lock().unwrap() = Padding::parse(b"stop=2\n0=10-10\n1=40-40").unwrap();
        assert_ne!(
            a.client.padding.lock().unwrap().md5,
            b.client.padding.lock().unwrap().md5
        );
        for _ in 0..20 {
            let (io, _) = tokio::io::duplex(128);
            a.client.recycle(session(Box::new(io)));
        }
        assert_eq!(a.client.idle.lock().unwrap().len(), IDLE_LIMIT);
    }
}

#[cfg(test)]
mod reference_tests;
