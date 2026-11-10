//! Datagram adapter for Hysteria2 wire obfuscation and destination port hopping.
use crate::config::model::Hy2Obfs;
use anyhow::{Result, ensure};
use blake2::{Blake2b, Digest, digest::consts::U32};
use quinn::{
    AsyncUdpSocket, UdpPoller,
    udp::{RecvMeta, Transmit},
};
use rand::{Rng, RngCore};
use std::{
    collections::HashMap,
    fmt,
    future::Future,
    io::{self, IoSliceMut},
    net::SocketAddr,
    ops::RangeInclusive,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, ready},
    time::{Duration, Instant},
};
use tokio::{io::ReadBuf, net::UdpSocket};

#[derive(Clone)]
pub(super) struct Options {
    hash: Option<Blake2b<U32>>,
    gecko: Option<(usize, usize)>,
    ports: Vec<RangeInclusive<u16>>,
    interval: Duration,
    interval_max: Duration,
}
impl Options {
    pub fn new(
        obfs: Option<&Hy2Obfs>,
        ports: Option<&str>,
        interval: u64,
        interval_max: u64,
    ) -> Result<Option<Arc<Self>>> {
        if let Some(o) = obfs {
            o.validate()?;
        }
        ensure!(
            interval > 0 && interval_max >= interval && interval_max <= i64::MAX as u64 / 1_000_000,
            "Invalid Hysteria2 hop interval"
        );
        let mut ranges = Vec::new();
        if let Some(list) = ports {
            for item in list.split(',') {
                let (lo, hi) = item.split_once('-').unwrap_or((item, item));
                let (lo, hi) = (lo.parse::<u16>()?, hi.parse::<u16>()?);
                ensure!(lo > 0 && hi >= lo, "Invalid Hysteria2 port range");
                ranges.push(lo..=hi);
            }
        }
        if ranges.is_empty() && obfs.is_none() {
            return Ok(None);
        }
        ranges.sort_unstable_by_key(|r| *r.start());
        let mut merged: Vec<RangeInclusive<u16>> = Vec::new();
        for range in ranges {
            if let Some(last) = merged.last_mut()
                && *range.start() <= last.end().saturating_add(1)
            {
                *last = *last.start()..=(*last.end()).max(*range.end());
            } else {
                merged.push(range);
            }
        }
        Ok(Some(Arc::new(Self {
            hash: obfs.map(|o| Blake2b::new().chain_update(o.password.as_bytes())),
            gecko: obfs.filter(|o| o.kind == "gecko").map(Hy2Obfs::gecko_sizes),
            ports: merged,
            interval: Duration::from_millis(interval),
            interval_max: Duration::from_millis(interval_max),
        })))
    }
    fn next_port(&self, current: u16) -> u16 {
        let candidates: usize = self
            .ports
            .iter()
            .map(|r| usize::from(*r.end() - *r.start()) + 1 - usize::from(r.contains(&current)))
            .sum();
        if candidates == 0 {
            return current;
        }
        let mut index = rand::rng().random_range(0..candidates);
        for range in &self.ports {
            let skip = range.contains(&current);
            let count = usize::from(*range.end() - *range.start()) + 1 - usize::from(skip);
            if index < count {
                let port = *range.start() + index as u16;
                return port + u16::from(skip && port >= current);
            }
            index -= count;
        }
        unreachable!("candidate index is within the merged ranges")
    }
    pub fn endpoint(
        self: &Arc<Self>,
        bind: SocketAddr,
        peer: SocketAddr,
        config: quinn::EndpointConfig,
    ) -> Result<quinn::Endpoint> {
        let socket = std::net::UdpSocket::bind(bind)?;
        super::outbound_bind::apply_to_std_udp_socket(&socket, bind.is_ipv6())?;
        socket.set_nonblocking(true)?;
        let socket = Adapter {
            socket: Arc::new(UdpSocket::from_std(socket)?),
            options: self.clone(),
            peer,
            send: Mutex::new(SendState {
                wire: Vec::with_capacity(2048),
                message_id: rand::random(),
                port: peer.port(),
                last_hop: Instant::now(),
                next_hop: self.interval,
            }),
            recv: Mutex::new(RecvState {
                wire: vec![0; 65535],
                assemblies: HashMap::new(),
            }),
        };
        Ok(quinn::Endpoint::new_with_abstract_socket(
            config,
            None,
            Arc::new(socket),
            Arc::new(quinn::TokioRuntime),
        )?)
    }
}

struct SendState {
    wire: Vec<u8>,
    message_id: u8,
    port: u16,
    last_hop: Instant,
    next_hop: Duration,
}
struct Assembly {
    created: Instant,
    parts: Vec<Option<Vec<u8>>>,
}
struct RecvState {
    wire: Vec<u8>,
    assemblies: HashMap<u8, Assembly>,
}
struct Adapter {
    socket: Arc<UdpSocket>,
    options: Arc<Options>,
    peer: SocketAddr,
    send: Mutex<SendState>,
    recv: Mutex<RecvState>,
}

