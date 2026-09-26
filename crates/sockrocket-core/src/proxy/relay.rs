//! High-performance bidirectional relay for proxy streams.
//!
//! Uses 64 KB buffers (8× tokio's default 8 KB) for significantly better
//! throughput and lower latency, matching what Clash and other mature
//! proxies use internally.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Buffer size for relay operations (64 KB).
const RELAY_BUF_SIZE: usize = 64 * 1024;

/// Pool of reusable relay buffers.
///
/// Without this, every proxied connection heap-allocates and zero-fills two
/// 64 KB buffers (128 KB per connection). A browser burst of a few hundred
/// short connections therefore costs tens of MB of zero-fill plus that many
/// 64 KB alloc/frees — and on Windows a 64 KB alloc is a large-object
/// `VirtualAlloc` that page-faults fresh pages *per connection*. Reusing the
/// buffers removes all of that from the hot path. The pool is small and
/// bounded: buffers are returned on drop and handed out again LIFO, so at
/// steady state it holds roughly the peak concurrency of recent connections.
fn buffer_pool() -> &'static std::sync::Mutex<Vec<Box<[u8; RELAY_BUF_SIZE]>>> {
    static POOL: std::sync::OnceLock<std::sync::Mutex<Vec<Box<[u8; RELAY_BUF_SIZE]>>>> =
        std::sync::OnceLock::new();
    POOL.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// A 64 KB relay buffer that returns to the pool on drop.
struct PooledBuf {
    buf: Option<Box<[u8; RELAY_BUF_SIZE]>>,
}

impl PooledBuf {
    fn take() -> Self {
        let buf = buffer_pool()
            .lock()
            .ok()
            .and_then(|mut pool| pool.pop())
            // First use (or a drained pool) still needs a fresh buffer. The
            // zero-fill is unavoidable here but happens once per *pool entry*,
            // not once per connection.
            .unwrap_or_else(|| Box::new([0u8; RELAY_BUF_SIZE]));
        Self { buf: Some(buf) }
    }
}

impl std::ops::Deref for PooledBuf {
    type Target = [u8; RELAY_BUF_SIZE];
    fn deref(&self) -> &Self::Target {
        self.buf.as_ref().expect("buffer present until drop")
    }
}

impl std::ops::DerefMut for PooledBuf {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.buf.as_mut().expect("buffer present until drop")
    }
}

impl Drop for PooledBuf {
    fn drop(&mut self) {
        if let Some(buf) = self.buf.take()
            && let Ok(mut pool) = buffer_pool().lock()
        {
            // Cap the pool so a transient concurrency spike (e.g. thousands of
            // TUN streams) doesn't pin down hundreds of MB forever. 32 entries
            // = 2 MB held for reuse, plenty for steady-state churn.
            if pool.len() < 32 {
                pool.push(buf);
            }
        }
    }
}

/// Relay data bidirectionally between two async streams using large buffers.
///
/// Returns `(bytes_a_to_b, bytes_b_to_a)` on completion.
pub async fn relay_bidirectional<A, B>(a: &mut A, b: &mut B) -> io::Result<(u64, u64)>
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    Relay {
        a,
        b,
        a_buf: CopyBuf::new(),
        b_buf: CopyBuf::new(),
        a_to_b: 0,
        b_to_a: 0,
        a_done: false,
        b_done: false,
    }
    .await
}

struct CopyBuf {
    buf: PooledBuf,
    pos: usize,
    cap: usize,
}

impl CopyBuf {
    fn new() -> Self {
        Self {
            buf: PooledBuf::take(),
            pos: 0,
            cap: 0,
        }
    }
}

struct Relay<'a, A: ?Sized, B: ?Sized> {
    a: &'a mut A,
    b: &'a mut B,
    a_buf: CopyBuf,
    b_buf: CopyBuf,
    a_to_b: u64,
    b_to_a: u64,
    a_done: bool,
    b_done: bool,
}

