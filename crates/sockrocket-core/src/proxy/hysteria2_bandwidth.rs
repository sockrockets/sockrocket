//! Aggregate payload bandwidth budget shared by all streams and UDP sessions
//! of one outbound. QUIC retains BBR for congestion control; download capacity
//! is also advertised to the server using Hysteria-CC-RX.
use super::{
    connector::BoxProxyStream,
    datagram::{BoxDatagram, Datagram, DatagramFuture},
};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{Notify, futures::OwnedNotified},
    time::{Instant, Sleep},
};

// No future time slots are handed out. Only currently available credit can
// be reserved, so refunding one flow cannot overlap another flow's permit.
pub(super) struct Budget {
    rate: u64,
    capacity: usize,
    state: Mutex<Bucket>,
    changed: Arc<Notify>,
}
struct Bucket {
    tokens: f64,
    updated: Instant,
}
struct Ticket {
    budget: Arc<Budget>,
    bytes: usize,
}
impl Ticket {
    fn commit(mut self, used: usize) {
        self.bytes = self.bytes.saturating_sub(used);
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        self.budget.refund(self.bytes);
    }
}
impl Budget {
    pub fn new(rate: u64) -> Option<Arc<Self>> {
        (rate != 0).then(|| {
            let capacity = rate.min(16 * 1024) as usize;
            Arc::new(Self {
                rate,
                capacity,
                changed: Arc::new(Notify::new()),
                state: Mutex::new(Bucket {
                    tokens: capacity as f64,
                    updated: Instant::now(),
                }),
            })
        })
    }
    fn refill(&self, state: &mut Bucket) {
        let now = Instant::now();
        state.tokens = (state.tokens
            + now.duration_since(state.updated).as_secs_f64() * self.rate as f64)
            .min(self.capacity as f64);
        state.updated = now;
    }
    fn refund(&self, bytes: usize) {
        if bytes == 0 {
            return;
        }
        let mut state = self.state.lock().unwrap();
        self.refill(&mut state);
        state.tokens = (state.tokens + bytes as f64).min(self.capacity as f64);
        drop(state);
        self.changed.notify_waiters();
    }
    fn notification(&self) -> Pin<Box<OwnedNotified>> {
        let mut changed = Box::pin(self.changed.clone().notified_owned());
        changed.as_mut().enable();
        changed
    }
    fn try_acquire(self: &Arc<Self>, bytes: usize) -> Result<Ticket, Instant> {
        let mut state = self.state.lock().unwrap();
        self.refill(&mut state);
        let required = bytes.min(self.capacity) as f64;
        if state.tokens + f64::EPSILON >= required {
            state.tokens -= bytes as f64;
            Ok(Ticket {
                budget: self.clone(),
                bytes,
            })
        } else {
            Err(Instant::now()
                + Duration::from_secs_f64(
                    ((required - state.tokens) / self.rate as f64).max(0.000_000_001),
                ))
        }
    }
    async fn acquire(self: &Arc<Self>, bytes: usize) -> Ticket {
        loop {
            let changed = self.notification();
            match self.try_acquire(bytes) {
                Ok(ticket) => return ticket,
                Err(deadline) => tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => {},
                    _ = changed => {},
                },
            }
        }
    }
}
struct Wait {
    timer: Pin<Box<Sleep>>,
    changed: Pin<Box<OwnedNotified>>,
}
struct Stream {
    inner: BoxProxyStream,
    budget: Arc<Budget>,
    waiting: Option<Wait>,
    ticket: Option<Ticket>,
}
pub(super) fn stream(inner: BoxProxyStream, budget: &Option<Arc<Budget>>) -> BoxProxyStream {
    match budget {
        Some(budget) => Box::new(Stream {
            inner,
            budget: budget.clone(),
            waiting: None,
            ticket: None,
        }),
        None => inner,
    }
}
impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_read(cx, buf)
    }
}
impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        loop {
            if this.ticket.is_some() {
                break;
            }
            if let Some(waiting) = &mut this.waiting {
                if waiting.changed.as_mut().poll(cx).is_pending()
                    && waiting.timer.as_mut().poll(cx).is_pending()
                {
                    return Poll::Pending;
                }
                this.waiting = None;
            }
            let size = buf.len().min(this.budget.capacity);
            let changed = this.budget.notification();
            match this.budget.try_acquire(size) {
                Ok(ticket) => this.ticket = Some(ticket),
                Err(deadline) => {
                    this.waiting = Some(Wait {
                        timer: Box::pin(tokio::time::sleep_until(deadline)),
                        changed,
                    })
                }
            }
        }
        let size = buf.len().min(this.ticket.as_ref().unwrap().bytes);
        let result = Pin::new(&mut this.inner).poll_write(cx, &buf[..size]);
        if let Poll::Ready(result) = &result {
            this.ticket
                .take()
                .unwrap()
                .commit(result.as_ref().copied().unwrap_or(0));
        }
        result
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.ticket = None;
        this.waiting = None;
        Pin::new(&mut this.inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        this.ticket = None;
        this.waiting = None;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}
