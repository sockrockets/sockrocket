//! In-process WireGuard endpoint. No host TUN, route, DNS or proxy mutation.
use super::{
    connector::{BoxProxyStream, Outbound},
    datagram::{BoxDatagram, Datagram, DatagramFuture},
};
use crate::config::model::WireguardConfig;
use anyhow::{Context as _, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    future::Future,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{Arc, OnceLock, Weak},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{Mutex, OwnedSemaphorePermit, Semaphore},
};
use tokio_wireguard::{
    Interface, TcpStream, UdpSocket,
    config::{Address, Config, IpNet, Peer},
    interface::Options,
    x25519::{PublicKey, StaticSecret},
};
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
struct State {
    interface: Interface,
    _slot: OwnedSemaphorePermit,
    flows: Arc<Semaphore>,
    dns: Vec<IpAddr>,
    mtu: usize,
    allowed: Vec<IpNet>,
}
impl Drop for State {
    fn drop(&mut self) {
        self.interface.abort();
    }
}
pub struct WireguardOutbound {
    server: String,
    port: u16,
    config: WireguardConfig,
    state: Mutex<Weak<State>>,
}
fn key(text: &str) -> Result<[u8; 32]> {
    let bytes = STANDARD
        .decode(text)
        .map_err(|_| anyhow::anyhow!("Invalid WireGuard key encoding"))?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("WireGuard keys must contain 32 bytes"))
}
impl WireguardOutbound {
    pub fn new(server: &str, port: u16, config: &WireguardConfig) -> Result<Self> {
        ensure!(
            !server.is_empty() && port != 0,
            "WireGuard peer endpoint is required"
        );
        ensure!(
            key(&config.private_key)? != [0; 32] && key(&config.public_key)? != [0; 32],
            "Invalid WireGuard key"
        );
        if let Some(psk) = &config.preshared_key {
            let _ = key(psk)?;
        }
        ensure!(
            !config.local_addresses.is_empty() && config.local_addresses.len() <= 2,
            "WireGuard requires one local address per IP family"
        );
        let address: Address = config
            .local_addresses
            .join(",")
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid WireGuard local addresses"))?;
        ensure!(
            address
                .addresses()
                .all(|ip| !ip.is_unspecified() && !ip.is_multicast()),
            "Invalid WireGuard local address"
        );
        ensure!(
            (576..=1420).contains(&config.mtu) && (!address.is_v6() || config.mtu >= 1280),
            "WireGuard MTU must be 576–1420, or at least 1280 with IPv6"
        );
        ensure!(
            !config.allowed_ips.is_empty() && config.allowed_ips.len() <= 64,
            "WireGuard requires 1–64 allowed networks"
        );
        // Parse into the library's peer type without exposing key material in errors.
        let _ = Self::peer(config, None)?;
        ensure!(
            config.dns.len() <= 4
                && config
                    .dns
                    .iter()
                    .all(|ip| !ip.is_unspecified() && !ip.is_multicast()),
            "Invalid WireGuard DNS servers"
        );
        Ok(Self {
            server: server.into(),
            port,
            config: config.clone(),
            state: Mutex::new(Weak::new()),
        })
    }
    fn peer(config: &WireguardConfig, endpoint: Option<SocketAddr>) -> Result<Peer> {
        Ok(Peer {
            endpoint,
            public_key: PublicKey::from(key(&config.public_key)?),
            preshared_key: config.preshared_key.as_deref().map(key).transpose()?,
            allowed_ips: config
                .allowed_ips
                .iter()
                .map(|network| {
                    network
                        .parse()
                        .map_err(|_| anyhow::anyhow!("Invalid WireGuard allowed network"))
                })
                .collect::<Result<_>>()?,
            persistent_keepalive: (config.persistent_keepalive != 0)
                .then_some(config.persistent_keepalive),
        })
    }
    async fn endpoint(&self) -> Result<Arc<State>> {
        let mut cached = self.state.lock().await;
        if let Some(state) = cached.upgrade()
            && !state.interface.is_closed()
        {
            return Ok(state);
        }
        static INSTANCES: OnceLock<Arc<Semaphore>> = OnceLock::new();
        let slot = INSTANCES
            .get_or_init(|| Arc::new(Semaphore::new(8)))
            .clone()
            .try_acquire_owned()
            .map_err(|_| anyhow::anyhow!("WireGuard endpoint capacity reached"))?;
        let peer_ip = match self.server.trim_matches(['[', ']']).parse::<IpAddr>() {
            Ok(ip) => ip,
            Err(_) => *super::transport::resolve_server_ips(&self.server)
                .await
                .context("WireGuard peer resolution failed")?
                .first()
                .context("WireGuard peer has no address")?,
        };
        let endpoint = SocketAddr::new(peer_ip, self.port);
        let socket = std::net::UdpSocket::bind(if peer_ip.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        })?;
        super::outbound_bind::apply_to_std_udp_socket(&socket, peer_ip.is_ipv6())?;
        socket.set_nonblocking(true)?;
        let address = self
            .config
            .local_addresses
            .join(",")
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid WireGuard address"))?;
        let config = Config {
            interface: tokio_wireguard::config::Interface {
                address,
                listen_port: None,
                private_key: StaticSecret::from(key(&self.config.private_key)?),
                mtu: Some(self.config.mtu),
            },
            peers: vec![Self::peer(&self.config, Some(endpoint))?],
        };
        let mut options = Options::default();
        options.tcp.connect_timeout = CONNECT_TIMEOUT;
        options.tcp.recv_buffer_size = 32 * 1024;
        options.tcp.send_buffer_size = 32 * 1024;
        options.udp.recv_buffer_size = 64 * 1024;
        options.udp.send_buffer_size = 64 * 1024;
        options.poll_interval = Duration::from_millis(100);
        let interface = Interface::new_with_udp_socket(config, options, socket)?;
        let state = Arc::new(State {
            interface,
            _slot: slot,
            flows: Arc::new(Semaphore::new(128)),
            dns: self.config.dns.clone(),
            mtu: self.config.mtu,
            allowed: Self::peer(&self.config, None)?.allowed_ips,
        });
        *cached = Arc::downgrade(&state);
        Ok(state)
    }
    async fn tcp(&self, host: &str, port: u16) -> Result<BoxProxyStream> {
        ensure!(port != 0, "Invalid WireGuard target port");
        tokio::time::timeout(CONNECT_TIMEOUT, async {
            let state = self.endpoint().await?;
            let slot = state
                .flows
                .clone()
                .try_acquire_owned()
                .map_err(|_| anyhow::anyhow!("WireGuard flow capacity reached"))?;
            let address = state.resolve(host).await?;
            let stream =
                TcpStream::connect(SocketAddr::new(address, port), &state.interface).await?;
            Ok(Box::new(OwnedStream {
                stream,
                _state: state,
                _slot: slot,
            }) as BoxProxyStream)
        })
        .await
        .map_err(|_| anyhow::anyhow!("WireGuard TCP connection timed out"))?
    }
    async fn udp(&self, host: &str, port: u16) -> Result<BoxDatagram> {
        if !self.config.udp {
            return Err(super::connector::UdpUnsupported::new("WireGuard UDP is disabled").into());
        }
        ensure!(port != 0, "WireGuard UDP target port is invalid");
        tokio::time::timeout(CONNECT_TIMEOUT, async {
            let state = self.endpoint().await?;
            let slot = state
                .flows
                .clone()
                .try_acquire_owned()
                .map_err(|_| anyhow::anyhow!("WireGuard flow capacity reached"))?;
            let address = state.resolve(host).await?;
            let socket = state.udp_socket(address.is_ipv6()).await?;
            Ok(Box::new(OwnedDatagram {
                socket,
                peer: SocketAddr::new(address, port),
                _state: state,
                _slot: slot,
            }) as BoxDatagram)
        })
        .await
        .map_err(|_| anyhow::anyhow!("WireGuard UDP setup timed out"))?
    }
}
impl State {
    async fn udp_socket(&self, ipv6: bool) -> Result<UdpSocket> {
        let ip = if ipv6 {
            self.interface.address().v6().map(IpAddr::V6)
        } else {
            self.interface.address().v4().map(IpAddr::V4)
        }
        .context("WireGuard target IP family has no local address")?;
        Ok(UdpSocket::bind(SocketAddr::new(ip, 0), &self.interface).await?)
    }
    async fn resolve(&self, host: &str) -> Result<IpAddr> {
        if let Ok(ip) = host.parse::<IpAddr>() {
            ensure!(
                self.allowed.iter().any(|net| net.contains(&ip)),
                "WireGuard target is outside the peer's allowed networks"
            );
            return Ok(ip);
        }
        ensure!(
            host.len() <= 253
                && host.trim_end_matches('.').split('.').all(|s| !s.is_empty()
                    && s.len() <= 63
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')),
            "Invalid WireGuard target hostname"
        );
        ensure!(
            !self.dns.is_empty(),
            "WireGuard hostname targets require explicit tunnel DNS; no system DNS fallback was used"
        );
        let qtypes = if self.interface.address().is_v4() {
            vec![1, 28]
        } else {
            vec![28]
        };
        for &server in &self.dns {
            if !self.allowed.iter().any(|net| net.contains(&server))
                || (server.is_ipv6() && !self.interface.address().is_v6())
                || (server.is_ipv4() && !self.interface.address().is_v4())
            {
                continue;
            }
            for &qtype in &qtypes {
                if qtype == 28 && !self.interface.address().is_v6() {
                    continue;
                }
                let socket = self.udp_socket(server.is_ipv6()).await?;
                let query = crate::dns::wire::build_dns_query(host, qtype);
                let peer = SocketAddr::new(server, 53);
                let reply = tokio::time::timeout(Duration::from_secs(2), async {
                    socket.send_to(&query, peer).await?;
                    let mut bytes = Vec::with_capacity(4096);
                    let (_, source) = socket.recv_from(&mut bytes).await?;
                    ensure!(
                        source == peer
                            && bytes.len() >= query.len()
                            && bytes[..2] == query[..2]
                            && bytes[12..query.len()] == query[12..],
                        "WireGuard DNS response did not match query"
                    );
                    crate::dns::wire::parse_dns_response(&bytes)
                })
                .await;
                if let Ok(Ok(ips)) = reply
                    && let Some(ip) = ips.into_iter().find(|ip| {
                        self.allowed.iter().any(|net| net.contains(ip))
                            && if qtype == 1 {
                                ip.is_ipv4()
                            } else {
                                ip.is_ipv6()
                            }
                    })
                {
                    return Ok(ip);
                }
            }
        }
        anyhow::bail!("WireGuard tunnel DNS lookup failed")
    }
}
struct OwnedStream {
    stream: TcpStream,
    _state: Arc<State>,
    _slot: OwnedSemaphorePermit,
}
impl AsyncRead for OwnedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}
impl AsyncWrite for OwnedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, data)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
struct OwnedDatagram {
    socket: UdpSocket,
    peer: SocketAddr,
    _state: Arc<State>,
    _slot: OwnedSemaphorePermit,
}
impl Datagram for OwnedDatagram {
    fn send<'a>(&'a self, packet: &'a [u8]) -> DatagramFuture<'a, ()> {
        Box::pin(async move {
            let max_payload =
                self._state
                    .mtu
                    .saturating_sub(if self.peer.is_ipv6() { 48 } else { 28 });
            ensure!(
                packet.len() <= max_payload,
                "WireGuard UDP payload exceeds the tunnel MTU; IP fragmentation is not supported"
            );
            let n = self.socket.send_to(packet, self.peer).await?;
            ensure!(n == packet.len(), "Partial WireGuard datagram write");
            Ok(())
        })
    }
    fn recv<'a>(&'a self, buffer: &'a mut [u8]) -> DatagramFuture<'a, usize> {
        Box::pin(async move {
            for _ in 0..16 {
                let mut bytes = Vec::with_capacity(65535);
                let (n, peer) = self.socket.recv_from(&mut bytes).await?;
                if peer != self.peer {
                    continue;
                }
                ensure!(
                    n <= buffer.len(),
                    "WireGuard datagram exceeds receive buffer"
                );
                buffer[..n].copy_from_slice(&bytes[..n]);
                return Ok(n);
            }
            anyhow::bail!("WireGuard received too many unrelated datagrams")
        })
    }
}
impl Outbound for WireguardOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let host = host.to_owned();
        Box::pin(async move { self.tcp(&host, port).await })
    }
    fn connect_datagram(&self, host: &str, port: u16) -> DatagramFuture<'_, BoxDatagram> {
        let host = host.to_owned();
        Box::pin(async move { self.udp(&host, port).await })
    }
    fn name(&self) -> &str {
        "wireguard"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    struct Server(Interface);
    impl Drop for Server {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    fn pair() -> (Server, WireguardOutbound) {
        let server_private = StaticSecret::from([11; 32]);
        let client_private = StaticSecret::from([22; 32]);
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        let mut options = Options::default();
        options.poll_interval = Duration::from_millis(10);
        options.tcp.connect_timeout = Duration::from_secs(2);
        let interface = Interface::new_with_udp_socket(
            Config {
                interface: tokio_wireguard::config::Interface {
                    private_key: server_private.clone(),
                    address: "203.0.113.2/32".parse().unwrap(),
                    listen_port: None,
                    mtu: Some(1280),
                },
                peers: vec![Peer {
                    public_key: PublicKey::from(&client_private),
                    preshared_key: Some([33; 32]),
                    endpoint: None,
                    allowed_ips: vec!["10.200.0.2/32".parse().unwrap()],
                    persistent_keepalive: None,
                }],
            },
            options,
            socket,
        )
        .unwrap();
        let config = WireguardConfig {
            private_key: STANDARD.encode(client_private.to_bytes()),
            public_key: STANDARD.encode(PublicKey::from(&server_private).as_bytes()),
            preshared_key: Some(STANDARD.encode([33; 32])),
            local_addresses: vec!["10.200.0.2/32".into()],
            dns: vec!["203.0.113.2".parse().unwrap()],
            ..Default::default()
        };
        (
            Server(interface),
            WireguardOutbound::new("127.0.0.1", port, &config).unwrap(),
        )
    }
    #[tokio::test]
    #[ignore = "requires checksum-verified SOCKROCKET_REFERENCE_SING_BOX test server"]
    async fn reference_wireguard_tcp_udp_and_cleanup() {
        struct Reference(std::process::Child, std::path::PathBuf);
        impl Drop for Reference {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
                let _ = std::fs::remove_dir_all(&self.1);
            }
        }
        let server_key = StaticSecret::from([11; 32]);
        let client_key = StaticSecret::from([22; 32]);
        let port = std::net::UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let dir =
            std::env::temp_dir().join(format!("sockrocket-wg-reference-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let config = serde_json::json!({
            "log":{"level":"error"},
            "endpoints":[{"type":"wireguard","tag":"wg","system":false,"address":["203.0.113.2/32"],"private_key":STANDARD.encode(server_key.to_bytes()),"listen_port":port,"mtu":1280,"workers":1,
                "peers":[{"public_key":STANDARD.encode(PublicKey::from(&client_key).as_bytes()),"pre_shared_key":STANDARD.encode([33;32]),"allowed_ips":["10.200.0.2/32"]}]}],
            "outbounds":[{"type":"direct","tag":"direct"}], "route":{"rules":[{"inbound":["wg"],"action":"route","outbound":"direct","override_address":"127.0.0.1"}],"final":"direct"}
        });
        let path = dir.join("server.json");
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let child = std::process::Command::new(
            std::env::var_os("SOCKROCKET_REFERENCE_SING_BOX").expect("reference server"),
        )
        .args(["run", "-c"])
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
        let mut reference = Reference(child, dir);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            reference.0.try_wait().unwrap().is_none(),
            "reference server exited"
        );
        let out = WireguardOutbound::new(
            "127.0.0.1",
            port,
            &WireguardConfig {
                private_key: STANDARD.encode(client_key.to_bytes()),
                public_key: STANDARD.encode(PublicKey::from(&server_key).as_bytes()),
                preshared_key: Some(STANDARD.encode([33; 32])),
                local_addresses: vec!["10.200.0.2/32".into()],
                ..Default::default()
            },
        )
        .unwrap();
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target = tcp.local_addr().unwrap();
        let task = tokio::spawn(async move {
            for _ in 0..8 {
                let (mut stream, _) = tcp.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut bytes = [0; 8192];
                    stream.read_exact(&mut bytes).await.unwrap();
                    stream.write_all(&bytes).await.unwrap();
                    stream.shutdown().await.unwrap();
                });
            }
        });
        let mut streams = Vec::new();
        for _ in 0..8 {
            streams.push(out.tcp("203.0.113.2", target.port()).await.unwrap());
        }
        for stream in &mut streams {
            let data = [0x67; 8192];
            stream.write_all(&data).await.unwrap();
            let mut reply = [0; 8192];
            tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut reply))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(reply, data);
        }
        task.await.unwrap();
        let udp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let target = udp.local_addr().unwrap();
        let task = tokio::spawn(async move {
            for _ in 0..2 {
                let mut data = [0; 4096];
                let (n, peer) = udp.recv_from(&mut data).await.unwrap();
                udp.send_to(&data[..n], peer).await.unwrap();
            }
        });
        let socket = out.udp("203.0.113.2", target.port()).await.unwrap();
        for data in [b"hello".as_slice(), b"second".as_slice()] {
            socket.send(data).await.unwrap();
            let mut reply = [0; 4096];
            let n = tokio::time::timeout(Duration::from_secs(5), socket.recv(&mut reply))
                .await
                .unwrap_or_else(|_| panic!("WireGuard reply timed out for {} bytes", data.len()))
                .unwrap();
            assert_eq!(&reply[..n], data);
        }
        task.await.unwrap();
        let weak = out.state.lock().await.clone();
        drop(streams);
        drop(socket);
        assert!(weak.upgrade().is_none());
    }
    #[tokio::test]
    async fn split_and_closing_streams_keep_the_port_reserved() {
        let (server, out) = pair();
        let listener = tokio_wireguard::TcpListener::bind("203.0.113.2:8443", &server.0)
            .await
            .unwrap();
        let accept = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            (listener, stream)
        });
        let state = out.endpoint().await.unwrap();
        let stream = TcpStream::connect("203.0.113.2:8443", &state.interface)
            .await
            .unwrap();
        let local = stream.local_addr().unwrap();
        let (read, write) = stream.into_split();
        drop(read);
        assert!(
            tokio_wireguard::TcpListener::bind(local, &state.interface)
                .await
                .is_err(),
            "a live split writer still owns its port"
        );
        drop(write);
        assert!(
            tokio_wireguard::TcpListener::bind(local, &state.interface)
                .await
                .is_err(),
            "the closing TCP socket must retain its port until teardown completes"
        );
        drop(accept.await.unwrap());
    }
    #[tokio::test]
    async fn disallowed_target_is_rejected_without_leaking_an_endpoint() {
        let (_server, mut out) = pair();
        out.config.allowed_ips = vec!["203.0.113.2/32".into()];
        let result = out.udp("198.51.100.1", 443).await;
        let Err(error) = result else {
            panic!("disallowed target was accepted")
        };
        assert!(error.to_string().contains("allowed networks"));
        assert!(out.state.lock().await.upgrade().is_none());
    }
    #[tokio::test]
    async fn wireguard_oversized_udp_fails_before_silent_packet_loss() {
        let (_server, out) = pair();
        let socket = out.udp("203.0.113.2", 8443).await.unwrap();
        assert!(
            socket
                .send(&vec![0; 1280])
                .await
                .unwrap_err()
                .to_string()
                .contains("MTU")
        );
        assert!(socket.send(&[0; 1200]).await.is_ok());
    }
    #[tokio::test]
    async fn native_wireguard_tcp_crosses_isolated_subnets_and_releases_endpoint_on_drop() {
        let (server, out) = pair();
        let listener = tokio_wireguard::TcpListener::bind("203.0.113.2:8443", &server.0)
            .await
            .unwrap();
        let echo = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut data = [0; 8192];
            stream.read_exact(&mut data).await.unwrap();
            stream.write_all(&data).await.unwrap();
        });
        let mut stream = out.tcp("203.0.113.2", 8443).await.unwrap();
        let data = vec![0x67; 8192];
        stream.write_all(&data).await.unwrap();
        let mut reply = vec![0; 8192];
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut reply))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply, data);
        let weak = out.state.lock().await.clone();
        drop(stream);
        echo.await.unwrap();
        assert!(
            weak.upgrade().is_none(),
            "last flow must release the userspace endpoint"
        );
    }
    #[tokio::test]
    async fn native_wireguard_udp_preserves_packets_and_closes_without_host_network_changes() {
        let (server, out) = pair();
        let echo = UdpSocket::bind("203.0.113.2:8443", &server.0)
            .await
            .unwrap();
        let task = tokio::spawn(async move {
            for _ in 0..3 {
                let mut data = Vec::new();
                let (_, peer) = echo.recv_from(&mut data).await.unwrap();
                echo.send_to(&data, peer).await.unwrap();
            }
        });
        let socket = out.udp("203.0.113.2", 8443).await.unwrap();
        for data in [
            b"first".as_slice(),
            b"second packet".as_slice(),
            b"".as_slice(),
        ] {
            socket.send(data).await.unwrap();
            let mut buffer = [0; 100];
            let n = tokio::time::timeout(Duration::from_secs(5), socket.recv(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&buffer[..n], data);
        }
        drop(socket);
        task.await.unwrap();
        assert!(out.state.lock().await.upgrade().is_none());
    }
    #[tokio::test]
    async fn domain_resolution_uses_configured_dns_inside_the_tunnel() {
        let (server, out) = pair();
        let dns = UdpSocket::bind("203.0.113.2:53", &server.0).await.unwrap();
        let task = tokio::spawn(async move {
            let mut query = Vec::new();
            let (_, peer) = dns.recv_from(&mut query).await.unwrap();
            query[2] = 0x81;
            query[3] = 0x80;
            query[6] = 0;
            query[7] = 1;
            query.extend([0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 30, 0, 4, 203, 0, 113, 2]);
            dns.send_to(&query, peer).await.unwrap();
        });
        let state = out.endpoint().await.unwrap();
        assert_eq!(
            state.resolve("synthetic.example").await.unwrap(),
            "203.0.113.2".parse::<IpAddr>().unwrap()
        );
        task.await.unwrap();
        drop(state);
        assert!(out.state.lock().await.upgrade().is_none());
    }
    #[test]
    fn key_mtu_and_network_validation_never_echo_secrets() {
        let mut config = WireguardConfig {
            private_key: "private-invalid-value".into(),
            public_key: STANDARD.encode([2; 32]),
            local_addresses: vec!["10.0.0.2/32".into()],
            ..Default::default()
        };
        let error = WireguardOutbound::new("127.0.0.1", 51820, &config)
            .err()
            .unwrap()
            .to_string();
        assert!(!error.contains("private-invalid-value"));
        config.private_key = STANDARD.encode([1; 32]);
        config.mtu = 100000;
        assert!(WireguardOutbound::new("127.0.0.1", 51820, &config).is_err());
        config.mtu = 1280;
        config.allowed_ips = vec!["not-a-network".into()];
        assert!(WireguardOutbound::new("127.0.0.1", 51820, &config).is_err());
        assert!(!format!("{config:?}").contains(&config.private_key));
    }
}
