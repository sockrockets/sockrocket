//! DNS wire codec. No resolver, socket or runtime state belongs here.
use anyhow::{Result, bail};
use std::net::{IpAddr, Ipv4Addr};

pub(crate) fn build_dns_query(domain: &str, qtype: u16) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);

    // Header
    let id: u16 = rand::random();
    buf.extend_from_slice(&id.to_be_bytes()); // ID
    buf.extend_from_slice(&[0x01, 0x00]); // Flags: standard query, recursion desired
    buf.extend_from_slice(&[0x00, 0x01]); // QDCOUNT: 1
    buf.extend_from_slice(&[0x00, 0x00]); // ANCOUNT: 0
    buf.extend_from_slice(&[0x00, 0x00]); // NSCOUNT: 0
    buf.extend_from_slice(&[0x00, 0x00]); // ARCOUNT: 0

    // Question section
    for label in domain
        .trim_end_matches('.')
        .split('.')
        .filter(|label| !label.is_empty())
    {
        let bytes = label.as_bytes();
        buf.push(bytes.len() as u8);
        buf.extend_from_slice(bytes);
    }
    buf.push(0x00); // Root label

    buf.extend_from_slice(&qtype.to_be_bytes()); // QTYPE
    buf.extend_from_slice(&[0x00, 0x01]); // QCLASS: IN

    buf
}

/// Parse a DNS response and extract IP addresses.
pub(crate) fn parse_dns_response(data: &[u8]) -> Result<Vec<IpAddr>> {
    if data.len() < 12 {
        bail!("DNS response too short");
    }

    if data[2] & 0x80 == 0 || data[2] & 0x02 != 0 {
        bail!("Not a complete DNS response");
    }
    let ancount = u16::from_be_bytes([data[6], data[7]]) as usize;
    let rcode = data[3] & 0x0F;

    if rcode != 0 {
        bail!("DNS response error: rcode={}", rcode);
    }

    // Skip header (12 bytes) and question section
    let mut pos = 12;

    // Skip question section (QDCOUNT = data[4..6])
    let qdcount = u16::from_be_bytes([data[4], data[5]]) as usize;
    for _ in 0..qdcount {
        pos = skip_dns_name(data, pos)?;
        pos += 4; // QTYPE + QCLASS
        if pos > data.len() {
            bail!("Truncated DNS question");
        }
    }

    // Parse answer section
    let mut addrs = Vec::new();
    for _ in 0..ancount {
        if pos >= data.len() {
            bail!("Missing DNS answer");
        }
        pos = skip_dns_name(data, pos)?;
        if pos + 10 > data.len() {
            bail!("Truncated DNS answer header");
        }

        let rtype = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let rdlength = u16::from_be_bytes([data[pos + 8], data[pos + 9]]) as usize;
        pos += 10;

        if pos + rdlength > data.len() {
            bail!("Truncated DNS record");
        }

        match rtype {
            1 if rdlength == 4 => {
                // A record
                let ip = Ipv4Addr::new(data[pos], data[pos + 1], data[pos + 2], data[pos + 3]);
                addrs.push(IpAddr::V4(ip));
            }
            28 if rdlength == 16 => {
                // AAAA record
                let mut octets = [0u8; 16];
                octets.copy_from_slice(&data[pos..pos + 16]);
                addrs.push(IpAddr::V6(octets.into()));
            }
            _ => {}
        }
        pos += rdlength;
    }

    Ok(addrs)
}

