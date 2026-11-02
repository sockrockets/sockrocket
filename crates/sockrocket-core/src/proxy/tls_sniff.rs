//! Bounded ClientHello inspection for TUN connections whose DNS name was lost.
//! Bytes are replayed unchanged; encrypted application data is never inspected.
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};

const PREFIX_LIMIT: usize = 4096;
const DEADLINE: Duration = Duration::from_millis(300);

#[derive(Debug, PartialEq)]
enum Detection {
    More,
    Other,
    Host(String),
}

fn word(bytes: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?) as usize)
}

// None means an incomplete prefix; Some(None) is a complete rejection.
fn hello_name(bytes: &[u8]) -> Option<Option<String>> {
    if *bytes.first()? != 1 {
        return Some(None);
    }
    let length = bytes.get(1..4)?;
    let end = 4 + ((length[0] as usize) << 16 | (length[1] as usize) << 8 | length[2] as usize);
    let bytes = &bytes[..bytes.len().min(end)];
    let mut at = 38; // handshake header, legacy version, random
    at += 1 + *bytes.get(at)? as usize;
    let cipher_length = word(bytes, at)?;
    if cipher_length == 0 || cipher_length % 2 != 0 {
        return Some(None);
    }
    at += 2 + cipher_length;
    at += 1 + *bytes.get(at)? as usize;
    let extensions_end = at + 2 + word(bytes, at)?;
    at += 2;
    if extensions_end != end {
        return Some(None);
    }
    while at < extensions_end {
        let kind = word(bytes, at)?;
        let size = word(bytes, at + 2)?;
        at += 4;
        if at + size > extensions_end {
            return Some(None);
        }
        if kind == 0 {
            let names = bytes.get(at..at + size)?;
            if word(names, 0)? + 2 != size {
                return Some(None);
            }
            let mut offset = 2;
            let mut host = None;
            while offset < names.len() {
                let name_kind = *names.get(offset)?;
                let size = word(names, offset + 1)?;
                offset += 3;
                let name = names.get(offset..offset + size)?;
                offset += size;
                if name_kind == 0 {
                    if host.is_some() {
                        return Some(None);
                    }
                    let Ok(name) = std::str::from_utf8(name) else {
                        return Some(None);
                    };
                    let name = name.trim_end_matches('.').to_ascii_lowercase();
                    if !crate::config::dns::valid_domain(&name)
                        || name.parse::<std::net::IpAddr>().is_ok()
                    {
                        return Some(None);
                    }
                    host = Some(name);
                }
            }
            return Some(host);
        }
        bytes.get(at..at + size)?;
        at += size;
    }
    Some(None)
}

fn detect(prefix: &[u8]) -> Detection {
    let mut at = 0;
    let mut hello = Vec::new();
    while at < prefix.len() {
        if prefix[at] != 22 {
            return Detection::Other;
        }
        let Some(header) = prefix.get(at..at + 5) else {
            return Detection::More;
        };
        if header[1] != 3 || header[2] > 4 {
            return Detection::Other;
        }
        let size = word(header, 3).unwrap();
        if size == 0 || size > 18432 {
            return Detection::Other;
        }
        at += 5;
        let end = at + size;
        hello.extend_from_slice(&prefix[at..end.min(prefix.len())]);
        match hello_name(&hello) {
            Some(Some(name)) => return Detection::Host(name),
            Some(None) => return Detection::Other,
            None => {}
        }
        if hello.len() >= 4 {
            let declared =
                4 + ((hello[1] as usize) << 16 | (hello[2] as usize) << 8 | hello[3] as usize);
            if hello.len() >= declared {
                return Detection::Other;
            }
        }
        if end > prefix.len() {
            return Detection::More;
        }
        at = end;
    }
    Detection::More
}

