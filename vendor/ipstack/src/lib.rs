#![doc = include_str!("../README.md")]

use ahash::AHashMap;
use packet::{NetworkPacket, NetworkTuple, TransportHeader};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    select,
    sync::mpsc::{self},
    task::JoinHandle,
};

mod queue;
pub(crate) use queue::{PacketReceiver, PacketSender};
pub(crate) type SessionCollection = AHashMap<NetworkTuple, PacketSender>;

mod error;
mod packet;
mod stream;

pub use self::error::{IpStackError, Result};
pub use self::stream::{
    IpStackStream, IpStackTcpStream, IpStackUdpStream, IpStackUnknownTransport,
};
pub use self::stream::{TcpConfig, TcpOptions};
pub use etherparse::IpNumber;

/// Cumulative TCP termination counts for this stack, without flow addresses.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TcpEventCounts {
    pub peer_resets: usize,
    pub handshake_resets: usize,
    pub task_errors: usize,
}

#[cfg(unix)]
const TTL: u8 = 64;

#[cfg(windows)]
const TTL: u8 = 128;

#[cfg(unix)]
const TUN_FLAGS: [u8; 2] = [0x00, 0x00];

#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_os = "espidf"
))]
const TUN_PROTO_IP6: [u8; 2] = [0x86, 0xdd];
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_os = "espidf"
))]
const TUN_PROTO_IP4: [u8; 2] = [0x08, 0x00];

#[cfg(any(target_os = "macos", target_os = "ios"))]
const TUN_PROTO_IP6: [u8; 2] = [0x00, 0x0A];
#[cfg(any(target_os = "macos", target_os = "ios"))]
const TUN_PROTO_IP4: [u8; 2] = [0x00, 0x02];

/// Minimum MTU required for IPv6 (per RFC 8200 §5: MTU ≥ 1280).
/// Also satisfies IPv4 minimum MTU (RFC 791 §3.1: 68 bytes).
const MIN_MTU: u16 = 1280;

/// Configuration for the IP stack.
///
/// This structure holds configuration parameters that control the behavior of the IP stack,
/// including network settings and protocol-specific timeouts.
///
/// # Examples
///
/// ```
/// use ipstack::IpStackConfig;
/// use std::time::Duration;
///
/// let mut config = IpStackConfig::default();
/// config.mtu(1500).expect("Failed to set MTU")
///       .udp_timeout(Duration::from_secs(60))
///       .packet_information(false);
/// ```
#[non_exhaustive]
pub struct IpStackConfig {
    /// Maximum Transmission Unit (MTU) size in bytes.
    /// Default is `MIN_MTU` (1280).
    pub mtu: u16,
    /// Whether to include packet information headers (Unix platforms only).
    /// Default is `false`.
    pub packet_information: bool,
    /// TCP-specific configuration parameters.
    pub tcp_config: Arc<TcpConfig>,
    /// Timeout for UDP connections.
    /// Default is 30 seconds.
    pub udp_timeout: Duration,
}

impl Default for IpStackConfig {
    fn default() -> Self {
        IpStackConfig {
            mtu: MIN_MTU,
            packet_information: false,
            tcp_config: Arc::new(TcpConfig::default()),
            udp_timeout: Duration::from_secs(30),
        }
    }
}

impl IpStackConfig {
    /// Set custom TCP configuration.
    ///
    /// # Arguments
    ///
    /// * `config` - The TCP configuration to use
    ///
    /// # Examples
    ///
    /// ```
    /// use ipstack::{IpStackConfig, TcpConfig};
    ///
    /// let mut config = IpStackConfig::default();
    /// config.with_tcp_config(TcpConfig::default());
    /// ```
    pub fn with_tcp_config(&mut self, config: TcpConfig) -> &mut Self {
        self.tcp_config = Arc::new(config);
        self
    }

    /// Set the UDP connection timeout.
    ///
    /// # Arguments
    ///
    /// * `timeout` - The timeout duration for UDP connections
    ///
    /// # Examples
    ///
    /// ```
    /// use ipstack::IpStackConfig;
    /// use std::time::Duration;
    ///
    /// let mut config = IpStackConfig::default();
    /// config.udp_timeout(Duration::from_secs(60));
    /// ```
    pub fn udp_timeout(&mut self, timeout: Duration) -> &mut Self {
        self.udp_timeout = timeout;
        self
    }

