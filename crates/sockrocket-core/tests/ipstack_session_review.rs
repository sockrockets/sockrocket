//! Memory-only regression for ipstack session replacement.
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{Duration, timeout};
fn packet() -> Vec<u8> {
    let builder =
        etherparse::PacketBuilder::ipv4([192, 0, 2, 2], [198, 51, 100, 2], 64).udp(12345, 443);
    let mut bytes = Vec::new();
    builder.write(&mut bytes, b"sample").unwrap();
    bytes
}
#[tokio::test(flavor = "current_thread")]
async fn closing_and_reopening_udp_tuple_preserves_first_packet_and_replacement() {
    for _ in 0..500 {
        let (device, mut client) = tokio::io::duplex(4096);
        let mut stack = ipstack::IpStack::new(ipstack::IpStackConfig::default(), device);
        let bytes = packet();
        client.write_all(&bytes).await.unwrap();
        let old = timeout(Duration::from_secs(1), stack.accept())
            .await
            .unwrap()
            .unwrap();
        drop(old);
        client.write_all(&bytes).await.unwrap();
        let new = timeout(Duration::from_secs(1), stack.accept())
            .await
            .expect("first packet lost")
            .unwrap();
        let ipstack::IpStackStream::Udp(mut new) = new else {
            panic!("expected UDP")
        };
        let mut payload = [0; 6];
        new.read_exact(&mut payload).await.unwrap();
        assert_eq!(&payload, b"sample");
        // Let the old generation's delayed removal run before sending again.
        tokio::task::yield_now().await;
        client.write_all(&bytes).await.unwrap();
        timeout(Duration::from_secs(1), new.read_exact(&mut payload))
            .await
            .expect("old cleanup deleted replacement session")
            .unwrap();
        assert_eq!(&payload, b"sample");
    }
}
