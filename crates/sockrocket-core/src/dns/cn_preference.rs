//! Mainland address selection for the default rule policy. Raw resolver
//! replies retain their separate primary/direct cache and in-flight keys. A
//! selected or synthetic reply is never stored under either upstream's key.
use super::*;
use std::sync::OnceLock;

impl DnsResolver {
    pub(super) fn can_prefer_cn(&self, domain: &str, qtype: u16) -> bool {
        if !self.config.prefer_cn_answers
            || self.direct_resolution
            || !matches!(qtype, QTYPE_A | QTYPE_AAAA)
            || self.config.hosts.contains_key(domain)
            || domain.parse::<IpAddr>().is_ok()
            || self.is_server_domain(domain)
        {
            return false;
        }
        let Some((_, tld)) = domain.rsplit_once('.') else {
            return false;
        };
        if matches!(
            tld,
            "localhost"
                | "local"
                | "arpa"
                | "lan"
                | "internal"
                | "home"
                | "onion"
                | "invalid"
                | "example"
                | "test"
        ) {
            return false;
        }
        // An explicit primary DNS suffix is just as intentional as a direct
        // suffix. Match at label boundaries without creating suffix strings.
        let suffixes = std::iter::once(domain).chain(
            domain
                .match_indices('.')
                .map(|(offset, _)| &domain[offset + 1..]),
        );
        for suffix in suffixes {
            if self.config.domain_rules.contains_key(suffix)
                || self.config.node_hosts.contains(suffix)
            {
                return false;
            }
        }
        self.outbound.as_ref().is_none_or(|outbound| {
            !outbound.0.is_direct()
                && outbound
                    .0
                    .routing_policy()
                    .is_none_or(|router| router.route_domain_only(domain).is_none())
        })
    }

    /// Both client-facing requests and real-address route resolution enter
    /// here, before the parsed-address cache. Only the original wire replies
    /// are cached, so policy changes cannot reuse a selected/fake address.
    pub(super) async fn cn_preferred_reply(
        &self,
        query: &[u8],
        domain: &str,
        qtype: u16,
    ) -> Result<Vec<u8>> {
        let budget = Duration::from_secs(self.config.timeout_secs.clamp(1, 12));
        let latest_probe = tokio::time::Instant::now() + budget - Duration::from_millis(20);
        let primary = self.exchange_raw(query).await?;
        if primary[3] & 15 != 0 {
            return Ok(primary);
        }
        let original = address_records(&primary, qtype).unwrap_or_default();
        if original.is_empty() {
            // A CNAME-only or negative answer retains its authority section.
            // Unrelated addresses must not be returned as a successful route.
            if parse_dns_response(&primary)?.is_empty() {
                return Ok(primary);
            }
            return build_dns_response_with_ttl(query, &[], 0);
        }
        static MAINLAND: OnceLock<GeoIpDb> = OnceLock::new();
        let geo = self
            .geoip
            .as_deref()
            .unwrap_or_else(|| MAINLAND.get_or_init(crate::router::china_geoip_db));
        let qualifies = |ip: &IpAddr| public_address(*ip) && geo.is_cn(*ip);
        let mut selected: Vec<_> = original.iter().copied().filter(qualifies).collect();
        let mut ttl = wire::answer_ttl(&primary)?;
        let primary_servers = self
            .primary_override
            .as_deref()
            .unwrap_or(&self.config.servers);
        if selected.is_empty()
            && original.iter().all(|ip| public_address(*ip))
            && !self.config.fallback.is_empty()
            && primary_servers != &self.config.fallback
        {
            // The primary has already succeeded. Leave the LAN listener time
            // to deliver its response even when every optional resolver stalls.
            let now = tokio::time::Instant::now();
            if now < latest_probe {
                let mut direct = self.clone();
                direct.direct_resolution = true;
                // Do not copy the client's EDNS subnet/cookie into the optional probe.
                let probe = build_dns_query(domain, qtype);
                let deadline = latest_probe.min(now + Duration::from_millis(250));
                if let Ok(Ok(reply)) =
                    tokio::time::timeout_at(deadline, direct.exchange_raw(&probe)).await
                    && let Ok(ips) = address_records(&reply, qtype)
                {
                    selected = ips.into_iter().filter(qualifies).collect();
                    ttl = ttl.min(wire::answer_ttl(&reply)?);
                }
            }
        }
        if !selected.is_empty() {
            selected.truncate(128);
            let keep_primary = selected == original && parse_dns_response(&primary)? == original;
            if self.config.cache_ttl != 0 {
                ttl = ttl.min(self.config.cache_ttl);
            }
            if let Some(pool) = &self.fakeip {
                ttl = ttl.min(30);
                self.record_real_answers(domain, pool, &selected, ttl, true);
            }
            if keep_primary {
                // Retain the original CNAME chain and flags when every address
                // was already suitable. Changing TTL does not alter DNS owners.
                let mut reply = primary;
                wire::cap_ttl(&mut reply, ttl)?;
                return Ok(reply);
            }
            return build_dns_response_with_ttl(query, &selected, ttl);
        }
        let Some(pool) = &self.fakeip else {
            if parse_dns_response(&primary)? == original && original.len() <= 128 {
                return Ok(primary);
            }
            // A response may contain addresses belonging to an unrelated
            // owner. Keep those out of both the wire answer and route input.
            let count = original.len().min(128);
            return build_dns_response_with_ttl(
                query,
                &original[..count],
                wire::answer_ttl(&primary)?,
            );
        };
        let fake = if qtype == QTYPE_A {
            vec![IpAddr::V4(pool.allocate(domain))]
        } else if self.config.dual_stack_fakeip {
            vec![IpAddr::V6(pool.allocate_v6(domain))]
        } else {
            Vec::new()
        };
        build_dns_response_with_ttl(query, &fake, 5)
    }
}

