//! Bounded UDP/TCP listeners with one coordinated shutdown owner.
use super::*;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::watch,
};

pub(super) async fn serve(
    bind: &str,
    port: u16,
    resolver: DnsResolver,
    shutdown: Arc<tokio::sync::Notify>,
) -> Result<()> {
    let udp = UdpSocket::bind((bind, port)).await?;
    let actual = udp.local_addr()?.port();
    let tcp = TcpListener::bind((bind, actual)).await?;
    let udp_stop = Arc::new(tokio::sync::Notify::new());
    let (tcp_stop, tcp_stop_rx) = watch::channel(false);
    let budget = crate::resources::ResourceBudget::detect();
    let mut tasks = tokio::task::JoinSet::new();
    tasks.spawn(serve_dns_socket(
        udp,
        resolver.clone(),
        udp_stop.clone(),
        budget.dns_queries,
    ));
    tasks.spawn(serve_tcp(tcp, resolver, tcp_stop_rx));
    tracing::info!("DNS listener: udp+tcp://{bind}:{actual}");
    let failure = tokio::select! {
        _ = shutdown.notified() => None,
        result = tasks.join_next() => Some(result),
    };
    udp_stop.notify_one();
    let _ = tcp_stop.send(true);
    while tasks.join_next().await.is_some() {}
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
