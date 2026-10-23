use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;

use anyhow::Result;
use shadowsocks::ProxyClientStream;
use shadowsocks::config::{ServerAddr, ServerConfig, ServerType};
use shadowsocks::context::Context;
use shadowsocks::crypto::CipherKind;
use shadowsocks::net::ConnectOpts;

use super::connector::{BoxProxyStream, Outbound};

/// Shadowsocks outbound connector using the shadowsocks crate.
pub struct SsOutbound {
    server_config: ServerConfig,
    udp_enabled: bool,
    context: shadowsocks::context::SharedContext,
}

impl SsOutbound {
    pub fn new(server: &str, port: u16, cipher: &str, password: &str) -> Result<Self> {
        let method = CipherKind::from_str(cipher)
            .map_err(|_| anyhow::anyhow!("Unknown SS cipher: {}", cipher))?;

        let server_addr: ServerAddr = (server.to_string(), port).into();
        let server_config = ServerConfig::new(server_addr, password, method)
            .map_err(|e| anyhow::anyhow!("Invalid SS server config: {}", e))?;

        let context = Context::new_shared(ServerType::Local);

        Ok(Self {
            server_config,
            udp_enabled: true,
            context,
        })
    }
    pub fn with_udp(mut self, enabled: bool) -> Self {
        self.udp_enabled = enabled;
        self
    }
}

/// Build the SS target address: IP literals go out as ATYP IPv4/IPv6, not as
/// a domain string — a "::1" DomainNameAddress would be DNS-resolved by the
/// server and fail. Brackets are stripped defensively (some config paths
/// keep them).
fn target_address(host: &str, port: u16) -> shadowsocks::relay::Address {
    match host.trim_matches(['[', ']']).parse::<std::net::IpAddr>() {
        Ok(ip) => shadowsocks::relay::Address::SocketAddress(std::net::SocketAddr::new(ip, port)),
        Err(_) => shadowsocks::relay::Address::DomainNameAddress(host.to_string(), port),
    }
}

impl Outbound for SsOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        let addr = target_address(host, port);
        Box::pin(async move {
            // Shadowsocks dials its own socket, so it never reaches
            // `tune_socket`. Keep TCP_NODELAY, and leave both buffer sizes
            // unset: pinning 64 KiB disables Linux autotune and caps the
            // flow at buffer/RTT. `None` means the crate does not call
            // setsockopt, matching every other outbound.
            let opts = ConnectOpts {
                bind_interface: super::outbound_bind::outbound_bind().and_then(|bind| bind.if_name),
                tcp: shadowsocks::net::TcpSocketOpts {
                    send_buffer_size: None,
                    recv_buffer_size: None,
                    nodelay: true,
                    ..Default::default()
                },
                ..Default::default()
            };
            let stream = ProxyClientStream::connect_with_opts(
                self.context.clone(),
                &self.server_config,
                addr,
                &opts,
            )
            .await?;

            Ok(Box::new(stream) as BoxProxyStream)
        })
    }

    fn connect_datagram(
        &self,
        host: &str,
        port: u16,
    ) -> super::datagram::DatagramFuture<'_, super::datagram::BoxDatagram> {
        let address = target_address(host, port);
        Box::pin(async move {
            if !self.udp_enabled {
                return Err(super::connector::UdpUnsupported::new(
                    "UDP disabled for this Shadowsocks node",
                )
                .into());
            }
            let opts = ConnectOpts {
                bind_interface: super::outbound_bind::outbound_bind().and_then(|bind| bind.if_name),
                ..Default::default()
            };
            let socket =
                shadowsocks::relay::udprelay::proxy_socket::ProxySocket::connect_with_opts(
                    self.context.clone(),
                    &self.server_config,
                    &opts,
                )
                .await?;
            Ok(Box::new(SsDatagram {
                socket,
                address,
                session: rand::random(),
                packet: std::sync::atomic::AtomicU64::new(0),
                replay: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            }) as super::datagram::BoxDatagram)
        })
    }

    fn name(&self) -> &str {
        "shadowsocks"
    }
}