    /// Set the Maximum Transmission Unit (MTU) size.
    ///
    /// # Arguments
    ///
    /// * `mtu` - The MTU size in bytes
    ///
    /// # Examples
    ///
    /// ```
    /// use ipstack::IpStackConfig;
    ///
    /// let mut config = IpStackConfig::default();
    /// config.mtu(1500).expect("Failed to set MTU");
    /// ```
    pub fn mtu(&mut self, mtu: u16) -> Result<&mut Self, IpStackError> {
        if mtu < MIN_MTU {
            return Err(IpStackError::InvalidMtuSize(mtu));
        }
        self.mtu = mtu;
        Ok(self)
    }

    /// Set the Maximum Transmission Unit (MTU) size without validation.
    pub fn mtu_unchecked(&mut self, mtu: u16) -> &mut Self {
        self.mtu = mtu;
        self
    }

    /// Enable or disable packet information headers (Unix platforms only).
    ///
    /// When enabled on Unix platforms, the TUN device will include 4-byte packet
    /// information headers.
    ///
    /// # Arguments
    ///
    /// * `packet_information` - Whether to include packet information headers
    ///
    /// # Examples
    ///
    /// ```
    /// use ipstack::IpStackConfig;
    ///
    /// let mut config = IpStackConfig::default();
    /// config.packet_information(true);
    /// ```
    pub fn packet_information(&mut self, packet_information: bool) -> &mut Self {
        self.packet_information = packet_information;
        self
    }
}

/// The main IP stack instance.
///
/// `IpStack` provides a userspace TCP/IP stack implementation for TUN devices.
/// It processes network packets and creates stream abstractions for TCP, UDP, and
/// unknown transport protocols.
///
/// # Examples
///
/// ```no_run
/// use ipstack::{IpStack, IpStackConfig, IpStackStream};
/// use std::net::Ipv4Addr;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     // Configure TUN device
///     let mut config = tun::Configuration::default();
///     config
///         .address(Ipv4Addr::new(10, 0, 0, 1))
///         .netmask(Ipv4Addr::new(255, 255, 255, 0))
///         .up();
///
///     // Create IP stack
///     let ipstack_config = IpStackConfig::default();
///     let mut ip_stack = IpStack::new(ipstack_config, tun::create_as_async(&config)?);
///
///     // Accept incoming streams
///     while let Ok(stream) = ip_stack.accept().await {
///         match stream {
///             IpStackStream::Tcp(tcp) => {
///                 // Handle TCP connection
///             }
///             IpStackStream::Udp(udp) => {
///                 // Handle UDP connection
///             }
///             _ => {}
///         }
///     }
///     Ok(())
/// }
/// ```
pub struct IpStack {
    accept_receiver: mpsc::Receiver<IpStackStream>,
    dns_accept_receiver: mpsc::Receiver<IpStackStream>,
    packet_budget: Arc<queue::Budget>,
    handle: JoinHandle<Result<()>>,
}

