//! Full wire exchanges. A checked-out pooled stream is owned by the future:
//! cancellation drops it instead of leaving an orphan reply for the next query.
use super::*;

impl DnsResolver {
    pub(super) async fn exchange_raw(&self, query: &[u8]) -> Result<Vec<u8>> {
        let (domain, _) = parse_dns_query(query)?;
        // Preserve EDNS and request flags in cache keys; only the transaction
        // ID is interchangeable. Bound wire residency to 1 MiB per generation.
        let mut key = query.to_vec();
        key[..2].fill(0);
        if self.config.cache_enabled {
            let mut cache = self.wire_cache.lock().await;
            if let Some((reply, expiry)) = cache.entries.get(&key)
                && *expiry > Instant::now()
            {
                let mut reply = reply.clone();
                wire::cap_ttl(
                    &mut reply,
                    expiry.saturating_duration_since(Instant::now()).as_secs() as u32,
                )?;
                reply[..2].copy_from_slice(&query[..2]);
                return Ok(reply);
            }
        }
        let domain = domain.trim_end_matches('.').to_ascii_lowercase();
        let servers = match self.match_domain_group(&domain) {
            DnsGroup::Primary => self
                .primary_override
                .as_deref()
                .unwrap_or(&self.config.servers),
            DnsGroup::Fallback => &self.config.fallback,
        };
        let mut pending = tokio::task::JoinSet::new();
        for server in servers {
            let resolver = self.clone();
            let server = server.clone();
            let query = query.to_vec();
            pending.spawn(async move {
                resolver
                    .scope(resolver.exchange_server(&server, &query))
                    .await
            });
        }
        let mut error = anyhow::anyhow!("No DNS servers configured");
        while let Some(result) = pending.join_next().await {
            match result {
                Ok(Ok(reply)) => {
                    if matches!(reply[3] & 15, 2 | 5) {
                        error = anyhow::anyhow!("DNS upstream returned a temporary failure");
                        continue;
                    }
                    if self.config.cache_enabled && reply[3] & 15 == 0 {
                        let ttl = wire::answer_ttl(&reply)?;
                        let ttl = if self.config.cache_ttl > 0 {
                            ttl.min(self.config.cache_ttl)
                        } else {
                            ttl
                        };
                        if ttl > 0 {
                            self.wire_cache.lock().await.put(
                                key,
                                reply.clone(),
                                Instant::now() + Duration::from_secs(u64::from(ttl)),
                            );
                        }
                    }
                    return Ok(reply);
                }
                Ok(Err(e)) => error = e,
                Err(e) => error = e.into(),
            }
        }
        Err(error)
    }

    pub(super) async fn exchange_server(
        &self,
        server: &DnsServer,
        query: &[u8],
    ) -> Result<Vec<u8>> {
        tokio::time::timeout(
            Duration::from_secs(self.config.timeout_secs.clamp(1, 12)),
            self.exchange_server_inner(server, query),
        )
        .await
        .map_err(|_| anyhow::anyhow!("DNS exchange timeout"))?
    }

