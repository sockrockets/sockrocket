use crate::{
    IpStackError, PacketReceiver, PacketSender, TTL,
    packet::{IpHeader, NetworkPacket, TransportHeader},
};
use etherparse::{
    Icmpv4Header, Icmpv4Type, Icmpv6Header, Icmpv6Type, IpNumber, Ipv4Header, Ipv6FlowLabel,
    Ipv6Header, UdpHeader,
};
use std::{
    future::Future,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    pin::Pin,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::Sleep,
};

/// A UDP stream in the IP stack.
///
/// This type represents a UDP connection and implements `AsyncRead` and `AsyncWrite`
/// for bidirectional data transfer. UDP streams have a configurable timeout and
/// automatically handle packet fragmentation based on MTU.
///
/// # Examples
///
/// ```no_run
/// use ipstack::{IpStack, IpStackConfig, IpStackStream};
/// use tokio::io::{AsyncReadExt, AsyncWriteExt};
///
/// # async fn example(mut ip_stack: IpStack) -> Result<(), Box<dyn std::error::Error>> {
/// if let IpStackStream::Udp(mut udp_stream) = ip_stack.accept().await? {
///     println!("New UDP stream from {}", udp_stream.peer_addr());
///
///     // Read data
///     let mut buffer = [0u8; 1024];
///     let n = udp_stream.read(&mut buffer).await?;
///
///     // Write data
///     udp_stream.write_all(b"Response").await?;
/// }
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct IpStackUdpStream {
    src_addr: SocketAddr,
    dst_addr: SocketAddr,
    stream_sender: PacketSender,
    stream_receiver: PacketReceiver,
    up_pkt_sender: PacketSender,
    first_payload: Option<Vec<u8>>,
    timeout: Pin<Box<Sleep>>,
    timeout_interval: Duration,
    mtu: u16,
    destroy_messenger: Option<::tokio::sync::oneshot::Sender<()>>,
}

impl IpStackUdpStream {
    pub(crate) fn new(
        src_addr: SocketAddr,
        dst_addr: SocketAddr,
        payload: Vec<u8>,
        up_pkt_sender: PacketSender,
        mtu: u16,
        timeout_interval: Duration,
        destroy_messenger: Option<::tokio::sync::oneshot::Sender<()>>,
    ) -> Self {
        let (stream_sender, stream_receiver) =
            crate::queue::channel(64 * 1024, up_pkt_sender.global());
        let deadline = tokio::time::Instant::now() + timeout_interval;
        IpStackUdpStream {
            src_addr,
            dst_addr,
            stream_sender,
            stream_receiver,
            up_pkt_sender,
            first_payload: Some(payload),
            timeout: Box::pin(tokio::time::sleep_until(deadline)),
            timeout_interval,
            mtu,
            destroy_messenger,
        }
    }

    pub(crate) fn stream_sender(&self) -> PacketSender {
        self.stream_sender.clone()
    }

    fn create_rev_packet(&self, ttl: u8, mut payload: Vec<u8>) -> std::io::Result<NetworkPacket> {
        const UHS: usize = 8; // udp header size is 8
        match (self.dst_addr.ip(), self.src_addr.ip()) {
            (std::net::IpAddr::V4(dst), std::net::IpAddr::V4(src)) => {
                let mut ip_h = Ipv4Header::new(0, ttl, IpNumber::UDP, dst.octets(), src.octets())
                    .map_err(IpStackError::from)?;
                let line_buffer = self.mtu.saturating_sub((ip_h.header_len() + UHS) as u16);
                payload.truncate(line_buffer as usize);
                ip_h.set_payload_len(payload.len() + UHS)
                    .map_err(IpStackError::from)?;
                let udp_header = UdpHeader::with_ipv4_checksum(
                    self.dst_addr.port(),
                    self.src_addr.port(),
                    &ip_h,
                    &payload,
                )
                .map_err(IpStackError::from)?;
                Ok(NetworkPacket {
                    ip: IpHeader::Ipv4(ip_h),
                    transport: TransportHeader::Udp(udp_header),
                    payload: Some(payload),
                })
            }
            (std::net::IpAddr::V6(dst), std::net::IpAddr::V6(src)) => {
                let mut ip_h = Ipv6Header {
                    traffic_class: 0,
                    flow_label: Ipv6FlowLabel::ZERO,
                    payload_length: 0,
                    next_header: IpNumber::UDP,
                    hop_limit: ttl,
                    source: dst.octets(),
                    destination: src.octets(),
                };
                let line_buffer = self.mtu.saturating_sub((ip_h.header_len() + UHS) as u16);

                payload.truncate(line_buffer as usize);

                ip_h.payload_length = (payload.len() + UHS) as u16;
                let udp_header = UdpHeader::with_ipv6_checksum(
                    self.dst_addr.port(),
                    self.src_addr.port(),
                    &ip_h,
                    &payload,
                )
                .map_err(IpStackError::from)?;
                Ok(NetworkPacket {
                    ip: IpHeader::Ipv6(ip_h),
                    transport: TransportHeader::Udp(udp_header),
                    payload: Some(payload),
                })
            }
            _ => unreachable!(),
        }
    }

