//! Packet-level warning regression; no real sockets, TUN or routing changes.
use etherparse::{PacketBuilder, SlicedPacket, TransportSlice};
use ipstack::{IpStack, IpStackConfig, IpStackStream, TcpEventCounts};
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::time::{Duration, timeout};

struct TcpLogs(Mutex<Vec<(log::Level, String)>>);
impl log::Log for TcpLogs {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target() == "ipstack::stream::tcp"
    }
    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            self.0
                .lock()
                .unwrap()
                .push((record.level(), record.args().to_string()));
        }
    }
    fn flush(&self) {}
}
static LOGS: TcpLogs = TcpLogs(Mutex::new(Vec::new()));

fn packet(seq: u32, ack: Option<u32>, syn: bool, rst: bool, payload: &[u8]) -> Vec<u8> {
    let builder =
        PacketBuilder::ipv4([192, 0, 2, 2], [198, 51, 100, 2], 64).tcp(12345, 443, seq, 65535);
    let builder = if syn { builder.syn() } else { builder };
    let builder = if rst { builder.rst() } else { builder };
    let builder = if let Some(ack) = ack {
        builder.ack(ack)
    } else {
        builder
    };
    let mut bytes = Vec::new();
    builder.write(&mut bytes, payload).unwrap();
    bytes
}

async fn read_packet(client: &mut DuplexStream) -> Vec<u8> {
    let mut header = [0; 20];
    client.read_exact(&mut header).await.unwrap();
    let size = u16::from_be_bytes([header[2], header[3]]) as usize;
    let mut bytes = header.to_vec();
    bytes.resize(size, 0);
    client.read_exact(&mut bytes[20..]).await.unwrap();
    bytes
}

#[tokio::test]
async fn peer_resets_preserve_errors_counts_and_handshake_warnings() {
    log::set_logger(&LOGS).unwrap();
    log::set_max_level(log::LevelFilter::Debug);

    for established in [false, true] {
        let (device, mut client) = tokio::io::duplex(65536);
        let mut stack = IpStack::new(IpStackConfig::default(), device);
        assert_eq!(stack.tcp_event_counts(), TcpEventCounts::default());
        client
            .write_all(&packet(1000, None, true, false, &[]))
            .await
            .unwrap();
        let IpStackStream::Tcp(mut stream) = stack.accept().await.unwrap() else {
            panic!("expected TCP")
        };
        let synack = read_packet(&mut client).await;
        let parsed = SlicedPacket::from_ip(&synack).unwrap();
        let Some(TransportSlice::Tcp(header)) = parsed.transport else {
            panic!("expected TCP SYN-ACK")
        };
        let ack = header.sequence_number().wrapping_add(1);
        let reset_seq = if established {
            client
                .write_all(&packet(1001, Some(ack), false, false, b"x"))
                .await
                .unwrap();
            let mut marker = [0];
            stream.read_exact(&mut marker).await.unwrap();
            assert_eq!(&marker, b"x");
            read_packet(&mut client).await;
            1002
        } else {
            1001
        };
        client
            .write_all(&packet(reset_seq, Some(ack), false, true, &[]))
            .await
            .unwrap();
        let mut data = [0; 1];
        let error = timeout(Duration::from_secs(1), stream.read(&mut data))
            .await
            .expect("RST must still wake readers")
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
        assert_eq!(
            stack.tcp_event_counts(),
            TcpEventCounts {
                peer_resets: 1,
                handshake_resets: usize::from(!established),
                task_errors: 0,
            }
        );
        // Repeated error reads do not count a reset more than once.
        assert!(stream.read(&mut data).await.is_err());
        assert_eq!(stack.tcp_event_counts().peer_resets, 1);
    }

    let logs = LOGS.0.lock().unwrap();
    assert_eq!(
        logs.iter()
            .filter(|(level, message)| *level == log::Level::Warn
                && message.contains("peer reset before TCP handshake completed"))
            .count(),
        1
    );
    assert_eq!(
        logs.iter()
            .filter(|(level, message)| *level == log::Level::Debug
                && message.contains("peer reset TCP connection"))
            .count(),
        1
    );
    assert!(
        !logs
            .iter()
            .any(|(level, message)| *level == log::Level::Warn && message.contains("task error:"))
    );
}