impl IpStack {
    /// Create a new IP stack instance.
    ///
    /// # Arguments
    ///
    /// * `config` - Configuration for the IP stack
    /// * `device` - An async TUN device implementing `AsyncRead` + `AsyncWrite`
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use ipstack::{IpStack, IpStackConfig};
    /// use std::net::Ipv4Addr;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let mut tun_config = tun::Configuration::default();
    /// tun_config.address(Ipv4Addr::new(10, 0, 0, 1))
    ///           .netmask(Ipv4Addr::new(255, 255, 255, 0))
    ///           .up();
    ///
    /// let ipstack_config = IpStackConfig::default();
    /// let ip_stack = IpStack::new(ipstack_config, tun::create_as_async(&tun_config)?);
    /// # Ok(())
    /// # }
    /// ```
    pub fn new<Device>(config: IpStackConfig, device: Device) -> IpStack
    where
        Device: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (accept_sender, accept_receiver) = mpsc::channel::<IpStackStream>(128);
        let (dns_accept_sender, dns_accept_receiver) = mpsc::channel::<IpStackStream>(32);
        let packet_budget = queue::Budget::new(8 * 1024 * 1024);
        IpStack {
            accept_receiver,
            dns_accept_receiver,
            packet_budget: packet_budget.clone(),
            handle: run(
                config,
                device,
                accept_sender,
                dns_accept_sender,
                packet_budget,
            ),
        }
    }

    /// Accept an incoming network stream.
    ///
    /// This method waits for and returns the next incoming network connection or packet.
    /// The returned `IpStackStream` enum indicates the type of stream (TCP, UDP, or unknown).
    ///
    /// # Returns
    ///
    /// * `Ok(IpStackStream)` - The next incoming stream
    /// * `Err(IpStackError::AcceptError)` - If the IP stack has been shut down
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use ipstack::{IpStack, IpStackConfig, IpStackStream};
    ///
    /// # async fn example(mut ip_stack: IpStack) -> Result<(), Box<dyn std::error::Error>> {
    /// match ip_stack.accept().await? {
    ///     IpStackStream::Tcp(tcp) => {
    ///         println!("New TCP connection from {}", tcp.peer_addr());
    ///     }
    ///     IpStackStream::Udp(udp) => {
    ///         println!("New UDP stream from {}", udp.peer_addr());
    ///     }
    ///     IpStackStream::UnknownTransport(unknown) => {
    ///         println!("Unknown transport protocol: {:?}", unknown.ip_protocol());
    ///     }
    ///     IpStackStream::UnknownNetwork(data) => {
    ///         println!("Unknown network packet: {} bytes", data.len());
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    /// Current queued packet bytes and their hard global bound.
    /// State counts (listen, syn-received, established, fin-wait-1/2,
    /// closing, time-wait, close-wait, last-ack, closed), oldest closing
    /// age, and receive payload bytes retained by live flows.
    pub fn tcp_health(&self) -> ([usize; 10], u64, usize) {
        self.packet_budget.flow_health()
    }
    pub fn tcp_event_counts(&self) -> TcpEventCounts {
        self.packet_budget.tcp_event_counts()
    }
    pub fn packet_queue_usage(&self) -> (usize, usize) {
        (self.packet_budget.used(), self.packet_budget.limit())
    }
    pub async fn accept(&mut self) -> Result<IpStackStream, IpStackError> {
        tokio::select! {
            biased;
            stream = self.dns_accept_receiver.recv() => stream.ok_or(IpStackError::AcceptError),
            stream = self.accept_receiver.recv() => stream.ok_or(IpStackError::AcceptError),
        }
    }
}

impl Drop for IpStack {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

fn run<Device: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    config: IpStackConfig,
    mut device: Device,
    accept_sender: mpsc::Sender<IpStackStream>,
    dns_accept_sender: mpsc::Sender<IpStackStream>,
    packet_budget: Arc<queue::Budget>,
) -> JoinHandle<Result<()>> {
    let mut sessions: SessionCollection = AHashMap::new();
    let (session_remove_tx, mut session_remove_rx) =
        mpsc::channel::<(NetworkTuple, PacketSender)>(1024);
    let pi = config.packet_information;
    let offset = if pi && cfg!(unix) { 4 } else { 0 };
    let mut buffer = vec![0_u8; config.mtu as usize + offset];
    let (up_pkt_sender, mut up_pkt_receiver) = queue::channel(4 * 1024 * 1024, packet_budget);

    tokio::spawn(async move {
        loop {
            select! {
                received = device.read(&mut buffer) => {
                    let n = received?;
                    if n == 0 { return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into()); }
                    if n < offset { continue; }
                    if let Err(e) = process_device_read(&buffer[offset..n], &mut sessions, &session_remove_tx, &up_pkt_sender, &config, &accept_sender, &dns_accept_sender).await {
                        let io_err: std::io::Error = e.into();
                        if io_err.kind() == std::io::ErrorKind::ConnectionRefused {
                            log::trace!("Received junk data: {io_err}");
                        } else {
                            log::warn!("process_device_read error: {io_err}");
                        }
                    }
                }
                Some((network_tuple, sender)) = session_remove_rx.recv() => {
                    // A closed flow may already have been replaced by a new
                    // one with the same tuple. Remove only this generation.
                    remove_session(&mut sessions, network_tuple, &sender);
                }
                Some(packet) = up_pkt_receiver.recv() => {
                    tokio::time::timeout(std::time::Duration::from_secs(10), process_upstream_recv(packet, &mut device, #[cfg(unix)]pi))
                        .await.map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut,"TUN device write timed out"))??;
                }
            }
        }
    })
}

