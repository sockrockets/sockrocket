//! Protocol-defined UDP framing over streams. Read framing is owned by one
//! task so cancellation of a caller's recv cannot discard half a frame.
use super::{
    connector::BoxProxyStream,
    datagram::{BoxDatagram, Datagram, DatagramFuture},
};
use anyhow::{Result, bail, ensure};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt, WriteHalf},
    sync::{Mutex, mpsc},
};
#[derive(Clone)]
pub(super) enum Framing {
    Length16,
    Xudp(Vec<u8>),
    Trojan(Vec<u8>),
    NativeChunks(usize),
}
pub(super) struct StreamDatagram {
    writer: Mutex<WriteHalf<BoxProxyStream>>,
    received: Mutex<mpsc::Receiver<Result<Vec<u8>>>>,
    task: tokio::task::JoinHandle<()>,
    framing: Framing,
    poisoned: AtomicBool,
    request_started: AtomicBool,
}
impl Drop for StreamDatagram {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl StreamDatagram {
    pub fn boxed(stream: BoxProxyStream, framing: Framing) -> BoxDatagram {
        let (mut reader, writer) = tokio::io::split(stream);
        let (tx, rx) = mpsc::channel(2);
        let frame = framing.clone();
        let task = tokio::spawn(async move {
            loop {
                let packet = read_packet(&mut reader, &frame).await;
                let failed = packet.is_err();
                if tx.send(packet).await.is_err() || failed {
                    break;
                }
            }
        });
        Box::new(Self {
            writer: Mutex::new(writer),
            received: Mutex::new(rx),
            task,
            framing,
            poisoned: AtomicBool::new(false),
            request_started: AtomicBool::new(false),
        })
    }
}
impl Datagram for StreamDatagram {
    fn send<'a>(&'a self, packet: &'a [u8]) -> DatagramFuture<'a, ()> {
        Box::pin(async move {
            ensure!(packet.len() <= 65535, "UDP payload too large");
            if let Framing::NativeChunks(max) = self.framing {
                ensure!(
                    !packet.is_empty() && packet.len() <= max,
                    "VMess UDP packet exceeds negotiated chunk size or is empty"
                );
            }
            let mut writer = self.writer.lock().await;
            ensure!(
                !self.poisoned.swap(true, Ordering::SeqCst),
                "Previous UDP frame write was cancelled"
            );
            match &self.framing {
                Framing::Length16 => writer.write_u16(packet.len() as u16).await?,
                Framing::Xudp(address) => {
                    writer.write_u16((5 + address.len()) as u16).await?;
                    let status = if self.request_started.swap(true, Ordering::SeqCst) {
                        2
                    } else {
                        1
                    };
                    writer.write_all(&[0, 0, status, 1, 2]).await?;
                    writer.write_all(address).await?;
                    writer.write_u16(packet.len() as u16).await?;
                }
                Framing::Trojan(address) => {
                    writer.write_all(address).await?;
                    writer.write_u16(packet.len() as u16).await?;
                    writer.write_all(b"\r\n").await?;
                }
                Framing::NativeChunks(_) => {}
            }
            writer.write_all(packet).await?;
            writer.flush().await?;
            self.poisoned.store(false, Ordering::SeqCst);
            Ok(())
        })
    }
    fn recv<'a>(&'a self, buffer: &'a mut [u8]) -> DatagramFuture<'a, usize> {
        Box::pin(async move {
            let packet = self
                .received
                .lock()
                .await
                .recv()
                .await
                .ok_or_else(|| anyhow::anyhow!("UDP stream closed"))??;
            ensure!(packet.len() <= buffer.len(), "UDP receive buffer too small");
            buffer[..packet.len()].copy_from_slice(&packet);
            Ok(packet.len())
        })
    }
}
async fn read_packet<R: AsyncRead + Unpin>(reader: &mut R, frame: &Framing) -> Result<Vec<u8>> {
    // The generic path below uses the same bounded address parser as SOCKS.
    if matches!(frame, Framing::Trojan(_)) {
        let count = match reader.read_u8().await? {
            1 => 4,
            4 => 16,
            3 => reader.read_u8().await? as usize,
            _ => bail!("Invalid Trojan UDP address"),
        };
        reader.read_exact(&mut vec![0; count + 2]).await?;
    }
    if let Framing::Xudp(_) = frame {
        loop {
            let size = reader.read_u16().await? as usize;
            ensure!((4..=512).contains(&size), "Invalid XUDP header length");
            let mut header = vec![0; size];
            reader.read_exact(&mut header).await?;
            ensure!(header[..2] == [0, 0], "Unexpected XUDP session");
            ensure!(
                matches!(header[2], 2..=4) && header[3] & !3 == 0 && header[3] & 2 == 0,
                "XUDP session closed or invalid status"
            );
            ensure!(header[2] != 3, "XUDP session ended");
            if size > 4 {
                ensure!(size >= 8 && header[4] == 2, "Invalid XUDP address");
                let address = &header[5..];
                let length = match address[2] {
                    1 => 7,
                    3 => 19,
                    2 => {
                        ensure!(address.len() >= 4, "Invalid XUDP domain");
                        4 + address[3] as usize
                    }
                    _ => bail!("Invalid XUDP address family"),
                };
                ensure!(address.len() == length, "Invalid XUDP address length");
            }
            if header[3] & 1 == 0 {
                tokio::task::yield_now().await;
                continue;
            }
            let size = reader.read_u16().await? as usize;
            let mut packet = vec![0; size];
            reader.read_exact(&mut packet).await?;
            return Ok(packet);
        }
    }
    if let Framing::NativeChunks(max) = frame {
        let mut packet = vec![0; *max + 16];
        let size = reader.read(&mut packet).await?;
        ensure!(size > 0, "UDP stream closed");
        packet.truncate(size);
        return Ok(packet);
    }
    let len = reader.read_u16().await? as usize;
    if matches!(frame, Framing::Trojan(_)) {
        let mut crlf = [0; 2];
        reader.read_exact(&mut crlf).await?;
        ensure!(&crlf == b"\r\n", "Invalid Trojan UDP delimiter");
    }
    let mut packet = vec![0; len];
    reader.read_exact(&mut packet).await?;
    Ok(packet)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn xudp_preserves_payload_and_rejects_invalid_control_headers() {
        let framing = Framing::Xudp(vec![0, 53, 1, 127, 0, 0, 1]);
        let wire = [0, 4, 0, 0, 2, 1, 0, 3, 1, 2, 3];
        assert_eq!(
            read_packet(&mut &wire[..], &framing).await.unwrap(),
            vec![1, 2, 3]
        );
        for invalid in [
            &[0, 3, 0, 0, 2][..],
            &[0, 4, 0, 1, 2, 1, 0, 0],
            &[0, 4, 0, 0, 3, 0],
            &[0, 4, 0, 0, 2, 3],
            &[0, 4, 0, 0, 2, 1, 0, 2, 1],
        ] {
            assert!(read_packet(&mut &invalid[..], &framing).await.is_err());
        }
        let (client, mut peer) = tokio::io::duplex(64);
        let client = StreamDatagram::boxed(Box::new(client), framing);
        for status in [1, 2] {
            client.send(b"packet").await.unwrap();
            assert_eq!(peer.read_u16().await.unwrap(), 12);
            let mut header = [0; 12];
            peer.read_exact(&mut header).await.unwrap();
            assert_eq!(&header[..5], &[0, 0, status, 1, 2]);
            assert_eq!(&header[5..], &[0, 53, 1, 127, 0, 0, 1]);
            assert_eq!(peer.read_u16().await.unwrap(), 6);
            let mut payload = [0; 6];
            peer.read_exact(&mut payload).await.unwrap();
            assert_eq!(&payload, b"packet");
        }
    }

    #[tokio::test]
    async fn framed_protocols_preserve_messages_and_empty_payloads() {
        for framing in [
            Framing::Length16,
            Framing::Trojan(vec![1, 127, 0, 0, 1, 0, 53]),
        ] {
            let (client, server) = tokio::io::duplex(64);
            let client = StreamDatagram::boxed(Box::new(client), framing.clone());
            let server = StreamDatagram::boxed(Box::new(server), framing);
            for packet in [vec![], vec![0x41; 4096], b"next".to_vec()] {
                let mut buffer = vec![0; 65535];
                let (sent, received) = tokio::join!(client.send(&packet), server.recv(&mut buffer));
                sent.unwrap();
                let n = received.unwrap();
                assert_eq!(&buffer[..n], packet.as_slice());
                let (sent, received) = tokio::join!(server.send(&packet), client.recv(&mut buffer));
                sent.unwrap();
                let n = received.unwrap();
                assert_eq!(&buffer[..n], packet.as_slice());
            }
        }
    }
    #[tokio::test]
    async fn cancelled_recv_does_not_lose_partial_header() {
        let (client, mut server) = tokio::io::duplex(64);
        let client = StreamDatagram::boxed(Box::new(client), Framing::Length16);
        server.write_all(&[0]).await.unwrap();
        let mut buffer = [0; 64];
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(10),
                client.recv(&mut buffer)
            )
            .await
            .is_err()
        );
        server
            .write_all(&[5, b'h', b'e', b'l', b'l', b'o'])
            .await
            .unwrap();
        let n = client.recv(&mut buffer).await.unwrap();
        assert_eq!(&buffer[..n], b"hello");
    }
}