    /// Returns the local socket address of the UDP stream.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use ipstack::IpStackUdpStream;
    /// # fn example(udp_stream: &IpStackUdpStream) {
    /// let local_addr = udp_stream.local_addr();
    /// println!("Local address: {}", local_addr);
    /// # }
    /// ```
    pub fn local_addr(&self) -> SocketAddr {
        self.src_addr
    }

    /// Returns the remote socket address of the UDP stream.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use ipstack::IpStackUdpStream;
    /// # fn example(udp_stream: &IpStackUdpStream) {
    /// let peer_addr = udp_stream.peer_addr();
    /// println!("Peer address: {}", peer_addr);
    /// # }
    /// ```
    pub fn peer_addr(&self) -> SocketAddr {
        self.dst_addr
    }

    /// Tell the client this destination has no UDP service.
    ///
    /// The reply uses the original destination as its source, matching the
    /// kernel `icmp-port-unreachable` reject. Best-effort: a full queue
    /// returns an error and the caller still drops the flow.
    pub fn reject_port_unreachable(&self) -> std::io::Result<()> {
        let packet = self.port_unreachable_packet()?;
        self.up_pkt_sender.send(packet).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "TUN ICMP queue did not accept the port-unreachable reply",
            )
        })
    }

    fn port_unreachable_packet(&self) -> std::io::Result<NetworkPacket> {
        // RFC 1122 3.2.2 / RFC 4443 2.4: no ICMP errors about multicast,
        // broadcast or unspecified addresses.
        let no_error = |ip: IpAddr| match ip {
            IpAddr::V4(ip) => ip.is_multicast() || ip.is_broadcast() || ip.is_unspecified(),
            IpAddr::V6(ip) => ip.is_multicast() || ip.is_unspecified(),
        };
        if no_error(self.src_addr.ip()) || no_error(self.dst_addr.ip()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "no ICMP error for multicast, broadcast or unspecified addresses",
            ));
        }
        match (self.src_addr.ip(), self.dst_addr.ip()) {
            (IpAddr::V4(client), IpAddr::V4(target)) => self.ipv4_unreachable(client, target),
            (IpAddr::V6(client), IpAddr::V6(target)) => self.ipv6_unreachable(client, target),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "UDP flow mixes IPv4 and IPv6",
            )),
        }
    }

    fn ipv4_unreachable(
        &self,
        client: Ipv4Addr,
        target: Ipv4Addr,
    ) -> std::io::Result<NetworkPacket> {
        let mut quoted_ip = Ipv4Header::new(
            UdpHeader::LEN_U16,
            TTL,
            IpNumber::UDP,
            client.octets(),
            target.octets(),
        )
        .map_err(invalid_packet)?;
        quoted_ip.header_checksum = quoted_ip.calc_header_checksum();
        let quoted_udp =
            UdpHeader::without_ipv4_checksum(self.src_addr.port(), self.dst_addr.port(), 0)
                .map_err(invalid_packet)?;
        let mut quoted = Vec::new();
        quoted_ip.write(&mut quoted)?;
        quoted_udp.write(&mut quoted)?;
        let mut icmp = Icmpv4Header::new(Icmpv4Type::DestinationUnreachable(
            etherparse::icmpv4::DestUnreachableHeader::Port,
        ));
        icmp.update_checksum(&quoted);
        let mut payload = icmp.to_bytes().to_vec();
        payload.extend_from_slice(&quoted);
        let mut reply = Ipv4Header::new(
            payload.len() as u16,
            TTL,
            IpNumber::ICMP,
            target.octets(),
            client.octets(),
        )
        .map_err(invalid_packet)?;
        reply.header_checksum = reply.calc_header_checksum();
        Ok(NetworkPacket {
            ip: IpHeader::Ipv4(reply),
            transport: TransportHeader::Unknown,
            payload: Some(payload),
        })
    }

    fn ipv6_unreachable(
        &self,
        client: Ipv6Addr,
        target: Ipv6Addr,
    ) -> std::io::Result<NetworkPacket> {
        let quoted_ip = Ipv6Header {
            traffic_class: 0,
            flow_label: Ipv6FlowLabel::ZERO,
            payload_length: UdpHeader::LEN_U16,
            next_header: IpNumber::UDP,
            hop_limit: TTL,
            source: client.octets(),
            destination: target.octets(),
        };
        let quoted_udp =
            UdpHeader::without_ipv4_checksum(self.src_addr.port(), self.dst_addr.port(), 0)
                .map_err(invalid_packet)?;
        let mut quoted = Vec::new();
        quoted_ip.write(&mut quoted)?;
        quoted_udp.write(&mut quoted)?;
        let mut icmp = Icmpv6Header::new(Icmpv6Type::DestinationUnreachable(
            etherparse::icmpv6::DestUnreachableCode::Port,
        ));
        icmp.update_checksum(target.octets(), client.octets(), &quoted)
            .map_err(invalid_packet)?;
        let mut payload = icmp.to_bytes().to_vec();
        payload.extend_from_slice(&quoted);
        let reply = Ipv6Header {
            traffic_class: 0,
            flow_label: Ipv6FlowLabel::ZERO,
            payload_length: payload.len() as u16,
            next_header: IpNumber::IPV6_ICMP,
            hop_limit: TTL,
            source: target.octets(),
            destination: client.octets(),
        };
        Ok(NetworkPacket {
            ip: IpHeader::Ipv6(reply),
            transport: TransportHeader::Unknown,
            payload: Some(payload),
        })
    }

    fn reset_timeout(&mut self) {
        let deadline = tokio::time::Instant::now() + self.timeout_interval;
        self.timeout.as_mut().reset(deadline);
    }
}