fn xor(hash: &Blake2b<U32>, salt: &[u8], payload: &mut [u8]) {
    let mask = hash.clone().chain_update(salt).finalize();
    for chunk in payload.chunks_mut(32) {
        for (byte, key) in chunk.iter_mut().zip(&mask) {
            *byte ^= key;
        }
    }
}
impl Adapter {
    fn send_frame(
        &self,
        wire: &mut Vec<u8>,
        payload: &[u8],
        dest: SocketAddr,
        fragment: Option<(u8, u8, u8, usize)>,
    ) -> io::Result<()> {
        let Some(hash) = &self.options.hash else {
            return self.socket.try_send_to(payload, dest).map(|_| ());
        };
        wire.clear();
        wire.extend(rand::random::<[u8; 8]>());
        if let Some((id, idx, total, pad)) = fragment {
            wire.extend([0x80, id, (idx << 4) | total]);
            wire.extend((pad as u16).to_be_bytes());
            wire.resize(13 + pad, 0);
            rand::rng().fill_bytes(&mut wire[13..]);
        }
        wire.extend_from_slice(payload);
        let (salt, data) = wire.split_at_mut(8);
        xor(hash, salt, data);
        self.socket.try_send_to(wire, dest).map(|_| ())
    }
}
impl fmt::Debug for Adapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Hysteria2 UDP socket")
    }
}
struct Writable {
    socket: Arc<UdpSocket>,
    wait: Option<Pin<Box<dyn Future<Output = io::Result<()>> + Send + Sync>>>,
}
impl fmt::Debug for Writable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("UDP writability poller")
    }
}
impl UdpPoller for Writable {
    fn poll_writable(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.wait.is_none() {
            let socket = self.socket.clone();
            self.wait = Some(Box::pin(async move { socket.writable().await }));
        }
        let result = ready!(self.wait.as_mut().unwrap().as_mut().poll(cx));
        self.wait = None;
        Poll::Ready(result)
    }
}
impl AsyncUdpSocket for Adapter {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(Writable {
            socket: self.socket.clone(),
            wait: None,
        })
    }
    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }
    fn try_send(&self, tx: &Transmit<'_>) -> io::Result<()> {
        let mut s = self.send.lock().unwrap();
        let mut dest = tx.destination;
        if !self.options.ports.is_empty() {
            if s.last_hop.elapsed() >= s.next_hop {
                s.port = self.options.next_port(s.port);
                s.last_hop = Instant::now();
                s.next_hop = Duration::from_millis(rand::rng().random_range(
                    self.options.interval.as_millis() as u64
                        ..=self.options.interval_max.as_millis() as u64,
                ));
            }
            dest.set_port(s.port);
        }
        if let Some((min, max)) = self.options.gecko
            && tx.contents.first().is_some_and(|b| b & 128 != 0)
        {
            let count = rand::rng().random_range(2..=8u8);
            s.message_id = s.message_id.wrapping_add(1);
            let id = s.message_id;
            let step = tx.contents.len() / count as usize;
            for i in 0..count {
                let end = if i + 1 == count {
                    tx.contents.len()
                } else {
                    (i as usize + 1) * step
                };
                let chunk = &tx.contents[i as usize * step..end];
                let base = 13 + chunk.len();
                let pad = if base >= max {
                    0
                } else {
                    rand::rng().random_range(min.max(base)..=max) - base
                };
                self.send_frame(&mut s.wire, chunk, dest, Some((id, i, count, pad)))?;
            }
            Ok(())
        } else {
            self.send_frame(&mut s.wire, tx.contents, dest, None)
        }
    }
    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        let mut recv = self.recv.lock().unwrap();
        let RecvState { wire, assemblies } = &mut *recv;
        for _ in 0..32 {
            let mut buf = ReadBuf::new(wire);
            let source = ready!(self.socket.poll_recv_from(cx, &mut buf))?;
            if source != self.peer
                && !(source.ip() == self.peer.ip()
                    && self
                        .options
                        .ports
                        .iter()
                        .any(|r| r.contains(&source.port())))
            {
                continue;
            }
            let bytes = buf.filled_mut();
            let packet = if let Some(hash) = &self.options.hash {
                if bytes.len() <= 8 {
                    continue;
                }
                let (salt, data) = bytes.split_at_mut(8);
                xor(hash, salt, data);
                data
            } else {
                bytes
            };
            let out = &mut bufs[0];
            let len =
                if self.options.gecko.is_some() && packet.first().is_some_and(|b| b & 128 != 0) {
                    if packet.len() < 5 || packet.len() > 2048 {
                        continue;
                    }
                    let (id, idx, total) = (
                        packet[1],
                        (packet[2] >> 4) as usize,
                        (packet[2] & 15) as usize,
                    );
                    let offset = 5 + u16::from_be_bytes([packet[3], packet[4]]) as usize;
                    if !(2..=8).contains(&total) || idx >= total || offset > packet.len() {
                        continue;
                    }
                    assemblies.retain(|_, a| a.created.elapsed() < Duration::from_secs(8));
                    if assemblies.len() >= 8 && !assemblies.contains_key(&id) {
                        continue;
                    }
                    let a = assemblies.entry(id).or_insert_with(|| Assembly {
                        created: Instant::now(),
                        parts: vec![None; total],
                    });
                    if a.parts.len() != total || a.parts[idx].is_some() {
                        continue;
                    }
                    a.parts[idx] = Some(packet[offset..].to_vec());
                    if a.parts.iter().any(Option::is_none) {
                        continue;
                    }
                    let parts = assemblies.remove(&id).unwrap().parts;
                    let len: usize = parts.iter().flatten().map(Vec::len).sum();
                    if len == 0 || len > out.len() {
                        continue;
                    }
                    let mut pos = 0;
                    for part in parts.into_iter().flatten() {
                        out[pos..pos + part.len()].copy_from_slice(&part);
                        pos += part.len();
                    }
                    len
                } else {
                    if packet.is_empty() || packet.len() > out.len() {
                        continue;
                    }
                    out[..packet.len()].copy_from_slice(packet);
                    packet.len()
                };
            meta[0] = RecvMeta {
                addr: self.peer,
                len,
                stride: len,
                ecn: None,
                dst_ip: None,
            };
            return Poll::Ready(Ok(1));
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}
