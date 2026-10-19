//! Bounded UDP associations owned by the authenticated TCP control channel.
use super::{
    ProxyStats,
    connector::{SharedOutbound, UPSTREAM_CONNECT_TIMEOUT},
    datagram::Datagram,
};
use anyhow::{Result, ensure};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    net::{TcpStream, UdpSocket},
    sync::Semaphore,
    task::JoinHandle,
    time::Instant,
};
struct Flow {
    remote: Arc<dyn Datagram>,
    activity: Arc<Mutex<Instant>>,
    task: JoinHandle<()>,
}
impl Drop for Flow {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn budget() -> &'static Arc<Semaphore> {
    static B: OnceLock<Arc<Semaphore>> = OnceLock::new();
    B.get_or_init(|| Arc::new(Semaphore::new(128)))
}
fn decode(packet: &[u8]) -> Result<(String, u16, usize)> {
    ensure!(
        packet.len() >= 4 && packet[..3] == [0, 0, 0],
        "Fragmented or malformed UDP request"
    );
    let (host, end) = match packet[3] {
        1 => {
            ensure!(packet.len() >= 10, "Truncated IPv4 address");
            (
                std::net::Ipv4Addr::from(<[u8; 4]>::try_from(&packet[4..8])?).to_string(),
                8,
            )
        }
        4 => {
            ensure!(packet.len() >= 22, "Truncated IPv6 address");
            (
                std::net::Ipv6Addr::from(<[u8; 16]>::try_from(&packet[4..20])?).to_string(),
                20,
            )
        }
        3 => {
            ensure!(packet.len() >= 5, "Truncated domain");
            let end = 5 + packet[4] as usize;
            ensure!(end > 5 && packet.len() >= end + 2, "Invalid domain");
            (std::str::from_utf8(&packet[5..end])?.to_owned(), end)
        }
        _ => anyhow::bail!("Invalid UDP address type"),
    };
    let port = u16::from_be_bytes([packet[end], packet[end + 1]]);
    ensure!(port != 0, "Invalid UDP destination port");
    Ok((host, port, end + 2))
}
pub(super) async fn serve(
    mut control: TcpStream,
    outbound: SharedOutbound,
    requested_host: String,
    requested_port: u16,
    stats: ProxyStats,
) -> Result<()> {
    let peer = control.peer_addr()?;
    let requested: IpAddr = requested_host
        .parse()
        .map_err(|_| anyhow::anyhow!("UDP client must use an IP address"))?;
    ensure!(
        requested.is_unspecified() || requested == peer.ip(),
        "UDP client address differs from TCP peer"
    );
    let socket = Arc::new(UdpSocket::bind(SocketAddr::new(control.local_addr()?.ip(), 0)).await?);
    let address = socket.local_addr()?;
    let mut reply = vec![5, 0, 0];
    match address.ip() {
        IpAddr::V4(ip) => {
            reply.push(1);
            reply.extend(ip.octets());
        }
        IpAddr::V6(ip) => {
            reply.push(4);
            reply.extend(ip.octets());
        }
    }
    reply.extend(address.port().to_be_bytes());
    tokio::io::AsyncWriteExt::write_all(&mut control, &reply).await?;
    let association = async {
        let mut client = (requested_port != 0).then(|| SocketAddr::new(peer.ip(), requested_port));
        let mut flows: HashMap<(String, u16), Flow> = HashMap::new();
        let mut packet = vec![0; 65535];
        loop {
            let (n, source) =
                tokio::time::timeout(Duration::from_secs(120), socket.recv_from(&mut packet))
                    .await??;
            if source.ip() != peer.ip() || client.is_some_and(|client| client != source) {
                continue;
            }
            let Ok((host, port, offset)) = decode(&packet[..n]) else {
                continue;
            };
            client = Some(source);
            let key = (host.clone(), port);
            flows.retain(|_, flow| !flow.task.is_finished());
            if !flows.contains_key(&key) {
                if flows.len() >= 16
                    && let Some(oldest) = flows
                        .iter()
                        .min_by_key(|(_, f)| *f.activity.lock().unwrap())
                        .map(|(key, _)| key.clone())
                {
                    flows.remove(&oldest);
                }
                let Ok(permit) = budget().clone().try_acquire_owned() else {
                    continue;
                };
                let snapshot = outbound
                    .0
                    .runtime_snapshot()
                    .unwrap_or_else(|| outbound.clone());
                let Ok(Ok(remote)) = tokio::time::timeout(
                    UPSTREAM_CONNECT_TIMEOUT,
                    snapshot.0.connect_datagram(&host, port),
                )
                .await
                else {
                    continue;
                };
                let remote: Arc<dyn Datagram> = Arc::from(remote);
                let activity = Arc::new(Mutex::new(Instant::now()));
                let (read, active, socket, stats) = (
                    remote.clone(),
                    activity.clone(),
                    socket.clone(),
                    stats.clone(),
                );
                let header = packet[..offset].to_vec();
                let task = tokio::spawn(async move {
                    let _permit = permit;
                    let mut body = vec![0; 65535];
                    loop {
                        tokio::select! {
                            result = read.recv(&mut body) => {
                                let Ok(n) = result else { break; };
                                if n+header.len() > 65507 { continue; }
                                let mut response = header.clone(); response.extend_from_slice(&body[..n]);
                                if socket.send_to(&response,source).await.is_err() { break; }
                                stats.add_bytes(0,n as u64); *active.lock().unwrap()=Instant::now();
                            }
                            _ = tokio::time::sleep(Duration::from_secs(5)) => {
                                if active.lock().unwrap().elapsed() >= Duration::from_secs(30) { break; }
                            }
                        }
                    }
                });
                flows.insert(
                    key.clone(),
                    Flow {
                        remote,
                        activity,
                        task,
                    },
                );
            }
            if let Some(flow) = flows.get(&key) {
                if matches!(
                    tokio::time::timeout(
                        Duration::from_secs(5),
                        flow.remote.send(&packet[offset..n])
                    )
                    .await,
                    Ok(Ok(()))
                ) {
                    stats.add_bytes((n - offset) as u64, 0);
                    *flow.activity.lock().unwrap() = Instant::now();
                } else {
                    flows.remove(&key);
                }
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    tokio::select! {
        result = association => result,
        _ = async { let mut byte=[0]; loop { if !matches!(control.read(&mut byte).await, Ok(n) if n>0) { break; } } } => Ok(()),
    }
}