fn public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, _, _] = ip.octets();
            a != 0
                && a < 224
                && !ip.is_private()
                && !ip.is_link_local()
                && !ip.is_loopback()
                && !ip.is_documentation()
                && !(a == 100 && (64..128).contains(&b))
                && !(a == 198 && (18..=19).contains(&b))
                && !(a == 192 && b == 0 && ip.octets()[2] == 0)
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            segments[0] >> 13 == 1
                && !(segments[0] == 0x2001 && segments[1] < 0x0200)
                && !(segments[0] == 0x2001 && segments[1] == 0xdb8)
                && segments[0] != 0x2002
                && !(segments[0] == 0x3fff && segments[1] < 0x1000)
        }
    }
}

// Compare DNS names as lower-cased length-prefixed labels; embedded dots in a
// label cannot alias the requested name. Bound pointer depth and expanded size.
fn read_name(data: &[u8], mut pos: usize) -> Option<(Vec<u8>, usize)> {
    let mut name = Vec::new();
    let mut end = None;
    let mut pointers = 0;
    loop {
        let len = usize::from(*data.get(pos)?);
        if len & 0xc0 == 0xc0 {
            let pointer = ((len & 0x3f) << 8) | usize::from(*data.get(pos + 1)?);
            if pointer >= pos || pointers == 32 {
                return None;
            }
            pointers += 1;
            end.get_or_insert(pos + 2);
            pos = pointer;
        } else if len == 0 {
            return Some((name, end.unwrap_or(pos + 1)));
        } else {
            if len > 63 || name.len() + len + 1 > 254 {
                return None;
            }
            name.push(len as u8);
            name.extend(
                data.get(pos + 1..pos + 1 + len)?
                    .iter()
                    .map(u8::to_ascii_lowercase),
            );
            pos += 1 + len;
        }
    }
}