    async fn exchange_server_inner(&self, server: &DnsServer, query: &[u8]) -> Result<Vec<u8>> {
        if let DnsServer::Udp(address) | DnsServer::Tcp(address) | DnsServer::Tls(address, _) =
            server
        {
            self.reject_self_resolver(*address).await?;
        }
        let reply = match server {
            DnsServer::Endpoint {
                url,
                proxy,
                bootstrap,
            } => {
                self.exchange_endpoint(server, url, *proxy, bootstrap, query)
                    .await?
            }
            DnsServer::Tcp(addr) | DnsServer::Tls(addr, _) => {
                let stream = connect_upstream(
                    &SharedOutbound::direct(),
                    &addr.ip().to_string(),
                    addr.port(),
                )
                .await?;
                let stream = if let DnsServer::Tls(_, sni) = server {
                    Box::new(
                        self.tls_connector
                            .connect(rustls_pki_types::ServerName::try_from(sni.clone())?, stream)
                            .await?,
                    ) as BoxProxyStream
                } else {
                    stream
                };
                raw_tcp(stream, query).await?
            }
            DnsServer::Udp(addr) => {
                let socket = UdpSocket::bind(if addr.is_ipv4() {
                    "0.0.0.0:0"
                } else {
                    "[::]:0"
                })
                .await?;
                crate::proxy::outbound_bind::apply_to_udp_socket(&socket, addr.is_ipv6())?;
                socket.connect(addr).await?;
                socket.send(query).await?;
                let mut reply = vec![0; 65535];
                let len =
                    tokio::time::timeout(Duration::from_secs(3), socket.recv(&mut reply)).await??;
                reply.truncate(len);
                if reply.len() >= 12 && reply[..2] == query[..2] && reply[2] & 0x02 != 0 {
                    // RFC 1035 truncation: retry the same server over TCP.
                    let stream = connect_upstream(
                        &SharedOutbound::direct(),
                        &addr.ip().to_string(),
                        addr.port(),
                    )
                    .await?;
                    raw_tcp(stream, query).await?
                } else {
                    reply
                }
            }
            DnsServer::DoH(url) => {
                let response = self
                    .doh_client
                    .post(url)
                    .header("Content-Type", "application/dns-message")
                    .header("Accept", "application/dns-message")
                    .body(query.to_vec())
                    .send()
                    .await?
                    .error_for_status()?;
                // Limit the complete body even when Content-Length is missing.
                let mut response = response;
                let mut bytes = Vec::new();
                while let Some(chunk) = response.chunk().await? {
                    anyhow::ensure!(bytes.len() + chunk.len() <= 65535, "Oversized DoH response");
                    bytes.extend_from_slice(&chunk);
                }
                bytes
            }
            DnsServer::ProxyTcp(addr) | DnsServer::ProxyTls(addr, _) => {
                let configured = self.outbound.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("ProxyTcp/ProxyTls DNS configured but no outbound set")
                })?;
                let raw_outbound = configured.0.proxy_transport();
                let outbound = raw_outbound.as_ref().unwrap_or(configured);
                let pool = if matches!(server, DnsServer::ProxyTls(..)) {
                    &self.proxy_tls_conn
                } else {
                    &self.proxy_tcp_conn
                };
                // Hold the slot through a miss so concurrent proxy dials stay
                // bounded by the pool. A matching stream is reused; anything
                // else is put back before this slot dials.
                let mut slot = pool.acquire().await;
                let mut stream = match slot.take() {
                    Some(entry)
                        if Arc::ptr_eq(&entry.outbound.0, &outbound.0)
                            && entry.server == *server =>
                    {
                        entry.stream
                    }
                    other => {
                        *slot = other;
                        let stream =
                            connect_upstream(outbound, &addr.ip().to_string(), addr.port()).await?;
                        if let DnsServer::ProxyTls(_, sni) = server {
                            let name = rustls_pki_types::ServerName::try_from(sni.clone())?;
                            Box::new(self.tls_connector.connect(name, stream).await?)
                                as BoxProxyStream
                        } else {
                            stream
                        }
                    }
                };
                let reply = raw_tcp_ref(&mut stream, query).await?;
                validate_response(query, &reply)?;
                *slot = Some(PooledDnsStream {
                    stream,
                    outbound: outbound.clone(),
                    server: server.clone(),
                });
                reply
            }
        };
        validate_response(query, &reply)?;
        Ok(reply)
    }

    /// No packets are sent: the kernel's chosen source identifies an endpoint
    /// on this host, including public router addresses hidden behind a name.
    async fn reject_self_resolver(&self, address: SocketAddr) -> Result<()> {
        if !self.config.reject_local_addresses {
            return Ok(());
        }
        let socket = UdpSocket::bind(if address.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        })
        .await?;
        socket.connect(address).await?;
        let normalize = |ip: IpAddr| match ip {
            IpAddr::V6(ip) => ip
                .to_ipv4_mapped()
                .map(IpAddr::V4)
                .unwrap_or(IpAddr::V6(ip)),
            ip => ip,
        };
        anyhow::ensure!(
            normalize(socket.local_addr()?.ip()) != normalize(address.ip()),
            "DNS resolver loop: endpoint belongs to a local interface"
        );
        Ok(())
    }

    async fn endpoint_addresses(
        &self,
        host: &str,
        bootstrap: &[SocketAddr],
        via_proxy: Option<SharedOutbound>,
    ) -> Result<Vec<IpAddr>> {
        if let Ok(ip) = host.parse() {
            return Ok(vec![ip]);
        }
        let key = (host.to_owned(), bootstrap.to_vec(), via_proxy.is_some());
        loop {
            {
                let mut cache = self.endpoint_cache.lock().await;
                if let Some((expiry, ips)) = cache.get(&key)
                    && *expiry > Instant::now()
                {
                    return Ok(ips.clone());
                }
            }
            enum BootstrapRole {
                Lead(Arc<tokio::sync::Notify>),
                Wait(Arc<tokio::sync::Notify>),
            }
            let role = {
                let mut waits = self
                    .endpoint_wait
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if let Some(existing) = waits.get(&key).cloned() {
                    BootstrapRole::Wait(existing)
                } else {
                    let notify = Arc::new(tokio::sync::Notify::new());
                    waits.insert(key.clone(), notify.clone());
                    BootstrapRole::Lead(notify)
                }
            };
            let notify = match role {
                BootstrapRole::Wait(notify) => {
                    let notified = notify.notified();
                    tokio::pin!(notified);
                    notified.as_mut().enable();
                    {
                        let mut cache = self.endpoint_cache.lock().await;
                        if let Some((expiry, ips)) = cache.get(&key)
                            && *expiry > Instant::now()
                        {
                            return Ok(ips.clone());
                        }
                    }
                    let still_waiting = self
                        .endpoint_wait
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .get(&key)
                        .is_some_and(|current| Arc::ptr_eq(current, &notify));
                    if still_waiting {
                        notified.await;
                    }
                    continue;
                }
                BootstrapRole::Lead(notify) => notify,
            };
            let guard = EndpointWaitGuard {
                map: self.endpoint_wait.clone(),
                key: key.clone(),
                notify,
            };
            let ips = self
                .query_bootstrap(host, bootstrap, via_proxy.clone())
                .await?;
            self.endpoint_cache
                .lock()
                .await
                .put(key, (Instant::now() + Duration::from_secs(60), ips.clone()));
            drop(guard);
            return Ok(ips);
        }
    }

    async fn query_bootstrap(
        &self,
        host: &str,
        bootstrap: &[SocketAddr],
        via_proxy: Option<SharedOutbound>,
    ) -> Result<Vec<IpAddr>> {
        let mut tasks = tokio::task::JoinSet::new();
        if let Some(outbound) = via_proxy {
            // The TCP query leaves through the proxy. Asking a domestic
            // resolver directly would expose the DoH/DoT name on the LAN path.
            for address in bootstrap.iter().take(4) {
                for qtype in [1, 28] {
                    let address = *address;
                    let outbound = outbound.clone();
                    let query = build_dns_query(host, qtype);
                    tasks.spawn(async move {
                        let stream = tokio::time::timeout(Duration::from_secs(5), async {
                            connect_upstream(&outbound, &address.ip().to_string(), address.port())
                                .await
                        })
                        .await
                        .map_err(|_| anyhow::anyhow!("DNS bootstrap timeout"))??;
                        let response = raw_tcp(stream, &query).await?;
                        validate_response(&query, &response)?;
                        parse_dns_response(&response)
                    });
                }
            }
            return collect_bootstrap(&mut tasks).await;
        }
        for address in bootstrap.iter().take(4) {
            for qtype in [1, 28] {
                let address = *address;
                let query = build_dns_query(host, qtype);
                let checker = self.clone();
                tasks.spawn(async move {
                    checker.reject_self_resolver(address).await?;
                    tokio::time::timeout(Duration::from_secs(2), async {
                        let socket = UdpSocket::bind(if address.is_ipv4() {
                            "0.0.0.0:0"
                        } else {
                            "[::]:0"
                        })
                        .await?;
                        crate::proxy::outbound_bind::apply_to_udp_socket(
                            &socket,
                            address.is_ipv6(),
                        )?;
                        socket.connect(address).await?;
                        socket.send(&query).await?;
                        let mut response = vec![0; 4096];
                        let size = socket.recv(&mut response).await?;
                        response.truncate(size);
                        if response.len() >= 12
                            && response[..2] == query[..2]
                            && response[2] & 2 != 0
                        {
                            let stream = connect_upstream(
                                &SharedOutbound::direct(),
                                &address.ip().to_string(),
                                address.port(),
                            )
                            .await?;
                            response = raw_tcp(stream, &query).await?;
                        }
                        validate_response(&query, &response)?;
                        parse_dns_response(&response)
                    })
                    .await
                    .map_err(|_| anyhow::anyhow!("DNS bootstrap timeout"))?
                });
            }
        }
        collect_bootstrap(&mut tasks).await
    }

    async fn exchange_endpoint(
        &self,
        server: &DnsServer,
        value: &str,
        proxy: bool,
        bootstrap: &[SocketAddr],
        query: &[u8],
    ) -> Result<Vec<u8>> {
        use http_body_util::{BodyExt, Full};
        let url = url::Url::parse(value).map_err(|_| anyhow::anyhow!("Invalid DNS endpoint"))?;
        let host = url.host_str().unwrap_or("").trim_matches(['[', ']']);
        let server_name = url
            .query_pairs()
            .find(|(key, _)| key == "server_name")
            .map(|(_, value)| value.into_owned())
            .unwrap_or_else(|| host.to_owned());
        let port = url.port().unwrap_or(match url.scheme() {
            "https" => 443,
            "tls" => 853,
            _ => 53,
        });
        let direct = SharedOutbound::direct();
        let configured = if proxy {
            self.outbound
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Proxy DNS has no outbound"))?
        } else {
            &direct
        };
        let raw_outbound = if proxy {
            configured.0.proxy_transport()
        } else {
            None
        };
        let outbound = raw_outbound.as_ref().unwrap_or(configured);
        // HTTPS is a one-shot HTTP/1 exchange and must not occupy a TLS pool
        // slot during bootstrap or the handshake. TLS/TCP DNS reuse a slot
        // only for the exchange itself.
        let https = url.scheme() == "https";
        let reused = if https {
            None
        } else {
            let mut slot = self.proxy_tls_conn.acquire().await;
            if slot.as_ref().is_some_and(|entry| {
                entry.server == *server
                    && (if proxy {
                        Arc::ptr_eq(&entry.outbound.0, &outbound.0)
                    } else {
                        entry.outbound.0.is_direct()
                    })
            }) {
                slot.take().map(|entry| entry.stream)
            } else {
                None
            }
        };
        let mut stream = if let Some(stream) = reused {
            stream
        } else {
            let ips = self
                .endpoint_addresses(host, bootstrap, proxy.then(|| outbound.clone()))
                .await?;
            let mut attempts = tokio::task::JoinSet::new();
            for (index, ip) in ips.into_iter().take(8).enumerate() {
                let outbound = outbound.clone();
                let resolver = self.clone();
                attempts.spawn(async move {
                    if !proxy {
                        resolver.reject_self_resolver((ip, port).into()).await?;
                    }
                    tokio::time::sleep(Duration::from_millis(200 * index as u64)).await;
                    resolver
                        .scope(connect_upstream(&outbound, &ip.to_string(), port))
                        .await
                });
            }
            let mut connected = None;
            while let Some(result) = attempts.join_next().await {
                if let Ok(Ok(stream)) = result {
                    connected = Some(stream);
                    break;
                }
            }
            drop(attempts);
            let stream =
                connected.ok_or_else(|| anyhow::anyhow!("DNS endpoint connection failed"))?;
            if matches!(url.scheme(), "tls" | "https") {
                Box::new(
                    self.tls_connector
                        .connect(
                            rustls_pki_types::ServerName::try_from(if url.scheme() == "tls" {
                                server_name
                            } else {
                                host.to_owned()
                            })?,
                            stream,
                        )
                        .await
                        .map_err(|_| {
                            anyhow::anyhow!("DNS TLS handshake or certificate verification failed")
                        })?,
                ) as BoxProxyStream
            } else {
                stream
            }
        };
        if !https {
            let reply = raw_tcp_ref(&mut stream, query).await?;
            validate_response(query, &reply)?;
            *self.proxy_tls_conn.acquire().await = Some(PooledDnsStream {
                stream,
                outbound: outbound.clone(),
                server: server.clone(),
            });
            return Ok(reply);
        }
        let io = hyper_util::rt::TokioIo::new(stream);
        let (mut sender, connection) = hyper::client::conn::http1::handshake(io).await?;
        // Owned by this exchange even on timeout/cancellation; never detach
        // the HTTP driver into a permanent background task.
        let exchange = async {
            let path = if url.path().is_empty() {
                "/dns-query"
            } else {
                url.path()
            };
            let path = match url.query() {
                Some(q) => format!("{path}?{q}"),
                None => path.to_owned(),
            };
            let authority = &url[url::Position::BeforeHost..url::Position::AfterPort];
            let request = http::Request::post(path)
                .header("Host", authority)
                .header("Content-Type", "application/dns-message")
                .header("Accept", "application/dns-message")
                .body(Full::new(bytes::Bytes::copy_from_slice(query)))?;
            let response = sender.send_request(request).await?;
            anyhow::ensure!(
                response.status().is_success(),
                "DNS HTTPS returned HTTP {}",
                response.status().as_u16()
            );
            anyhow::ensure!(
                response
                    .headers()
                    .get("content-type")
                    .and_then(|s| s.to_str().ok())
                    .is_some_and(|s| s.split(';').next().is_some_and(|kind| kind
                        .trim()
                        .eq_ignore_ascii_case("application/dns-message"))),
                "DNS HTTPS returned an invalid content type"
            );
            let mut body = response.into_body();
            let mut reply = Vec::new();
            while let Some(frame) = body.frame().await {
                if let Ok(data) = frame?.into_data() {
                    anyhow::ensure!(reply.len() + data.len() <= 65535, "Oversized DoH response");
                    reply.extend_from_slice(&data);
                }
            }
            Ok::<_, anyhow::Error>(reply)
        };
        tokio::pin!(connection);
        tokio::pin!(exchange);
        tokio::select! {
            biased;
            result = &mut exchange => result,
            _ = &mut connection => exchange.await,
        }
    }
}

