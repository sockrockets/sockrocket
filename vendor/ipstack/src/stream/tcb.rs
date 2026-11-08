use super::seqnum::SeqNum;
use etherparse::TcpHeader;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::{collections::BTreeMap, time::Duration};

pub(super) const MAX_UNACK: u32 = 1024 * 16; // 16KB
pub(super) const READ_BUFFER_SIZE: usize = 1024 * 16; // 16KB
pub(super) const MAX_COUNT_FOR_DUP_ACK: usize = 3; // Maximum number of duplicate ACKs before retransmission

const MAX_READ_CHUNKS: usize = 64;
const MAX_REASSEMBLY_ENTRIES: usize = 64;
const MAX_REASSEMBLY_CHUNK: usize = 16 * 1024;

/// Retransmission timeout
pub(super) const RTO: std::time::Duration = std::time::Duration::from_secs(1);

/// Six backed-off retries allow ~123 seconds without an ACK at the default
/// one-second RTO, rather than treating a short blackout as a dead connection.
pub(super) const MAX_RETRANSMIT_COUNT: usize = 6;

#[derive(Debug)]
struct PersistTimer {
    deadline: tokio::time::Instant,
    interval: Duration,
    unanswered: usize,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) enum TcpState {
    // Init, /* Since we always act as a server, it starts from `Listen`, so we don't use states Init & SynSent. */
    // SynSent,
    Listen,
    SynReceived,
    Established,
    FinWait1, // act as a client, actively send a farewell packet to the other side, followed with FinWait2, TimeWait, Closed
    FinWait2,
    Closing, // peer FIN received before our FIN is acknowledged
    TimeWait,
    CloseWait, // act as a server, followed with LastAck, Closed
    LastAck,
    Closed,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub(super) enum PacketType {
    WindowUpdate,
    Invalid,
    RetransmissionRequest,
    NewPacket,
    Ack,
    KeepAlive,
}

/// TCP Control Block
/// - `inflight_packets` is prerepresented bytes stream from upstream application,
///   which have been sent to the lower device but not yet acknowledged.
/// - `unordered_packets` is the bytes stream received from the lower device,
///   which can be acknowledged and extracted by `consume_unordered_packets` method
///   then can be read by upstream application via `Tcp::poll_read` method.
#[derive(Debug)]
pub(crate) struct Tcb {
    pub(crate) payload_budget: Arc<crate::TcpPayloadBudget>,
    read_reserved: usize,
    send_reserved: usize,
    seq: SeqNum,
    ack: SeqNum,
    mtu: u16,
    /// Receive MSS advertised by the peer, independent of the local TUN MTU.
    send_mss: u16,
    last_received_ack: SeqNum,
    /// Peer's receive window in bytes. A raw TCP header only holds 16 bits;
    /// when window scaling was negotiated this is `header << snd_scale`.
    send_window: u32,
    /// Shift from the peer's SYN. Zero when scaling was not negotiated.
    snd_scale: u8,
    /// Shift we advertise. Zero when scaling was not negotiated, so the
    /// header value stays the real byte window.
    rcv_scale: u8,
    scale_ok: bool,
    /// Live byte cap shared by every flow. Tun lowers it as more TCP
    /// sessions appear so a speed test can use a wide window without
    /// reserving that width for every idle flow.
    window_limit: Arc<AtomicU32>,
    state: TcpState,
    state_since: std::time::Instant,
    inflight_packets: BTreeMap<SeqNum, InflightPacket>,
    unordered_packets: BTreeMap<SeqNum, Vec<u8>>,
    duplicate_ack_count: usize,
    duplicate_ack_count_helper: SeqNum,
    max_unacked_bytes: u32,
    read_buffer_size: usize,
    queued_read_bytes: usize,
    queued_read_chunks: usize,
    max_count_for_dup_ack: usize,
    rto: std::time::Duration,
    current_rto: Duration,
    retransmit_deadline: Option<tokio::time::Instant>,
    smoothed_rtt: Option<Duration>,
    rtt_variance: Duration,
    rtt_probe: Option<(SeqNum, tokio::time::Instant)>,
    persist: Option<PersistTimer>,
    max_retransmit_count: usize,
}

impl Tcb {
    pub(super) fn new(
        ack: SeqNum,
        mtu: u16,
        max_unacked_bytes: u32,
        read_buffer_size: usize,
        max_count_for_dup_ack: usize,
        rto: std::time::Duration,
        max_retransmit_count: usize,
        window_limit: Arc<AtomicU32>,
        payload_budget: Arc<crate::TcpPayloadBudget>,
    ) -> Tcb {
        #[cfg(debug_assertions)]
        let seq = 100;
        #[cfg(not(debug_assertions))]
        let seq = rand::Rng::random::<u32>(&mut rand::rng());
        Tcb {
            payload_budget,
            read_reserved: 0,
            send_reserved: 0,
            seq: seq.into(),
            ack,
            mtu,
            // IpStackTcpStream installs the SYN option/family default before
            // exposing this control block. Unit fixtures may use the MTU alone.
            send_mss: u16::MAX,
            last_received_ack: seq.into(),
            // Until the peer's ACK arrives, allow one budget's worth. The
            // handshake ACK replaces this with the scaled header window.
            send_window: max_unacked_bytes,
            snd_scale: 0,
            rcv_scale: 0,
            scale_ok: false,
            window_limit,
            state: TcpState::Listen,
            state_since: std::time::Instant::now(),
            inflight_packets: BTreeMap::new(),
            unordered_packets: BTreeMap::new(),
            duplicate_ack_count: 0,
            duplicate_ack_count_helper: seq.into(),
            max_unacked_bytes,
            read_buffer_size,
            queued_read_bytes: 0,
            queued_read_chunks: 0,
            max_count_for_dup_ack,
            rto,
            current_rto: rto,
            retransmit_deadline: None,
            smoothed_rtt: None,
            rtt_variance: Duration::ZERO,
            rtt_probe: None,
            persist: None,
            max_retransmit_count,
        }
    }

    pub fn calculate_payload_max_len(
        &self,
        ip_header_size: usize,
        tcp_header_size: usize,
    ) -> usize {
        // The advertised window includes data already in flight. Only its
        // remaining suffix is available to a new write, also bounded by the
        // live per-flow allowance when concurrency reduces that allowance.
        let outstanding = self.seq.distance(self.get_last_received_ack()) as usize;
        let send_window = self
            .buffer_limit()
            .min(self.get_send_window() as usize)
            .saturating_sub(outstanding);
        send_window.min(self.max_segment_payload_len(ip_header_size, tcp_header_size))
    }

    pub(super) fn set_peer_mss(&mut self, mss: u16) {
        self.send_mss = mss.max(1);
    }

    pub(super) fn max_segment_payload_len(
        &self,
        ip_header_size: usize,
        tcp_header_size: usize,
    ) -> usize {
        // MSS excludes the fixed IP/TCP headers; TCP options consume part of
        // that allowance (RFC 9293 3.7.1). Outbound IP headers have no options.
        let mss = usize::from(self.send_mss).saturating_sub(tcp_header_size.saturating_sub(20));
        mss.min(usize::from(self.mtu).saturating_sub(ip_header_size + tcp_header_size))
    }

    pub fn update_duplicate_ack_count(&mut self, rcvd_ack: SeqNum, unchanged_pure_ack: bool) {
        if rcvd_ack < self.last_received_ack {
            return;
        }
        if unchanged_pure_ack
            && rcvd_ack == self.last_received_ack
            && rcvd_ack == self.duplicate_ack_count_helper
            && rcvd_ack < self.seq
        {
            self.duplicate_ack_count = self.duplicate_ack_count.saturating_add(1);
        } else {
            self.duplicate_ack_count_helper = rcvd_ack;
            self.duplicate_ack_count = 0; // reset duplicate ACK count
        }
    }

    pub fn is_duplicate_ack_count_exceeded(&self) -> bool {
        self.duplicate_ack_count == self.max_count_for_dup_ack
    }

    pub(super) fn add_unordered_packet(&mut self, mut seq: SeqNum, mut buf: Vec<u8>) {
        if seq < self.ack {
            // A retransmission may cover both acknowledged and new bytes.
            let consumed = self.ack.distance(seq) as usize;
            if consumed >= buf.len() {
                return;
            }
            buf = into_tail(buf, consumed);
            seq = self.ack;
        }
        if buf.is_empty() {
            return;
        }
        let old = self.unordered_packets.get(&seq).map_or(0, Vec::len);
        if old >= buf.len() {
            return;
        }
        let mut overlap = old;
        let mut available = self.get_available_read_buffer_size();
        let mut append_to = None;
        if seq != self.ack {
            // Leave per-flow room for the segment that closes the leading gap.
            let first = self
                .unordered_packets
                .first_key_value()
                .map_or(seq, |(&key, _)| key.min(seq));
            available = available
                .saturating_sub(first.distance(self.ack).min(u32::from(self.mtu)) as usize);
            if old == 0 {
                // A burst following one lost packet should consume one bounded
                // chunk per run of bytes, not one map entry per wire packet.
                if let Some((&start, bytes)) = self.unordered_packets.range_mut(..seq).next_back() {
                    let offset = seq.distance(start) as usize;
                    if offset <= bytes.len() {
                        let covered = bytes.len() - offset;
                        if covered >= buf.len() {
                            return;
                        }
                        if offset + buf.len() <= MAX_REASSEMBLY_CHUNK {
                            overlap = covered;
                            append_to = Some(bytes);
                        }
                    }
                }
            }
        }
        let increase = buf.len() - overlap;
        if increase > available {
            return;
        }
        let Some(credit) = self.payload_budget.reserve(increase) else {
            return;
        };
        if let Some(bytes) = append_to {
            bytes.extend_from_slice(&buf[overlap..]);
        } else {
            if seq != self.ack && old == 0 && self.unordered_packets.len() >= MAX_REASSEMBLY_ENTRIES
            {
                return;
            }
            self.unordered_packets.insert(seq, buf);
        }
        credit.commit(increase);
        self.read_reserved += increase;
    }
    pub(super) fn get_available_read_buffer_size(&self) -> usize {
        if self.queued_read_chunks >= MAX_READ_CHUNKS {
            return 0;
        }
        self.buffer_limit()
            .saturating_sub(self.get_unordered_packets_total_len())
            .saturating_sub(self.queued_read_bytes)
    }

    /// Bytes this flow may keep in flight right now. The atomic is the
    /// shared live cap; the config fields are the per-flow ceiling.
    fn buffer_limit(&self) -> usize {
        let live = self.window_limit.load(Ordering::Relaxed) as usize;
        live.min(self.read_buffer_size)
            .min(self.max_unacked_bytes as usize)
    }
    #[inline]
    pub(crate) fn get_unordered_packets_total_len(&self) -> usize {
        self.unordered_packets.values().map(|p| p.len()).sum()
    }

    pub(super) fn consume_unordered_packets(&mut self, max_bytes: usize) -> Option<Vec<u8>> {
        let mut data = Vec::new();
        let mut remaining_bytes = max_bytes;

        while remaining_bytes > 0 {
            if let Some(seq) = self.unordered_packets.keys().next().copied() {
                if seq > self.ack {
                    break; // sequence number is not continuous, stop extracting
                }

                // remove and get the first packet
                let mut payload = self.unordered_packets.remove(&seq).unwrap();
                // Overlapping out-of-order segments can become partly or fully
                // acknowledged when a preceding segment fills the gap.
                let duplicate = (self.ack.distance(seq) as usize).min(payload.len());
                if duplicate > 0 {
                    self.payload_budget.release(duplicate);
                    self.read_reserved -= duplicate;
                    payload = into_tail(payload, duplicate);
                }
                let payload_len = payload.len();

                if payload_len <= remaining_bytes {
                    // current packet can be fully extracted
                    if data.is_empty() {
                        data = payload;
                    } else {
                        data.extend(payload);
                    }
                    self.ack += payload_len as u32;
                    remaining_bytes -= payload_len;
                } else {
                    // current packet can only be partially extracted
                    let mut remaining_payload = payload.split_off(remaining_bytes);
                    if data.is_empty() {
                        data = payload;
                    } else {
                        data.extend_from_slice(&payload);
                    }
                    self.ack += remaining_bytes as u32;
                    // Splitting may land on another segment's starting sequence.
                    // Keep the longer tail and release only duplicate bytes.
                    if let Some(overlap) = self.unordered_packets.remove(&self.ack) {
                        let duplicate = overlap.len().min(remaining_payload.len());
                        if overlap.len() > remaining_payload.len() {
                            remaining_payload = overlap;
                        }
                        self.payload_budget.release(duplicate);
                        self.read_reserved -= duplicate;
                    }
                    self.unordered_packets.insert(self.ack, remaining_payload);
                    break;
                }
            } else {
                break; // no more packets to extract
            }
        }

        if data.is_empty() { None } else { Some(data) }
    }

    pub(super) fn can_queue_read(&self) -> bool {
        self.queued_read_chunks < MAX_READ_CHUNKS
    }
    #[cfg(test)]
    pub(super) fn queued_read_chunks(&self) -> usize {
        self.queued_read_chunks
    }
    pub(super) fn queue_read(&mut self, n: usize) {
        self.queued_read_bytes += n;
        self.queued_read_chunks += 1;
    }
    pub(super) fn consume_read(&mut self, n: usize, completed_chunks: usize) {
        let consumed = n.min(self.queued_read_bytes);
        self.payload_budget.release(consumed);
        self.read_reserved -= consumed;
        self.queued_read_bytes = self.queued_read_bytes.saturating_sub(n);
        self.queued_read_chunks = self.queued_read_chunks.saturating_sub(completed_chunks);
    }
    pub(super) fn increase_seq(&mut self) {
        self.seq += 1;
    }
    pub(super) fn get_seq(&self) -> SeqNum {
        self.seq
    }
    pub(super) fn increase_ack(&mut self) {
        self.ack += 1;
    }
    pub(super) fn get_ack(&self) -> SeqNum {
        self.ack
    }
    pub(super) fn get_mtu(&self) -> u16 {
        self.mtu
    }
    pub(super) fn get_last_received_ack(&self) -> SeqNum {
        self.last_received_ack
    }
    pub(super) fn change_state(&mut self, state: TcpState) {
        if self.state != state {
            self.state_since = std::time::Instant::now();
        }
        self.state = state;
    }
    pub(crate) fn health(&self) -> (usize, u64, usize) {
        let closing = matches!(
            self.state,
            TcpState::FinWait1
                | TcpState::FinWait2
                | TcpState::Closing
                | TcpState::CloseWait
                | TcpState::LastAck
                | TcpState::TimeWait
        );
        (
            self.state as usize,
            if closing {
                self.state_since.elapsed().as_secs()
            } else {
                0
            },
            self.queued_read_bytes + self.get_unordered_packets_total_len(),
        )
    }
    pub(super) fn get_state(&self) -> TcpState {
        self.state
    }
    pub(super) fn enable_window_scale(&mut self, peer: u8, ours: u8) {
        self.scale_ok = true;
        self.snd_scale = peer.min(14);
        self.rcv_scale = ours.min(14);
    }

    /// Scale to put on our SYN-ACK. `None` when the peer did not offer it —
    /// sending the option only on our side would leave the two shifts
    /// disagreeing about the same header field.
    pub(super) fn advertised_scale(&self) -> Option<u8> {
        self.scale_ok.then_some(self.rcv_scale)
    }

    pub(super) fn update_send_window(&mut self, window: u16) {
        let was_closed = self.send_window == 0;
        self.send_window = Self::scale_window(window, self.snd_scale);
        if self.send_window == 0 {
            // Receiver flow control is not a network RTT measurement.
            self.rtt_probe = None;
            if let Some(persist) = self.persist.as_mut() {
                // A valid ACK, even with a closed window, proves peer liveness.
                persist.unanswered = 0;
            } else if !self.inflight_packets.is_empty() {
                self.ensure_persist_timer();
            }
        } else {
            self.persist = None;
        }
        if was_closed && self.send_window > 0 {
            self.restart_retransmission_timer();
        }
    }
    pub(super) fn get_send_window(&self) -> u32 {
        self.send_window
    }
    pub(super) fn scaled_peer_window(&self, window: u16) -> u32 {
        Self::scale_window(window, self.snd_scale)
    }
    pub(super) fn get_recv_window(&self) -> u16 {
        u16::try_from(self.recv_window_bytes() >> self.rcv_scale).unwrap_or(u16::MAX)
    }
    pub(super) fn recv_window_bytes(&self) -> usize {
        // The reader may lag behind packet ingress and the outbound socket.
        // Bound the advertised burst by both queues, while retaining the
        // larger backing buffer for reordered and already-in-flight data.
        let queued = self.queued_read_bytes;
        let ingress = crate::queue::TCP_INGRESS_PAYLOAD_WINDOW.saturating_sub(queued);
        let slots = MAX_READ_CHUNKS.saturating_sub(self.queued_read_chunks);
        let per_segment = usize::from(self.mtu.saturating_sub(60).max(1));
        let mut bytes = self
            .get_available_read_buffer_size()
            .min(ingress)
            .min(slots * per_segment);
        if self.unordered_packets.len() >= MAX_REASSEMBLY_ENTRIES
            && let Some((&first, _)) = self.unordered_packets.first_key_value()
            && first > self.ack
        {
            // At the metadata limit only promise space to repair the first
            // hole. Keep a sub-scale-unit hole visible on the wire.
            let gap = first.distance(self.ack) as usize;
            let unit = 1usize << self.rcv_scale;
            bytes = bytes.min(gap.div_ceil(unit) * unit);
        }
        bytes
    }

    fn scale_window(window: u16, shift: u8) -> u32 {
        u32::from(window)
            .checked_shl(u32::from(shift.min(14)))
            .unwrap_or(u32::MAX)
    }
    // #[inline(always)]
    // pub(super) fn buffer_size(&self, payload_len: u16) -> u16 {
    //     match MAX_UNACK - self.inflight_packets.len() as u32 {
    //         // b if b.saturating_sub(payload_len as u32 + 64) != 0 => payload_len,
    //         // b if b < 128 && b >= 4 => (b / 2) as u16,
    //         // b if b < 4 => b as u16,
    //         // b => (b - 64) as u16,
    //         b if b >= payload_len as u32 * 2 && b > 0 => payload_len,
    //         b if b < 4 => b as u16,
    //         b => (b / 2) as u16,
    //     }
    // }

    pub(super) fn check_pkt_type(&self, tcp_header: &TcpHeader, payload: &[u8]) -> PacketType {
        let rcvd_ack = SeqNum(tcp_header.acknowledgment_number);
        let rcvd_seq = SeqNum(tcp_header.sequence_number);
        let rcvd_window = Self::scale_window(tcp_header.window_size, self.snd_scale);
        let len = payload.len();
        let res = if rcvd_ack > self.seq {
            PacketType::Invalid
        } else {
            match rcvd_ack.cmp(&self.get_last_received_ack()) {
                std::cmp::Ordering::Less => PacketType::Invalid,
                std::cmp::Ordering::Equal => {
                    if self.ack - 1 == rcvd_seq && payload.len() <= 1 {
                        PacketType::KeepAlive
                    } else if !payload.is_empty() {
                        PacketType::NewPacket
                    } else if self.get_send_window() == rcvd_window
                        && self.seq != rcvd_ack
                        && self.is_duplicate_ack_count_exceeded()
                    {
                        PacketType::RetransmissionRequest
                    } else {
                        PacketType::WindowUpdate
                    }
                }
                std::cmp::Ordering::Greater => {
                    if payload.is_empty() {
                        PacketType::Ack
                    } else {
                        PacketType::NewPacket
                    }
                }
            }
        };
        #[rustfmt::skip]
        log::trace!("received {{ ack = {:08X?}, seq = {:08X?}, window = {rcvd_window} }}, self {{ ack = {:08X?}, seq = {:08X?}, send_window = {} }}, len = {len}, {res:?}", rcvd_ack.0, rcvd_seq.0, self.ack.0, self.seq.0, self.get_send_window());
        res
    }

    #[cfg(test)]
    pub(super) fn add_inflight_packet(&mut self, buf: Vec<u8>) -> std::io::Result<()> {
        let credit = self
            .payload_budget
            .reserve(buf.len())
            .ok_or(std::io::ErrorKind::WouldBlock)?;
        self.add_reserved_inflight_packet(buf.into(), credit)
    }
    pub(crate) fn add_reserved_inflight_packet(
        &mut self,
        buf: bytes::Bytes,
        credit: crate::tcp_budget::Reservation,
    ) -> std::io::Result<()> {
        if buf.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Empty payload",
            ));
        }
        let buf_len = buf.len() as u32;
        let was_empty = self.inflight_packets.is_empty();
        if let Some(old) = self
            .inflight_packets
            .insert(self.seq, InflightPacket::new(self.seq, buf))
        {
            self.send_reserved -= old.payload.len();
            self.payload_budget.release(old.payload.len());
        }
        credit.commit(buf_len as usize);
        self.send_reserved += buf_len as usize;
        self.seq += buf_len;
        if self.rtt_probe.is_none() {
            self.rtt_probe = Some((self.seq, tokio::time::Instant::now()));
        }
        if was_empty {
            self.restart_retransmission_timer();
        }
        Ok(())
    }

    pub(super) fn update_last_received_ack(&mut self, ack: SeqNum) {
        self.last_received_ack = ack;
    }

    pub(crate) fn update_inflight_packet_queue(&mut self, ack: SeqNum) {
        // Defend the queue independently of packet classification: an ACK
        // cannot release credit for bytes that have never been transmitted.
        if ack > self.seq || ack < self.last_received_ack {
            return;
        }
        // Cumulative ACKs only remove a prefix. Do not scan every outstanding
        // packet (twice for byte sums plus retain) on every LAN ACK.
        let mut released = 0;
        while let Some(entry) = self.inflight_packets.first_entry() {
            if ack <= *entry.key() {
                break;
            }
            let distance = ack.distance(entry.get().seq) as usize;
            let (_, mut inflight_packet) = entry.remove_entry();
            let len = inflight_packet.payload.len();
            if distance < len {
                // `drain` would keep the already-acknowledged prefix's
                // allocation until the rest of the segment is ACKed.
                let payload = std::mem::take(&mut inflight_packet.payload);
                inflight_packet.payload = bytes::Bytes::copy_from_slice(&payload[distance..]);
                inflight_packet.seq = ack;
                self.inflight_packets.insert(ack, inflight_packet);
                released += distance;
                break;
            }
            released += len;
        }
        self.send_reserved -= released;
        self.payload_budget.release(released);
        if released > 0 {
            if let Some(mut oldest) = self.inflight_packets.first_entry() {
                // Failure counts consecutive timeouts without ACK progress,
                // not the lifetime of a segment that may be partly delivered.
                oldest.get_mut().retransmit_count = 0;
            }
            if let Some((end, sent)) = self.rtt_probe
                && ack >= end
            {
                self.rtt_probe = None;
                self.record_rtt(sent.elapsed());
            }
            // RFC 6298 section 5.3: newly acknowledged bytes restart the
            // timer. Duplicate/stale ACKs must not keep a dead flow alive.
            self.restart_retransmission_timer();
            if self.inflight_packets.is_empty() {
                // A blocked writer is woken by the ACK path and can re-arm
                // persistence. An otherwise idle stream needs no probes.
                self.persist = None;
            }
        }
    }

    fn restart_retransmission_timer(&mut self) {
        self.retransmit_deadline = (!self.inflight_packets.is_empty())
            .then(|| tokio::time::Instant::now() + self.current_rto);
    }

    fn record_rtt(&mut self, sample: Duration) {
        // RFC 6298 sections 2-3. Retransmission invalidates the active RTT
        // probe (Karn's rule); only fresh data supplies a timing sample.
        let smoothed = match self.smoothed_rtt {
            Some(previous) => {
                let difference = previous.abs_diff(sample);
                self.rtt_variance = self.rtt_variance * 3 / 4 + difference / 4;
                previous * 7 / 8 + sample / 8
            }
            None => {
                self.rtt_variance = sample / 2;
                sample
            }
        };
        self.smoothed_rtt = Some(smoothed);
        self.current_rto = smoothed
            .saturating_add((self.rtt_variance * 4).max(Duration::from_millis(1)))
            .clamp(self.rto, Duration::from_secs(60).max(self.rto));
    }

    pub(crate) fn find_inflight_packet(&self, seq: SeqNum) -> Option<&InflightPacket> {
        self.inflight_packets.get(&seq)
    }

    pub(crate) fn ensure_persist_timer(&mut self) -> bool {
        if self.send_window != 0 || self.persist.is_some() {
            return false;
        }
        self.persist = Some(PersistTimer {
            deadline: tokio::time::Instant::now() + self.current_rto,
            interval: self.current_rto,
            unanswered: 0,
        });
        true
    }

    pub(crate) fn window_probe_due(&self) -> std::io::Result<bool> {
        let Some(persist) = &self.persist else {
            return Ok(false);
        };
        if self.state == TcpState::Closed
            || self.send_window != 0
            || persist.deadline > tokio::time::Instant::now()
        {
            return Ok(false);
        }
        if persist.unanswered >= self.max_retransmit_count {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "TCP zero-window probes unanswered",
            ));
        }
        Ok(true)
    }

    pub(crate) fn window_probe_packet(&self) -> Option<InflightPacket> {
        self.inflight_packets
            .first_key_value()
            .map(|(&seq, packet)| {
                // Re-send one owned byte, without advancing sequence or credit.
                InflightPacket::new(seq, bytes::Bytes::copy_from_slice(&packet.payload[..1]))
            })
    }

    pub(crate) fn mark_window_probe_sent(&mut self) {
        if let Some(persist) = self.persist.as_mut() {
            persist.unanswered += 1;
            persist.interval = persist
                .interval
                .saturating_mul(2)
                .min(Duration::from_secs(60).max(self.rto));
            persist.deadline = tokio::time::Instant::now() + persist.interval;
        }
    }

    pub(crate) fn next_retransmit_deadline(&self) -> Option<tokio::time::Instant> {
        // Flow control pauses data retries. Separate, paced window probes
        // recover a lost reopening update without spending data retries.
        if self.state == TcpState::Closed {
            return None;
        }
        if self.send_window == 0 {
            return self.persist.as_ref().map(|persist| persist.deadline);
        }
        self.retransmit_deadline
    }

    pub(crate) fn due_retransmission(&self) -> std::io::Result<Option<InflightPacket>> {
        if self.send_window == 0 || self.state == TcpState::Closed {
            return Ok(None);
        }
        // RFC 6298 section 5.4: timeout recovery retransmits the earliest
        // unacknowledged segment, not every later segment's expired timer.
        let Some((_, packet)) = self.inflight_packets.first_key_value() else {
            return Ok(None);
        };
        if self
            .retransmit_deadline
            .is_none_or(|deadline| deadline > tokio::time::Instant::now())
        {
            return Ok(None);
        }
        // Keep the payload until its final retry has had a full RTO to be
        // acknowledged. Exhaustion fails the flow; it never creates a hole
        // in an otherwise live byte stream.
        if packet.retransmit_count >= self.max_retransmit_count {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "TCP retransmission limit reached",
            ));
        }
        Ok(Some(packet.clone()))
    }

    pub(crate) fn mark_retransmitted(&mut self, seq: SeqNum, timeout: bool) {
        if let Some(packet) = self.inflight_packets.get_mut(&seq) {
            if timeout {
                packet.retransmit_count += 1;
                self.current_rto = self
                    .current_rto
                    .saturating_mul(2)
                    .min(Duration::from_secs(60).max(self.rto));
            }
            self.rtt_probe = None;
            self.restart_retransmission_timer();
        }
    }

    pub(crate) fn discard_failed_payload(&mut self) {
        self.payload_budget
            .release(self.send_reserved + self.get_unordered_packets_total_len());
        self.send_reserved = 0;
        self.read_reserved -= self.get_unordered_packets_total_len();
        self.inflight_packets.clear();
        self.retransmit_deadline = None;
        self.rtt_probe = None;
        self.persist = None;
        self.unordered_packets.clear();
    }

    pub(crate) fn get_inflight_packets_total_len(&self) -> usize {
        self.send_reserved
    }

    #[allow(dead_code)]
    pub(crate) fn get_all_inflight_packets(&self) -> Vec<&InflightPacket> {
        self.inflight_packets.values().collect::<Vec<_>>()
    }

    pub fn is_send_buffer_full(&self) -> bool {
        // To respect the receiver's window (remote_window) size and avoid sending too many unacknowledged packets, which may cause packet loss
        // Simplified version: min(cwnd, rwnd)
        let byte_limit = self.buffer_limit() as u32;
        // One full-sized segment per slot, so a wider byte window is not
        // silently cut back to 64 packets (~90 KiB).
        let packet_limit = (byte_limit / 1400).max(64) as usize;
        self.inflight_packets.len() >= packet_limit
            || self.seq.distance(self.get_last_received_ack())
                >= byte_limit.min(self.get_send_window())
    }
}