// The upstream has already validated the response against the question. Only
// addresses owned by its terminal CNAME belong to that lookup. Unrelated
// answer records and records in additional/authority cannot select a route.
fn address_records(reply: &[u8], qtype: u16) -> Result<Vec<IpAddr>> {
    anyhow::ensure!(
        reply.len() >= 12 && reply[3] & 15 == 0,
        "no successful DNS answer"
    );
    let count = usize::from(u16::from_be_bytes([reply[6], reply[7]]));
    anyhow::ensure!(count <= 256, "too many address records");
    let (mut target, mut pos) =
        read_name(reply, 12).ok_or_else(|| anyhow::anyhow!("invalid question name"))?;
    pos += 4;
    let mut aliases = HashMap::new();
    let mut ips = Vec::new();
    for _ in 0..count {
        let (owner, next) =
            read_name(reply, pos).ok_or_else(|| anyhow::anyhow!("invalid answer name"))?;
        let header = reply
            .get(next..next + 10)
            .ok_or_else(|| anyhow::anyhow!("missing record header"))?;
        let kind = u16::from_be_bytes([header[0], header[1]]);
        let size = usize::from(u16::from_be_bytes([header[8], header[9]]));
        let start = next + 10;
        pos = start + size;
        let body = reply
            .get(start..pos)
            .ok_or_else(|| anyhow::anyhow!("truncated record"))?;
        if header[2..4] != [0, 1] {
            continue;
        }
        if kind == 5 {
            let (name, end) =
                read_name(reply, start).ok_or_else(|| anyhow::anyhow!("invalid alias"))?;
            anyhow::ensure!(
                end == pos && aliases.insert(owner, name).is_none(),
                "ambiguous alias"
            );
        } else if kind == qtype {
            let ip = match (kind, size) {
                (QTYPE_A, 4) => IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(body)?)),
                (QTYPE_AAAA, 16) => IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(body)?)),
                _ => continue,
            };
            ips.push((owner, ip));
        }
    }
    for _ in 0..=aliases.len() {
        if let Some(next) = aliases.get(&target) {
            target = next.clone();
        } else {
            return Ok(ips
                .into_iter()
                .filter_map(|(owner, ip)| (owner == target).then_some(ip))
                .collect());
        }
    }
    bail!("CNAME loop")
}

pub(super) fn requests_dnssec(query: &[u8]) -> bool {
    if query.len() < 12 || query[3] & 0x30 != 0 {
        return true;
    }
    if query[6..12] == [0; 6] {
        return false;
    }
    // Only standard EDNS OPT extensions take part. A DNSSEC-aware client
    // must see the existing policy and the original signed RRsets.
    if query[6..10] != [0; 4] || query[10..12] != [0, 1] {
        return true;
    }
    let Some((_, end)) = read_name(query, 12) else {
        return true;
    };
    let Some(opt) = query.get(end + 4..) else {
        return true;
    };
    opt.len() < 11 || opt[..3] != [0, 0, 41] || opt[7] & 0x80 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_addresses_on_the_answer_alias_chain_are_eligible() {
        let query = build_dns_query("web.deepseek.com", QTYPE_A);
        let mut reply = build_dns_response_with_ttl(&query, &[], 3).unwrap();
        reply[7] = 3;
        let cn = [116, 205, 40, 114];
        let other = [1, 1, 1, 1];
        // An unrelated owner appears before the CNAME; the real endpoint
        // refers to the compressed name inside the alias RDATA.
        reply.extend_from_slice(b"\x01x\xc0\x10");
        reply.extend_from_slice(&[0, 1, 0, 1, 0, 0, 0, 2, 0, 4]);
        reply.extend_from_slice(&other);
        reply.extend_from_slice(&[0xc0, 12, 0, 5, 0, 1, 0, 0, 0, 3, 0, 7]);
        let target = reply.len();
        reply.extend_from_slice(b"\x04edge\xc0\x10");
        reply.extend_from_slice(&[0xc0, target as u8, 0, 1, 0, 1, 0, 0, 0, 4, 0, 4]);
        reply.extend_from_slice(&cn);
        assert_eq!(
            address_records(&reply, QTYPE_A).unwrap(),
            vec![IpAddr::from(cn)]
        );
        assert!(address_records(&reply, QTYPE_AAAA).unwrap().is_empty());
    }

    #[test]
    fn synthetic_and_non_public_prefixes_are_ineligible() {
        for ip in [
            "127.0.0.1",
            "100.64.0.2",
            "198.19.0.1",
            "192.0.2.2",
            "224.0.0.1",
            "2001:db8::8",
            "2001:2:0:4765::c612:2",
            "fd00::1",
            "::ffff:116.205.40.114",
        ] {
            assert!(!public_address(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["116.205.40.114", "2400:3200::1"] {
            assert!(public_address(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn preserves_dnssec_queries() {
        let mut query = build_dns_query("chat.deepseek.com", QTYPE_AAAA);
        assert!(!requests_dnssec(&query));
        query[11] = 1;
        query.extend_from_slice(&[0, 0, 41, 4, 208, 0, 0, 0, 0, 0, 0]);
        assert!(!requests_dnssec(&query));
        let do_flag = query.len() - 4;
        query[do_flag] = 0x80;
        assert!(requests_dnssec(&query));
    }
}
