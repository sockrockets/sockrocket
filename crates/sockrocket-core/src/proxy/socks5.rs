use anyhow::{Context, Result, bail};
use std::{borrow::Cow, net::IpAddr, sync::Arc};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use super::ProxyStats;
use super::connector::{SharedOutbound, connect_upstream};
use super::relay::relay_bidirectional;
use crate::dns::fakeip::{FakeIpPool, fake_ipv4};

/// SOCKS5 proxy server.
pub struct Socks5Server {
    listener: TcpListener,
    outbound: SharedOutbound,
    stats: ProxyStats,
}

impl Socks5Server {
    pub async fn bind(
        addr: &str,
        port: u16,
        outbound: SharedOutbound,
        stats: ProxyStats,
    ) -> Result<Self> {
        let listener = TcpListener::bind((addr.trim_matches(['[', ']']), port)).await?;
        tracing::info!("SOCKS5 server listening on {}:{}", addr, port);
        Ok(Self {
            listener,
            outbound,
            stats,
        })
    }

    pub fn local_port(&self) -> u16 {
        self.listener.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    pub async fn run(self, shutdown: watch::Receiver<bool>) -> Result<()> {
        self.run_with_fakeip(shutdown, None).await
    }

    pub(super) async fn run_with_fakeip(
        self,
        mut shutdown: watch::Receiver<bool>,
        fakeip: Option<Arc<FakeIpPool>>,
    ) -> Result<()> {
        let mut connections = tokio::task::JoinSet::new();
        let mut rejected = 0u64;
        loop {
            tokio::select! {
                biased;
                _ = shutdown.changed() => {
                    tracing::info!("SOCKS5 server shutting down");
                    break;
                }
                Some(_) = connections.join_next(), if !connections.is_empty() => {},
                result = self.listener.accept() => {
                    let (stream, peer) = match result {
                        Ok(accepted) => accepted,
                        Err(error) => { connections.abort_all(); while connections.join_next().await.is_some() {} return Err(error.into()); }
                    };
                    let Some(slot) = self.stats.try_slot() else {
                        rejected = rejected.saturating_add(1);
                        if rejected.is_power_of_two() { tracing::warn!(rejected, "Local proxy connection capacity reached"); }
                        continue;
                    };
                    // Buffer sizes stay at the kernel default so autotune can
                    // grow the window. A fixed 64 KiB lock capped Wi-Fi clients
                    // as well as the WAN hop.
                    stream.set_nodelay(true).ok();
                    let outbound = self.outbound.clone();
                    let stats = self.stats.clone();
                    let pool = fakeip.clone();
                    connections.spawn(async move {
                        let _slot = slot;
                        let result = super::routing::with_client_source(
                            Some(peer.ip()),
                            crate::traffic::with_traffic_scope(handle_socks5(stream, outbound, stats, pool)),
                        ).await;
                        if let Err(e) = result {
                            tracing::debug!("SOCKS5 connection from {} error: {}", peer, e);
                        }
                    });
                }
            }
        }
        connections.abort_all();
        while connections.join_next().await.is_some() {}
        Ok(())
    }
}

/// A client can resolve through the gateway before sending its SOCKS request.
/// Its synthetic address identifies both the original domain and its DNS family.
pub(super) fn restore_destination<'a>(
    host: &'a str,
    pool: Option<&FakeIpPool>,
) -> Result<(Cow<'a, str>, Option<IpAddr>)> {
    if let Some(pool) = pool
        && let Ok(ip) = host.parse::<IpAddr>()
        && let Some(token) = fake_ipv4(ip)
    {
        let domain = pool
            .lookup(token)
            .with_context(|| format!("fake-IP {ip} has no live mapping"))?;
        return Ok((Cow::Owned(domain), Some(ip)));
    }
    Ok((Cow::Borrowed(host), None))
}

/// Parse the SOCKS5 destination address from a stream.
async fn read_socks5_addr<R: AsyncRead + Unpin>(stream: &mut R) -> Result<(String, u16)> {
    let atyp = stream.read_u8().await?;
    let host = match atyp {
        0x01 => {
            let mut buf = [0u8; 4];
            stream.read_exact(&mut buf).await?;
            std::net::Ipv4Addr::from(buf).to_string()
        }
        0x03 => {
            let len = stream.read_u8().await?;
            let mut buf = vec![0u8; len as usize];
            stream.read_exact(&mut buf).await?;
            String::from_utf8(buf)?
        }
        0x04 => {
            let mut buf = [0u8; 16];
            stream.read_exact(&mut buf).await?;
            std::net::Ipv6Addr::from(buf).to_string()
        }
        _ => bail!("Unsupported SOCKS5 address type: 0x{:02x}", atyp),
    };
    let port = stream.read_u16().await?;
    Ok((host, port))
}