impl Drop for Tcb {
    fn drop(&mut self) {
        self.payload_budget
            .release(self.read_reserved + self.send_reserved);
    }
}

/// Drop `buf` and return the unconsumed tail in a fresh allocation.
///
/// `split_off` leaves the consumed prefix in the original `Vec`, capacity
/// included. Dropping that `Vec` is what returns the pages; `drain` and
/// `clear` do not.
pub(crate) fn into_tail(mut buf: Vec<u8>, consumed: usize) -> Vec<u8> {
    let tail = buf.split_off(consumed.min(buf.len()));
    drop(buf);
    tail
}

/// Shift that lets a 16-bit header represent `buffer` bytes.
pub(super) fn window_scale_shift(buffer: usize) -> u8 {
    let mut shift = 0u8;
    while shift < 14 && (usize::from(u16::MAX) << shift) < buffer {
        shift += 1;
    }
    shift
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flow(pool: Arc<crate::TcpPayloadBudget>) -> Tcb {
        Tcb::new(
            SeqNum(1),
            1500,
            8192,
            8192,
            3,
            Duration::from_secs(1),
            3,
            Arc::new(AtomicU32::new(8192)),
            pool,
        )
    }

    #[test]
    fn in_order_receive_moves_the_existing_allocation_into_the_read_queue() {
        let pool = crate::TcpPayloadBudget::new(8192);
        let mut tcb = flow(pool.clone());
        let payload = vec![0x5a; 1400];
        let original = payload.as_ptr();
        tcb.add_unordered_packet(SeqNum(1), payload);
        let read = tcb.consume_unordered_packets(8192).unwrap();
        assert_eq!(read.as_ptr(), original);
        assert_eq!(read, vec![0x5a; 1400]);
        tcb.queue_read(read.len());
        tcb.consume_read(read.len(), 1);
        assert_eq!(pool.usage().0, 0);
    }

    #[test]
    fn peer_mss_and_packet_options_both_limit_payload_without_changing_window_credit() {
        let mut tcb = flow(crate::TcpPayloadBudget::new(8192));
        tcb.set_peer_mss(900);
        assert_eq!(tcb.calculate_payload_max_len(20, 20), 900);
        assert_eq!(tcb.calculate_payload_max_len(20, 32), 888);
        assert_eq!(tcb.calculate_payload_max_len(40, 20), 900);
        tcb.set_peer_mss(2000);
        assert_eq!(tcb.max_segment_payload_len(20, 20), 1460);
        assert_eq!(tcb.max_segment_payload_len(40, 20), 1440);
        assert_eq!(tcb.payload_budget.usage().0, 0);
    }

    #[test]
    fn cumulative_ack_prefix_matches_remaining_stream_across_wrap_and_invalid_acks() {
        for base in [1, u32::MAX - 1000] {
            let pool = crate::TcpPayloadBudget::new(1024 * 1024);
            let mut tcb = flow(pool.clone());
            tcb.seq = SeqNum(base);
            tcb.last_received_ack = SeqNum(base);
            let mut sent = Vec::new();
            for n in 1..=128 {
                let bytes = vec![n as u8; n * 7];
                sent.extend_from_slice(&bytes);
                tcb.add_inflight_packet(bytes).unwrap();
            }
            let mut consumed = 0;
            while consumed < sent.len() {
                let before = tcb.send_reserved;
                let deadline = tcb.next_retransmit_deadline();
                tcb.update_inflight_packet_queue(tcb.seq + 1);
                assert_eq!(tcb.send_reserved, before);
                assert_eq!(tcb.next_retransmit_deadline(), deadline);
                consumed = (consumed + 631).min(sent.len());
                let ack = SeqNum(base) + consumed as u32;
                tcb.update_inflight_packet_queue(ack);
                tcb.update_last_received_ack(ack);
                let remaining: Vec<_> = tcb
                    .inflight_packets
                    .values()
                    .flat_map(|p| p.payload.iter().copied())
                    .collect();
                assert_eq!(remaining, sent[consumed..]);
                assert_eq!(tcb.send_reserved, remaining.len());
                assert_eq!(tcb.get_inflight_packets_total_len(), remaining.len());
                assert_eq!(pool.usage().0, remaining.len());
                tcb.update_inflight_packet_queue(ack - 1);
                assert_eq!(pool.usage().0, remaining.len());
            }
            assert!(tcb.inflight_packets.is_empty());
            assert_eq!(pool.usage().0, 0);
        }
    }

    /// Isolated cumulative-ACK cost, excluding connection/packet setup.
    #[test]
    #[ignore = "manual release-mode ACK processing microbenchmark"]
    fn ack_prefix_microbenchmark() {
        let mut elapsed = Duration::ZERO;
        for _ in 0..128 {
            let pool = crate::TcpPayloadBudget::new(1024 * 1024);
            let mut tcb = flow(pool.clone());
            tcb.max_unacked_bytes = 512 * 1024;
            tcb.read_buffer_size = 512 * 1024;
            tcb.window_limit.store(512 * 1024, Ordering::Relaxed);
            tcb.enable_window_scale(4, 4);
            tcb.update_send_window(u16::MAX);
            let start = tcb.get_seq();
            for _ in 0..256 {
                tcb.add_inflight_packet(vec![7; 1360]).unwrap();
            }
            let began = std::time::Instant::now();
            for i in 1..=256 {
                let ack = start + i * 1360;
                tcb.update_inflight_packet_queue(std::hint::black_box(ack));
                tcb.update_last_received_ack(ack);
            }
            elapsed += began.elapsed();
            assert_eq!(tcb.get_inflight_packets_total_len(), 0);
            assert_eq!(pool.usage().0, 0);
        }
        eprintln!("32768 cumulative ACKs (256 in-flight segments per round): {elapsed:?}");
    }

    #[test]
    fn upload_burst_behind_one_gap_is_delivered_without_retransmitting_the_tail() {
        let pool = crate::TcpPayloadBudget::new(512 * 1024);
        let mut tcb = flow(pool.clone());
        tcb.read_buffer_size = 512 * 1024;
        tcb.max_unacked_bytes = 512 * 1024;
        tcb.window_limit.store(512 * 1024, Ordering::Relaxed);
        let base = SeqNum(u32::MAX - 4096);
        tcb.ack = base;
        let segment = 1352;
        let body: Vec<_> = (0..180 * segment).map(|i| (i % 251) as u8).collect();
        for (i, bytes) in body.chunks(segment).enumerate().skip(1) {
            tcb.add_unordered_packet(base + (i * segment) as u32, bytes.to_vec());
        }
        assert!(tcb.unordered_packets.len() <= MAX_REASSEMBLY_ENTRIES);
        let reserved = pool.usage().0;
        for (i, bytes) in body.chunks(segment).enumerate().skip(1) {
            tcb.add_unordered_packet(base + (i * segment) as u32, bytes.to_vec());
        }
        assert_eq!(pool.usage().0, reserved);
        tcb.add_unordered_packet(base, body[..segment].to_vec());
        let mut delivered = Vec::new();
        while let Some(bytes) = tcb.consume_unordered_packets(8192) {
            tcb.queue_read(bytes.len());
            delivered.extend_from_slice(&bytes);
            tcb.consume_read(bytes.len(), 1);
        }
        assert_eq!(delivered.len(), body.len());
        assert_eq!(delivered, body);
        assert_eq!(pool.usage().0, 0);
    }

    #[test]
    fn sparse_full_reassembly_table_advertises_only_the_leading_gap() {
        let mut tcb = flow(crate::TcpPayloadBudget::new(8192));
        let base = tcb.get_ack();
        for i in 0..64 {
            tcb.add_unordered_packet(base + (100 + 2 * i), vec![1]);
        }
        assert_eq!(tcb.get_recv_window(), 100);
        tcb.add_unordered_packet(base, vec![1; 100]);
        assert_eq!(tcb.consume_unordered_packets(8192).unwrap().len(), 101);
        assert!(tcb.get_recv_window() > 100);
    }

    #[test]
    fn out_of_order_bytes_leave_room_to_repair_the_gap() {
        let pool = crate::TcpPayloadBudget::new(8192);
        let mut tcb = flow(pool.clone());
        let base = tcb.get_ack();
        tcb.add_unordered_packet(base + 1352, vec![2; 6840]);
        tcb.add_unordered_packet(base + 8192, vec![3; 1352]);
        assert_eq!(pool.usage().0, 6840);
        tcb.add_unordered_packet(base, vec![1; 1352]);
        let data = tcb.consume_unordered_packets(8192).unwrap();
        assert_eq!(data.len(), 8192);
        assert_eq!(tcb.get_ack(), base + 8192);
        tcb.queue_read(data.len());
        tcb.consume_read(data.len(), 1);
        assert_eq!(pool.usage().0, 0);
    }

    #[test]
    fn receive_overlap_preserves_bytes_and_returns_duplicate_credit_across_wrap() {
        for base in [1u32, u32::MAX - 4] {
            let pool = crate::TcpPayloadBudget::new(8192);
            let mut tcb = flow(pool.clone());
            tcb.ack = SeqNum(base);
            tcb.add_unordered_packet(SeqNum(base) + 4, b"efgh".to_vec());
            tcb.add_unordered_packet(SeqNum(base) + 2, b"cdef".to_vec());
            // A shorter resend must not erase the buffered segment's tail.
            tcb.add_unordered_packet(SeqNum(base) + 2, b"cd".to_vec());
            tcb.add_unordered_packet(SeqNum(base), b"ab".to_vec());
            let data = tcb.consume_unordered_packets(8192).unwrap();
            assert_eq!(data, b"abcdefgh");
            assert_eq!(tcb.get_ack(), SeqNum(base) + 8);
            assert_eq!(pool.usage().0, 8);
            tcb.queue_read(data.len());
            tcb.consume_read(data.len(), 1);
            assert_eq!(pool.usage().0, 0);
            tcb.add_unordered_packet(SeqNum(base) + 6, b"ghij".to_vec());
            let tail = tcb.consume_unordered_packets(8192).unwrap();
            assert_eq!(tail, b"ij");
            assert_eq!(pool.usage().0, 2);
            drop(tcb);
            assert_eq!(pool.usage().0, 0);
        }
    }

    #[test]
    fn partial_extraction_keeps_longer_overlapping_tail_and_refunds_duplicates() {
        for later_len in [1000, 128] {
            let pool = crate::TcpPayloadBudget::new(32768);
            let mut tcb = flow(pool.clone());
            tcb.read_buffer_size = 16384;
            tcb.max_unacked_bytes = 16384;
            tcb.window_limit.store(16384, Ordering::Relaxed);
            for i in 0..7 {
                tcb.add_unordered_packet(SeqNum(1 + i * 1000), vec![1; 1000]);
            }
            tcb.add_unordered_packet(SeqNum(7001), vec![1; 1440]);
            tcb.add_unordered_packet(SeqNum(8193), vec![1; later_len]);
            let first = tcb.consume_unordered_packets(8192).unwrap();
            assert_eq!(first.len(), 8192);
            tcb.queue_read(first.len());
            let tail = tcb.consume_unordered_packets(8192).unwrap();
            let expected = (8440 - 8192).max(later_len);
            assert_eq!(tail.len(), expected);
            assert!(tail.iter().all(|byte| *byte == 1));
            tcb.queue_read(tail.len());
            assert_eq!(tcb.get_ack(), SeqNum(8193 + expected as u32));
            assert_eq!(pool.usage().0, 8192 + expected);
            tcb.consume_read(first.len(), 1);
            tcb.consume_read(tail.len(), 1);
            assert_eq!(pool.usage().0, 0);
            drop(tcb);
            assert_eq!(pool.usage().0, 0);
        }
    }

    #[test]
    fn rtt_estimator_preserves_variance_floor_and_backoff_cap() {
        let mut tcb = flow(crate::TcpPayloadBudget::new(8192));
        tcb.record_rtt(Duration::from_secs(2));
        assert_eq!(tcb.current_rto, Duration::from_secs(6));
        tcb.record_rtt(Duration::from_secs(1));
        assert_eq!(tcb.smoothed_rtt, Some(Duration::from_millis(1875)));
        assert_eq!(tcb.rtt_variance, Duration::from_secs(1));
        assert_eq!(tcb.current_rto, Duration::from_millis(5875));
        tcb.record_rtt(Duration::from_secs(100));
        assert_eq!(tcb.current_rto, Duration::from_secs(60));
        let mut fresh = flow(crate::TcpPayloadBudget::new(8192));
        fresh.record_rtt(Duration::from_millis(1));
        assert_eq!(fresh.current_rto, Duration::from_secs(1));
    }

    #[test]
    fn appending_data_does_not_postpone_the_running_timer() {
        let mut tcb = flow(crate::TcpPayloadBudget::new(8192));
        let seq = tcb.get_seq();
        tcb.add_inflight_packet(vec![1; 2]).unwrap();
        let deadline = tcb.next_retransmit_deadline();
        tcb.add_inflight_packet(vec![2; 2]).unwrap();
        assert_eq!(tcb.next_retransmit_deadline(), deadline);
        assert_eq!(tcb.rtt_probe.unwrap().0, seq + 2);
    }

    #[test]
    fn busy_connections_can_measure_fresh_data_without_restarting_the_timer() {
        let mut tcb = flow(crate::TcpPayloadBudget::new(8192));
        let seq = tcb.get_seq();
        tcb.add_inflight_packet(vec![1; 2]).unwrap();
        tcb.add_inflight_packet(vec![2; 2]).unwrap();
        tcb.update_inflight_packet_queue(seq + 2);
        assert!(tcb.rtt_probe.is_none());
        let deadline = tcb.next_retransmit_deadline();
        tcb.add_inflight_packet(vec![3; 2]).unwrap();
        assert_eq!(tcb.rtt_probe.unwrap().0, seq + 6);
        assert_eq!(tcb.next_retransmit_deadline(), deadline);
    }

    #[test]
    fn a_connection_timer_only_selects_the_oldest_unacknowledged_segment() {
        let mut tcb = flow(crate::TcpPayloadBudget::new(8192));
        let first = tcb.get_seq();
        tcb.add_inflight_packet(vec![1; 2]).unwrap();
        tcb.add_inflight_packet(vec![2; 2]).unwrap();
        let now = tokio::time::Instant::now();
        tcb.retransmit_deadline = Some(now + Duration::from_secs(4));
        assert!(tcb.due_retransmission().unwrap().is_none());
        assert_eq!(
            tcb.next_retransmit_deadline(),
            Some(now + Duration::from_secs(4))
        );
        tcb.retransmit_deadline = Some(now - Duration::from_secs(1));
        assert_eq!(tcb.due_retransmission().unwrap().unwrap().seq, first);
    }

    #[test]
    fn reopening_a_zero_window_does_not_expire_an_old_payload_timer_immediately() {
        let pool = crate::TcpPayloadBudget::new(8192);
        let mut tcb = flow(pool.clone());
        let seq = tcb.get_seq();
        tcb.add_inflight_packet(vec![1; 4]).unwrap();
        let packet = tcb.inflight_packets.get_mut(&seq).unwrap();
        packet.retransmit_count = 3;
        tcb.retransmit_deadline = Some(tokio::time::Instant::now() - Duration::from_secs(2));
        tcb.update_send_window(0);
        assert!(tcb.next_retransmit_deadline().is_some());
        assert!(!tcb.window_probe_due().unwrap());
        tcb.update_send_window(4096);
        assert!(tcb.due_retransmission().unwrap().is_none());
        assert_eq!(pool.usage().0, 4);
        tcb.update_inflight_packet_queue(seq + 4);
        assert_eq!(pool.usage().0, 0);
    }

    #[test]
    fn acknowledging_all_held_data_stops_idle_window_probes() {
        let mut tcb = flow(crate::TcpPayloadBudget::new(8192));
        let seq = tcb.get_seq();
        tcb.add_inflight_packet(vec![1; 1]).unwrap();
        tcb.update_send_window(0);
        assert!(tcb.next_retransmit_deadline().is_some());
        tcb.update_inflight_packet_queue(seq + 1);
        assert!(tcb.next_retransmit_deadline().is_none());
        assert!(tcb.ensure_persist_timer());
    }

    #[test]
    fn duplicate_ack_does_not_extend_the_retransmission_deadline() {
        let mut tcb = flow(crate::TcpPayloadBudget::new(8192));
        let seq = tcb.get_seq();
        tcb.add_inflight_packet(vec![1; 4]).unwrap();
        let deadline = tcb.next_retransmit_deadline();
        tcb.update_inflight_packet_queue(seq);
        assert_eq!(tcb.next_retransmit_deadline(), deadline);
        tcb.update_inflight_packet_queue(seq + 1);
        let progressed = tcb.next_retransmit_deadline();
        tcb.update_inflight_packet_queue(seq + 1);
        assert_eq!(tcb.next_retransmit_deadline(), progressed);
    }

    #[test]
    fn send_credit_and_ack_validation_survive_sequence_wrap() {
        let pool = crate::TcpPayloadBudget::new(8192);
        let mut tcb = flow(pool.clone());
        let first = SeqNum(u32::MAX - 2);
        tcb.seq = first;
        tcb.last_received_ack = first;
        tcb.update_send_window(6);
        tcb.add_inflight_packet(vec![1; 4]).unwrap();
        assert_eq!(tcb.calculate_payload_max_len(20, 20), 2);
        tcb.update_inflight_packet_queue(tcb.seq + 500);
        assert_eq!(pool.usage().0, 4);
        tcb.update_inflight_packet_queue(first + 2);
        tcb.update_last_received_ack(first + 2);
        assert_eq!(pool.usage().0, 2);
        assert_eq!(tcb.calculate_payload_max_len(20, 20), 4);
        tcb.update_inflight_packet_queue(first + 1);
        assert_eq!(pool.usage().0, 2);
        drop(tcb);
        assert_eq!(pool.usage().0, 0);
    }

    #[test]
    fn consumed_prefix_drops_the_original_allocation() {
        let mut buf = Vec::with_capacity(64 * 1024);
        buf.extend_from_slice(&[1, 2, 3, 4, 5]);
        let tail = into_tail(buf, 2);
        assert_eq!(tail, [3, 4, 5]);
        assert_eq!(tail.capacity(), tail.len());
        assert!(into_tail(vec![9, 9], 2).is_empty());
    }

    #[test]
    fn receive_credit_survives_queueing_and_returns_on_read_or_drop() {
        let pool = crate::TcpPayloadBudget::new(8192);
        let mut first = flow(pool.clone());
        let mut second = flow(pool.clone());
        for i in 0..5 {
            first.add_unordered_packet(SeqNum(1 + i * 1200), vec![1; 1200]);
        }
        let data = first.consume_unordered_packets(8192).unwrap();
        first.queue_read(data.len());
        assert_eq!(
            pool.usage().0,
            6000,
            "moving data to the application queue released credit early"
        );
        second.add_unordered_packet(SeqNum(1), vec![2; 3600]);
        assert_eq!(
            second.get_unordered_packets_total_len(),
            0,
            "full shared pool must reject before ACK advances"
        );
        assert_eq!(second.get_ack(), SeqNum(1));
        first.consume_read(3000, 0);
        second.add_unordered_packet(SeqNum(1), vec![2; 3600]);
        assert_eq!(pool.usage().0, 6600);
        drop(first);
        assert_eq!(pool.usage().0, 3600);
        drop(second);
        assert_eq!(pool.usage().0, 0);
    }

    #[test]
    fn partial_ack_failure_and_cancellation_return_shared_send_credits() {
        let pool = crate::TcpPayloadBudget::new(8192);
        let mut first = flow(pool.clone());
        let mut second = flow(pool.clone());
        let seq = first.get_seq();
        first.add_inflight_packet(vec![1; 6000]).unwrap();
        assert_eq!(
            second
                .add_inflight_packet(vec![2; 4000])
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::WouldBlock
        );
        first.update_inflight_packet_queue(seq + 3000);
        assert_eq!(pool.usage().0, 3000);
        second.add_inflight_packet(vec![2; 4000]).unwrap();
        assert_eq!(pool.usage().0, 7000);
        first.discard_failed_payload();
        assert_eq!(pool.usage().0, 4000);
        drop(first);
        drop(second);
        assert_eq!(pool.usage().0, 0);
    }

    #[test]
    fn read_and_write_share_one_budget_without_short_resends_erasing_data() {
        let pool = crate::TcpPayloadBudget::new(8192);
        let mut tcb = flow(pool.clone());
        tcb.add_unordered_packet(SeqNum(1), vec![1; 4000]);
        tcb.add_unordered_packet(SeqNum(1), vec![1; 2000]);
        assert_eq!(pool.usage().0, 4000);
        assert!(tcb.add_inflight_packet(vec![2; 6000]).is_err());
        tcb.add_inflight_packet(vec![2; 4000]).unwrap();
        tcb.add_unordered_packet(SeqNum(1), vec![1; 6000]);
        assert_eq!(tcb.get_unordered_packets_total_len(), 4000);
        assert_eq!(pool.usage().0, 8000);
        tcb.discard_failed_payload();
        assert_eq!(pool.usage().0, 0);
    }

    #[test]
    fn changing_per_flow_windows_does_not_oversubscribe_the_shared_payload_budget() {
        let limit = Arc::new(AtomicU32::new(512 * 1024));
        let pool = crate::TcpPayloadBudget::new(512 * 1024);
        let make_flow = || {
            Tcb::new(
                SeqNum(1),
                1500,
                512 * 1024,
                512 * 1024,
                3,
                Duration::from_secs(1),
                3,
                limit.clone(),
                pool.clone(),
            )
        };
        let mut first = make_flow();
        let mut second = make_flow();
        first.add_unordered_packet(SeqNum(1), vec![1; 400 * 1024]);
        limit.store(256 * 1024, Ordering::Relaxed);
        second.add_unordered_packet(SeqNum(1), vec![2; 200 * 1024]);
        assert!(
            first.get_unordered_packets_total_len() + second.get_unordered_packets_total_len()
                <= 512 * 1024,
            "shrinking a per-flow window did not reclaim already buffered bytes"
        );
    }

    #[test]
    fn scale_shift_covers_the_buffer_without_wasting_bits() {
        assert_eq!(window_scale_shift(16 * 1024), 0);
        assert_eq!(window_scale_shift(64 * 1024), 1);
        assert_eq!(window_scale_shift(512 * 1024), 4);
    }

    #[test]
    fn advertised_upload_burst_fits_the_ingress_queue_before_receiver_runs() {
        let mut tcb = flow(crate::TcpPayloadBudget::new(512 * 1024));
        tcb.max_unacked_bytes = 512 * 1024;
        tcb.read_buffer_size = 512 * 1024;
        tcb.window_limit.store(512 * 1024, Ordering::Relaxed);
        let (sender, _receiver) = crate::queue::channel(
            crate::queue::TCP_INGRESS_BYTES,
            crate::queue::Budget::new(4 * 1024 * 1024),
        );
        tcb.enable_window_scale(7, 4);
        let window = usize::from(tcb.get_recv_window()) << 4;
        assert!(window >= 16 * 1024);
        let mut offset = 0;
        while offset < window {
            let len = (window - offset).min(1352);
            let mut wire = Vec::new();
            etherparse::PacketBuilder::ipv4([192, 0, 2, 1], [192, 0, 2, 2], 64)
                .tcp(1000, 443, offset as u32, 4096)
                .ack(1)
                .write(&mut wire, &vec![0; len])
                .unwrap();
            sender
                .send(crate::packet::NetworkPacket::parse(&wire).unwrap())
                .unwrap();
            offset += len;
        }
    }

    #[test]
    fn negotiated_scale_widens_the_peer_window_and_the_advertisement() {
        let limit = Arc::new(AtomicU32::new(512 * 1024));
        let mut tcb = Tcb::new(
            SeqNum(1),
            1500,
            512 * 1024,
            512 * 1024,
            3,
            Duration::from_secs(1),
            3,
            limit,
            crate::TcpPayloadBudget::new(16 * 1024 * 1024),
        );
        tcb.enable_window_scale(7, window_scale_shift(512 * 1024));
        tcb.update_send_window(1000);
        assert_eq!(tcb.get_send_window(), 1000 << 7);
        assert_eq!(tcb.advertised_scale(), Some(4));
        assert_eq!(
            usize::from(tcb.get_recv_window()) << 4,
            crate::queue::TCP_INGRESS_PAYLOAD_WINDOW
        );
    }

    #[test]
    fn missing_scale_keeps_the_raw_16_bit_window() {
        let limit = Arc::new(AtomicU32::new(512 * 1024));
        let mut tcb = Tcb::new(
            SeqNum(1),
            1500,
            512 * 1024,
            512 * 1024,
            3,
            Duration::from_secs(1),
            3,
            limit,
            crate::TcpPayloadBudget::new(16 * 1024 * 1024),
        );
        tcb.update_send_window(1000);
        assert_eq!(tcb.get_send_window(), 1000);
        assert_eq!(tcb.advertised_scale(), None);
        assert_eq!(
            usize::from(tcb.get_recv_window()),
            usize::from(u16::MAX)
        );
        let burst = crate::queue::TCP_INGRESS_PAYLOAD_WINDOW - 1024;
        tcb.add_unordered_packet(tcb.get_ack(), vec![0; burst]);
        let queued = tcb.consume_unordered_packets(burst).unwrap();
        tcb.queue_read(queued.len());
        assert_eq!(tcb.get_recv_window(), 1024);
        tcb.consume_read(queued.len(), 1);
        assert_eq!(
            usize::from(tcb.get_recv_window()),
            usize::from(u16::MAX)
        );
        // The outstanding download allowance is independent of our upload window.
        tcb.update_send_window(60_000);
        assert_eq!(tcb.get_send_window(), 60_000);
    }
}

#[derive(Debug, Clone)]
pub struct InflightPacket {
    pub seq: SeqNum,
    pub payload: bytes::Bytes,
    pub retransmit_count: usize,
}

impl InflightPacket {
    fn new(seq: SeqNum, payload: bytes::Bytes) -> Self {
        Self {
            seq,
            payload,
            retransmit_count: 0,
        }
    }
}
