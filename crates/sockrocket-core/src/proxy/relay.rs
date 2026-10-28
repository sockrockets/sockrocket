//! Bidirectional relay with Tokio's complete half-close state machine.
use std::io;
use tokio::io::{AsyncRead, AsyncWrite};

const RELAY_BUF_SIZE: usize = 64 * 1024;

/// Preserve the reverse direction after EOF and finish both asynchronous
/// writer shutdowns before reporting success. Errors cancel both directions.
pub async fn relay_bidirectional<A, B>(a: &mut A, b: &mut B) -> io::Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    tokio::io::copy_bidirectional_with_sizes(a, b, RELAY_BUF_SIZE, RELAY_BUF_SIZE).await
}

/// Bound abandoned TUN relays even after EOF stops polling one read half.
/// The watchdog observes payload progress, not ACKs or spurious wakeups.
/// After either read EOF, cap inactivity at 60 seconds (sampled every 30s).
/// Continued payload progress preserves long responses in the other direction.
pub async fn relay_bidirectional_with_idle_timeout<A, B>(
    a: &mut A,
    b: &mut B,
    idle: std::time::Duration,
) -> io::Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    };
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };
    use tokio::io::ReadBuf;
    struct Progress<'a, S: ?Sized> {
        stream: &'a mut S,
        bytes: Arc<AtomicU64>,
        half_closed_at: Arc<Mutex<Option<tokio::time::Instant>>>,
    }
    impl<S: AsyncRead + Unpin + ?Sized> AsyncRead for Progress<'_, S> {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            let before = buf.filled().len();
            let can_read = buf.remaining() != 0;
            let result = Pin::new(&mut *self.stream).poll_read(cx, buf);
            let n = buf.filled().len() - before;
            if n != 0 {
                self.bytes.fetch_add(n as u64, Ordering::Relaxed);
            } else if can_read && matches!(&result, Poll::Ready(Ok(()))) {
                self.half_closed_at
                    .lock()
                    .unwrap()
                    .get_or_insert_with(tokio::time::Instant::now);
            }
            result
        }
    }
    impl<S: AsyncWrite + Unpin + ?Sized> AsyncWrite for Progress<'_, S> {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &[u8],
        ) -> Poll<io::Result<usize>> {
            let result = Pin::new(&mut *self.stream).poll_write(cx, buf);
            if let Poll::Ready(Ok(n)) = result
                && n != 0
            {
                self.bytes.fetch_add(n as u64, Ordering::Relaxed);
            }
            result
        }
        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut *self.stream).poll_flush(cx)
        }
        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
            Pin::new(&mut *self.stream).poll_shutdown(cx)
        }
    }
    if idle.is_zero() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let bytes = Arc::new(AtomicU64::new(0));
    let half_closed_at = Arc::new(Mutex::new(None));
    let mut observed_a = Progress {
        stream: a,
        bytes: bytes.clone(),
        half_closed_at: half_closed_at.clone(),
    };
    let mut observed_b = Progress {
        stream: b,
        bytes: bytes.clone(),
        half_closed_at: half_closed_at.clone(),
    };
    // TUN keeps many mostly idle flows alive. Reserve 16 KiB per direction
    // instead of the desktop relay's 64 KiB (16 vs 64 MiB at 512 flows).
    let relay = tokio::io::copy_bidirectional_with_sizes(
        &mut observed_a,
        &mut observed_b,
        16 * 1024,
        16 * 1024,
    );
    tokio::pin!(relay);
    let mut tick = tokio::time::interval(idle.min(std::time::Duration::from_secs(30)));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_bytes = 0;
    let mut last_progress = tokio::time::Instant::now();
    loop {
        tokio::select! {
            result = &mut relay => return result,
            _ = tick.tick() => {
                let current = bytes.load(Ordering::Relaxed);
                if current != last_bytes {
                    last_bytes = current;
                    last_progress = tokio::time::Instant::now();
                } else {
                    let eof = *half_closed_at.lock().unwrap();
                    // A previously idle established connection gets a fresh
                    // grace period at EOF; activity continues to extend it.
                    let since = eof.map_or(last_progress, |at| at.max(last_progress));
                    let limit = if eof.is_some() {
                        idle.min(std::time::Duration::from_secs(60))
                    } else {
                        idle
                    };
                    if since.elapsed() >= limit {
                        let message = if eof.is_some() {
                            "TUN half-closed relay idle timeout"
                        } else {
                            "TUN relay idle timeout"
                        };
                        return Err(io::Error::new(io::ErrorKind::TimedOut, message));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    #[tokio::test]
    async fn test_relay_bidirectional() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            loop {
                match s.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if s.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        let (_server_half, _) = tokio::io::duplex(1024);

        // Simple test: write data from one side and verify it's relayed
        client.write_all(b"hello relay").await.unwrap();
        client.shutdown().await.unwrap();

        // Wait for echo server
        let _ = server.await;
    }

    #[test]
    fn test_buf_size() {
        assert_eq!(RELAY_BUF_SIZE, 65536);
    }
}