async fn collect_bootstrap(
    tasks: &mut tokio::task::JoinSet<Result<Vec<IpAddr>>>,
) -> Result<Vec<IpAddr>> {
    let mut ips = Vec::new();
    while let Some(result) = tasks.join_next().await {
        if let Ok(Ok(found)) = result {
            for ip in found {
                if crate::config::dns::valid_resolver_address((ip, 53).into()) && !ips.contains(&ip)
                {
                    ips.push(ip);
                }
            }
        }
    }
    anyhow::ensure!(
        !ips.is_empty(),
        "DNS bootstrap returned no usable addresses"
    );
    ips.sort_by_key(|ip| ip.is_ipv6());
    Ok(ips)
}

struct EndpointWaitGuard {
    map: Arc<std::sync::Mutex<HashMap<super::EndpointKey, Arc<tokio::sync::Notify>>>>,
    key: super::EndpointKey,
    notify: Arc<tokio::sync::Notify>,
}
impl Drop for EndpointWaitGuard {
    fn drop(&mut self) {
        if let Ok(mut waits) = self.map.lock()
            && waits
                .get(&self.key)
                .is_some_and(|notify| Arc::ptr_eq(notify, &self.notify))
        {
            waits.remove(&self.key);
        }
        self.notify.notify_waiters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::connector::Outbound;
    use crate::proxy::datagram::{BoxDatagram, DatagramFuture};
    use std::net::Ipv4Addr;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    struct Dial {
        hosts: Arc<Mutex<Vec<String>>>,
    }
    impl Outbound for Dial {
        fn connect(
            &self,
            host: &str,
            port: u16,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<BoxProxyStream>> + Send + '_>>
        {
            self.hosts.lock().unwrap().push(host.to_owned());
            let host = host.to_owned();
            Box::pin(async move {
                if host != "127.0.0.1" {
                    anyhow::bail!("name was not resolved through the proxy: {host}");
                }
                let stream = tokio::net::TcpStream::connect((host.as_str(), port)).await?;
                Ok(Box::new(stream) as BoxProxyStream)
            })
        }
        fn connect_datagram(&self, _: &str, _: u16) -> DatagramFuture<'_, BoxDatagram> {
            Box::pin(async { anyhow::bail!("unused") })
        }
        fn name(&self) -> &str {
            "dial"
        }
    }

    #[tokio::test]
    async fn proxied_endpoint_name_is_not_resolved_on_the_lan_path() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut len = [0u8; 2];
                    if socket.read_exact(&mut len).await.is_err() {
                        return;
                    }
                    let mut query = vec![0; u16::from_be_bytes(len) as usize];
                    if socket.read_exact(&mut query).await.is_err() {
                        return;
                    }
                    let Ok(reply) = super::super::wire::build_dns_response(
                        &query,
                        &[std::net::IpAddr::V4(Ipv4Addr::new(203, 0, 113, 10))],
                    ) else {
                        return;
                    };
                    let _ = socket.write_all(&(reply.len() as u16).to_be_bytes()).await;
                    let _ = socket.write_all(&reply).await;
                });
            }
        });
        let hosts = Arc::new(Mutex::new(Vec::new()));
        let mut resolver = DnsResolver::new();
        resolver.set_outbound(SharedOutbound(Arc::new(Dial {
            hosts: hosts.clone(),
        })));
        let query = build_dns_query("example.com", 1);
        let result = resolver
            .exchange_server(
                &DnsServer::Endpoint {
                    url: "tcp://dns.example:53".into(),
                    proxy: true,
                    bootstrap: vec![format!("127.0.0.1:{port}").parse().unwrap()],
                },
                &query,
            )
            .await;
        assert!(result.is_err());
        let seen = hosts.lock().unwrap().clone();
        assert!(
            seen.iter().any(|host| host == "127.0.0.1"),
            "bootstrap query did not leave through the proxy: {seen:?}"
        );
        assert!(
            seen.iter().all(|host| host != "dns.example"),
            "proxied DNS name was dialed directly: {seen:?}"
        );
    }
}

async fn raw_tcp(mut stream: BoxProxyStream, query: &[u8]) -> Result<Vec<u8>> {
    raw_tcp_ref(&mut stream, query).await
}
async fn raw_tcp_ref(stream: &mut BoxProxyStream, query: &[u8]) -> Result<Vec<u8>> {
    anyhow::ensure!(query.len() <= 65535, "Oversized DNS query");
    stream
        .write_all(&(query.len() as u16).to_be_bytes())
        .await?;
    stream.write_all(query).await?;
    let len = stream.read_u16().await? as usize;
    anyhow::ensure!(len >= 12, "Truncated TCP DNS header");
    let mut reply = vec![0; len];
    stream.read_exact(&mut reply).await?;
    validate_response(query, &reply)?;
    Ok(reply)
}