struct Packet {
    inner: BoxDatagram,
    budget: Arc<Budget>,
}
pub(super) fn datagram(inner: BoxDatagram, budget: &Option<Arc<Budget>>) -> BoxDatagram {
    match budget {
        Some(budget) => Box::new(Packet {
            inner,
            budget: budget.clone(),
        }),
        None => inner,
    }
}
impl Datagram for Packet {
    fn send<'a>(&'a self, bytes: &'a [u8]) -> DatagramFuture<'a, ()> {
        Box::pin(async move {
            let ticket = self.budget.acquire(bytes.len()).await;
            self.inner.send(bytes).await?;
            ticket.commit(bytes.len());
            Ok(())
        })
    }
    fn recv<'a>(&'a self, bytes: &'a mut [u8]) -> DatagramFuture<'a, usize> {
        self.inner.recv(bytes)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn throttled_tls_handshake_and_payload_work_under_backpressure() {
        use sha2::{Digest, Sha256};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let _ = rustls::crypto::ring::default_provider().install_default();
        let cert = rustls::pki_types::CertificateDer::from(
            include_bytes!("testdata/localhost-cert.der").to_vec(),
        );
        let key =
            rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
                include_bytes!("testdata/localhost-key.der").to_vec(),
            ));
        let mut config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert.clone()], key)
            .unwrap();
        config.send_tls13_tickets = 0;
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let (client, server) = tokio::io::duplex(64);
        let server = tokio::spawn(async move {
            let mut tls = acceptor.accept(server).await.unwrap();
            let mut received = [0; 4096];
            tls.read_exact(&mut received).await.unwrap();
            assert_eq!(received, [7; 4096]);
            tls.write_all(b"ok").await.unwrap();
            tls.flush().await.unwrap();
        });
        let budget = Budget::new(20_000);
        // Force an actual pending write before the ClientHello.
        let budget_ref = budget.as_ref().unwrap();
        budget_ref
            .try_acquire(budget_ref.capacity)
            .unwrap_or_else(|_| panic!("initial credit"))
            .commit(budget_ref.capacity);
        let io = stream(Box::new(client), &budget);
        let tls = crate::config::model::TlsConfig {
            sni: Some("localhost".into()),
            skip_cert_verify: true,
            alpn: None,
            fingerprint: None,
        };
        let connector = super::super::transport::build_tls_connector_pinned(
            Some(&tls),
            Some(Sha256::digest(cert.as_ref()).into()),
        );
        let mut client = tokio::time::timeout(
            Duration::from_secs(5),
            connector.connect(
                rustls_pki_types::ServerName::try_from("localhost").unwrap(),
                io,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        tokio::time::timeout(Duration::from_secs(5), client.write_all(&[7; 4096]))
            .await
            .expect("payload write deadline")
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), client.flush())
            .await
            .expect("flush deadline")
            .unwrap();
        let mut response = [0; 2];
        tokio::time::timeout(Duration::from_secs(5), client.read_exact(&mut response))
            .await
            .expect("response deadline")
            .unwrap();
        assert_eq!(&response, b"ok");
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .expect("server deadline")
            .unwrap();
    }
    #[tokio::test(start_paused = true)]
    async fn throttled_stream_keeps_payload_intact_and_waits_between_writes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let budget = Budget::new(1000);
        let (writer, mut reader) = tokio::io::duplex(4096);
        let mut writer = stream(Box::new(writer), &budget);
        let started = Instant::now();
        writer.write_all(&[1; 1000]).await.unwrap();
        writer.write_all(&[2; 1000]).await.unwrap();
        assert!(started.elapsed() >= Duration::from_secs(1));
        let mut received = [0; 2000];
        reader.read_exact(&mut received).await.unwrap();
        assert_eq!(&received[..1000], &[1; 1000]);
        assert_eq!(&received[1000..], &[2; 1000]);
    }
    #[tokio::test(start_paused = true)]
    async fn cancelled_and_partial_tickets_never_overlap_concurrent_credit() {
        let budget = Budget::new(1000).unwrap();
        let first = budget
            .try_acquire(1000)
            .unwrap_or_else(|_| panic!("initial credit"));
        assert!(budget.try_acquire(1000).is_err());
        first.commit(100);
        assert!(budget.try_acquire(1000).is_err());
        tokio::time::advance(Duration::from_millis(100)).await;
        let second = budget
            .try_acquire(1000)
            .unwrap_or_else(|_| panic!("refilled credit"));
        assert!(budget.try_acquire(1000).is_err());
        // Cancellation returns unused tokens immediately, not future slots.
        drop(second);
        let third = budget
            .try_acquire(1000)
            .unwrap_or_else(|_| panic!("refunded credit"));
        third.commit(1000);
        assert!(budget.try_acquire(1).is_err());
        assert!(Budget::new(0).is_none());
    }
    #[tokio::test(start_paused = true)]
    async fn cancelling_large_packet_wakes_existing_waiters_immediately() {
        let budget = Budget::new(1).unwrap();
        let packet = budget
            .try_acquire(60_000)
            .unwrap_or_else(|_| panic!("initial packet"));
        let waiter = tokio::spawn({
            let budget = budget.clone();
            async move { budget.acquire(1).await }
        });
        tokio::task::yield_now().await;
        let started = Instant::now();
        drop(packet);
        let ticket = tokio::time::timeout(Duration::from_millis(1), waiter)
            .await
            .expect("refund notification")
            .unwrap();
        assert!(started.elapsed() < Duration::from_millis(1));
        ticket.commit(1);
    }
    #[tokio::test(start_paused = true)]
    async fn oversized_datagrams_repay_debt_and_cancelled_waiters_cost_nothing() {
        let budget = Budget::new(1000).unwrap();
        budget
            .try_acquire(2000)
            .unwrap_or_else(|_| panic!("one packet burst"))
            .commit(2000);
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(budget.try_acquire(1).is_err());
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(budget.try_acquire(1000).is_ok());
    }
}
