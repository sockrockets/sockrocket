//! Bounded UDP/TCP listeners with one coordinated shutdown owner.
use super::*;
use std::net::{Ipv6Addr, SocketAddrV6};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::watch,
};

/// Bind an IPv6-only socket on `port` without taking the IPv4 port.
///
/// `UdpSocket::bind("[::]")` claims the IPv4 port on some kernels unless
/// `IPV6_V6ONLY` is set before bind. The IPv4 listener already owns that port.
fn bind_ipv6_udp(port: u16) -> Result<UdpSocket> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV6,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    socket.set_only_v6(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, port, 0, 0).into())?;
    Ok(UdpSocket::from_std(std::net::UdpSocket::from(socket))?)
}

fn bind_ipv6_tcp(port: u16) -> Result<TcpListener> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV6,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    socket.set_only_v6(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddrV6::new(Ipv6Addr::UNSPECIFIED, port, 0, 0).into())?;
    socket.listen(128)?;
    Ok(TcpListener::from_std(std::net::TcpListener::from(socket))?)
}

pub(super) async fn serve(
    bind: &str,
    port: u16,
    resolver: DnsResolver,
    shutdown: Arc<tokio::sync::Notify>,
    ipv6_open: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> Result<()> {
    let udp = UdpSocket::bind((bind, port)).await?;
    let actual = udp.local_addr()?.port();
    let tcp = TcpListener::bind((bind, actual)).await?;
    // One Notify per UDP socket: notify_one stores a permit, so a socket
    // busy outside its select! still sees the stop.
    let udp_stop = Arc::new(tokio::sync::Notify::new());
    let v6_udp_stop = Arc::new(tokio::sync::Notify::new());
    let (tcp_stop, tcp_stop_rx) = watch::channel(false);
    let budget = crate::resources::ResourceBudget::detect();
    let mut required = tokio::task::JoinSet::new();
    let mut extra = tokio::task::JoinSet::new();
    required.spawn(serve_dns_socket(
        udp,
        resolver.clone(),
        udp_stop.clone(),
        budget.dns_queries,
        None,
    ));
    required.spawn(serve_tcp(tcp, resolver.clone(), tcp_stop_rx.clone(), None));
    if let Some(open) = ipv6_open {
        match (bind_ipv6_udp(actual), bind_ipv6_tcp(actual)) {
            (Ok(v6_udp), Ok(v6_tcp)) => {
                tracing::info!("DNS listener: udp+tcp://[::]:{actual} (LAN interception only)");
                extra.spawn(serve_dns_socket(
                    v6_udp,
                    resolver.clone(),
                    v6_udp_stop.clone(),
                    budget.dns_queries,
                    Some(open.clone()),
                ));
                extra.spawn(serve_tcp(v6_tcp, resolver, tcp_stop_rx, Some(open)));
            }
            (Err(error), _) | (_, Err(error)) => {
                tracing::warn!("IPv6 DNS listener unavailable: {error:#}");
            }
        }
    }
    tracing::info!("DNS listener: udp+tcp://{bind}:{actual}");
    let mut failure = None;
    loop {
        tokio::select! {
            biased;
            _ = shutdown.notified() => break,
            result = required.join_next() => {
                failure = Some(result);
                break;
            }
            result = extra.join_next(), if !extra.is_empty() => {
                match result {
                    Some(Ok(Err(error))) => {
                        tracing::warn!("IPv6 DNS listener stopped: {error:#}");
                    }
                    Some(Err(error)) => {
                        tracing::warn!("IPv6 DNS listener stopped: {error}");
                    }
                    _ => {}
                }
            }
        }
    }
    udp_stop.notify_one();
    v6_udp_stop.notify_one();
    let _ = tcp_stop.send(true);
    while required.join_next().await.is_some() {}
    extra.abort_all();
    while extra.join_next().await.is_some() {}
    match failure {
        None => Ok(()),
        Some(Some(Ok(Err(e)))) => Err(e),
        _ => anyhow::bail!("DNS listener exited unexpectedly"),
    }
}

async fn serve_tcp(
    listener: TcpListener,
    resolver: DnsResolver,
    mut stop: watch::Receiver<bool>,
    gate: Option<Arc<std::sync::atomic::AtomicBool>>,
) -> Result<()> {
    let limit = crate::resources::ResourceBudget::detect().dns_tcp_clients;
    let mut clients = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            Some(_) = clients.join_next(), if !clients.is_empty() => {},
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                if gate.as_ref().is_some_and(|open| !open.load(std::sync::atomic::Ordering::Relaxed)) {
                    continue;
                }
                if clients.len() >= limit { continue; }
                let resolver = resolver.clone();
                clients.spawn(async move { let _ = serve_client(stream, resolver).await; });
            }
        }
    }
    clients.abort_all();
    while clients.join_next().await.is_some() {}
    Ok(())
}
async fn serve_client(mut stream: TcpStream, resolver: DnsResolver) -> Result<()> {
    for _ in 0..64 {
        let len = tokio::time::timeout(Duration::from_secs(10), stream.read_u16()).await?? as usize;
        anyhow::ensure!(len >= 12, "Invalid TCP DNS frame");
        let mut query = vec![0; len];
        tokio::time::timeout(Duration::from_secs(5), stream.read_exact(&mut query)).await??;
        let reply = answer_raw_dns_query(&resolver, &query).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            stream
                .write_all(&(reply.len() as u16).to_be_bytes())
                .await?;
            stream.write_all(&reply).await
        })
        .await??;
    }
    Ok(())
}

/// Probe the complete local DNS query path, not merely an open socket.
pub async fn check_listener(address: std::net::SocketAddr) -> Result<()> {
    let query = build_dns_query("www.baidu.com", 1);
    let socket = UdpSocket::bind(if address.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    })
    .await?;
    socket.connect(address).await?;
    socket.send(&query).await?;
    let mut buffer = [0; 4096];
    let size = tokio::time::timeout(Duration::from_secs(3), socket.recv(&mut buffer)).await??;
    validate_response(&query, &buffer[..size])?;
    anyhow::ensure!(
        !parse_dns_response(&buffer[..size])?.is_empty(),
        "DNS health probe returned no addresses"
    );
    Ok(())
}