pub(super) async fn read_name<S: AsyncRead + Unpin>(
    stream: &mut S,
) -> io::Result<(Option<String>, Vec<u8>)> {
    let mut prefix = Vec::new();
    let inspection = async {
        let mut chunk = [0u8; 1024];
        while prefix.len() < PREFIX_LIMIT {
            let capacity = chunk.len().min(PREFIX_LIMIT - prefix.len());
            let count = stream.read(&mut chunk[..capacity]).await?;
            if count == 0 {
                break;
            }
            prefix.extend_from_slice(&chunk[..count]);
            match detect(&prefix) {
                Detection::Host(name) => return Ok(Some(name)),
                Detection::Other => break,
                Detection::More => {}
            }
        }
        Ok::<Option<String>, io::Error>(None)
    };
    let name = tokio::time::timeout(DEADLINE, inspection)
        .await
        .unwrap_or(Ok(None))?;
    Ok((name, prefix))
}

pub(super) struct Replay<S> {
    stream: S,
    prefix: Vec<u8>,
    position: usize,
}

impl<S> Replay<S> {
    pub(super) fn new(stream: S, prefix: Vec<u8>) -> Self {
        Self {
            stream,
            prefix,
            position: 0,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Replay<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if this.position < this.prefix.len() && buf.remaining() > 0 {
            let size = buf.remaining().min(this.prefix.len() - this.position);
            buf.put_slice(&this.prefix[this.position..this.position + size]);
            this.position += size;
            if this.position == this.prefix.len() {
                this.prefix = Vec::new();
                this.position = 0;
            }
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.stream).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Replay<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().stream).poll_write(cx, bytes)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{MatchRule, RouteAction, Router, RuleSet, rule_mode_ruleset};
    use std::sync::Arc;
    use tokio::io::AsyncWriteExt;

    fn hello(host: &str) -> Vec<u8> {
        hello_with_padding(host, 0)
    }

    fn hello_with_padding(host: &str, padding: usize) -> Vec<u8> {
        let mut names = vec![0];
        names.extend((host.len() as u16).to_be_bytes());
        names.extend(host.as_bytes());
        let mut extension = Vec::new();
        extension.extend((names.len() as u16).to_be_bytes());
        extension.extend(names);
        let mut extensions = Vec::new();
        if padding > 0 {
            extensions.extend([0, 21]);
            extensions.extend((padding as u16).to_be_bytes());
            extensions.resize(extensions.len() + padding, 0);
        }
        extensions.extend([0, 0]);
        extensions.extend((extension.len() as u16).to_be_bytes());
        extensions.extend(extension);
        let mut body = vec![3, 3];
        body.extend([0; 32]);
        body.extend([0, 0, 2, 0x13, 1, 1, 0]);
        body.extend((extensions.len() as u16).to_be_bytes());
        body.extend(extensions);
        let length = (body.len() as u32).to_be_bytes();
        let mut handshake = vec![1, length[1], length[2], length[3]];
        handshake.extend(body);
        record(&handshake)
    }

    fn record(body: &[u8]) -> Vec<u8> {
        let mut bytes = vec![22, 3, 1];
        bytes.extend((body.len() as u16).to_be_bytes());
        bytes.extend(body);
        bytes
    }

    #[test]
    fn fragmented_records_and_incomplete_prefixes() {
        let bytes = hello("WWW.Kimi.COM");
        for end in 0..bytes.len() {
            assert_eq!(detect(&bytes[..end]), Detection::More, "prefix {end}");
        }
        assert_eq!(detect(&bytes), Detection::Host("www.kimi.com".into()));
        let body = &bytes[5..];
        for split in 1..body.len() {
            let mut fragmented = record(&body[..split]);
            fragmented.extend(record(&body[split..]));
            assert_eq!(detect(&fragmented), Detection::Host("www.kimi.com".into()));
        }
    }

    #[test]
    fn invalid_and_arbitrary_input_is_rejected_without_panics() {
        for host in ["127.0.0.1", "bad/name", "bad..name", "foo\0bar"] {
            assert_eq!(detect(&hello(host)), Detection::Other);
        }
        let mut state = 1u32;
        for size in 0..512 {
            let mut bytes = Vec::new();
            for _ in 0..size {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                bytes.push((state >> 24) as u8);
            }
            let _ = detect(&bytes);
        }
    }

    #[tokio::test]
    async fn inspected_bytes_are_replayed_exactly_once_and_writes_still_work() {
        let expected = hello("www.kimi.com");
        let original = expected.clone();
        let (mut client, mut server) = tokio::io::duplex(1);
        let writer = tokio::spawn(async move {
            client.write_all(&original).await.unwrap();
            client.write_all(b"encrypted tail").await.unwrap();
            client.shutdown().await.unwrap();
            let mut reply = Vec::new();
            client.read_to_end(&mut reply).await.unwrap();
            assert_eq!(reply, b"response");
        });
        let (host, prefix) = read_name(&mut server).await.unwrap();
        assert_eq!(host.as_deref(), Some("www.kimi.com"));
        let mut replay = Replay::new(server, prefix);
        let mut actual = Vec::new();
        replay.read_to_end(&mut actual).await.unwrap();
        assert_eq!(actual, [expected.as_slice(), b"encrypted tail"].concat());
        replay.write_all(b"response").await.unwrap();
        replay.shutdown().await.unwrap();
        writer.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_and_oversized_handshakes_remain_bounded_and_lossless() {
        let (mut client, mut server) = tokio::io::duplex(8192);
        client.write_all(&[22, 3]).await.unwrap();
        let start = tokio::time::Instant::now();
        let (name, prefix) = read_name(&mut server).await.unwrap();
        assert!(name.is_none());
        assert_eq!(prefix, [22, 3]);
        assert!(start.elapsed() <= DEADLINE + Duration::from_millis(1));
        let bytes = hello_with_padding("www.kimi.com", 5000);
        client.write_all(&bytes).await.unwrap();
        client.shutdown().await.unwrap();
        let (name, prefix) = read_name(&mut server).await.unwrap();
        assert!(name.is_none());
        assert!(prefix.len() <= PREFIX_LIMIT);
        let mut replay = Replay::new(server, prefix);
        let mut actual = Vec::new();
        replay.read_to_end(&mut actual).await.unwrap();
        assert_eq!(actual, bytes);
    }

    #[tokio::test]
    async fn stale_overseas_ip_recovers_kimi_rule_without_losing_ip_rule_priority() {
        let router = Arc::new(Router::new(rule_mode_ruleset(&[])));
        let bytes = hello("www.kimi.com");
        let (mut writer, mut stream) = tokio::io::duplex(1024);
        writer.write_all(&bytes).await.unwrap();
        let ip = "192.0.2.99";
        let (host, prefix) =
            super::super::tun::tun_tls_target(&mut stream, ip, 443, false, Some(&router))
                .await
                .unwrap();
        assert_eq!(host, "www.kimi.com");
        assert_eq!(prefix, bytes);
        crate::proxy::routing::with_tun_destination(Some(ip.parse().unwrap()), false, async {
            assert_eq!(router.route_resolving(ip, 443).await, RouteAction::Proxy);
            assert_eq!(
                router.route_resolving(&host, 443).await,
                RouteAction::Direct
            );
            let mut rules = RuleSet::new();
            rules.add_rule(
                MatchRule::IpCidr {
                    addr: ip.parse().unwrap(),
                    prefix_len: 32,
                },
                RouteAction::Reject,
            );
            rules.add_rule(MatchRule::Domain(host.clone()), RouteAction::Direct);
            assert_eq!(
                Router::new(rules).route_resolving(&host, 443).await,
                RouteAction::Reject
            );
        })
        .await;
    }

    #[tokio::test]
    async fn known_names_modes_and_unruled_sni_retain_existing_targets() {
        let router = Arc::new(Router::new(rule_mode_ruleset(&[])));
        let (mut writer, mut stream) = tokio::io::duplex(1024);
        for (host, port, force, policy) in [
            ("www.kimi.com", 443, false, Some(&router)),
            ("192.0.2.99", 443, true, Some(&router)),
            ("192.0.2.99", 443, false, None),
            ("192.0.2.99", 80, false, Some(&router)),
        ] {
            let (target, prefix) =
                super::super::tun::tun_tls_target(&mut stream, host, port, force, policy)
                    .await
                    .unwrap();
            assert_eq!(target, host);
            assert!(prefix.is_empty());
        }
        writer
            .write_all(&hello("unruled.example.test"))
            .await
            .unwrap();
        let (target, prefix) =
            super::super::tun::tun_tls_target(&mut stream, "192.0.2.99", 443, false, Some(&router))
                .await
                .unwrap();
        assert_eq!(target, "192.0.2.99");
        assert!(!prefix.is_empty());
    }
}
