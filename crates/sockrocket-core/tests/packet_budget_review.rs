use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf},
    sync::mpsc,
    time::{Duration, timeout},
};
struct Device {
    input: mpsc::Receiver<Vec<u8>>,
    read: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
impl AsyncRead for Device {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.input.poll_recv(cx) {
            Poll::Ready(Some(packet)) => {
                self.read.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                buf.put_slice(&packet);
                Poll::Ready(Ok(()))
            }
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Pending => Poll::Pending,
        }
    }
}
impl AsyncWrite for Device {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
fn packet(source: u16, destination: u16, body: &[u8]) -> Vec<u8> {
    let builder = etherparse::PacketBuilder::ipv4([192, 0, 2, 2], [198, 51, 100, 2], 64)
        .udp(source, destination);
    let mut packet = Vec::new();
    builder.write(&mut packet, body).unwrap();
    packet
}
#[tokio::test]
async fn bulk_queue_is_bounded_and_dns_keeps_admission_and_buffer_capacity() {
    let (tx, rx) = mpsc::channel(32);
    let read = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut stack = ipstack::IpStack::new(
        Default::default(),
        Device {
            input: rx,
            read: read.clone(),
        },
    );
    let mut streams = Vec::new();
    for port in 10000..10256 {
        tx.send(packet(port, 443, b"first")).await.unwrap();
        streams.push(stack.accept().await.unwrap());
    }
    let data = vec![42; 1200];
    for port in 10000..10256 {
        for _ in 0..64 {
            tx.send(packet(port, 443, &data)).await.unwrap();
        }
    }
    // Fill the regular accept backlog without consuming it.
    for port in 20000..20128 {
        tx.send(packet(port, 443, b"queued")).await.unwrap();
    }
    tx.send(packet(5353, 53, b"query")).await.unwrap();
    timeout(Duration::from_secs(2), async {
        while read.load(std::sync::atomic::Ordering::SeqCst) < 256 + 256 * 64 + 128 + 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let ipstack::IpStackStream::Udp(mut dns) = timeout(Duration::from_secs(2), stack.accept())
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("expected DNS UDP")
    };
    assert_eq!(dns.peer_addr().port(), 53);
    let (used, limit) = stack.packet_queue_usage();
    assert!(used > 1024 * 1024);
    assert!(
        used <= limit - limit / 8,
        "bulk consumed the control reserve"
    );
    let mut q = [0; 5];
    dns.read_exact(&mut q).await.unwrap();
    assert_eq!(&q, b"query");
    tx.send(packet(5353, 53, b"again")).await.unwrap();
    timeout(Duration::from_secs(2), dns.read_exact(&mut q))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&q, b"again");
    streams.clear();
    tokio::task::yield_now().await;
    assert!(stack.packet_queue_usage().0 < used);
}
