//! Full wire exchanges. A checked-out pooled stream is owned by the future:
//! cancellation drops it instead of leaving an orphan reply for the next query.
use super::*;

impl DnsResolver {
    pub(super) async fn exchange_raw(&self, query: &[u8]) -> Result<Vec<u8>> {
        let (domain, _) = parse_dns_query(query)?;
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
            pending.spawn(async move { resolver.exchange_server(&server, &query).await });
        }
        let mut error = anyhow::anyhow!("No DNS servers configured");
        while let Some(result) = pending.join_next().await {
            match result {
                Ok(Ok(reply)) => return Ok(reply),
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
            Duration::from_secs(8),
            self.exchange_server_inner(server, query),
        )
        .await
        .map_err(|_| anyhow::anyhow!("DNS exchange timeout"))?
    }

    async fn exchange_server_inner(&self, server: &DnsServer, query: &[u8]) -> Result<Vec<u8>> {
        let reply = match server {
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
                let outbound = self.outbound.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("ProxyTcp/ProxyTls DNS configured but no outbound set")
                })?;
                let pool = if matches!(server, DnsServer::ProxyTls(..)) {
                    &self.proxy_tls_conn
                } else {
                    &self.proxy_tcp_conn
                };
                let mut slot = pool.acquire().await;
                let mut stream = match slot.take() {
                    Some(entry)
                        if Arc::ptr_eq(&entry.outbound.0, &outbound.0)
                            && entry.server == *server =>
                    {
                        entry.stream
                    }
                    _ => {
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