fn invalid_packet<E: std::fmt::Display>(error: E) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string())
}

impl AsyncRead for IpStackUdpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        if buf.remaining() == 0 {
            return std::task::Poll::Ready(Ok(()));
        }
        if let Some(p) = self.first_payload.take() {
            if p.len() > buf.remaining() {
                return std::task::Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "UDP read buffer too small",
                )));
            }
            buf.put_slice(&p);
            self.reset_timeout();
            return std::task::Poll::Ready(Ok(()));
        }
        if matches!(self.timeout.as_mut().poll(cx), std::task::Poll::Ready(_)) {
            return std::task::Poll::Ready(Err(std::io::Error::from(std::io::ErrorKind::TimedOut)));
        }

        match self.stream_receiver.poll_recv(cx) {
            std::task::Poll::Ready(Some(p)) => {
                if let Some(payload) = p.payload {
                    if payload.len() > buf.remaining() {
                        return std::task::Poll::Ready(Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "UDP read buffer too small",
                        )));
                    }
                    buf.put_slice(&payload);
                    self.reset_timeout();
                }
                std::task::Poll::Ready(Ok(()))
            }
            std::task::Poll::Ready(None) => std::task::Poll::Ready(Ok(())),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}

impl AsyncWrite for IpStackUdpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let header_len = if self.src_addr.is_ipv4() { 28 } else { 48 };
        if buf.len() > self.mtu.saturating_sub(header_len) as usize {
            return std::task::Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "UDP datagram exceeds TUN MTU",
            )));
        }
        let packet = self.create_rev_packet(TTL, buf.to_vec())?;
        let payload_len = packet.payload.as_ref().map(|p| p.len()).unwrap_or(0);
        match self.up_pkt_sender.send(packet) {
            Ok(()) => {}
            Err(error) if error.1 => {
                self.up_pkt_sender.register(cx);
                match self.up_pkt_sender.send(error.0) {
                    Ok(()) => {}
                    Err(error) if error.1 => return std::task::Poll::Pending,
                    Err(_) => {
                        return std::task::Poll::Ready(Err(
                            std::io::ErrorKind::UnexpectedEof.into()
                        ));
                    }
                }
            }
            Err(_) => return std::task::Poll::Ready(Err(std::io::ErrorKind::UnexpectedEof.into())),
        }
        self.reset_timeout();
        std::task::Poll::Ready(Ok(payload_len))
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

impl Drop for IpStackUdpStream {
    fn drop(&mut self) {
        if let Some(messenger) = self.destroy_messenger.take() {
            let _ = messenger.send(());
        }
    }
}