/// Build a SOCKS5 reply packet.
fn socks5_reply(rep: u8) -> [u8; 10] {
    // VER=5, REP, RSV=0, ATYP=1(IPv4), BND.ADDR=0.0.0.0, BND.PORT=0
    [0x05, rep, 0x00, 0x01, 0, 0, 0, 0, 0, 0]
}

/// Handle a single SOCKS5 connection.
async fn handle_socks5(
    mut stream: TcpStream,
    outbound: SharedOutbound,
    stats: ProxyStats,
    fakeip: Option<Arc<FakeIpPool>>,
) -> Result<()> {
    let (cmd, host, port) = tokio::time::timeout(super::PROXY_HANDSHAKE_TIMEOUT, async {
        // --- Auth Negotiation ---
        let version = stream.read_u8().await?;
        if version != 0x05 {
            bail!("Invalid SOCKS version: {}", version);
        }
        let nmethods = stream.read_u8().await?;
        let mut methods = vec![0u8; nmethods as usize];
        stream.read_exact(&mut methods).await?;

        // We accept NO AUTHENTICATION (0x00)
        if !methods.contains(&0x00) {
            // Reply: no acceptable methods
            stream.write_all(&[0x05, 0xFF]).await?;
            bail!("Client does not support NO AUTH method");
        }
        // Reply: chose NO AUTH
        stream.write_all(&[0x05, 0x00]).await?;

        // --- Request ---
        let ver = stream.read_u8().await?;
        if ver != 0x05 {
            bail!("Invalid SOCKS5 request version: {}", ver);
        }
        let cmd = stream.read_u8().await?;
        let rsv = stream.read_u8().await?;
        anyhow::ensure!(rsv == 0, "Invalid SOCKS5 reserved byte");

        let (host, port) = if cmd == 0x01 || cmd == 0x03 {
            read_socks5_addr(&mut stream).await?
        } else {
            (String::new(), 0)
        };
        Ok::<_, anyhow::Error>((cmd, host, port))
    })
    .await
    .map_err(|_| anyhow::anyhow!("SOCKS5 proxy handshake timed out"))??;

    match cmd {
        0x01 => {
            // CONNECT
            let (host, original_ip) = match restore_destination(&host, fakeip.as_deref()) {
                Ok(target) => target,
                Err(error) => {
                    stream.write_all(&socks5_reply(0x04)).await?;
                    return Err(error);
                }
            };
            tracing::debug!("SOCKS5 CONNECT to {}:{}", host, port);
            let connection = stats.track(format!("{}:{}", host, port), "SOCKS5");

            let connect = connect_upstream(&outbound, &host, port);
            match super::routing::with_tun_family(original_ip, connect).await {
                Ok(mut remote) => {
                    stream.write_all(&socks5_reply(0x00)).await?; // success
                    let (up, down) =
                        relay_bidirectional(&mut connection.observe(&mut stream), &mut remote)
                            .await?;
                    connection.finish(up, down);
                    tracing::debug!(
                        "SOCKS5 relay {}:{} done: up={} down={}",
                        host,
                        port,
                        up,
                        down
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "SOCKS5 outbound connect failed for {}:{}: {}",
                        host,
                        port,
                        e
                    );
                    stream.write_all(&socks5_reply(0x04)).await?; // host unreachable
                    return Err(e);
                }
            }
        }
        0x02 => {
            // BIND - not supported
            stream.write_all(&socks5_reply(0x07)).await?;
            bail!("SOCKS5 BIND not supported");
        }
        0x03 => {
            super::socks_udp::serve(stream, outbound, host, port, stats, fakeip.as_deref()).await?;
        }
        _ => {
            stream.write_all(&socks5_reply(0x07)).await?;
            bail!("Unknown SOCKS5 command: 0x{:02x}", cmd);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::connector::DirectOutbound;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn connect_and_udp_restore_fake_destinations_before_routing() {
        use crate::proxy::connector::{BoxProxyStream, Outbound};
        use crate::proxy::datagram::{BoxDatagram, DatagramFuture};
        use crate::proxy::routing::{TunFamily, tun_family};
        use tokio::sync::mpsc;

        struct Recorder(mpsc::UnboundedSender<(String, Option<TunFamily>)>);
        impl Outbound for Recorder {
            fn name(&self) -> &str {
                "recorder"
            }
            fn connect(&self, host: &str, _: u16) -> DatagramFuture<'_, BoxProxyStream> {
                self.0.send((host.to_owned(), tun_family())).unwrap();
                Box::pin(async { bail!("recorded") })
            }
            fn connect_datagram(&self, host: &str, _: u16) -> DatagramFuture<'_, BoxDatagram> {
                self.0.send((host.to_owned(), tun_family())).unwrap();
                Box::pin(async { bail!("recorded") })
            }
        }
        async fn request(port: u16, cmd: u8, address: &[u8]) -> (TcpStream, [u8; 10]) {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
            let mut bytes = vec![5, 1, 0, 5, cmd, 0];
            bytes.extend_from_slice(address);
            stream.write_all(&bytes).await.unwrap();
            let mut auth = [0; 2];
            stream.read_exact(&mut auth).await.unwrap();
            assert_eq!(auth, [5, 0]);
            let mut reply = [0; 10];
            stream.read_exact(&mut reply).await.unwrap();
            (stream, reply)
        }

        let pool = FakeIpPool::new();
        let domain = "chat.example.test";
        let v4 = pool.allocate(domain);
        let v6 = pool.allocate_v6(domain);
        let (tx, mut seen) = mpsc::unbounded_channel();
        let server = Socks5Server::bind(
            "127.0.0.1",
            0,
            SharedOutbound(Arc::new(Recorder(tx))),
            ProxyStats::default(),
        )
        .await
        .unwrap();
        let port = server.local_port();
        let (stop, rx) = watch::channel(false);
        let task = tokio::spawn(server.run_with_fakeip(rx, Some(pool)));
        let (control, relay) = request(port, 3, &[1, 0, 0, 0, 0, 0, 0]).await;
        assert_eq!(relay[1], 0);
        let udp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let relay_port = u16::from_be_bytes([relay[8], relay[9]]);
        for (atyp, octets, host, family) in [
            (1, v4.octets().to_vec(), domain, Some(TunFamily::V4)),
            (4, v6.octets().to_vec(), domain, Some(TunFamily::V6)),
            (1, vec![192, 0, 2, 1], "192.0.2.1", None),
            (
                3,
                [vec![domain.len() as u8], domain.as_bytes().to_vec()].concat(),
                domain,
                None,
            ),
        ] {
            let mut address = vec![atyp];
            address.extend(octets);
            address.extend(443u16.to_be_bytes());
            let (_, reply) = request(port, 1, &address).await;
            assert_eq!(reply[1], 4);
            assert_eq!(seen.try_recv().unwrap(), (host.into(), family));
            let mut packet = vec![0; 3];
            packet.extend(address);
            packet.push(42);
            udp.send_to(&packet, ("127.0.0.1", relay_port))
                .await
                .unwrap();
            let target = tokio::time::timeout(std::time::Duration::from_secs(2), seen.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(target, (host.into(), family));
        }
        let (_, unknown) = request(port, 1, &[1, 198, 18, 255, 1, 1, 187]).await;
        assert_eq!(unknown[1], 4);
        assert!(seen.try_recv().is_err());
        drop(control);
        stop.send(true).unwrap();
        task.await.unwrap().unwrap();
    }

    /// Helper: start a simple echo server.
    async fn echo_server() -> (u16, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if stream.write_all(&buf[..n]).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });
        (port, handle)
    }

    #[tokio::test]
    async fn test_socks5_connect_ipv4() {
        let (echo_port, _echo) = echo_server().await;

        let stats = ProxyStats::default();
        let outbound = SharedOutbound(Arc::new(DirectOutbound));
        let server = Socks5Server::bind("127.0.0.1", 0, outbound, stats)
            .await
            .unwrap();
        let socks_port = server.local_port();

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let server_handle = tokio::spawn(async move { server.run(shutdown_rx).await });

        // Connect as SOCKS5 client
        let mut client = TcpStream::connect(format!("127.0.0.1:{}", socks_port))
            .await
            .unwrap();

        // Auth: NO AUTH
        client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut resp = [0u8; 2];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp, [0x05, 0x00]);

        // CONNECT to echo server (IPv4: 127.0.0.1)
        let mut req = vec![0x05, 0x01, 0x00, 0x01, 127, 0, 0, 1];
        req.extend_from_slice(&echo_port.to_be_bytes());
        client.write_all(&req).await.unwrap();

        let mut resp = [0u8; 10];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp[0], 0x05); // version
        assert_eq!(resp[1], 0x00); // success

        // Send and receive data through tunnel
        client.write_all(b"hello socks5").await.unwrap();
        let mut buf = [0u8; 12];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello socks5");

        drop(client);
        let _ = shutdown_tx.send(true);
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn test_socks5_connect_domain() {
        let (echo_port, _echo) = echo_server().await;

        let stats = ProxyStats::default();
        let outbound = SharedOutbound(Arc::new(DirectOutbound));
        let server = Socks5Server::bind("127.0.0.1", 0, outbound, stats)
            .await
            .unwrap();
        let socks_port = server.local_port();

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let server_handle = tokio::spawn(async move { server.run(shutdown_rx).await });

        let mut client = TcpStream::connect(format!("127.0.0.1:{}", socks_port))
            .await
            .unwrap();

        // Auth
        client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut resp = [0u8; 2];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp, [0x05, 0x00]);

        // CONNECT with domain name "localhost"
        let domain = b"localhost";
        let mut req = vec![0x05, 0x01, 0x00, 0x03, domain.len() as u8];
        req.extend_from_slice(domain);
        req.extend_from_slice(&echo_port.to_be_bytes());
        client.write_all(&req).await.unwrap();

        let mut resp = [0u8; 10];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp[1], 0x00); // success

        client.write_all(b"domain test").await.unwrap();
        let mut buf = [0u8; 11];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"domain test");

        drop(client);
        let _ = shutdown_tx.send(true);
        let _ = server_handle.await;
    }

    #[tokio::test]
    async fn test_read_socks5_addr_ipv6() {
        // ATYP 0x04: 16-byte IPv6 address + port.
        let mut req = vec![0x04u8];
        req.extend_from_slice(&std::net::Ipv6Addr::LOCALHOST.octets());
        req.extend_from_slice(&8080u16.to_be_bytes());
        let mut cursor: &[u8] = &req;
        let (host, port) = read_socks5_addr(&mut cursor).await.unwrap();
        assert_eq!(host, "::1");
        assert_eq!(port, 8080);
    }

    /// SOCKS5 CONNECT to an IPv6 target end-to-end over the v6 loopback.
    /// Skips silently on hosts without an IPv6 loopback interface.
    #[tokio::test]
    async fn test_socks5_connect_ipv6() {
        let Ok(listener) = TcpListener::bind("[::1]:0").await else {
            return; // no IPv6 loopback on this host
        };
        let echo_port = listener.local_addr().unwrap().port();
        let _echo = tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if stream.write_all(&buf[..n]).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });

        let stats = ProxyStats::default();
        let outbound = SharedOutbound(Arc::new(DirectOutbound));
        let server = Socks5Server::bind("127.0.0.1", 0, outbound, stats)
            .await
            .unwrap();
        let socks_port = server.local_port();

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let server_handle = tokio::spawn(async move { server.run(shutdown_rx).await });

        let mut client = TcpStream::connect(format!("127.0.0.1:{}", socks_port))
            .await
            .unwrap();

        // Auth: NO AUTH
        client.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut resp = [0u8; 2];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp, [0x05, 0x00]);

        // CONNECT to the echo server via ATYP 0x04 (IPv6 ::1)
        let mut req = vec![0x05, 0x01, 0x00, 0x04];
        req.extend_from_slice(&std::net::Ipv6Addr::LOCALHOST.octets());
        req.extend_from_slice(&echo_port.to_be_bytes());
        client.write_all(&req).await.unwrap();

        let mut resp = [0u8; 10];
        client.read_exact(&mut resp).await.unwrap();
        assert_eq!(resp[0], 0x05);
        assert_eq!(resp[1], 0x00); // success

        client.write_all(b"hello ipv6").await.unwrap();
        let mut buf = [0u8; 10];
        client.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello ipv6");

        drop(client);
        let _ = shutdown_tx.send(true);
        let _ = server_handle.await;
    }
}