struct SsDatagram {
    socket: shadowsocks::relay::udprelay::proxy_socket::ProxySocket<shadowsocks::net::UdpSocket>,
    address: shadowsocks::relay::Address,
    session: u64,
    packet: std::sync::atomic::AtomicU64,
    replay: tokio::sync::Mutex<std::collections::HashMap<u64, ReplayWindow>>,
}
#[derive(Default)]
struct ReplayWindow {
    highest: u64,
    bits: u128,
}
impl ReplayWindow {
    fn accept(&mut self, id: u64) -> bool {
        if self.bits == 0 {
            self.highest = id;
            self.bits = 1;
            return true;
        }
        if id > self.highest {
            let shift = id - self.highest;
            self.bits = if shift >= 128 {
                1
            } else {
                (self.bits << shift) | 1
            };
            self.highest = id;
            return true;
        }
        let age = self.highest - id;
        if age >= 128 || self.bits & (1u128 << age) != 0 {
            return false;
        }
        self.bits |= 1u128 << age;
        true
    }
}
impl super::datagram::Datagram for SsDatagram {
    fn send<'a>(&'a self, packet: &'a [u8]) -> super::datagram::DatagramFuture<'a, ()> {
        Box::pin(async move {
            use shadowsocks::relay::udprelay::options::UdpSocketControlData;
            use std::sync::atomic::Ordering;
            let mut id = self.packet.load(Ordering::Relaxed);
            let id = loop {
                let next = id
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("Shadowsocks UDP packet counter exhausted"))?;
                match self.packet.compare_exchange_weak(
                    id,
                    next,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break id,
                    Err(current) => id = current,
                }
            };
            let mut control = UdpSocketControlData::default();
            control.client_session_id = self.session;
            control.packet_id = id;
            self.socket
                .send_with_ctrl(&self.address, &control, packet)
                .await?;
            Ok(())
        })
    }
    fn recv<'a>(&'a self, packet: &'a mut [u8]) -> super::datagram::DatagramFuture<'a, usize> {
        Box::pin(async move {
            let mut wire = vec![0; 65536];
            loop {
                let (size, _, _, control) = self.socket.recv_with_ctrl(&mut wire).await?;
                if let Some(control) = control {
                    if control.client_session_id != self.session {
                        continue;
                    }
                    let mut replay = self.replay.lock().await;
                    anyhow::ensure!(
                        replay.len() < 4 || replay.contains_key(&control.server_session_id),
                        "Too many Shadowsocks server sessions"
                    );
                    if !replay
                        .entry(control.server_session_id)
                        .or_default()
                        .accept(control.packet_id)
                    {
                        continue;
                    }
                }
                anyhow::ensure!(
                    size <= packet.len(),
                    "Shadowsocks UDP receive buffer too small"
                );
                packet[..size].copy_from_slice(&wire[..size]);
                return Ok(size);
            }
        })
    }
}

