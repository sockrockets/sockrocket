//! Startup-only MTU policy. ICMP probes measure the *outer* path, not the
//! proxy protocol or the LAN path. Missing ICMP/DF support is inconclusive.
//! Never turn packet loss into a claimed path-MTU measurement.
use std::{
    net::IpAddr,
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::config::model::{Node, ProxyProtocol};

pub const FALLBACK_MTU: u16 = 1400;
const MIN_MTU: u16 = 1280;
const MAX_AUTO_MTU: u16 = 1500;
const STARTUP_BUDGET: Duration = Duration::from_secs(6);
// Router startup competes with DNS/firewall work; a sub-second limit can
// discard a healthy international RTT plus process startup overhead.
const COMMAND_BUDGET: Duration = Duration::from_millis(1200);
const CACHE_VERSION: u8 = 2;
const CACHE_ENTRIES: usize = 32;

pub fn validate(mtu: u16) -> Result<()> {
    ensure!(
        (MIN_MTU..=9000).contains(&mtu),
        "TUN MTU must be between 1280 and 9000 (got {mtu})"
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Probe {
    Fits,
    TooBig,
    Unknown,
}

#[derive(Debug)]
struct Link {
    interface_mtu: u16,
    mtu: u16,
    // Includes selected route, source address and gateway. Do not reuse a
    // measurement just because an interface happens to have the same name.
    fingerprint: String,
}

#[derive(Serialize, Deserialize)]
struct Cache {
    version: u8,
    entries: Vec<CachedPath>,
}

#[derive(Serialize, Deserialize)]
struct CachedPath {
    key: String,
    measured_at: u64,
    path_mtu: u16,
}

/// Blocking preflight: call on a blocking worker, BEFORE installing TUN
/// routes. Only configured node IPs are probed; no third-party probe service.
/// A manual override performs no discovery and is never silently clamped.
pub fn select(
    node: Option<&Node>,
    targets: &[IpAddr],
    manual: Option<u16>,
    cache_path: Option<&Path>,
) -> Result<u16> {
    if let Some(mtu) = manual {
        validate(mtu)?;
        tracing::info!(mtu, "TUN MTU: manual override");
        return Ok(mtu);
    }
    let deadline = Instant::now() + STARTUP_BUDGET;
    let targets = unique_targets(targets);
    let reserve = headroom(node);
    let mut selected = MAX_AUTO_MTU;
    let mut observed = false;
    // Bound work even for multi-address DNS answers. Inconclusive/unexamined
    // paths cap the result at the fallback; probing one IP proves no others.
    for (path_index, target) in targets.iter().take(CACHE_ENTRIES).enumerate() {
        let family = if target.is_ipv4() { "ipv4" } else { "ipv6" };
        let Some(link) = inspect_link(*target, deadline) else {
            if route_unreachable(*target, deadline) {
                tracing::info!(
                    path_index,
                    family,
                    "TUN MTU: skipping node address with no outbound route"
                );
                continue;
            }
            tracing::info!(
                path_index,
                family,
                "TUN MTU: egress discovery unavailable; using conservative fallback"
            );
            selected = selected.min(FALLBACK_MTU);
            continue;
        };
        observed = true;
        let upper = link.mtu.min(MAX_AUTO_MTU);
        let fallback = tun_from_path(upper, reserve).min(FALLBACK_MTU);
        let key = format!(
            "v2|{target}|{}|{}|{}|{}|{}",
            node.map(|n| n.server.as_str()).unwrap_or("direct"),
            node.map(|n| n.port).unwrap_or(0),
            node.map(|n| n.protocol.name()).unwrap_or("direct"),
            reserve,
            link.fingerprint
        );
        let cached = cache_path.and_then(|path| read_cache(path, &key, upper));
        // A known node/network reuses its saved result without sending any
        // echo packets. A changed network identity or explicit cache removal
        // triggers discovery. Do not turn this into a manual config override.
        let measured = cached_or_discover(cached, || {
            discover_with_feedback(
                upper,
                |mtu| probe(*target, mtu, deadline),
                || inspect_link(*target, deadline).map(|link| link.mtu),
            )
        });
        let mtu = match measured {
            Some(path_mtu) => {
                if cached.is_none()
                    && let Some(path) = cache_path
                {
                    write_cache(path, &key, path_mtu);
                }
                tun_from_path(path_mtu, reserve)
            }
            None => fallback,
        };
        tracing::info!(path_index, family, link_mtu = link.interface_mtu, route_mtu = link.mtu, path_mtu = ?measured, reused = cached.is_some(), reserve, mtu,
            "TUN startup MTU (outer-path ICMP is advisory, not proxy validation)");
        selected = selected.min(mtu);
    }
    if !observed || targets.len() > CACHE_ENTRIES || targets.is_empty() {
        selected = selected.min(FALLBACK_MTU);
    }
    if let Some(Node {
        protocol: ProxyProtocol::Wireguard { config },
        ..
    }) = node
    {
        // WireGuard has its own inner-IP MTU setting. Keep the TUN side no
        // larger, but leave enforcement/encapsulation to that outbound.
        selected = selected.min(config.mtu.clamp(MIN_MTU as usize, MAX_AUTO_MTU as usize) as u16);
    }
    tracing::info!(
        mtu = selected,
        "TUN MTU selected; device, stack and TCP MSS will use this value"
    );
    Ok(selected)
}

fn unique_targets(targets: &[IpAddr]) -> Vec<IpAddr> {
    let mut unique = Vec::new();
    for target in targets {
        if !unique.contains(target) {
            unique.push(*target);
        }
    }
    unique
}

fn route_unreachable(target: IpAddr, deadline: Instant) -> bool {
    #[cfg(target_os = "linux")]
    {
        let args = vec![
            if target.is_ipv4() { "-4" } else { "-6" }.into(),
            "route".into(),
            "get".into(),
            target.to_string(),
        ];
        let Some(output) = command("ip", &args, deadline) else {
            return false;
        };
        no_outbound_route(
            output.status.success(),
            &String::from_utf8_lossy(&output.stderr),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (target, deadline);
        false
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn no_outbound_route(success: bool, message: &str) -> bool {
    // Only an explicit routing error is skippable. A missing tool, timeout,
    // parsing failure or silent ICMP target must keep the conservative cap.
    !success
        && message
            .to_ascii_lowercase()
            .contains("network is unreachable")
}

fn headroom(node: Option<&Node>) -> u16 {
    // Conservative policy margins, NOT an exact encapsulation formula.
    // TCP outbounds re-segment streams; QUIC manages its own outer PMTU.
    // Neither operation is correctly modelled as an IP-in-IP tunnel.
    match node.map(|n| &n.protocol) {
        Some(ProxyProtocol::Shadowsocks { udp: true, .. }) => 128,
        Some(ProxyProtocol::Hysteria2 { .. } | ProxyProtocol::Tuic { .. }) => 100,
        _ => 80,
    }
}

fn tun_from_path(path: u16, reserve: u16) -> u16 {
    path.saturating_sub(reserve).clamp(MIN_MTU, MAX_AUTO_MTU)
}

fn cached_or_discover(cached: Option<u16>, discover: impl FnOnce() -> Option<u16>) -> Option<u16> {
    cached.or_else(discover)
}

/// BusyBox ping can receive an ICMP fragmentation-needed error without
/// printing it or exiting. Linux still installs the advertised PMTU in its
/// route cache. After an inconclusive probe, consult that explicit kernel
/// feedback and verify the smaller ceiling with another DF echo. Silence
/// alone never supplies a smaller MTU.
fn discover_with_feedback(
    upper: u16,
    mut probe: impl FnMut(u16) -> Probe,
    mut feedback: impl FnMut() -> Option<u16>,
) -> Option<u16> {
    if let Some(mtu) = discover(upper, &mut probe) {
        return Some(mtu);
    }
    let learned = feedback()?;
    if (MIN_MTU..upper).contains(&learned) && probe(learned) == Probe::Fits {
        tracing::info!(
            upper,
            learned,
            "TUN MTU: validated kernel PMTU feedback after inconclusive ping"
        );
        Some(learned)
    } else {
        None
    }
}

/// Binary search on an eight-byte grid. Only explicit TooBig feedback can
/// lower the upper bound. Any timeout/unreachable/missing utility is unknown.
fn discover(upper: u16, mut probe: impl FnMut(u16) -> Probe) -> Option<u16> {
    if upper < MIN_MTU {
        return None;
    }
    // Most paths accept the interface ceiling. Try that first so a normal
    // dual-address node needs two probes total, not four.
    match probe(upper) {
        Probe::Fits => return Some(upper),
        Probe::Unknown => return None,
        Probe::TooBig => {}
    }
    if upper == MIN_MTU || probe(MIN_MTU) != Probe::Fits {
        return None;
    }
    let mut low = MIN_MTU;
    let mut high = upper;
    while high - low > 8 {
        let mid = low + ((high - low) / 16).max(1) * 8;
        match probe(mid) {
            Probe::Fits => low = mid,
            Probe::TooBig => high = mid,
            Probe::Unknown => return None,
        }
    }
    Some(low)
}

fn probe(target: IpAddr, mtu: u16, deadline: Instant) -> Probe {
    let Some((program, args)) = ping_args(target, mtu) else {
        return Probe::Unknown;
    };
    let Some(output) = command(program, &args, deadline) else {
        return Probe::Unknown;
    };
    classify_probe(
        output.status.success(),
        &format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

fn classify_probe(success: bool, output: &str) -> Probe {
    if success {
        return Probe::Fits;
    }
    let text = output.to_ascii_lowercase();
    if [
        "message too long",
        "frag needed",
        "fragmentation needed",
        "packet too big",
        "needs to be fragmented",
    ]
    .iter()
    .any(|s| text.contains(s))
    {
        Probe::TooBig
    } else {
        Probe::Unknown
    }
}

fn ping_args(target: IpAddr, mtu: u16) -> Option<(&'static str, Vec<String>)> {
    let payload = mtu
        .checked_sub(if target.is_ipv4() { 28 } else { 48 })?
        .to_string();
    let ip = target.to_string();
    #[cfg(target_os = "linux")]
    {
        Some((
            "ping",
            vec![
                if target.is_ipv4() { "-4" } else { "-6" }.into(),
                // IP literals need no name resolution. BusyBox ping on
                // Merlin supports DF (-M do), but does not support -n.
                "-c".into(),
                "1".into(),
                "-M".into(),
                "do".into(),
                "-s".into(),
                payload,
                ip,
            ],
        ))
    }
    #[cfg(target_os = "macos")]
    {
        if target.is_ipv4() {
            Some((
                "ping",
                vec![
                    "-n".into(),
                    "-c".into(),
                    "1".into(),
                    "-D".into(),
                    "-s".into(),
                    payload,
                    ip,
                ],
            ))
        } else {
            None
        }
    }
    #[cfg(target_os = "windows")]
    {
        if target.is_ipv4() {
            Some((
                "ping",
                vec![
                    "-4".into(),
                    "-n".into(),
                    "1".into(),
                    "-f".into(),
                    "-l".into(),
                    payload,
                    ip,
                ],
            ))
        } else {
            None
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = (payload, ip);
        None
    }
}

fn inspect_link(target: IpAddr, deadline: Instant) -> Option<Link> {
    #[cfg(target_os = "linux")]
    {
        let args = vec![
            if target.is_ipv4() { "-4" } else { "-6" }.into(),
            "route".into(),
            "get".into(),
            target.to_string(),
        ];
        let output = command("ip", &args, deadline)?;
        if !output.status.success() {
            return None;
        }
        let route = String::from_utf8(output.stdout).ok()?;
        let iface = token_after(&route, "dev")?;
        if !valid_interface(iface) {
            return None;
        }
        let mtu: u16 = std::fs::read_to_string(format!("/sys/class/net/{iface}/mtu"))
            .ok()?
            .trim()
            .parse()
            .ok()?;
        let route_mtu = token_after(&route, "mtu")
            .and_then(|s| s.parse::<u16>().ok())
            .unwrap_or(mtu);
        if mtu < MIN_MTU {
            return None;
        }
        let fingerprint = linux_route_fingerprint(&route, mtu)?;
        Some(Link {
            interface_mtu: mtu,
            mtu: mtu.min(route_mtu),
            fingerprint,
        })
    }
    #[cfg(target_os = "macos")]
    {
        let output = command(
            "route",
            &["-n".into(), "get".into(), target.to_string()],
            deadline,
        )?;
        if !output.status.success() {
            return None;
        }
        let route = String::from_utf8(output.stdout).ok()?;
        let iface = token_after(&route, "interface:")?;
        if !valid_interface(iface) {
            return None;
        }
        let output = command("ifconfig", &[iface.into()], deadline)?;
        if !output.status.success() {
            return None;
        }
        let config = String::from_utf8(output.stdout).ok()?;
        let mtu = token_after(&config, "mtu")?.parse().ok()?;
        if mtu < MIN_MTU {
            return None;
        }
        let fingerprint = macos_route_fingerprint(&route, &config, mtu)?;
        Some(Link {
            interface_mtu: mtu,
            mtu,
            fingerprint,
        })
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (target, deadline);
        None
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn macos_route_fingerprint(route: &str, config: &str, link_mtu: u16) -> Option<String> {
    // Avoid MAC addresses, address lifetimes and volatile interface flags.
    Some(format!(
        "route-v2|{}|{}|{}|{}|{}",
        token_after(route, "interface:")?,
        link_mtu,
        token_after(route, "gateway:").unwrap_or(""),
        token_after(config, "inet").unwrap_or(""),
        token_after(config, "inet6").unwrap_or(""),
    ))
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn linux_route_fingerprint(route: &str, link_mtu: u16) -> Option<String> {
    // Expiring PMTU/cache fields change after every probe; they describe the
    // measurement, not network identity, and must not invalidate its cache.
    Some(format!(
        "route-v2|{}|{}|{}|{}|{}",
        token_after(route, "dev")?,
        link_mtu,
        token_after(route, "via").unwrap_or(""),
        token_after(route, "src").unwrap_or(""),
        token_after(route, "table").unwrap_or("main"),
    ))
}

fn valid_interface(iface: &str) -> bool {
    !iface.starts_with('-')
        && !iface.contains('/')
        && !iface.contains('\\')
        && !iface.is_empty()
        && !iface.starts_with("sockrocket")
        && !iface.starts_with("utun")
}

fn token_after<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let mut words = text.split_whitespace();
    while let Some(word) = words.next() {
        if word == key {
            return words.next();
        }
    }
    None
}

/// Kill AND reap timed-out children. No detached ping processes, no shell.
fn command(program: &str, args: &[String], deadline: Instant) -> Option<Output> {
    let remaining = deadline.checked_duration_since(Instant::now())?;
    if remaining.is_zero() {
        return None;
    }
    let until = Instant::now() + remaining.min(COMMAND_BUDGET);
    // Resolve known system utilities without running `which` (unbounded).
    let path = ["/sbin", "/usr/sbin", "/bin", "/usr/bin"]
        .into_iter()
        .map(|dir| format!("{dir}/{program}"))
        .find(|path| Path::new(path).is_file())
        .unwrap_or_else(|| program.into());
    let mut child = Command::new(path)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if Instant::now() < until => std::thread::sleep(Duration::from_millis(5)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn read_cache(path: &Path, key: &str, upper: u16) -> Option<u16> {
    let cache = read_store(path)?;
    cache
        .entries
        .iter()
        .find(|entry| {
            entry.key == key
                && entry.measured_at <= now()
                && (MIN_MTU..=upper).contains(&entry.path_mtu)
        })
        .map(|entry| entry.path_mtu)
}

fn read_store(path: &Path) -> Option<Cache> {
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > 64 * 1024 {
        return None;
    }
    let cache: Cache = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    (cache.version == CACHE_VERSION && cache.entries.len() <= CACHE_ENTRIES).then_some(cache)
}

fn write_cache(path: &Path, key: &str, path_mtu: u16) {
    let mut cache = read_store(path).unwrap_or(Cache {
        version: CACHE_VERSION,
        entries: Vec::new(),
    });
    cache.entries.retain(|entry| entry.key != key);
    cache.entries.sort_by_key(|entry| entry.measured_at);
    if cache.entries.len() >= CACHE_ENTRIES {
        cache.entries.remove(0);
    }
    cache.entries.push(CachedPath {
        key: key.into(),
        measured_at: now(),
        path_mtu,
    });
    if let Ok(json) = serde_json::to_string(&cache) {
        // The shared atomic writer expects a real parent directory, including
        // when CLI was launched with a bare relative config filename.
        let absolute;
        let path = if path.is_relative() {
            let Ok(dir) = std::env::current_dir() else {
                return;
            };
            absolute = dir.join(path);
            absolute.as_path()
        } else {
            path
        };
        // Same private/atomic persistence as configuration, without backups.
        if let Err(error) = crate::config::storage::atomic_write_private(path, &json, false) {
            tracing::debug!(%error, "MTU cache unavailable; startup continues");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_pppoe_and_smaller_paths() {
        for limit in [1280, 1360, 1400, 1492, 1500] {
            let found = discover(1500, |size| {
                if size <= limit {
                    Probe::Fits
                } else {
                    Probe::TooBig
                }
            })
            .unwrap();
            assert!(found <= limit && limit - found <= 8);
        }
    }

    #[test]
    fn packet_loss_and_blocked_icmp_are_not_mtu_measurements() {
        assert_eq!(discover(1500, |_| Probe::Unknown), None);
        assert_eq!(
            discover(1500, |size| if size == 1280 {
                Probe::Fits
            } else {
                Probe::Unknown
            }),
            None
        );
        assert_eq!(
            discover(1500, |size| match size {
                1280 => Probe::Fits,
                1500 => Probe::TooBig,
                _ => Probe::Unknown,
            }),
            None
        );
        for message in [
            "100% packet loss",
            "Destination Host Unreachable",
            "invalid option -- M",
        ] {
            assert_eq!(classify_probe(false, message), Probe::Unknown);
        }
        assert_eq!(
            classify_probe(false, "Frag needed and DF set (mtu = 1492)"),
            Probe::TooBig
        );
    }

    #[test]
    fn kernel_feedback_recovers_busybox_silent_fragmentation_errors() {
        let mut sizes = Vec::new();
        let result = discover_with_feedback(
            1500,
            |size| {
                sizes.push(size);
                if size == 1492 {
                    Probe::Fits
                } else {
                    Probe::Unknown
                }
            },
            || Some(1492),
        );
        assert_eq!(result, Some(1492));
        assert_eq!(sizes, vec![1500, 1492]);
        // Neither loss alone nor an unverified kernel ceiling proves success.
        assert_eq!(
            discover_with_feedback(1500, |_| Probe::Unknown, || Some(1500)),
            None
        );
        assert_eq!(
            discover_with_feedback(1500, |_| Probe::Unknown, || Some(1492)),
            None
        );
        assert_eq!(
            discover_with_feedback(1500, |_| Probe::Unknown, || None),
            None
        );
        assert_eq!(
            discover_with_feedback(1500, |_| Probe::Fits, || panic!("feedback not needed")),
            Some(1500)
        );
    }

    #[test]
    fn route_identity_ignores_cache_expiry_but_tracks_network_changes() {
        let base = "203.0.113.10 via 192.0.2.1 dev eth0 src 192.0.2.2 uid 0\n cache";
        let learned = format!("{base} expires 597sec mtu 1492");
        assert_eq!(
            linux_route_fingerprint(base, 1500),
            linux_route_fingerprint(&learned, 1500)
        );
        assert_ne!(
            linux_route_fingerprint(base, 1500),
            linux_route_fingerprint(&base.replace("192.0.2.1", "192.0.2.254"), 1500)
        );
        assert_ne!(
            linux_route_fingerprint(base, 1500),
            linux_route_fingerprint(base, 1492)
        );
    }

    #[test]
    fn macos_cache_identity_ignores_lifetimes_and_tracks_address_changes() {
        let route = "gateway: 192.0.2.1 interface: en0 expire: 60";
        let config =
            "en0: flags=8863 mtu 1500 inet 192.0.2.2 inet6 fe80::1 pltime 3600 vltime 7200";
        let key = macos_route_fingerprint(route, config, 1500);
        assert_eq!(
            key,
            macos_route_fingerprint(
                &route.replace("60", "30"),
                &config.replace("3600", "3000"),
                1500
            )
        );
        assert_ne!(
            key,
            macos_route_fingerprint(route, &config.replace("192.0.2.2", "192.0.2.3"), 1500)
        );
    }

    #[test]
    fn manual_override_is_validated_without_network_access() {
        assert_eq!(select(None, &[], Some(1492), None).unwrap(), 1492);
        assert!(select(None, &[], Some(1279), None).is_err());
        assert_eq!(select(None, &[], None, None).unwrap(), FALLBACK_MTU);
        assert_eq!(tun_from_path(1492, 80), 1412);
        assert_eq!(tun_from_path(1280, 128), 1280);
    }

    #[test]
    fn cache_retains_multiple_paths_and_rejects_changed_network_or_invalid_sizes() {
        let dir = std::env::temp_dir().join(format!("sockrocket-mtu-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("mtu.json");
        write_cache(&path, "network-a", 1492);
        write_cache(&path, "network-a-second-ip", 1480);
        assert_eq!(read_cache(&path, "network-a", 1500), Some(1492));
        assert_eq!(read_cache(&path, "network-a-second-ip", 1500), Some(1480));
        assert_eq!(read_cache(&path, "network-b", 1500), None);
        assert_eq!(read_cache(&path, "network-a", 1400), None);
        for (version, measured_at, path_mtu) in [
            (CACHE_VERSION, now() + 100, 1492),
            (CACHE_VERSION, now(), 1279),
            (1, now(), 1492),
        ] {
            let cache = Cache {
                version,
                entries: vec![CachedPath {
                    key: "network-a".into(),
                    measured_at,
                    path_mtu,
                }],
            };
            std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();
            assert_eq!(read_cache(&path, "network-a", 1500), None);
        }
        std::fs::write(&path, "broken cache").unwrap();
        assert_eq!(read_cache(&path, "network-a", 1500), None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cached_network_skips_probe_and_has_no_periodic_expiry() {
        assert_eq!(
            cached_or_discover(Some(1492), || panic!("warm startup must not probe")),
            Some(1492)
        );
        assert_eq!(cached_or_discover(None, || Some(1480)), Some(1480));
        let dir = std::env::temp_dir().join(format!("sockrocket-mtu-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("mtu.json");
        let cache = Cache {
            version: CACHE_VERSION,
            entries: vec![CachedPath {
                key: "same-network".into(),
                measured_at: 1,
                path_mtu: 1492,
            }],
        };
        std::fs::write(&path, serde_json::to_vec(&cache).unwrap()).unwrap();
        assert_eq!(read_cache(&path, "same-network", 1500), Some(1492));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn only_explicit_unreachable_routes_are_skipped_and_dns_is_deduplicated() {
        assert!(no_outbound_route(
            false,
            "RTNETLINK answers: Network is unreachable"
        ));
        assert!(!no_outbound_route(false, "ip: not found"));
        assert!(!no_outbound_route(false, "operation not permitted"));
        assert!(!no_outbound_route(true, "Network is unreachable"));
        let a: IpAddr = "192.0.2.1".parse().unwrap();
        let b: IpAddr = "2001:db8::1".parse().unwrap();
        assert_eq!(unique_targets(&[a, a, b, a]), vec![a, b]);
    }

    #[test]
    #[cfg(unix)]
    fn external_commands_obey_startup_deadline() {
        let start = Instant::now();
        assert!(command("sleep", &["5".into()], start + Duration::from_millis(50)).is_none());
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(
            command(
                "sleep",
                &["5".into()],
                Instant::now() - Duration::from_millis(1)
            )
            .is_none()
        );
    }

    #[test]
    fn manual_mtu_survives_config_roundtrip() {
        let config = crate::config::model::AppConfig {
            tun_mtu: Some(1412),
            ..Default::default()
        };
        let yaml = serde_yaml::to_string(&config).unwrap();
        assert_eq!(
            crate::config::model::AppConfig::from_yaml_str(&yaml)
                .unwrap()
                .tun_mtu,
            Some(1412)
        );
        let legacy = serde_yaml::to_string(&crate::config::model::AppConfig::default()).unwrap();
        assert!(!legacy.contains("tun_mtu"));
        assert_eq!(
            crate::config::model::AppConfig::from_yaml_str(&legacy)
                .unwrap()
                .tun_mtu,
            None
        );
    }
}