/// Skip a DNS name (handles compression pointers).
fn skip_dns_name(data: &[u8], mut pos: usize) -> Result<usize> {
    let start = pos;
    loop {
        let len = *data
            .get(pos)
            .ok_or_else(|| anyhow::anyhow!("Truncated DNS name"))? as usize;
        if len == 0 {
            return Ok(pos + 1);
        }
        if len & 0xc0 == 0xc0 {
            let low = *data
                .get(pos + 1)
                .ok_or_else(|| anyhow::anyhow!("Truncated DNS pointer"))?;
            let target = ((len & 0x3f) << 8) | low as usize;
            if target >= pos {
                bail!("Invalid DNS compression pointer");
            }
            return Ok(pos + 2);
        }
        if len > 63 || pos + 1 + len > data.len() || pos + 1 + len - start > 254 {
            bail!("Invalid DNS name length");
        }
        pos += 1 + len;
    }
}

/// Extract the queried domain name and query type from a raw DNS packet.
///
/// Returns `(domain, qtype)` where qtype is 1 for A, 28 for AAAA, etc.
pub fn parse_dns_query(data: &[u8]) -> Result<(String, u16)> {
    let question = question(data)?;
    Ok((question.domain, question.qtype))
}

struct Question {
    domain: String,
    qtype: u16,
    end: usize,
}

// One validated question boundary is shared by decoding and response builders.
fn question(data: &[u8]) -> Result<Question> {
    if data.len() < 12 || data[2] & 0xf8 != 0 || data[4..6] != [0, 1] {
        bail!("Expected a standard DNS query with one question");
    }
    let mut pos = 12;
    let mut labels = Vec::new();
    loop {
        let len = *data
            .get(pos)
            .ok_or_else(|| anyhow::anyhow!("Missing DNS name"))? as usize;
        pos += 1;
        if len == 0 {
            break;
        }
        if len > 63 || pos + len > data.len() || pos + len - 12 > 254 {
            bail!("Invalid DNS label or name length");
        }
        labels.push(std::str::from_utf8(&data[pos..pos + len])?);
        pos += len;
    }
    if pos + 4 > data.len() || data[pos + 2..pos + 4] != [0, 1] {
        bail!("Missing question fields or unsupported DNS class");
    }
    Ok(Question {
        domain: labels.join("."),
        qtype: u16::from_be_bytes([data[pos], data[pos + 1]]),
        end: pos + 4,
    })
}

/// Build a DNS response packet from a query packet and a set of resolved IPs.
///
/// Preserves the query ID and question section, adds A/AAAA answers with
/// the default TTL (300s).
pub fn build_dns_response(query: &[u8], addresses: &[IpAddr]) -> Result<Vec<u8>> {
    build_dns_response_with_ttl(query, addresses, 300)
}

/// Like [`build_dns_response`] but with an explicit answer TTL.
///
/// Fake-IP answers need a short TTL (a few seconds): the fake address is
/// only meaningful while the pool remembers the mapping, and long client /
/// dnsmasq caching would outlive evictions and resolvers being restarted.
pub fn build_dns_response_with_ttl(
    query: &[u8],
    addresses: &[IpAddr],
    ttl: u32,
) -> Result<Vec<u8>> {
    let qend = question(query)?.end;
    if addresses.len() > u16::MAX as usize {
        bail!("Too many DNS answer records");
    }

    let an_count = addresses.len() as u16;
    let mut resp = Vec::with_capacity(qend + addresses.len() * 16 + 32);

    // Copy header from query
    resp.extend_from_slice(&query[..2]); // ID

    // Flags: response, recursion desired+available, no error
    resp.extend_from_slice(&[0x80 | (query[2] & 1), 0x80]);

    // QDCOUNT = 1
    resp.extend_from_slice(&[0x00, 0x01]);
    // ANCOUNT
    resp.extend_from_slice(&an_count.to_be_bytes());
    // NSCOUNT, ARCOUNT = 0
    resp.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);

    // Copy question section from query
    resp.extend_from_slice(&query[12..qend]);

    // Answer records
    for addr in addresses {
        // Name pointer to question
        resp.extend_from_slice(&[0xC0, 0x0C]);
        match addr {
            IpAddr::V4(v4) => {
                resp.extend_from_slice(&[0x00, 0x01]); // A
                resp.extend_from_slice(&[0x00, 0x01]); // IN
                resp.extend_from_slice(&ttl.to_be_bytes());
                resp.extend_from_slice(&[0x00, 0x04]); // RDLENGTH
                resp.extend_from_slice(&v4.octets());
            }
            IpAddr::V6(v6) => {
                resp.extend_from_slice(&[0x00, 0x1C]); // AAAA
                resp.extend_from_slice(&[0x00, 0x01]); // IN
                resp.extend_from_slice(&ttl.to_be_bytes());
                resp.extend_from_slice(&[0x00, 0x10]); // RDLENGTH
                resp.extend_from_slice(&v6.octets());
            }
        }
    }

    Ok(resp)
}