async fn process_device_read(
    data: &[u8],
    sessions: &mut SessionCollection,
    session_remove_tx: &mpsc::Sender<(NetworkTuple, PacketSender)>,
    up_pkt_sender: &PacketSender,
    config: &IpStackConfig,
    accept_sender: &mpsc::Sender<IpStackStream>,
    dns_accept_sender: &mpsc::Sender<IpStackStream>,
) -> Result<()> {
    let Ok(packet) = NetworkPacket::parse(data) else {
        let stream = IpStackStream::UnknownNetwork(data.to_owned());
        let _ = accept_sender.try_send(stream);
        return Ok(());
    };

    if let TransportHeader::Unknown = packet.transport_header() {
        let stream = IpStackStream::UnknownTransport(IpStackUnknownTransport::new(
            packet.src_addr().ip(),
            packet.dst_addr().ip(),
            packet.payload.unwrap_or_default(),
            &packet.ip,
            config.mtu,
            up_pkt_sender.clone(),
        ));
        let _ = accept_sender.try_send(stream);
        return Ok(());
    }

    let network_tuple = packet.network_tuple();
    let packet = match sessions.entry(network_tuple) {
        std::collections::hash_map::Entry::Occupied(entry) => {
            let len = packet.payload.as_ref().map_or(0, Vec::len);
            log::trace!("packet sent to stream: {network_tuple} len {len}");
            match entry.get().send(packet) {
                Ok(()) => return Ok(()),
                Err(error) if error.1 => return Ok(()),
                Err(error) => {
                    // The receiver can close before its asynchronous removal
                    // reaches this loop. Recover ownership of the packet and
                    // retire the old sender before creating a replacement.
                    entry.remove();
                    error.0
                }
            }
        }
        std::collections::hash_map::Entry::Vacant(_) => packet,
    };
    let is_dns = matches!(packet.transport_header(), TransportHeader::Udp(header) if header.destination_port == 53);
    let accept_sender = if is_dns {
        dns_accept_sender
    } else {
        accept_sender
    };
    if sessions.len() >= if is_dns { 1056 } else { 1024 } || accept_sender.capacity() == 0 {
        return Ok(());
    }
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let stream = create_stream(packet, config, up_pkt_sender.clone(), Some(tx))?;
    let sender = stream.stream_sender()?;
    let removal_sender = sender.clone();
    let session_remove_tx = session_remove_tx.clone();
    tokio::spawn(async move {
        rx.await.ok();
        // Dropping the whole stack closes the removal receiver normally.
        let _ = session_remove_tx
            .send((network_tuple, removal_sender))
            .await;
    });
    accept_sender
        .try_send(stream)
        .map_err(|_| IpStackError::AcceptError)?;
    sessions.insert(network_tuple, sender);
    log::debug!("session created: {network_tuple}");
    Ok(())
}

fn remove_session(sessions: &mut SessionCollection, tuple: NetworkTuple, sender: &PacketSender) {
    if let std::collections::hash_map::Entry::Occupied(entry) = sessions.entry(tuple)
        && entry.get().same_channel(sender)
    {
        entry.remove();
        log::debug!("session destroyed: {tuple}");
    }
}

fn create_stream(
    packet: NetworkPacket,
    cfg: &IpStackConfig,
    up_pkt_sender: PacketSender,
    msgr: Option<::tokio::sync::oneshot::Sender<()>>,
) -> Result<IpStackStream> {
    let src_addr = packet.src_addr();
    let dst_addr = packet.dst_addr();
    match packet.transport_header() {
        TransportHeader::Tcp(h) => {
            let stream = IpStackTcpStream::new(
                src_addr,
                dst_addr,
                h.clone(),
                up_pkt_sender,
                cfg.mtu,
                msgr,
                cfg.tcp_config.clone(),
            )?;
            Ok(IpStackStream::Tcp(stream))
        }
        TransportHeader::Udp(_) => {
            let payload = packet.payload.unwrap_or_default();
            let stream = IpStackUdpStream::new(
                src_addr,
                dst_addr,
                payload,
                up_pkt_sender,
                cfg.mtu,
                cfg.udp_timeout,
                msgr,
            );
            Ok(IpStackStream::Udp(stream))
        }
        TransportHeader::Unknown => Err(IpStackError::UnsupportedTransportProtocol),
    }
}

async fn process_upstream_recv<Device: AsyncWrite + Unpin + 'static>(
    up_packet: NetworkPacket,
    device: &mut Device,
    #[cfg(unix)] packet_information: bool,
) -> Result<()> {
    #[allow(unused_mut)]
    let Ok(mut packet_bytes) = up_packet.to_bytes() else {
        log::warn!("to_bytes error");
        return Ok(());
    };
    #[cfg(unix)]
    if packet_information {
        if up_packet.src_addr().is_ipv4() {
            packet_bytes.splice(0..0, [TUN_FLAGS, TUN_PROTO_IP4].concat());
        } else {
            packet_bytes.splice(0..0, [TUN_FLAGS, TUN_PROTO_IP6].concat());
        }
    }
    device.write_all(&packet_bytes).await?;
    // device.flush().await?;

    Ok(())
}