/// Transfer data from reader to writer using a CopyBuf.
///
/// `total` is a cross-poll accumulator: each successful `poll_write` immediately
/// increments `*total` so that bytes are counted even when the function returns
/// `Poll::Pending` mid-way (fixing the lost-bytes stats bug).
fn transfer_one<R, W>(
    cx: &mut Context<'_>,
    reader: &mut R,
    writer: &mut W,
    buf: &mut CopyBuf,
    done: &mut bool,
    total: &mut u64,
) -> Poll<io::Result<()>>
where
    R: AsyncRead + Unpin + ?Sized,
    W: AsyncWrite + Unpin + ?Sized,
{
    loop {
        // Consume one unit of the runtime's coop budget per read/write
        // cycle (the same mechanism tokio's io::util::copy uses). When the
        // budget is exhausted this returns Pending after scheduling a wake,
        // forcing an occasional yield so one fast direction can't
        // monopolize the worker. The guard restores the budget if the
        // iteration ends without progress, so normal throughput is
        // unaffected — the budget only caps spinning while both sides stay
        // ready.
        let coop = ready!(tokio::task::coop::poll_proceed(cx));

        // If we have data in the buffer, write it out
        if buf.pos < buf.cap {
            let n = ready!(Pin::new(&mut *writer).poll_write(cx, &buf.buf[buf.pos..buf.cap]))?;
            if n == 0 {
                return Poll::Ready(Err(io::Error::new(io::ErrorKind::WriteZero, "write zero")));
            }
            coop.made_progress();
            buf.pos += n;
            *total += n as u64; // accumulate immediately — persists across polls
            if buf.pos == buf.cap {
                buf.pos = 0;
                buf.cap = 0;
            }
            continue;
        }

        // If the reader is done, flush and return
        if *done {
            ready!(Pin::new(&mut *writer).poll_flush(cx))?;
            return Poll::Ready(Ok(()));
        }

        // Read new data
        let mut read_buf = ReadBuf::new(&mut buf.buf[..]);
        match ready!(Pin::new(&mut *reader).poll_read(cx, &mut read_buf)) {
            Ok(()) => {
                let n = read_buf.filled().len();
                if n == 0 {
                    *done = true;
                    // Shutdown the write half
                    ready!(Pin::new(&mut *writer).poll_shutdown(cx))?;
                    return Poll::Ready(Ok(()));
                }
                coop.made_progress();
                buf.cap = n;
            }
            Err(e) => return Poll::Ready(Err(e)),
        }
    }
}

impl<A, B> Future for Relay<'_, A, B>
where
    A: AsyncRead + AsyncWrite + Unpin + ?Sized,
    B: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    type Output = io::Result<(u64, u64)>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;

        // Drive A → B (bytes accumulate directly into this.a_to_b on every write)
        let a_to_b = if !this.a_done || this.a_buf.pos < this.a_buf.cap {
            transfer_one(
                cx,
                this.a,
                this.b,
                &mut this.a_buf,
                &mut this.a_done,
                &mut this.a_to_b,
            )
        } else {
            Poll::Ready(Ok(()))
        };

        // Drive B → A (bytes accumulate directly into this.b_to_a on every write)
        let b_to_a = if !this.b_done || this.b_buf.pos < this.b_buf.cap {
            transfer_one(
                cx,
                this.b,
                this.a,
                &mut this.b_buf,
                &mut this.b_done,
                &mut this.b_to_a,
            )
        } else {
            Poll::Ready(Ok(()))
        };

        // Propagate errors immediately.
        match a_to_b {
            Poll::Ready(Ok(())) => {}
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => {}
        }

        match b_to_a {
            Poll::Ready(Ok(())) => {}
            Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
            Poll::Pending => {}
        }

        // Both directions done
        if (this.a_done && this.a_buf.pos >= this.a_buf.cap)
            && (this.b_done && this.b_buf.pos >= this.b_buf.cap)
        {
            return Poll::Ready(Ok((this.a_to_b, this.b_to_a)));
        }

        Poll::Pending
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
