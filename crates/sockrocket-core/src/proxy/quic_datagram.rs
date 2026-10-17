//! Multiplexed, bounded QUIC datagram associations for Hysteria2 and TUIC v5.
use super::datagram::{BoxDatagram, Datagram, DatagramFuture};
use anyhow::{Result, bail, ensure};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU16, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
#[derive(Clone, Copy)]
pub(super) enum Protocol {
    Hysteria2,
    Tuic,
}
// Absorb QUIC bursts without letting a stalled LAN client monopolize RAM.
// 32 associations share at most 4 MiB of queued payload, plus reassembly.
const RECEIVE_QUEUE_PACKETS: usize = 64;
const RECEIVE_QUEUE_BYTES: usize = 128 * 1024;
struct QueuedDatagram {
    data: Vec<u8>,
    _bytes: OwnedSemaphorePermit,
}
struct Channel {
    tx: mpsc::Sender<QueuedDatagram>,
    bytes: Arc<Semaphore>,
    dropped: u64,
    assembly: Reassembler,
}
impl Channel {
    fn new() -> (Self, mpsc::Receiver<QueuedDatagram>) {
        let (tx, rx) = mpsc::channel(RECEIVE_QUEUE_PACKETS);
        (
            Self {
                tx,
                bytes: Arc::new(Semaphore::new(RECEIVE_QUEUE_BYTES)),
                dropped: 0,
                assembly: Reassembler {
                    pending: HashMap::new(),
                },
            },
            rx,
        )
    }
    fn deliver(&mut self, data: Vec<u8>) {
        let permit = self
            .bytes
            .clone()
            .try_acquire_many_owned(data.len().max(1) as u32);
        let accepted = match permit {
            Ok(permit) => self
                .tx
                .try_send(QueuedDatagram {
                    data,
                    _bytes: permit,
                })
                .is_ok(),
            Err(_) => false,
        };
        if !accepted && !self.tx.is_closed() {
            self.dropped = self.dropped.saturating_add(1);
            // Logarithmic reporting bounds log volume, with no destination,
            // client address, subscription or packet contents in diagnostics.
            if self.dropped.is_power_of_two() {
                tracing::warn!(
                    dropped_packets = self.dropped,
                    "QUIC UDP receive queue saturated; packets dropped"
                );
            }
        }
    }
}
type Registry = Arc<Mutex<HashMap<u32, Channel>>>;
pub(super) struct Hub {
    connection: quinn::Connection,
    protocol: Protocol,
    sessions: Registry,
    next: AtomicU32,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Hub {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Hub {
    pub fn new(connection: quinn::Connection, protocol: Protocol) -> Arc<Self> {
        let sessions: Registry = Arc::new(Mutex::new(HashMap::new()));
        let registry = sessions.clone();
        let conn = connection.clone();
        let task = tokio::spawn(async move {
            let mut batch = 0;
            while let Ok(packet) = conn.read_datagram().await {
                if let Ok(fragment) = decode(protocol, &packet) {
                    let mut channels = registry.lock().unwrap();
                    if let Some(channel) = channels.get_mut(&fragment.session) {
                        if let Ok(Some(packet)) = channel.assembly.assemble(fragment) {
                            channel.deliver(packet);
                        }
                    }
                }
                batch += 1;
                if batch == 16 {
                    batch = 0;
                    tokio::task::yield_now().await;
                }
            }
            registry.lock().unwrap().clear();
        });
        Arc::new(Self {
            connection,
            protocol,
            sessions,
            next: AtomicU32::new(rand::random()),
            task,
        })
    }
    pub fn matches(&self, connection: &quinn::Connection) -> bool {
        self.connection.stable_id() == connection.stable_id()
            && self.connection.close_reason().is_none()
    }
    pub fn associate(self: &Arc<Self>, host: &str, port: u16) -> Result<BoxDatagram> {
        ensure!(
            self.connection.max_datagram_size().is_some(),
            "Server did not enable QUIC datagrams"
        );
        let address = address(self.protocol, host, port)?;
        let (channel, rx) = Channel::new();
        let mut sessions = self.sessions.lock().unwrap();
        ensure!(sessions.len() < 32, "QUIC UDP session limit reached");
        let id = loop {
            let mut id = self.next.fetch_add(1, Ordering::Relaxed);
            if matches!(self.protocol, Protocol::Tuic) {
                id &= 0xffff;
            }
            if !sessions.contains_key(&id) {
                break id;
            }
        };
        sessions.insert(id, channel);
        Ok(Box::new(Session {
            hub: self.clone(),
            id,
            address,
            packet: AtomicU16::new(rand::random()),
            received: tokio::sync::Mutex::new(rx),
        }))
    }
}
struct Session {
    hub: Arc<Hub>,
    id: u32,
    address: Vec<u8>,
    packet: AtomicU16,
    received: tokio::sync::Mutex<mpsc::Receiver<QueuedDatagram>>,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.hub.sessions.lock().unwrap().remove(&self.id);
        if matches!(self.hub.protocol, Protocol::Tuic) {
            // Dissociate is a unidirectional command, not a QUIC datagram.
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let connection = self.hub.connection.clone();
                let id = self.id as u16;
                runtime.spawn(async move {
                    let _ = tokio::time::timeout(Duration::from_secs(1), async move {
                        let mut stream = connection.open_uni().await?;
                        let mut command = vec![5, 3];
                        command.extend(id.to_be_bytes());
                        stream.write_all(&command).await?;
                        stream.finish()?;
                        Ok::<_, anyhow::Error>(())
                    })
                    .await;
                });
            }
        }
    }
}
struct Assembly {
    created: Instant,
    total: u8,
    bytes: usize,
    parts: Vec<Option<Vec<u8>>>,
}
struct Reassembler {
    pending: HashMap<u16, Assembly>,
}
impl Reassembler {
    fn assemble(&mut self, fragment: Fragment<'_>) -> Result<Option<Vec<u8>>> {
        self.pending
            .retain(|_, value| value.created.elapsed() < Duration::from_secs(5));
        if fragment.total == 1 {
            return Ok(Some(fragment.data.to_vec()));
        }
        if self.pending.len() >= 2 && !self.pending.contains_key(&fragment.packet) {
            self.pending.clear();
        }
        let item = self
            .pending
            .entry(fragment.packet)
            .or_insert_with(|| Assembly {
                created: Instant::now(),
                total: fragment.total,
                bytes: 0,
                parts: vec![None; fragment.total as usize],
            });
        ensure!(item.total == fragment.total, "UDP fragment count changed");
        if item.parts[fragment.index as usize].is_none() {
            item.bytes += fragment.data.len();
            ensure!(item.bytes <= 65535, "Oversized UDP fragments");
            item.parts[fragment.index as usize] = Some(fragment.data.to_vec());
        }
        if item.parts.iter().all(Option::is_some) {
            let item = self.pending.remove(&fragment.packet).unwrap();
            return Ok(Some(item.parts.into_iter().flatten().flatten().collect()));
        }
        Ok(None)
    }
}
impl Datagram for Session {
    fn send<'a>(&'a self, packet: &'a [u8]) -> DatagramFuture<'a, ()> {
        Box::pin(async move {
            ensure!(packet.len() <= 65535, "UDP payload too large");
            let max = self
                .hub
                .connection
                .max_datagram_size()
                .ok_or_else(|| anyhow::anyhow!("UDP unavailable"))?;
            let overhead = header(self.hub.protocol, self.id, 0, 0, 1, &self.address, 0).len();
            ensure!(max > overhead, "UDP address exceeds QUIC MTU");
            let chunk = max - overhead;
            let total = packet.len().max(1).div_ceil(chunk);
            ensure!(total <= 255, "Too many UDP fragments");
            let id = self.packet.fetch_add(1, Ordering::Relaxed);
            for index in 0..total {
                let start = (index * chunk).min(packet.len());
                let end = (start + chunk).min(packet.len());
                let data = &packet[start..end];
                let mut frame = header(
                    self.hub.protocol,
                    self.id,
                    id,
                    index as u8,
                    total as u8,
                    &self.address,
                    data.len(),
                );
                frame.extend_from_slice(data);
                self.hub.connection.send_datagram(frame.into())?;
            }
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
                .ok_or_else(|| anyhow::anyhow!("UDP association closed"))?;
            ensure!(
                packet.data.len() <= buffer.len(),
                "UDP receive buffer too small"
            );
            buffer[..packet.data.len()].copy_from_slice(&packet.data);
            Ok(packet.data.len())
        })
    }
}
fn address(protocol: Protocol, host: &str, port: u16) -> Result<Vec<u8>> {
    if matches!(protocol, Protocol::Hysteria2) {
        let address = super::transport::format_host_port(host, port).into_bytes();
        ensure!(address.len() < 16384, "UDP address too long");
        let len = quinn::VarInt::from_u32(address.len() as u32);
        let mut data = Vec::new();
        put_varint(&mut data, len.into_inner());
        data.extend(address);
        return Ok(data);
    }
    let mut data = Vec::new();
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => {
            data.push(1);
            data.extend(ip.octets());
        }
        Ok(std::net::IpAddr::V6(ip)) => {
            data.push(2);
            data.extend(ip.octets());
        }
        Err(_) => {
            ensure!(host.len() <= 255, "UDP domain too long");
            data.extend([0, host.len() as u8]);
            data.extend(host.as_bytes());
        }
    }
    data.extend(port.to_be_bytes());
    Ok(data)
}
fn put_varint(out: &mut Vec<u8>, value: u64) {
    if value < 64 {
        out.push(value as u8);
    } else {
        out.extend(((value as u16) | 0x4000).to_be_bytes());
    }
}
fn header(
    protocol: Protocol,
    session: u32,
    packet: u16,
    index: u8,
    total: u8,
    address: &[u8],
    len: usize,
) -> Vec<u8> {
    let mut frame = Vec::new();
    match protocol {
        Protocol::Hysteria2 => frame.extend(session.to_be_bytes()),
        Protocol::Tuic => {
            frame.extend([5, 2]);
            frame.extend((session as u16).to_be_bytes());
        }
    }
    frame.extend(packet.to_be_bytes());
    match protocol {
        Protocol::Hysteria2 => frame.extend([index, total]),
        Protocol::Tuic => {
            frame.extend([total, index]);
            frame.extend((len as u16).to_be_bytes());
        }
    }
    frame.extend(address);
    frame
}
struct Fragment<'a> {
    session: u32,
    packet: u16,
    index: u8,
    total: u8,
    data: &'a [u8],
}
fn decode(protocol: Protocol, frame: &[u8]) -> Result<Fragment<'_>> {
    ensure!(frame.len() >= 9, "Truncated UDP header");
    let session = match protocol {
        Protocol::Hysteria2 => u32::from_be_bytes(frame[..4].try_into()?),
        Protocol::Tuic => {
            ensure!(frame[..2] == [5, 2], "Unexpected TUIC command");
            u16::from_be_bytes(frame[2..4].try_into()?) as u32
        }
    };
    let packet = u16::from_be_bytes(frame[4..6].try_into()?);
    let (index, total, pos, expected) = match protocol {
        Protocol::Hysteria2 => {
            let n = 1usize << (frame[8] >> 6);
            ensure!(8 + n <= frame.len(), "Truncated address length");
            let mut len = u64::from(frame[8] & 63);
            for b in &frame[9..8 + n] {
                len = (len << 8) | u64::from(*b);
            }
            ensure!(len <= 65535, "Oversized address");
            (frame[6], frame[7], 8 + n + len as usize, None)
        }
        Protocol::Tuic => {
            ensure!(frame.len() >= 11, "Truncated TUIC header");
            let len = u16::from_be_bytes(frame[8..10].try_into()?) as usize;
            let end = match frame[10] {
                0 => {
                    ensure!(frame.len() >= 12, "Missing domain length");
                    12 + frame[11] as usize + 2
                }
                1 => 17,
                2 => 29,
                _ => bail!("Invalid TUIC address"),
            };
            (frame[7], frame[6], end, Some(len))
        }
    };
    ensure!(
        total > 0 && index < total && pos <= frame.len(),
        "Invalid UDP fragment"
    );
    let data = &frame[pos..];
    ensure!(
        data.len() <= 65535 && expected.is_none_or(|len| len == data.len()),
        "Invalid UDP payload length"
    );
    Ok(Fragment {
        session,
        packet,
        index,
        total,
        data,
    })
}