/// Build a DNS NXDOMAIN (or SERVFAIL) error response from a query packet.
pub fn build_dns_error_response(query: &[u8], rcode: u8) -> Vec<u8> {
    if query.len() < 12 {
        return Vec::new();
    }
    let end = question(query).ok().map(|q| q.end);
    let mut resp = query[..end.unwrap_or(12)].to_vec();
    resp[2] = 0x80 | (query[2] & 1); // QR and original RD
    resp[3] = 0x80 | (rcode & 0x0f);
    resp[4..12].fill(0);
    if end.is_some() {
        resp[5] = 1;
    }
    resp
}

/// Reject answers for a different exchange before they reach the cache.
pub(super) fn validate_response(query: &[u8], response: &[u8]) -> Result<()> {
    if response.len() < 12
        || query.len() < 12
        || response[..2] != query[..2]
        || response[2] & 0x80 == 0
        || response[2] & 0x02 != 0
    {
        bail!("DNS response identity mismatch or truncated response");
    }
    validate_records(response)?;
    let expected = question(query)?;
    let mut question_packet = response.to_vec();
    question_packet[2] &= 0x07;
    let actual = question(&question_packet)?;
    if !expected.domain.eq_ignore_ascii_case(&actual.domain) || expected.qtype != actual.qtype {
        bail!("DNS response question mismatch");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_truncated_question_is_rejected_without_panicking() {
        let query = build_dns_query("www.baidu.com", 1);
        for end in 0..query.len() {
            assert!(parse_dns_query(&query[..end]).is_err());
            assert!(build_dns_response(&query[..end], &[]).is_err());
            let error = build_dns_error_response(&query[..end], 1);
            if error.len() >= 12 {
                assert_eq!(&error[4..12], &[0; 8]);
            }
        }
        assert!(build_dns_response(&query, &[]).is_ok());
    }

    #[test]
    fn invalid_question_count_and_labels_are_rejected() {
        let query = build_dns_query("example.com", 1);
        for count in [0u16, 2, 65535] {
            let mut invalid = query.clone();
            invalid[4..6].copy_from_slice(&count.to_be_bytes());
            assert!(parse_dns_query(&invalid).is_err());
        }
        for len in [64, 128, 192, 255] {
            let mut invalid = query.clone();
            invalid[12] = len;
            assert!(parse_dns_query(&invalid).is_err());
        }
    }

    #[test]
    fn response_requires_matching_question_and_transaction() {
        let query = build_dns_query("example.com", 1);
        let mut response = build_dns_response(&query, &["192.0.2.1".parse().unwrap()]).unwrap();
        validate_response(&query, &response).unwrap();
        response[0] ^= 1;
        assert!(validate_response(&query, &response).is_err());
        response[0] ^= 1;
        response[13] = b'x';
        assert!(validate_response(&query, &response).is_err());
    }

    #[test]
    fn partial_answers_are_not_cached_as_success() {
        let query = build_dns_query("example.com", 1);
        let response = build_dns_response(&query, &["192.0.2.1".parse().unwrap()]).unwrap();
        for end in query.len()..response.len() {
            assert!(parse_dns_response(&response[..end]).is_err());
        }
    }
}

fn validate_records(message: &[u8]) -> Result<()> {
    let mut pos = skip_dns_name(message, 12)? + 4;
    ensure_bounds(message, pos)?;
    let records = [6usize, 8, 10]
        .into_iter()
        .map(|i| u16::from_be_bytes([message[i], message[i + 1]]) as usize)
        .sum::<usize>();
    for _ in 0..records {
        pos = skip_dns_name(message, pos)?;
        ensure_bounds(message, pos + 10)?;
        let len = u16::from_be_bytes([message[pos + 8], message[pos + 9]]) as usize;
        pos += 10 + len;
        ensure_bounds(message, pos)?;
    }
    anyhow::ensure!(pos == message.len(), "Trailing bytes in DNS response");
    Ok(())
}
fn ensure_bounds(message: &[u8], end: usize) -> Result<()> {
    anyhow::ensure!(end <= message.len(), "Truncated DNS record");
    Ok(())
}

pub(super) fn cap_ttl(message: &mut [u8], max: u32) -> Result<()> {
    validate_records(message)?;
    let mut pos = skip_dns_name(message, 12)? + 4;
    let count = [6usize, 8, 10]
        .into_iter()
        .map(|i| u16::from_be_bytes([message[i], message[i + 1]]) as usize)
        .sum::<usize>();
    for _ in 0..count {
        pos = skip_dns_name(message, pos)?;
        let kind = u16::from_be_bytes([message[pos], message[pos + 1]]);
        let len = u16::from_be_bytes([message[pos + 8], message[pos + 9]]) as usize;
        if kind != 41 {
            let ttl = u32::from_be_bytes(message[pos + 4..pos + 8].try_into()?);
            message[pos + 4..pos + 8].copy_from_slice(&ttl.min(max).to_be_bytes());
        }
        pos += 10 + len;
    }
    Ok(())
}

/// Honor classic 512-byte DNS and a bounded EDNS UDP payload size.
pub(super) fn udp_response(query: &[u8], reply: Vec<u8>) -> Vec<u8> {
    let mut limit = 512usize;
    if let Ok(q) = question(query) {
        let mut pos = q.end;
        let count = u16::from_be_bytes([query[10], query[11]]);
        for _ in 0..count {
            let Ok(end) = skip_dns_name(query, pos) else {
                break;
            };
            pos = end;
            if pos + 10 > query.len() {
                break;
            }
            let kind = u16::from_be_bytes([query[pos], query[pos + 1]]);
            let class = u16::from_be_bytes([query[pos + 2], query[pos + 3]]);
            let len = u16::from_be_bytes([query[pos + 8], query[pos + 9]]) as usize;
            if pos + 10 + len > query.len() {
                break;
            }
            if kind == 41 {
                limit = usize::from(class).clamp(512, 4096);
            }
            pos += 10 + len;
        }
    }
    if reply.len() <= limit {
        return reply;
    }
    let mut truncated = build_dns_error_response(query, reply.get(3).copied().unwrap_or(2) & 15);
    if truncated.len() >= 12 {
        truncated[2] |= 2;
    }
    truncated
}

/// Minimum answer TTL for the resolver's address cache (zero is not cacheable).
pub(super) fn answer_ttl(message: &[u8]) -> Result<u32> {
    validate_records(message)?;
    let mut pos = skip_dns_name(message, 12)? + 4;
    let count = u16::from_be_bytes([message[6], message[7]]);
    let mut ttl = None;
    for _ in 0..count {
        pos = skip_dns_name(message, pos)?;
        let value = u32::from_be_bytes(message[pos + 4..pos + 8].try_into()?);
        ttl = Some(ttl.map_or(value, |old: u32| old.min(value)));
        let len = u16::from_be_bytes([message[pos + 8], message[pos + 9]]) as usize;
        pos += 10 + len;
    }
    Ok(ttl.unwrap_or(0))
}
