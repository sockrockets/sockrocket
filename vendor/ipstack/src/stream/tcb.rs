use super::seqnum::SeqNum;
use etherparse::TcpHeader;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::{collections::BTreeMap, time::Duration};

pub(super) const MAX_UNACK: u32 = 1024 * 16; // 16KB
pub(super) const READ_BUFFER_SIZE: usize = 1024 * 16; // 16KB
pub(super) const MAX_COUNT_FOR_DUP_ACK: usize = 3; // Maximum number of duplicate ACKs before retransmission

/// Retransmission timeout
pub(super) const RTO: std::time::Duration = std::time::Duration::from_secs(1);

/// Maximum count of retransmissions before failing the connection.
pub(super) const MAX_RETRANSMIT_COUNT: usize = 3;

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
#[derive(Debug, Clone)]
pub(crate) struct Tcb {
    seq: SeqNum,
    ack: SeqNum,
    mtu: u16,
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
    ) -> Tcb {
        #[cfg(debug_assertions)]
        let seq = 100;
        #[cfg(not(debug_assertions))]
        let seq = rand::Rng::random::<u32>(&mut rand::rng());
        Tcb {
            seq: seq.into(),
            ack,
            mtu,
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
            max_retransmit_count,
        }
    }

    pub fn calculate_payload_max_len(
        &self,
        ip_header_size: usize,
        tcp_header_size: usize,
    ) -> usize {
        let send_window = self.get_send_window() as usize;
        let mtu = self.get_mtu() as usize;
        std::cmp::min(
            send_window,
            mtu.saturating_sub(ip_header_size + tcp_header_size),
        )
    }

    pub fn update_duplicate_ack_count(&mut self, rcvd_ack: SeqNum) {
        // If the received rcvd_ack is the same as duplicate_ack_count_helper and not all data has been acknowledged (rcvd_ack < self.seq), increment the count.
        if rcvd_ack == self.duplicate_ack_count_helper && rcvd_ack < self.seq {
            self.duplicate_ack_count = self.duplicate_ack_count.saturating_add(1);
        } else {
            self.duplicate_ack_count_helper = rcvd_ack;
            self.duplicate_ack_count = 0; // reset duplicate ACK count
        }
    }

    pub fn is_duplicate_ack_count_exceeded(&self) -> bool {
        self.duplicate_ack_count == self.max_count_for_dup_ack
    }

    pub(super) fn add_unordered_packet(&mut self, seq: SeqNum, buf: Vec<u8>) {
        if seq < self.ack {
            #[rustfmt::skip]
            log::warn!("{:?}: Received packet seq {seq} < self ack {}, len = {}", self.state, self.ack, buf.len());
            return;
        }
        if self.unordered_packets.len() >= 64
            && seq != self.ack
            && !self.unordered_packets.contains_key(&seq)
        {
            return;
        }
        let old = self.unordered_packets.get(&seq).map_or(0, Vec::len);
        if buf.len() > self.get_available_read_buffer_size().saturating_add(old) {
            return;
        }
        self.unordered_packets.insert(seq, buf);
    }
    pub(super) fn get_available_read_buffer_size(&self) -> usize {
        if self.queued_read_chunks >= 64 {
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
                if seq != self.ack {
                    break; // sequence number is not continuous, stop extracting
                }

                // remove and get the first packet
                let mut payload = self.unordered_packets.remove(&seq).unwrap();
                let payload_len = payload.len();

                if payload_len <= remaining_bytes {
                    // current packet can be fully extracted
                    data.extend(payload);
                    self.ack += payload_len as u32;
                    remaining_bytes -= payload_len;
                } else {
                    // current packet can only be partially extracted
                    let remaining_payload = payload.split_off(remaining_bytes);
                    data.extend_from_slice(&payload);
                    self.ack += remaining_bytes as u32;
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
        self.queued_read_chunks < 64
    }
    pub(super) fn queue_read(&mut self, n: usize) {
        self.queued_read_bytes += n;
        self.queued_read_chunks += 1;
    }
    pub(super) fn consume_read(&mut self, n: usize, completed_chunk: bool) {
        self.queued_read_bytes = self.queued_read_bytes.saturating_sub(n);
        if completed_chunk {
            self.queued_read_chunks = self.queued_read_chunks.saturating_sub(1);
        }
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
        self.send_window = Self::scale_window(window, self.snd_scale);
    }
    pub(super) fn get_send_window(&self) -> u32 {
        self.send_window
    }
    pub(super) fn get_recv_window(&self) -> u16 {
        let available = self.get_available_read_buffer_size() >> self.rcv_scale;
        u16::try_from(available).unwrap_or(u16::MAX)
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

    pub(super) fn add_inflight_packet(&mut self, buf: Vec<u8>) -> std::io::Result<()> {
        if buf.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Empty payload",
            ));
        }
        let buf_len = buf.len() as u32;
        self.inflight_packets
            .insert(self.seq, InflightPacket::new(self.seq, buf, self.rto));
        self.seq += buf_len;
        Ok(())
    }

    pub(super) fn update_last_received_ack(&mut self, ack: SeqNum) {
        self.last_received_ack = ack;
    }

    pub(crate) fn update_inflight_packet_queue(&mut self, ack: SeqNum) {
        match self.inflight_packets.first_key_value() {
            None => return,
            Some((&seq, _)) if ack < seq => return,
            _ => {}
        }
        if let Some(seq) = self
            .inflight_packets
            .iter()
            .find(|(_, p)| p.contains_seq_num(ack - 1))
            .map(|(&s, _)| s)
        {
            let mut inflight_packet = self.inflight_packets.remove(&seq).unwrap();
            let distance = ack.distance(inflight_packet.seq) as usize;
            if distance < inflight_packet.payload.len() {
                inflight_packet.payload.drain(0..distance);
                inflight_packet.seq = ack;
                self.inflight_packets.insert(ack, inflight_packet);
            }
        }
        self.inflight_packets
            .retain(|_, p| ack < p.seq + p.payload.len() as u32);
    }

    pub(crate) fn find_inflight_packet(&self, seq: SeqNum) -> Option<&InflightPacket> {
        self.inflight_packets.get(&seq)
    }

    pub(crate) fn next_retransmit_deadline(&self) -> Option<tokio::time::Instant> {
        // A closed receive window is flow control, not packet loss. Resume
        // data retries when a window update arrives instead of burning the
        // retry budget while the peer cannot accept data.
        if self.send_window == 0 || self.state == TcpState::Closed {
            return None;
        }
        self.inflight_packets
            .values()
            .map(|p| p.send_time + p.retransmit_timeout)
            .min()
    }

    pub(crate) fn due_retransmission(&self) -> std::io::Result<Option<InflightPacket>> {
        if self.send_window == 0 || self.state == TcpState::Closed {
            return Ok(None);
        }
        let Some(packet) = self.inflight_packets.values().find(|p| p.is_timed_out()) else {
            return Ok(None);
        };
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

    pub(crate) fn mark_retransmitted(&mut self, seq: SeqNum) {
        if let Some(packet) = self.inflight_packets.get_mut(&seq) {
            packet.retransmit_count += 1;
            packet.retransmit_timeout = packet
                .retransmit_timeout
                .saturating_mul(2)
                .min(Duration::from_secs(60).max(self.rto));
            packet.send_time = tokio::time::Instant::now();
        }
    }

    pub(crate) fn discard_failed_payload(&mut self) {
        self.inflight_packets.clear();
        self.unordered_packets.clear();
    }

    pub(crate) fn get_inflight_packets_total_len(&self) -> usize {
        self.inflight_packets
            .values()
            .map(|p| p.payload.len())
            .sum()
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

    #[test]
    fn scale_shift_covers_the_buffer_without_wasting_bits() {
        assert_eq!(window_scale_shift(16 * 1024), 0);
        assert_eq!(window_scale_shift(64 * 1024), 1);
        assert_eq!(window_scale_shift(512 * 1024), 4);
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
        );
        tcb.enable_window_scale(7, window_scale_shift(512 * 1024));
        tcb.update_send_window(1000);
        assert_eq!(tcb.get_send_window(), 1000 << 7);
        assert_eq!(tcb.advertised_scale(), Some(4));
        assert_eq!(tcb.get_recv_window(), ((512 * 1024) >> 4) as u16);
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
        );
        tcb.update_send_window(1000);
        assert_eq!(tcb.get_send_window(), 1000);
        assert_eq!(tcb.advertised_scale(), None);
        assert_eq!(tcb.get_recv_window(), u16::MAX);
    }
}

#[derive(Debug, Clone)]
pub struct InflightPacket {
    pub seq: SeqNum,
    pub payload: Vec<u8>,
    pub send_time: tokio::time::Instant,
    pub retransmit_count: usize,
    pub retransmit_timeout: std::time::Duration, // current retransmission timeout
}

impl InflightPacket {
    fn new(seq: SeqNum, payload: Vec<u8>, rto: Duration) -> Self {
        Self {
            seq,
            payload,
            send_time: tokio::time::Instant::now(),
            retransmit_count: 0,
            retransmit_timeout: rto,
        }
    }
    pub(crate) fn contains_seq_num(&self, seq: SeqNum) -> bool {
        self.seq <= seq && seq < self.seq + self.payload.len() as u32
    }
    pub(crate) fn is_timed_out(&self) -> bool {
        self.send_time.elapsed() >= self.retransmit_timeout
    }
}