/// Map a common cipher name (from Clash/V2Ray configs) to shadowsocks-crypto format.
pub fn normalize_ss_cipher(cipher: &str) -> &str {
    match cipher {
        "aes-128-gcm" => "aes-128-gcm",
        "aes-256-gcm" => "aes-256-gcm",
        "chacha20-ietf-poly1305" | "chacha20-poly1305" => "chacha20-ietf-poly1305",
        "2022-blake3-aes-128-gcm" => "2022-blake3-aes-128-gcm",
        "2022-blake3-aes-256-gcm" => "2022-blake3-aes-256-gcm",
        "2022-blake3-chacha20-poly1305" => "2022-blake3-chacha20-poly1305",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_window_rejects_duplicates_and_accepts_reordered_packets() {
        let mut window = ReplayWindow::default();
        assert!(window.accept(3));
        assert!(window.accept(1));
        assert!(!window.accept(1));
        assert!(!window.accept(3));
        assert!(window.accept(200));
        assert!(!window.accept(1));
        assert!(window.accept(199));
    }

    #[tokio::test]
    async fn encrypted_udp_and_aead2022_sessions_roundtrip() {
        use base64::Engine;
        use shadowsocks::relay::udprelay::proxy_socket::ProxySocket;
        for (cipher, password) in [
            ("aes-256-gcm", "test-password".to_string()),
            (
                "2022-blake3-aes-128-gcm",
                base64::engine::general_purpose::STANDARD.encode([42u8; 16]),
            ),
        ] {
            let server_config = ServerConfig::new(
                "127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap(),
                password.clone(),
                CipherKind::from_str(cipher).unwrap(),
            )
            .unwrap();
            let server = ProxySocket::bind(Context::new_shared(ServerType::Server), &server_config)
                .await
                .unwrap();
            let addr = server.local_addr().unwrap();
            let password2 = password.clone();
            let task = tokio::spawn(async move {
                let mut wire = vec![0; 65536];
                let mut sessions = std::collections::HashSet::new();
                for counter in 0..2 {
                    let (size, peer, target, _, control) =
                        server.recv_from_with_ctrl(&mut wire).await.unwrap();
                    let mut response = control.unwrap_or_default();
                    sessions.insert(response.client_session_id);
                    response.server_session_id = 17;
                    response.packet_id = counter;
                    server
                        .send_to_with_ctrl(peer, &target, &response, &wire[..size])
                        .await
                        .unwrap();
                }
                if cipher.starts_with("2022") {
                    assert_eq!(sessions.len(), 2);
                }
            });
            let outbound = SsOutbound::new("127.0.0.1", addr.port(), cipher, &password2).unwrap();
            for _ in 0..2 {
                let udp = outbound.connect_datagram("192.0.2.1", 53).await.unwrap();
                udp.send(b"hello").await.unwrap();
                let mut reply = [0; 5];
                let size =
                    tokio::time::timeout(std::time::Duration::from_secs(2), udp.recv(&mut reply))
                        .await
                        .unwrap()
                        .unwrap();
                assert_eq!(&reply[..size], b"hello");
            }
            task.await.unwrap();
        }
    }

    #[test]
    fn test_ss_outbound_creation() {
        let outbound = SsOutbound::new("127.0.0.1", 8388, "aes-256-gcm", "password123");
        assert!(outbound.is_ok());
        assert_eq!(outbound.unwrap().name(), "shadowsocks");
    }

    #[test]
    fn test_ss_outbound_aead_2022() {
        // AEAD 2022 ciphers require base64-encoded keys of specific length
        let key = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [0u8; 32]);
        let outbound = SsOutbound::new("127.0.0.1", 8388, "2022-blake3-aes-256-gcm", &key);
        assert!(outbound.is_ok());
    }

    #[test]
    fn test_ss_outbound_invalid_cipher() {
        let outbound = SsOutbound::new("127.0.0.1", 8388, "invalid-cipher", "password123");
        assert!(outbound.is_err());
    }

    #[test]
    fn test_target_address_ip_literals_become_socket_addresses() {
        use shadowsocks::relay::Address;

        match target_address("2001:db8::1", 443) {
            Address::SocketAddress(sa) => {
                assert!(sa.is_ipv6());
                assert_eq!(sa.port(), 443);
            }
            Address::DomainNameAddress(..) => panic!("v6 literal must not be a domain address"),
        }
        // Bracketed form is normalized to the same result.
        match target_address("[::1]", 1080) {
            Address::SocketAddress(sa) => assert_eq!(sa.ip().to_string(), "::1"),
            Address::DomainNameAddress(..) => panic!("bracketed v6 must not be a domain address"),
        }
        match target_address("127.0.0.1", 8080) {
            Address::SocketAddress(sa) => assert!(sa.is_ipv4()),
            Address::DomainNameAddress(..) => panic!("v4 literal must not be a domain address"),
        }
        match target_address("example.com", 443) {
            Address::DomainNameAddress(host, port) => {
                assert_eq!(host, "example.com");
                assert_eq!(port, 443);
            }
            Address::SocketAddress(..) => panic!("domain must stay a domain address"),
        }
    }

    #[test]
    fn test_normalize_cipher() {
        assert_eq!(
            normalize_ss_cipher("chacha20-poly1305"),
            "chacha20-ietf-poly1305"
        );
        assert_eq!(normalize_ss_cipher("aes-256-gcm"), "aes-256-gcm");
    }
}
