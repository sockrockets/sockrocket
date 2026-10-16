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
    Trojan(Vec<u8>),
    NativeChunks(usize),
}
pub(super) struct StreamDatagram {
    writer: Mutex<WriteHalf<BoxProxyStream>>,
    received: Mutex<mpsc::Receiver<Result<Vec<u8>>>>,
    task: tokio::task::JoinHandle<()>,
    framing: Framing,
    poisoned: AtomicBool,
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
