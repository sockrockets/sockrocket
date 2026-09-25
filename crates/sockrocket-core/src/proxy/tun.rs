use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, bail};
use etherparse::{Icmpv4Header, Icmpv6Header};
use ipstack::{IpNumber, IpStackConfig, IpStackStream, TcpConfig, TcpOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tun::AbstractDevice;

use super::connector::{SharedOutbound, connect_upstream};
use super::relay::relay_bidirectional;
use crate::dns::DnsResolver;
use crate::dns::fakeip::{FakeIpPool, is_fake_ip};
use crate::router::Router;

// ─── Global TUN exit state for emergency cleanup ───

/// Saved state for emergency route restoration when the process exits.
#[derive(Clone)]
struct TunExitState {
    orig_gateway: Ipv4Addr,
    orig_iface: String,
    bypass_ips: Vec<Ipv4Addr>,
    /// IPv6 routing state; `None` when TUN ran in IPv4-only mode.
    v6: Option<TunV6ExitState>,
}

/// IPv6 half of the saved TUN routing state.
#[derive(Clone)]
struct TunV6ExitState {
    /// The host's original IPv6 default gateway (used for bypass routes and,
    /// on macOS, for restoring the original default route).
    orig_gateway: Ipv6Addr,
    /// Interface (name) that carried the original IPv6 default route.
    orig_iface: String,
    /// The TUN device's virtual IPv6 gateway (fd9a:4c2e:17b0::1).
    /// Read on Linux/macOS teardown; on Windows routes are deleted by
    /// interface + prefix instead.
    #[allow(dead_code)]
    tun_gateway: Ipv6Addr,
    /// TUN device name (needed to delete the v6 default route on Windows/Linux).
    #[allow(dead_code)] // read under cfg(linux/windows) restore paths
    tun_name: String,
    bypass_ips: Vec<Ipv6Addr>,
}

static TUN_EXIT_STATE: std::sync::OnceLock<std::sync::Mutex<Option<TunExitState>>> =
    std::sync::OnceLock::new();

fn tun_exit_state() -> &'static std::sync::Mutex<Option<TunExitState>> {
    TUN_EXIT_STATE.get_or_init(|| std::sync::Mutex::new(None))
}

fn save_tun_exit_state(
    gateway: Ipv4Addr,
    iface: &str,
    bypass_ips: &[Ipv4Addr],
    v6: Option<TunV6ExitState>,
) {
    if let Ok(mut guard) = tun_exit_state().lock() {
        *guard = Some(TunExitState {
            orig_gateway: gateway,
            orig_iface: iface.to_string(),
            bypass_ips: bypass_ips.to_vec(),
            v6,
        });
    }
}

fn clear_tun_exit_state() {
    if let Ok(mut guard) = tun_exit_state().lock() {
        *guard = None;
    }
}

/// Emergency route restoration for use during process exit.
///
/// Reads the globally saved TUN state and restores the original default
/// route. Uses synchronous `Command::output()` (no thread spawning) to
/// ensure commands complete even during process teardown.
pub fn emergency_restore_routes() {
    let state = match tun_exit_state().lock() {
        Ok(guard) => guard.clone(),
        Err(_) => return,
    };

    let state = match state {
        Some(s) => s,
        None => return, // TUN was not active or already cleaned up
    };

    tracing::info!(
        "Emergency: restoring routes (gateway={}, iface={})",
        state.orig_gateway,
        state.orig_iface
    );

    #[cfg(target_os = "linux")]
    {
        // Setup only *added* a TUN default route alongside the original;
        // delete it (the original was never touched), then make sure the
        // original default exists as a safety net.
        let tun_gw = TUN_GATEWAY.to_string();
        let _ = run_cmd_sync("ip", &["route", "del", "default", "via", &tun_gw]);
        let gw = state.orig_gateway.to_string();
        let _ = run_cmd_sync(
            "ip",
            &[
                "route",
                "add",
                "default",
                "via",
                &gw,
                "dev",
                &state.orig_iface,
            ],
        );
        for ip in &state.bypass_ips {
            let ip_route = format!("{}/32", ip);
            let _ = run_cmd_sync("ip", &["route", "del", &ip_route]);
        }
    }

    #[cfg(target_os = "macos")]
    {
        let gw = state.orig_gateway.to_string();
        if run_cmd_sync("route", &["-n", "change", "default", &gw]).is_err() {
            let _ = run_cmd_sync("route", &["-n", "delete", "default"]);
            let _ = run_cmd_sync("route", &["-n", "add", "default", &gw]);
        }
        for ip in &state.bypass_ips {
            let _ = run_cmd_sync("route", &["-n", "delete", "-host", &ip.to_string()]);
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Setup only *added* a TUN default route alongside the original;
        // delete it (the original was never touched). If the original is
        // somehow missing (e.g. state left by an older version that replaced
        // it), re-add it as a safety net.
        let tun_gw = TUN_GATEWAY.to_string();
        let _ = run_cmd_sync("route", &["delete", "0.0.0.0", "mask", "0.0.0.0", &tun_gw]);
        let gw = state.orig_gateway.to_string();
        // Safety net only: re-add the original default if it is missing
        // (adding an existing one would pile up duplicate routes).
        let current_gw = get_default_gateway_windows().ok().map(|(g, _)| g);
        if current_gw != Some(state.orig_gateway) {
            let _ = run_cmd_sync(
                "route",
                &["add", "0.0.0.0", "mask", "0.0.0.0", &gw, "metric", "5"],
            );
        }
        for ip in &state.bypass_ips {
            let _ = run_cmd_sync("route", &["delete", &ip.to_string()]);
        }
    }

    // IPv6 teardown — symmetric with what setup installed.
    if let Some(v6) = &state.v6 {
        tracing::info!("Emergency: restoring IPv6 routes (iface={})", v6.orig_iface);

        #[cfg(target_os = "linux")]
        {
            let _ = run_cmd_sync_strs(
                "ip",
                &linux_v6_default_del_args(&v6.tun_gateway, &v6.tun_name),
            );
            for ip in &v6.bypass_ips {
                let _ = run_cmd_sync_strs("ip", &linux_v6_bypass_del_args(ip));
            }
        }

        #[cfg(target_os = "macos")]
        {
            let gw6 = scoped_v6_gateway(&v6.orig_gateway, &v6.orig_iface);
            if run_cmd_sync_strs("route", &macos_v6_change_default_args(&gw6)).is_err() {
                let _ = run_cmd_sync_strs("route", &macos_v6_delete_default_args());
                let _ = run_cmd_sync_strs("route", &macos_v6_add_default_args(&gw6));
            }
            for ip in &v6.bypass_ips {
                let _ = run_cmd_sync_strs("route", &macos_v6_bypass_del_args(ip));
            }
        }

        #[cfg(target_os = "windows")]
        {
            let _ = run_cmd_sync_strs("netsh", &windows_v6_del_default_args(&v6.tun_name));
            // Best-effort re-add of the original default route (normally a
            // no-op because the original was never deleted, or is re-learned
            // via router advertisements).
            let _ = run_cmd_sync_strs(
                "netsh",
                &windows_v6_add_default_args(&v6.orig_iface, &v6.orig_gateway, None),
            );
            for ip in &v6.bypass_ips {
                let _ = run_cmd_sync_strs("netsh", &windows_v6_bypass_del_args(ip, &v6.orig_iface));
            }
        }
    }

    // Clear the saved state so we don't restore twice
    clear_tun_exit_state();
    super::outbound_bind::clear_outbound_bind();
}

/// Run a command synchronously without spawning a thread.
/// Used during process exit when spawned threads may be killed by the OS.
fn run_cmd_sync(cmd: &str, args: &[&str]) -> Result<String> {
    let cmd_path = find_cmd(cmd).unwrap_or_else(|| cmd.to_string());
    let output = std::process::Command::new(&cmd_path)
        .args(args)
        .output()
        .with_context(|| format!("Failed to run {} {}", cmd, args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{} {} failed: {}", cmd, args.join(" "), stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// TUN device IP configuration.
const TUN_IPV4: Ipv4Addr = Ipv4Addr::new(10, 10, 0, 2);
const TUN_GATEWAY: Ipv4Addr = Ipv4Addr::new(10, 10, 0, 1);
const TUN_NETMASK: Ipv4Addr = Ipv4Addr::new(255, 255, 255, 0);
const TUN_MTU: u16 = 1500;

/// TUN device IPv6 configuration.
///
/// `fd9a:4c2e:17b0::/64` is a Unique Local Address prefix (RFC 4193, inside
/// `fd00::/8`) reserved for this application. ULA space is never routed on
/// the public Internet, so the prefix cannot collide with real IPv6
/// destinations; it only needs to not clash with other ULA on the LAN,
/// which a fixed /64 in `fd9a::/16` makes vanishingly unlikely.
const TUN_IPV6: Ipv6Addr = Ipv6Addr::new(0xfd9a, 0x4c2e, 0x17b0, 0, 0, 0, 0, 2);
const TUN_GATEWAY_V6: Ipv6Addr = Ipv6Addr::new(0xfd9a, 0x4c2e, 0x17b0, 0, 0, 0, 0, 1);
const TUN_PREFIX_V6: u8 = 64;

/// Route metric used for the TUN IPv6 default route. The original v6 default
/// route is left in place (except on macOS, where the route table only
/// tolerates one default), and this low metric makes the TUN route win.
const TUN_V6_ROUTE_METRIC: u32 = 3;
/// Metric for IPv6 bypass routes (/128 beats ::/0 on specificity anyway).
const TUN_V6_BYPASS_METRIC: u32 = 5;
/// Interface metric forced onto the TUN adapter on Windows so the effective
/// metric (interface + route) beats the physical interface's.
const TUN_V6_IFACE_METRIC: u32 = 5;

/// MSS announced to TCP peers on the TUN device. `ipstack` uses a single
/// `TcpConfig` for both address families, so the value must fit the larger
/// IPv6 header: MTU - 40 (IPv6 header) - 20 (TCP header). IPv4 peers
/// therefore see MSS 1440 instead of 1460; slightly conservative but correct
/// for both families.
fn tun_tcp_mss(mtu: u16) -> u16 {
    mtu - 60
}

/// TUN-based transparent proxy.
///
/// Creates a TUN device, intercepts all TCP/UDP traffic via the `ipstack`
/// userspace TCP/IP stack, and forwards connections through the proxy outbound.
pub struct TunProxy {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    tun_name: String,
    /// Signalled on stop so the accept loop leaves `accept()` immediately.
    shutdown: Arc<Notify>,
    /// `Some` when the TUN device was successfully assigned its IPv6 ULA
    /// address; `None` means IPv4-only mode.
    ipv6_gateway: Option<Ipv6Addr>,
}

/// Information needed to set up system routes for TUN mode.
#[derive(Debug, Clone)]
pub struct TunRouteInfo {
    pub tun_name: String,
    pub tun_gateway: Ipv4Addr,
    /// IPv6 gateway of the TUN device; `None` when IPv6 setup on the device
    /// failed (IPv4-only mode).
    pub tun_gateway_v6: Option<Ipv6Addr>,
}

impl TunProxy {
    /// Start the TUN proxy.
    ///
    /// Creates a TUN device, configures `ipstack`, and spawns the accept loop.
    /// **Requires root/admin privileges.**
    ///
    /// After calling `start()`, the caller should set up system routes via
    /// [`setup_tun_routes`] to direct traffic through the TUN device.
    ///
    /// `fakeip`: when the shared fake-IP pool is attached (router
    /// transparent-proxy deployments), the TUN-intercepted DNS resolver
    /// answers international A queries from the pool and stream handlers
    /// dial the mapped DOMAIN instead of the fake address. `None` keeps
    /// the classic behavior (dial the destination IP as-is) — the right
    /// choice on desktop clients, whose own system DNS never returns fake
    /// addresses.
    ///
    /// `router`: the user's routing rule set. When attached, explicit
    /// domain rules (domain/suffix/keyword) also apply to bare-IP traffic
    /// whose originating name is known from recent DNS answers — without
    /// this, TUN traffic would only honor rules for fake-IP (proxied)
    /// destinations.
    pub fn start(
        outbound: SharedOutbound,
        fakeip: Option<Arc<FakeIpPool>>,
        router: Option<Arc<Router>>,
    ) -> Result<Self> {
        validate_tun_environment()?;

        let mut tun_config = tun::Configuration::default();
        tun_config
            // Request a fixed, application-specific device name. Without
            // this, Linux assigns tun0/tun1/… from the shared TUN pool,
            // colliding with OpenVPN and other VPN clients on the same host.
            .tun_name(default_tun_name())
            .address(TUN_IPV4)
            .netmask(TUN_NETMASK)
            .destination(TUN_GATEWAY)
            .mtu(TUN_MTU)
            .up();

        #[cfg(target_os = "linux")]
        tun_config.platform_config(|p_cfg| {
            p_cfg.ensure_root_privileges(true);
        });

        let tun_dev =
            tun::create_as_async(&tun_config).with_context(tun_creation_failure_message)?;

        let tun_name = tun_dev
            .tun_name()
            .unwrap_or_else(|_| default_tun_name().to_string());

        tracing::info!("TUN device created: {} ({})", tun_name, TUN_IPV4);

        // Relax reverse-path filtering on the TUN device (Linux only).
        // Strict rp_filter (common router default, inherited from
        // conf/default at device creation) drops every packet we inject
        // back into the TUN: the packet's source is a public internet IP
        // whose main-table route points out the WAN, not the TUN, so the
        // kernel discards it and clients see ERR_CONNECTION_TIMED_OUT with
        // the proxy otherwise healthy. Loose mode (2) accepts sources
        // reachable via ANY interface — exactly the reinjection case.
        relax_tun_rp_filter(&tun_name);

        // Assign the IPv6 ULA address (tun crate configures IPv4 only).
        // Non-fatal: failure leaves TUN in IPv4-only mode.
        let ipv6_gateway = configure_tun_ipv6(&tun_name);

        let mut ipstack_config = IpStackConfig::default();
        ipstack_config.mtu(TUN_MTU).expect("valid MTU");

        let mut tcp_config = TcpConfig::default();
        // Idle-session timeout: ipstack resets this timer on every read/write
        // and RSTs any connection idle longer than it. 300s was too short —
        // long-lived push/heartbeat channels (chat clients, GCM :5228,
        // Apple Push :5223, QUIC-like :4430) routinely idle for longer and
        // were killed ("session timeout reached"), making the phone app show
        // the PC as offline and delaying push delivery. 3600s matches the
        // common NAT conntrack "established" timeout that home routers give
        // these connections on a direct path.
        tcp_config.timeout = std::time::Duration::from_secs(3600);
        tcp_config.options = Some(vec![TcpOptions::MaximumSegmentSize(tun_tcp_mss(TUN_MTU))]);
        tcp_config.max_unacked_bytes = 256 * 1024;
        ipstack_config.with_tcp_config(tcp_config);
        ipstack_config.udp_timeout(std::time::Duration::from_secs(30));

        let mut ip_stack = ipstack::IpStack::new(ipstack_config, tun_dev);

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = running.clone();

        let shutdown = Arc::new(Notify::new());
        let shutdown_clone = shutdown.clone();

        let handle = tokio::spawn(async move {
            // Create DNS resolver for intercepting TUN DNS queries.
            //
            // Intercepted queries may originate from clients hard-coded to
            // public DNS (e.g. 8.8.8.8) — the exact queries the GFW poisons
            // over UDP. Resolve international domains via DNS-over-TLS
            // through the proxy outbound, identical to the dnsmasq → 5300
            // pipeline, so every DNS path returns clean answers.
            let mut resolver =
                DnsResolver::with_config(crate::dns::china_split_dns_config_proxy_tls());
            resolver.set_outbound(outbound.clone());
            if let Some(pool) = &fakeip {
                resolver.set_fakeip(pool.clone());
            }
            let dns_resolver = Arc::new(resolver);

            // Track every per-stream task so a stop can abort them instead of
            // leaving them detached. Detached tasks kept their `IpStackStream`
            // (and thus their slot in ipstack's session table) alive after the
            // accept loop exited, which both leaked memory and produced the
            // "Failed to send session removal ... channel closed" flood.
            let mut streams: tokio::task::JoinSet<()> = tokio::task::JoinSet::new();

            tracing::info!("TUN accept loop started (with DNS interception)");
            while running_clone.load(Ordering::Relaxed) {
                // Race accept() against the shutdown signal.
                //
                // Without this, the loop sits in `accept()` until a packet
                // happens to arrive; a stop could only be noticed after the
                // next connection, which is what made TUN teardown feel like a
                // freeze. Selecting on the signal lets stop() return promptly
                // *without* tearing anything down early.
                let accepted = tokio::select! {
                    _ = shutdown_clone.notified() => break,
                    res = ip_stack.accept() => res,
                    // Reap finished stream tasks so the JoinSet itself does
                    // not grow unboundedly over a long TUN session.
                    Some(_) = streams.join_next(), if !streams.is_empty() => continue,
                };
                match accepted {
                    Ok(stream) => {
                        let outbound = outbound.clone();
                        let resolver = dns_resolver.clone();
                        let fakeip = fakeip.clone();
                        let router = router.clone();
                        streams.spawn(async move {
                            if let Err(e) =
                                handle_tun_stream(stream, outbound, resolver, fakeip, router).await
                            {
                                tracing::debug!("TUN stream error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        if running_clone.load(Ordering::Relaxed) {
                            tracing::warn!("TUN accept error: {}", e);
                        }
                        break;
                    }
                }
            }

            // The loop is done: cancel every in-flight stream and wait for the
            // cancellations to land before returning. The caller (stop / Drop)
            // tears the TUN device down right after this task finishes, so no
            // stream may still be holding an ipstack session at that point.
            streams.abort_all();
            while streams.join_next().await.is_some() {}

            tracing::info!("TUN accept loop exited");
        });

        Ok(TunProxy {
            running,
            handle: Some(handle),
            tun_name,
            shutdown,
            ipv6_gateway,
        })
    }

    /// Get the TUN device name.
    pub fn tun_name(&self) -> &str {
        &self.tun_name
    }

    /// Get route info for setting up system routes.
    pub fn route_info(&self) -> TunRouteInfo {
        TunRouteInfo {
            tun_name: self.tun_name.clone(),
            tun_gateway: TUN_GATEWAY,
            tun_gateway_v6: self.ipv6_gateway,
        }
    }

    /// Stop the TUN proxy and clean up.
    ///
    /// Signals the accept loop and **waits for it to finish** before returning.
    /// The wait must be unconditional: this function owns the TUN device through
    /// the spawned task, so returning early would let the caller destroy the
    /// adapter while `ipstack` is still using it — which is what caused the
    /// "channel closed" errors and the abort-on-panic crash seen when a stop()
    /// timeout was tried here.
    ///
    /// It is safe to wait because the loop now races `accept()` against the
    /// shutdown signal, so it returns immediately instead of blocking until the
    /// next packet.
    pub async fn stop(mut self) {
        self.running.store(false, Ordering::Relaxed);
        self.shutdown.notify_waiters();
        if let Some(h) = self.handle.take() {
            let _ = h.await;
        }
        tracing::info!("TUN proxy stopped");
    }
}

impl Drop for TunProxy {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        self.shutdown.notify_waiters();
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}

/// Handle a single TUN stream (TCP, UDP, or ICMP).
async fn handle_tun_stream(
    stream: IpStackStream,
    outbound: SharedOutbound,
    dns_resolver: Arc<DnsResolver>,
    fakeip: Option<Arc<FakeIpPool>>,
    router: Option<Arc<Router>>,
) -> Result<()> {
    match stream {
        IpStackStream::Tcp(mut tcp) => {
            let dst = tcp.peer_addr();
            // Fake-IP restore: the address is a pool token, not a routable
            // destination. Dial the mapped DOMAIN so the relay resolves it
            // server-side — the same path SOCKS5 clients use, and the only
            // one an unlock-model exit can actually complete.
            let (host, port, force_direct) = match tun_dial_target(&dst, &fakeip, router.as_ref()) {
                Ok(hp) => hp,
                Err(e) => {
                    tracing::debug!("TUN TCP {}: {}", dst, e);
                    let _ = tcp.shutdown().await;
                    return Ok(());
                }
            };
            tracing::debug!(
                "TUN TCP: -> {}:{}{}",
                host,
                port,
                if force_direct { " (direct)" } else { "" }
            );

            match connect_tun_upstream(&outbound, &host, port, force_direct).await {
                Ok(mut remote) => {
                    let result = relay_bidirectional(&mut tcp, &mut remote).await;
                    let _ = remote.shutdown().await;
                    let _ = tcp.shutdown().await;
                    if let Err(e) = result {
                        tracing::debug!("TUN TCP relay error {}:{}: {}", host, port, e);
                    }
                }
                Err(e) => {
                    tracing::debug!("TUN TCP connect failed {}:{}: {}", host, port, e);
                    let _ = tcp.shutdown().await;
                }
            }
        }
        IpStackStream::Udp(mut udp) => {
            let dst = udp.peer_addr();

            // Intercept DNS queries (UDP port 53) and resolve locally
            if dst.port() == 53 {
                handle_tun_dns(&mut udp, &dns_resolver).await?;
                return Ok(());
            }

            let (host, port, force_direct) = match tun_dial_target(&dst, &fakeip, router.as_ref()) {
                Ok(hp) => hp,
                Err(e) => {
                    tracing::debug!("TUN UDP {}: {}", dst, e);
                    let _ = udp.shutdown().await;
                    return Ok(());
                }
            };
            tracing::debug!(
                "TUN UDP: -> {}:{}{}",
                host,
                port,
                if force_direct { " (direct)" } else { "" }
            );

            // TODO: this relays UDP payload over a TCP-oriented outbound
            // (UDP-over-TCP) — incorrect semantics kept as a temporary
            // behavior until a proper UDP association path exists. The
            // connect_upstream() timeout at least bounds the error path so
            // a dead node can't hang this task forever.
            match connect_tun_upstream(&outbound, &host, port, force_direct).await {
                Ok(mut remote) => {
                    let result = relay_bidirectional(&mut udp, &mut remote).await;
                    let _ = remote.shutdown().await;
                    let _ = udp.shutdown().await;
                    if let Err(e) = result {
                        tracing::debug!("TUN UDP relay error {}:{}: {}", host, port, e);
                    }
                }
                Err(e) => {
                    tracing::debug!("TUN UDP connect failed {}:{}: {}", host, port, e);
                    let _ = udp.shutdown().await;
                }
            }
        }
        IpStackStream::UnknownTransport(pkt) => {
            if pkt.src_addr().is_ipv4()
                && pkt.ip_protocol() == IpNumber::ICMP
                && let Ok((icmp_header, req_payload)) = Icmpv4Header::from_slice(pkt.payload())
                && let etherparse::Icmpv4Type::EchoRequest(echo) = icmp_header.icmp_type
            {
                let mut resp = Icmpv4Header::new(etherparse::Icmpv4Type::EchoReply(echo));
                resp.update_checksum(req_payload);
                let mut payload = resp.to_bytes().to_vec();
                payload.extend_from_slice(req_payload);
                let _ = pkt.send(payload);
            } else if pkt.src_addr().is_ipv6()
                && pkt.ip_protocol() == IpNumber::IPV6_ICMP
                && let (IpAddr::V6(src), IpAddr::V6(dst)) = (pkt.src_addr(), pkt.dst_addr())
            {
                // Only ICMPv6 echo requests are answered. NDP (RS/RA/NS/NA)
                // and every other ICMPv6 control message is deliberately
                // dropped here — it must never be proxied upstream.
                if let Some(reply) = icmpv6_echo_reply(pkt.payload(), dst, src) {
                    let _ = pkt.send(reply);
                }
            }
        }
        IpStackStream::UnknownNetwork(_) => {}
    }
    Ok(())
}

/// Handle DNS queries intercepted from TUN.
///
/// Applications commonly reuse a UDP association for multiple queries
/// (e.g. A followed by AAAA), so keep answering until the association
/// closes instead of shutting the stream down after a single query.
/// Each query is resolved with the built-in DnsResolver (bypassing the
/// tunnel) and answered with a synthesized DNS response.
async fn handle_tun_dns<S>(udp: &mut S, resolver: &DnsResolver) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut buf = [0u8; 1500];
    loop {
        let n = udp.read(&mut buf).await?;
        if n == 0 {
            break; // association closed
        }
        if n < 12 {
            continue; // too short to be a DNS packet — keep the association alive
        }
        answer_dns_query(udp, resolver, &buf[..n]).await?;
    }

    let _ = udp.shutdown().await;
    Ok(())
}

/// Answer one intercepted DNS query: parse, resolve (5s timeout), respond.
///
/// Thin wrapper over [`crate::dns::answer_raw_dns_query`] so the TUN
/// interception path and the standalone router DNS listener share one
/// implementation.
async fn answer_dns_query<S>(udp: &mut S, resolver: &DnsResolver, query: &[u8]) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let resp = crate::dns::answer_raw_dns_query(resolver, query).await;
    udp.write_all(&resp).await?;
    Ok(())
}

/// Convert a SocketAddr to (host_string, port).
#[cfg(test)]
fn addr_to_host_port(addr: &SocketAddr) -> (String, u16) {
    (addr.ip().to_string(), addr.port())
}

/// Map a TUN destination to a dial target `(host, port, force_direct)`.
///
/// - Fake address with a live mapping → the mapped domain, dialed through
///   the routing outbound (server-side resolution at the relay).
/// - Real address recently answered for a domestic-group name →
///   `force_direct`, bypassing routing: geoip databases miss too many CDN
///   ranges to be trusted for these. Exception: when the user has an
///   explicit DOMAIN rule (domain/suffix/keyword) matching that name, the
///   name is dialed through the routing outbound instead, so user rules
///   (proxy/direct/reject) take precedence over the domestic default.
/// - Anything else → the plain IP, classic behavior.
/// - Fake address WITHOUT a mapping (evicted or from an older daemon) →
///   `Err`: there is no sensible fallback; drop the stream. The client's
///   short-TTL DNS will re-ask and get a fresh fake address.
fn tun_dial_target(
    addr: &SocketAddr,
    fakeip: &Option<Arc<FakeIpPool>>,
    router: Option<&Arc<Router>>,
) -> Result<(String, u16, bool)> {
    if let Some(pool) = fakeip {
        if let IpAddr::V4(v4) = addr.ip()
            && is_fake_ip(v4)
        {
            return match pool.lookup(v4) {
                Some(domain) => Ok((domain, addr.port(), false)),
                None => anyhow::bail!(
                    "fake-IP {} has no live mapping (evicted?) — dropping stream",
                    v4
                ),
            };
        }
        if let Some(domain) = pool.lookup_domestic(addr.ip()) {
            if let Some(action) = router.and_then(|r| r.route_domain_only(&domain)) {
                tracing::debug!(
                    "TUN {} (domestic {}): user domain rule → {:?}",
                    addr,
                    domain,
                    action
                );
                return Ok((domain, addr.port(), false));
            }
            return Ok((addr.ip().to_string(), addr.port(), true));
        }
    }
    Ok((addr.ip().to_string(), addr.port(), false))
}

/// Dial an upstream for a TUN stream. `force_direct` bypasses the routing
/// outbound entirely (domestic answers honored as direct, see
/// [`tun_dial_target`]).
async fn connect_tun_upstream(
    outbound: &SharedOutbound,
    host: &str,
    port: u16,
    force_direct: bool,
) -> Result<super::connector::BoxProxyStream> {
    if force_direct {
        connect_upstream(&SharedOutbound::direct(), host, port).await
    } else {
        connect_upstream(outbound, host, port).await
    }
}

/// Build an ICMPv6 echo reply for a received packet payload, or `None` when
/// the payload is not an echo request (NDP and other control traffic).
///
/// `reply_src`/`reply_dst` are the addresses of the *reply* packet (i.e. the
/// request's dst/src) — the ICMPv6 checksum covers the IPv6 pseudo-header, so
/// it must be computed over the swapped pair.
fn icmpv6_echo_reply(payload: &[u8], reply_src: Ipv6Addr, reply_dst: Ipv6Addr) -> Option<Vec<u8>> {
    let (icmp_header, req_payload) = Icmpv6Header::from_slice(payload).ok()?;
    let etherparse::Icmpv6Type::EchoRequest(echo) = icmp_header.icmp_type else {
        return None;
    };
    let mut resp = Icmpv6Header::new(etherparse::Icmpv6Type::EchoReply(echo));
    resp.update_checksum(reply_src.octets(), reply_dst.octets(), req_payload)
        .ok()?;
    let mut out = resp.to_bytes().to_vec();
    out.extend_from_slice(req_payload);
    Some(out)
}

// ─── Platform default TUN device name ───

fn default_tun_name() -> &'static str {
    // A fixed, application-specific name on every platform. On Linux the
    // kernel would otherwise hand out tun0/tun1/… from the shared TUN pool,
    // which collides with OpenVPN and other VPN clients on the same host —
    // and on a router it made the fwmark/route scripts operate on someone
    // else's device. Requesting "sockrocket-tun" explicitly keeps it unambiguous.
    #[cfg(target_os = "macos")]
    {
        // macOS utun numbering is assigned by the kernel; utun3 is a hint.
        "utun3"
    }
    #[cfg(not(target_os = "macos"))]
    {
        "sockrocket-tun"
    }
}

// ─── Cross-platform route setup ───

/// DNS server IPs that must bypass TUN to avoid a DNS resolution loop.
/// When TUN intercepts port-53 UDP and resolves via these servers,
/// the outbound DNS packets must NOT re-enter the TUN device.
const DNS_BYPASS_IPS: &[Ipv4Addr] = &[
    Ipv4Addr::new(8, 8, 8, 8),
    Ipv4Addr::new(1, 1, 1, 1),
    Ipv4Addr::new(223, 5, 5, 5),
    Ipv4Addr::new(119, 29, 29, 29),
];

/// IPv6 DNS servers that must bypass TUN, mirroring [`DNS_BYPASS_IPS`]:
/// Google, Cloudflare, AliDNS, DNSPod.
const DNS_BYPASS_IPS_V6: &[Ipv6Addr] = &[
    Ipv6Addr::new(0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888),
    Ipv6Addr::new(0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111),
    Ipv6Addr::new(0x2400, 0x3200, 0, 0, 0, 0, 0, 1),
    Ipv6Addr::new(0x2402, 0x4e00, 0, 0, 0, 0, 0, 0),
];

/// Add one loop-avoidance route for `ip` (platform-specific).
#[cfg(target_os = "windows")]
fn add_bypass_route(ip: &Ipv4Addr, gateway: &Ipv4Addr) {
    let gw = gateway.to_string();
    let _ = run_cmd(
        "route",
        &[
            "add",
            &ip.to_string(),
            "mask",
            "255.255.255.255",
            &gw,
            "metric",
            "5",
        ],
    );
}

/// Remove one loop-avoidance route for `ip` (platform-specific).
#[cfg(target_os = "windows")]
fn remove_bypass_route(ip: &Ipv4Addr, gateway: &Ipv4Addr) {
    let gw = gateway.to_string();
    let _ = run_cmd(
        "route",
        &["delete", &ip.to_string(), "mask", "255.255.255.255", &gw],
    );
}

#[cfg(target_os = "linux")]
fn add_bypass_route(ip: &Ipv4Addr, gateway: &Ipv4Addr) {
    let _ = run_cmd(
        "ip",
        &[
            "route",
            "add",
            &format!("{}/32", ip),
            "via",
            &gateway.to_string(),
        ],
    );
}

#[cfg(target_os = "linux")]
fn remove_bypass_route(_ip: &Ipv4Addr, _gateway: &Ipv4Addr) {
    let _ = run_cmd("ip", &["route", "del", &format!("{}/32", _ip)]);
}

#[cfg(target_os = "macos")]
fn add_bypass_route(ip: &Ipv4Addr, gateway: &Ipv4Addr) {
    let _ = run_cmd(
        "route",
        &["-n", "add", "-host", &ip.to_string(), &gateway.to_string()],
    );
}

#[cfg(target_os = "macos")]
fn remove_bypass_route(ip: &Ipv4Addr, _gateway: &Ipv4Addr) {
    let _ = run_cmd("route", &["-n", "delete", "-host", &ip.to_string()]);
}

// ─── IPv6 device configuration and bypass routes ───

/// Set the TUN device's `rp_filter` to loose mode (2) on Linux.
///
/// Writes `/proc/sys/net/ipv4/conf/<dev>/rp_filter` directly instead of
/// shelling out to `sysctl`: minimal busybox builds (e.g. AsusWRT-Merlin
/// routers) ship without a `sysctl` applet, and a silent failure there
/// leaves strict filtering in place, which blackholes the whole LAN.
/// Retries briefly because the interface's conf entry may lag device
/// creation by a few milliseconds on slow embedded kernels.
fn relax_tun_rp_filter(tun_name: &str) {
    #[cfg(target_os = "linux")]
    {
        let path = format!("/proc/sys/net/ipv4/conf/{}/rp_filter", tun_name);
        let mut last_err = None;
        for attempt in 0..10 {
            match std::fs::write(&path, "2") {
                Ok(()) => {
                    if attempt > 0 {
                        tracing::info!(
                            "TUN {}: rp_filter relaxed to loose (2) on attempt {}",
                            tun_name,
                            attempt + 1
                        );
                    } else {
                        tracing::debug!("TUN {}: rp_filter relaxed to loose (2)", tun_name);
                    }
                    return;
                }
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
        }
        tracing::warn!(
            "TUN {}: failed to relax rp_filter ({:?}); strict reverse-path \
             filtering will drop transparent-proxy replies",
            tun_name,
            last_err
        );
    }
    #[cfg(not(target_os = "linux"))]
    let _ = tun_name;
}

/// Assign the TUN IPv6 ULA address to the device.
///
/// The `tun` crate (0.8) only configures IPv4, so the v6 address is applied
/// with platform tools after device creation. Failure is **non-fatal**: the
/// caller falls back to IPv4-only mode (`None`).
fn configure_tun_ipv6(tun_name: &str) -> Option<Ipv6Addr> {
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = tun_name;
        return None;
    }

    #[cfg(target_os = "linux")]
    let result = run_cmd_strs("ip", &linux_v6_addr_args(tun_name));

    #[cfg(target_os = "macos")]
    let result = run_cmd_strs("ifconfig", &macos_v6_addr_args(tun_name));

    #[cfg(target_os = "windows")]
    let result = {
        // The WinTun adapter may take a moment to become configurable.
        let args = windows_v6_addr_args(tun_name);
        let mut last_err = None;
        let mut ok = false;
        for attempt in 0..3 {
            match run_cmd_strs("netsh", &args) {
                Ok(_) => {
                    ok = true;
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    if attempt < 2 {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                    }
                }
            }
        }
        if ok {
            Ok(String::new())
        } else {
            Err(last_err.unwrap_or_else(|| anyhow::anyhow!("netsh failed")))
        }
    };

    match result {
        Ok(_) => {
            tracing::info!(
                "TUN device {} IPv6 address: {}/{}",
                tun_name,
                TUN_IPV6,
                TUN_PREFIX_V6
            );
            Some(TUN_GATEWAY_V6)
        }
        Err(e) => {
            tracing::warn!(
                "IPv6 address setup on TUN device {} failed: {:#}. Continuing in IPv4-only mode.",
                tun_name,
                e
            );
            None
        }
    }
}

/// Add one IPv6 loop-avoidance route for `ip` (platform-specific).
#[cfg(target_os = "windows")]
fn add_bypass_route_v6(ip: &Ipv6Addr, gateway: &Ipv6Addr, iface: &str) {
    let _ = run_cmd_strs("netsh", &windows_v6_bypass_add_args(ip, iface, gateway));
}

/// Remove one IPv6 loop-avoidance route (platform-specific).
#[cfg(target_os = "windows")]
fn remove_bypass_route_v6(ip: &Ipv6Addr, _gateway: &Ipv6Addr, iface: &str) {
    let _ = run_cmd_strs("netsh", &windows_v6_bypass_del_args(ip, iface));
}

#[cfg(target_os = "linux")]
fn add_bypass_route_v6(ip: &Ipv6Addr, gateway: &Ipv6Addr, iface: &str) {
    let _ = run_cmd_strs("ip", &linux_v6_bypass_add_args(ip, gateway, iface));
}

#[cfg(target_os = "linux")]
fn remove_bypass_route_v6(ip: &Ipv6Addr, _gateway: &Ipv6Addr, _iface: &str) {
    let _ = run_cmd_strs("ip", &linux_v6_bypass_del_args(ip));
}

#[cfg(target_os = "macos")]
fn add_bypass_route_v6(ip: &Ipv6Addr, gateway: &Ipv6Addr, iface: &str) {
    let _ = run_cmd_strs(
        "route",
        &macos_v6_bypass_add_args(ip, &scoped_v6_gateway(gateway, iface)),
    );
}

#[cfg(target_os = "macos")]
fn remove_bypass_route_v6(ip: &Ipv6Addr, _gateway: &Ipv6Addr, _iface: &str) {
    let _ = run_cmd_strs("route", &macos_v6_bypass_del_args(ip));
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn remove_bypass_route_v6(_ip: &Ipv6Addr, _gateway: &Ipv6Addr, _iface: &str) {}

/// Replace the loop-avoidance routes while TUN is still running.
///
/// TUN sends everything to the virtual adapter, so the proxy server's own IP
/// must be exempted or our outbound connection to it would loop straight back
/// into TUN. That exempt set is built when TUN starts — which means it only
/// ever covered *the node selected at that moment*. Switching nodes swapped the
/// outbound but left the old exemptions in place, so the new node's IP was not
/// exempt: its traffic went into TUN, looped, and the node appeared
/// unreachable. Call this on every node change to keep the exemptions in sync.
pub fn update_tun_bypass_ips(proxy_server_ips: &[IpAddr]) -> Result<()> {
    let (gateway, iface, old_ips, old_v6) = {
        let guard = tun_exit_state()
            .lock()
            .map_err(|_| anyhow::anyhow!("TUN exit state lock poisoned"))?;
        match guard.as_ref() {
            Some(s) => (
                s.orig_gateway,
                s.orig_iface.clone(),
                s.bypass_ips.clone(),
                s.v6.clone(),
            ),
            None => anyhow::bail!("TUN is not running — no bypass routes to update"),
        }
    };

    // Same composition as setup_tun_routes: node IPs plus the fixed DNS set.
    let (mut new_ips, mut new_ips_v6) = split_by_family(proxy_server_ips);
    let node_has_v6 = !new_ips_v6.is_empty();
    for &ip in DNS_BYPASS_IPS {
        if !new_ips.contains(&ip) {
            new_ips.push(ip);
        }
    }
    for &ip in DNS_BYPASS_IPS_V6 {
        if !new_ips_v6.contains(&ip) {
            new_ips_v6.push(ip);
        }
    }

    for ip in &old_ips {
        if !new_ips.contains(ip) {
            remove_bypass_route(ip, &gateway);
        }
    }
    for ip in &new_ips {
        if !old_ips.contains(ip) {
            add_bypass_route(ip, &gateway);
        }
    }

    // IPv6 half: only when v6 routes were installed at setup time.
    let new_v6_state = if let Some(v6) = old_v6 {
        for ip in &v6.bypass_ips {
            if !new_ips_v6.contains(ip) {
                remove_bypass_route_v6(ip, &v6.orig_gateway, &v6.orig_iface);
            }
        }
        for ip in &new_ips_v6 {
            if !v6.bypass_ips.contains(ip) {
                add_bypass_route_v6(ip, &v6.orig_gateway, &v6.orig_iface);
            }
        }
        Some(TunV6ExitState {
            bypass_ips: new_ips_v6.clone(),
            ..v6
        })
    } else {
        if node_has_v6 {
            tracing::debug!(
                "node resolved to IPv6 addresses but TUN is in IPv4-only mode; no v6 bypass routes to update"
            );
        }
        None
    };

    save_tun_exit_state(gateway, &iface, &new_ips, new_v6_state);
    tracing::info!(
        "TUN bypass routes updated for node change ({} -> {} IPs)",
        old_ips.len(),
        new_ips.len()
    );
    Ok(())
}

/// Set up system routes to direct all traffic through the TUN device.
///
/// IPv4 routing is set up exactly as before (default route replaced, /32
/// bypass routes installed). IPv6 is **best-effort**: when the TUN device got
/// its v6 address and the host has an IPv6 default gateway, a v6 default
/// route via TUN plus /128 bypass routes are installed; any failure in the v6
/// path is logged and skipped so IPv4-only TUN keeps working.
///
/// Returns a [`TunRouteGuard`] that restores routes on drop.
pub fn setup_tun_routes(
    route_info: &TunRouteInfo,
    proxy_server_ips: &[IpAddr],
) -> Result<TunRouteGuard> {
    // Combine proxy server IPs and DNS server IPs for loop avoidance.
    let (mut bypass_ips, mut bypass_ips_v6) = split_by_family(proxy_server_ips);
    for &ip in DNS_BYPASS_IPS {
        if !bypass_ips.contains(&ip) {
            bypass_ips.push(ip);
        }
    }
    for &ip in DNS_BYPASS_IPS_V6 {
        if !bypass_ips_v6.contains(&ip) {
            bypass_ips_v6.push(ip);
        }
    }

    let (orig_gateway, orig_iface) =
        get_default_gateway().context("Failed to get default gateway")?;

    tracing::info!(
        "Original default route: {} via {}",
        orig_gateway,
        orig_iface
    );

    // Bind sockrocket's own outbound sockets to the physical interface so that
    // direct-routed connections don't follow the TUN default route straight
    // back into the TUN (infinite session loop). Must be active before the
    // routes below are changed.
    install_outbound_bind(&orig_iface);

    #[cfg(target_os = "linux")]
    setup_routes_linux(route_info, &bypass_ips, &orig_gateway, &orig_iface)?;

    #[cfg(target_os = "macos")]
    setup_routes_macos(route_info, &bypass_ips, &orig_gateway, &orig_iface)?;

    #[cfg(target_os = "windows")]
    setup_routes_windows(route_info, &bypass_ips, &orig_gateway, &orig_iface)?;

    tracing::info!("Default route set to TUN device {}", route_info.tun_name);

    // IPv6 half — never fails the whole setup.
    let v6_state = setup_tun_routes_v6(route_info, &bypass_ips_v6);

    // Save state globally so emergency_restore_routes() can recover even
    // if the TunRouteGuard is never properly dropped (e.g., process killed).
    save_tun_exit_state(orig_gateway, &orig_iface, &bypass_ips, v6_state.clone());

    Ok(TunRouteGuard {
        orig_gateway,
        orig_iface,
        bypass_ips,
        v6: v6_state,
        restored: AtomicBool::new(false),
    })
}

/// Activate outbound-socket binding to the physical interface for the
/// duration of TUN mode (see the `outbound_bind` module docs for why).
fn install_outbound_bind(orig_iface: &str) {
    #[cfg(windows)]
    {
        // On Windows the "interface" learned from `route print` is the
        // adapter's local IPv4 address, so resolve the index by address.
        let index = orig_iface
            .parse::<Ipv4Addr>()
            .ok()
            .and_then(super::outbound_bind::interface_index_by_ip);
        match index {
            Some(idx) => {
                tracing::info!("Binding outbound sockets to interface index {}", idx);
                super::outbound_bind::set_outbound_bind(super::outbound_bind::OutboundBind {
                    if_index_v4: Some(idx),
                    // apply() falls back to the v4 index for v6 sockets.
                    if_index_v6: None,
                    if_name: None,
                });
            }
            None => {
                tracing::warn!(
                    "Could not resolve interface index for {}; direct traffic may loop into TUN",
                    orig_iface
                );
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        super::outbound_bind::set_outbound_bind(super::outbound_bind::OutboundBind {
            if_index_v4: None,
            if_index_v6: None,
            if_name: Some(orig_iface.to_string()),
        });
    }
    #[cfg(target_os = "macos")]
    {
        match super::outbound_bind::interface_index_by_name(orig_iface) {
            Some(idx) => {
                super::outbound_bind::set_outbound_bind(super::outbound_bind::OutboundBind {
                    if_index_v4: Some(idx),
                    if_index_v6: Some(idx),
                    if_name: None,
                });
            }
            None => {
                tracing::warn!(
                    "Could not resolve interface index for {}; direct traffic may loop into TUN",
                    orig_iface
                );
            }
        }
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    let _ = orig_iface;
}

/// Set up IPv6 routes through the TUN device on a best-effort basis.
///
/// Returns the v6 routing state when routes were installed (needed for
/// symmetric teardown), or `None` when v6 is unavailable / failed and TUN
/// should continue in IPv4-only mode.
fn setup_tun_routes_v6(
    route_info: &TunRouteInfo,
    bypass_ips_v6: &[Ipv6Addr],
) -> Option<TunV6ExitState> {
    let tun_gateway_v6 = match route_info.tun_gateway_v6 {
        Some(gw) => gw,
        None => {
            tracing::info!("TUN device has no IPv6 address; skipping IPv6 route setup");
            return None;
        }
    };

    let (orig_gateway_v6, orig_iface_v6) = match get_default_gateway_v6() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "No IPv6 default gateway detected ({}); TUN continues in IPv4-only mode",
                e
            );
            return None;
        }
    };

    tracing::info!(
        "Original IPv6 default route: {} via {}",
        orig_gateway_v6,
        orig_iface_v6
    );

    #[cfg(target_os = "linux")]
    let result = setup_routes_linux_v6(
        route_info,
        &tun_gateway_v6,
        bypass_ips_v6,
        &orig_gateway_v6,
        &orig_iface_v6,
    );

    #[cfg(target_os = "macos")]
    let result = setup_routes_macos_v6(
        route_info,
        &tun_gateway_v6,
        bypass_ips_v6,
        &orig_gateway_v6,
        &orig_iface_v6,
    );

    #[cfg(target_os = "windows")]
    let result = setup_routes_windows_v6(
        route_info,
        &tun_gateway_v6,
        bypass_ips_v6,
        &orig_gateway_v6,
        &orig_iface_v6,
    );

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    let result: Result<()> = Ok(());

    match result {
        Ok(()) => {
            tracing::info!(
                "IPv6 default route set to TUN device {}",
                route_info.tun_name
            );
            Some(TunV6ExitState {
                orig_gateway: orig_gateway_v6,
                orig_iface: orig_iface_v6,
                tun_gateway: tun_gateway_v6,
                tun_name: route_info.tun_name.clone(),
                bypass_ips: bypass_ips_v6.to_vec(),
            })
        }
        Err(e) => {
            // Roll back whatever bypass routes may have been installed and
            // continue IPv4-only; v6 traffic simply keeps using the original
            // route (which, except on macOS, we never touched).
            tracing::warn!(
                "IPv6 route setup failed: {:#}. Removing v6 bypass routes; TUN continues in IPv4-only mode",
                e
            );
            for ip in bypass_ips_v6 {
                remove_bypass_route_v6(ip, &orig_gateway_v6, &orig_iface_v6);
            }
            None
        }
    }
}

/// Guard that restores system routes when dropped.
pub struct TunRouteGuard {
    #[allow(dead_code)] // read under cfg(macos) restore; stored for all platforms
    orig_gateway: Ipv4Addr,
    #[allow(dead_code)] // retained for diagnostics / future restore paths
    orig_iface: String,
    bypass_ips: Vec<Ipv4Addr>,
    /// IPv6 routing state; `None` when TUN ran in IPv4-only mode.
    v6: Option<TunV6ExitState>,
    restored: AtomicBool,
}

impl TunRouteGuard {
    /// Explicitly restore routes.
    pub fn restore(&self) {
        if self.restored.swap(true, Ordering::Relaxed) {
            return;
        }

        tracing::info!("Restoring original routes");

        #[cfg(target_os = "linux")]
        self.restore_linux();

        #[cfg(target_os = "macos")]
        self.restore_macos();

        #[cfg(target_os = "windows")]
        self.restore_windows();

        // Clear global state so emergency_restore_routes() is a no-op
        clear_tun_exit_state();
        super::outbound_bind::clear_outbound_bind();
    }
}

impl Drop for TunRouteGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

// ─── Linux implementation ───

#[cfg(target_os = "linux")]
fn setup_routes_linux(
    route_info: &TunRouteInfo,
    proxy_server_ips: &[Ipv4Addr],
    orig_gateway: &Ipv4Addr,
    orig_iface: &str,
) -> Result<()> {
    // Enable IP forwarding. Write the knob directly instead of shelling out
    // to sysctl(8) — stripped router firmwares (busybox without the sysctl
    // applet, no procps) don't ship the command at all.
    std::fs::write("/proc/sys/net/ipv4/ip_forward", "1").context("enable net.ipv4.ip_forward")?;

    let gw_str = orig_gateway.to_string();
    // Add direct routes for proxy server IPs (loop avoidance)
    for ip in proxy_server_ips {
        let ip_route = format!("{}/32", ip);
        let _ = run_cmd(
            "ip",
            &["route", "add", &ip_route, "via", &gw_str, "dev", orig_iface],
        );
        tracing::info!(
            "Route: {} -> {} via {} (loop avoidance)",
            ip,
            orig_gateway,
            orig_iface
        );
    }

    // Wait for TUN device to be fully ready before setting routes
    wait_for_tun_device(&route_info.tun_name)?;

    let tun_gw = route_info.tun_gateway.to_string();

    // Add the TUN default route *alongside* the original one with a low
    // metric. The original must stay: outbound sockets bound to the physical
    // device (SO_BINDTODEVICE, see outbound_bind) need a default route that
    // egresses that device, or every bound connect fails with "no route to
    // host". Unbound traffic still prefers the TUN route (metric 10 beats
    // the typical DHCP metric of 100).
    // Delete any stale TUN default first so re-runs are idempotent.
    let _ = run_cmd(
        "ip",
        &[
            "route",
            "del",
            "default",
            "via",
            &tun_gw,
            "dev",
            &route_info.tun_name,
        ],
    );
    run_cmd(
        "ip",
        &[
            "route",
            "add",
            "default",
            "via",
            &tun_gw,
            "dev",
            &route_info.tun_name,
            "metric",
            "10",
        ],
    )
    .with_context(|| {
        format!(
            "Failed to set default route through TUN device {}. {}",
            route_info.tun_name,
            tun_route_failure_hint()
        )
    })?;

    Ok(())
}

#[cfg(target_os = "linux")]
impl TunRouteGuard {
    fn restore_linux(&self) {
        // Setup only *added* a TUN default route alongside the original, so
        // teardown just deletes it — the original default was never touched.
        let tun_gw = TUN_GATEWAY.to_string();
        let _ = run_cmd("ip", &["route", "del", "default", "via", &tun_gw]);
        for ip in &self.bypass_ips {
            let ip_route = format!("{}/32", ip);
            let _ = run_cmd("ip", &["route", "del", &ip_route]);
        }

        // IPv6: the original default route was never touched (the TUN route
        // was added alongside it with a lower metric), so teardown only
        // deletes what setup installed.
        if let Some(v6) = &self.v6 {
            let _ = run_cmd_strs(
                "ip",
                &linux_v6_default_del_args(&v6.tun_gateway, &v6.tun_name),
            );
            for ip in &v6.bypass_ips {
                let _ = run_cmd_strs("ip", &linux_v6_bypass_del_args(ip));
            }
        }
    }
}

/// IPv6 route setup (Linux).
///
/// Installs /128 bypass routes via the original gateway, then adds a default
/// route via the TUN v6 gateway with a low metric. The original default route
/// is left untouched, which keeps teardown simple and fail-safe.
#[cfg(target_os = "linux")]
fn setup_routes_linux_v6(
    route_info: &TunRouteInfo,
    tun_gateway_v6: &Ipv6Addr,
    bypass_ips_v6: &[Ipv6Addr],
    orig_gateway: &Ipv6Addr,
    orig_iface: &str,
) -> Result<()> {
    // Enable IPv6 forwarding (direct write — sysctl may not exist on routers)
    let _ = std::fs::write("/proc/sys/net/ipv6/conf/all/forwarding", "1");

    for ip in bypass_ips_v6 {
        let _ = run_cmd_strs(
            "ip",
            &linux_v6_bypass_add_args(ip, orig_gateway, orig_iface),
        );
        tracing::info!(
            "Route: {} -> {} via {} (loop avoidance, IPv6)",
            ip,
            orig_gateway,
            orig_iface
        );
    }

    run_cmd_strs(
        "ip",
        &linux_v6_default_add_args(tun_gateway_v6, &route_info.tun_name),
    )
    .with_context(|| {
        format!(
            "Failed to set IPv6 default route through TUN device {}. {}",
            route_info.tun_name,
            tun_route_failure_hint()
        )
    })?;

    Ok(())
}

// ─── macOS implementation ───

#[cfg(target_os = "macos")]
fn setup_routes_macos(
    route_info: &TunRouteInfo,
    proxy_server_ips: &[Ipv4Addr],
    orig_gateway: &Ipv4Addr,
    _orig_iface: &str,
) -> Result<()> {
    let gw_str = orig_gateway.to_string();
    // Add direct routes for proxy server IPs (loop avoidance)
    for ip in proxy_server_ips {
        let ip_str = ip.to_string();
        let _ = run_cmd("route", &["-n", "add", "-host", &ip_str, &gw_str]);
        tracing::info!("Route: {} -> {} (loop avoidance)", ip, orig_gateway);
    }

    // Replace default route through TUN gateway
    let tun_gw = route_info.tun_gateway.to_string();

    // Try change first (atomic), then delete+add as fallback
    if run_cmd("route", &["-n", "change", "default", &tun_gw]).is_ok() {
        return Ok(());
    }
    tracing::warn!("route change failed, trying delete+add");

    let _ = run_cmd("route", &["-n", "delete", "default"]);
    run_cmd("route", &["-n", "add", "default", &tun_gw]).with_context(|| {
        format!(
            "Failed to set default route through TUN. {}",
            tun_route_failure_hint()
        )
    })?;

    Ok(())
}

#[cfg(target_os = "macos")]
impl TunRouteGuard {
    fn restore_macos(&self) {
        let gw_str = self.orig_gateway.to_string();
        // Try change first, then delete+add
        if run_cmd("route", &["-n", "change", "default", &gw_str]).is_err() {
            let _ = run_cmd("route", &["-n", "delete", "default"]);
            let _ = run_cmd("route", &["-n", "add", "default", &gw_str]);
        }
        for ip in &self.bypass_ips {
            let _ = run_cmd("route", &["-n", "delete", "-host", &ip.to_string()]);
        }

        // IPv6: macOS only tolerates one default route per family, so setup
        // *replaced* the original — restore it here (mirror of the v4 logic).
        if let Some(v6) = &self.v6 {
            let gw6 = scoped_v6_gateway(&v6.orig_gateway, &v6.orig_iface);
            if run_cmd_strs("route", &macos_v6_change_default_args(&gw6)).is_err() {
                let _ = run_cmd_strs("route", &macos_v6_delete_default_args());
                let _ = run_cmd_strs("route", &macos_v6_add_default_args(&gw6));
            }
            for ip in &v6.bypass_ips {
                let _ = run_cmd_strs("route", &macos_v6_bypass_del_args(ip));
            }
        }
    }
}

/// IPv6 route setup (macOS).
///
/// Unlike Linux/Windows, macOS refuses a second default route per family, so
/// this *replaces* the original v6 default (restored on teardown) — the same
/// strategy the v4 path uses.
#[cfg(target_os = "macos")]
fn setup_routes_macos_v6(
    _route_info: &TunRouteInfo,
    tun_gateway_v6: &Ipv6Addr,
    bypass_ips_v6: &[Ipv6Addr],
    orig_gateway: &Ipv6Addr,
    orig_iface: &str,
) -> Result<()> {
    let scoped_orig = scoped_v6_gateway(orig_gateway, orig_iface);
    for ip in bypass_ips_v6 {
        let _ = run_cmd_strs("route", &macos_v6_bypass_add_args(ip, &scoped_orig));
        tracing::info!("Route: {} -> {} (loop avoidance, IPv6)", ip, orig_gateway);
    }

    let tun_gw = tun_gateway_v6.to_string();

    // Try change first (atomic), then delete+add as fallback
    if run_cmd_strs("route", &macos_v6_change_default_args(&tun_gw)).is_ok() {
        return Ok(());
    }
    tracing::warn!("route -inet6 change failed, trying delete+add");

    let _ = run_cmd_strs("route", &macos_v6_delete_default_args());
    run_cmd_strs("route", &macos_v6_add_default_args(&tun_gw)).with_context(|| {
        format!(
            "Failed to set IPv6 default route through TUN. {}",
            tun_route_failure_hint()
        )
    })?;

    Ok(())
}

// ─── Windows implementation ───

#[cfg(target_os = "windows")]
fn setup_routes_windows(
    route_info: &TunRouteInfo,
    proxy_server_ips: &[Ipv4Addr],
    orig_gateway: &Ipv4Addr,
    _orig_iface: &str,
) -> Result<()> {
    let gw_str = orig_gateway.to_string();
    // Add direct routes for proxy server IPs (loop avoidance)
    for ip in proxy_server_ips {
        let ip_str = ip.to_string();
        let _ = run_cmd(
            "route",
            &[
                "add",
                &ip_str,
                "mask",
                "255.255.255.255",
                &gw_str,
                "metric",
                "5",
            ],
        );
        tracing::info!("Route: {} -> {} (loop avoidance)", ip, orig_gateway);
    }

    // Replace default route through TUN gateway
    let tun_gw = route_info.tun_gateway.to_string();

    // Add the TUN default route *alongside* the original one with a lower
    // metric. The original must stay: outbound sockets bound to the physical
    // interface (IP_UNICAST_IF, see outbound_bind) need a default route that
    // egresses that interface, or every bound connect fails instantly with
    // "no route to host". Unbound traffic still picks the TUN route (metric 3
    // beats the original's, typically 25-30).
    // Delete any stale TUN default first so re-runs are idempotent.
    let _ = run_cmd("route", &["delete", "0.0.0.0", "mask", "0.0.0.0", &tun_gw]);
    run_cmd(
        "route",
        &["add", "0.0.0.0", "mask", "0.0.0.0", &tun_gw, "metric", "3"],
    )
    .with_context(|| {
        format!(
            "Failed to set default route through TUN. {}",
            tun_route_failure_hint()
        )
    })?;

    Ok(())
}

#[cfg(target_os = "windows")]
impl TunRouteGuard {
    fn restore_windows(&self) {
        // Setup only *added* a TUN default route alongside the original, so
        // teardown just deletes it — the original default was never touched.
        let tun_gw = TUN_GATEWAY.to_string();
        let _ = run_cmd("route", &["delete", "0.0.0.0", "mask", "0.0.0.0", &tun_gw]);
        for ip in &self.bypass_ips {
            let _ = run_cmd("route", &["delete", &ip.to_string()]);
        }

        // IPv6: setup only *added* routes, so teardown deletes them. The
        // original default was never removed; the re-add is a best-effort
        // no-op safety net (otherwise RA re-learns it).
        if let Some(v6) = &self.v6 {
            let _ = run_cmd_strs("netsh", &windows_v6_del_default_args(&v6.tun_name));
            let _ = run_cmd_strs(
                "netsh",
                &windows_v6_add_default_args(&v6.orig_iface, &v6.orig_gateway, None),
            );
            for ip in &v6.bypass_ips {
                let _ = run_cmd_strs("netsh", &windows_v6_bypass_del_args(ip, &v6.orig_iface));
            }
        }
    }
}

/// IPv6 route setup (Windows, via netsh).
///
/// Strategy: lower the TUN interface metric, then *add* a default route via
/// the TUN v6 gateway with a low route metric. The original default route is
/// left in place, so teardown is just deleting the added route.
#[cfg(target_os = "windows")]
fn setup_routes_windows_v6(
    route_info: &TunRouteInfo,
    tun_gateway_v6: &Ipv6Addr,
    bypass_ips_v6: &[Ipv6Addr],
    orig_gateway: &Ipv6Addr,
    orig_iface: &str,
) -> Result<()> {
    for ip in bypass_ips_v6 {
        let _ = run_cmd_strs(
            "netsh",
            &windows_v6_bypass_add_args(ip, orig_iface, orig_gateway),
        );
        tracing::info!("Route: {} -> {} (loop avoidance, IPv6)", ip, orig_gateway);
    }

    // Effective route metric = interface metric + route metric. Force the
    // TUN interface metric low so the added default wins over the physical
    // interface's (typically automatic, i.e. high) metric.
    let _ = run_cmd_strs(
        "netsh",
        &windows_v6_set_iface_metric_args(&route_info.tun_name),
    );

    run_cmd_strs(
        "netsh",
        &windows_v6_add_default_args(
            &route_info.tun_name,
            tun_gateway_v6,
            Some(TUN_V6_ROUTE_METRIC),
        ),
    )
    .with_context(|| {
        format!(
            "Failed to set IPv6 default route through TUN. {}",
            tun_route_failure_hint()
        )
    })?;

    Ok(())
}

// ─── Cross-platform default gateway detection ───

fn get_default_gateway() -> Result<(Ipv4Addr, String)> {
    #[cfg(target_os = "linux")]
    {
        get_default_gateway_linux()
    }
    #[cfg(target_os = "macos")]
    {
        get_default_gateway_macos()
    }
    #[cfg(target_os = "windows")]
    {
        get_default_gateway_windows()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        bail!("TUN mode is not supported on this platform")
    }
}

#[cfg(target_os = "linux")]
fn get_default_gateway_linux() -> Result<(Ipv4Addr, String)> {
    // Try /proc/net/route first (most reliable, no external command)
    if let Ok(content) = std::fs::read_to_string("/proc/net/route") {
        for line in content.lines().skip(1) {
            let fields: Vec<&str> = line.split('\t').collect();
            if fields.len() < 8 {
                continue;
            }
            let iface = fields[0].trim();
            let dest = fields[1].trim();
            let gw_hex = fields[2].trim();
            let flags: u32 = u32::from_str_radix(fields[3].trim(), 16).unwrap_or(0);
            let mask = fields[7].trim();

            if dest == "00000000" && mask == "00000000" && (flags & 0x0002) != 0 {
                let gw_u32 = u32::from_str_radix(gw_hex, 16)
                    .context("Invalid gateway hex in /proc/net/route")?;
                // /proc/net/route stores in little-endian on x86
                let gateway = Ipv4Addr::new(
                    (gw_u32 & 0xFF) as u8,
                    ((gw_u32 >> 8) & 0xFF) as u8,
                    ((gw_u32 >> 16) & 0xFF) as u8,
                    ((gw_u32 >> 24) & 0xFF) as u8,
                );
                return Ok((gateway, iface.to_string()));
            }
        }
    }

    // Fallback: parse `ip route show default`
    let output = run_cmd("ip", &["route", "show", "default"])?;
    // Example: "default via 192.168.1.1 dev eth0 proto dhcp metric 100"
    let parts: Vec<&str> = output.split_whitespace().collect();
    if parts.len() >= 5 && parts[0] == "default" && parts[1] == "via" {
        let gw: Ipv4Addr = parts[2].parse().context("Failed to parse gateway IP")?;
        let iface = if parts[3] == "dev" { parts[4] } else { "eth0" };
        return Ok((gw, iface.to_string()));
    }

    bail!("No default gateway found on Linux")
}

#[cfg(target_os = "macos")]
fn get_default_gateway_macos() -> Result<(Ipv4Addr, String)> {
    // Use `route -n get default` to obtain the gateway and interface
    let output = run_cmd("route", &["-n", "get", "default"])?;
    let mut gateway: Option<Ipv4Addr> = None;
    let mut iface: Option<String> = None;

    for line in output.lines() {
        let line = line.trim();
        if let Some(gw) = line.strip_prefix("gateway:") {
            gateway = gw.trim().parse().ok();
        }
        if let Some(if_name) = line.strip_prefix("interface:") {
            iface = Some(if_name.trim().to_string());
        }
    }

    match (gateway, iface) {
        (Some(gw), Some(ifc)) => Ok((gw, ifc)),
        (Some(gw), None) => Ok((gw, "en0".to_string())),
        _ => {
            // Fallback: parse `netstat -rn`
            let ns_output = run_cmd("netstat", &["-rn"])?;
            for line in ns_output.lines() {
                let fields: Vec<&str> = line.split_whitespace().collect();
                if fields.len() >= 4
                    && fields[0] == "default"
                    && let Ok(gw) = fields[1].parse::<Ipv4Addr>()
                {
                    let if_name = fields.last().unwrap_or(&"en0");
                    return Ok((gw, if_name.to_string()));
                }
            }
            bail!("No default gateway found on macOS")
        }
    }
}

#[cfg(target_os = "windows")]
fn get_default_gateway_windows() -> Result<(Ipv4Addr, String)> {
    // Parse `route print 0.0.0.0` for the default route
    let output = run_cmd("route", &["print", "0.0.0.0"])?;
    // Look for lines like:
    //   0.0.0.0          0.0.0.0      192.168.1.1    192.168.1.100     25
    for line in output.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 4 && fields[0] == "0.0.0.0" && fields[1] == "0.0.0.0" {
            if let Ok(gw) = fields[2].parse::<Ipv4Addr>() {
                // Interface is the local IP (fields[3]), use it as the "iface" identifier
                let iface = fields.get(3).unwrap_or(&"0.0.0.0");
                return Ok((gw, iface.to_string()));
            }
        }
    }

    // Fallback: try PowerShell
    let ps_output = run_cmd(
        "powershell",
        &[
            "-Command",
            "(Get-NetRoute -DestinationPrefix '0.0.0.0/0' | Select-Object -First 1).NextHop",
        ],
    );
    if let Ok(ps_out) = ps_output {
        let gw_str = ps_out.trim();
        if let Ok(gw) = gw_str.parse::<Ipv4Addr>() {
            return Ok((gw, "0.0.0.0".to_string()));
        }
    }

    bail!("No default gateway found on Windows")
}

// ─── Cross-platform IPv6 default gateway detection ───
//
// Best-effort: callers treat an error as "host has no IPv6 connectivity" and
// keep TUN in IPv4-only mode.

fn get_default_gateway_v6() -> Result<(Ipv6Addr, String)> {
    #[cfg(target_os = "linux")]
    {
        get_default_gateway_v6_linux()
    }
    #[cfg(target_os = "macos")]
    {
        get_default_gateway_v6_macos()
    }
    #[cfg(target_os = "windows")]
    {
        get_default_gateway_v6_windows()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        bail!("IPv6 TUN mode is not supported on this platform")
    }
}

#[cfg(target_os = "linux")]
fn get_default_gateway_v6_linux() -> Result<(Ipv6Addr, String)> {
    // Example line: "default via fe80::1 dev eth0 proto ra metric 1024"
    let output = run_cmd("ip", &["-6", "route", "show", "default"])?;
    parse_linux_v6_default_route(&output).context("No IPv6 default gateway found on Linux")
}

#[cfg(target_os = "macos")]
fn get_default_gateway_v6_macos() -> Result<(Ipv6Addr, String)> {
    // `route -n get -inet6 default` prints "gateway:" and "interface:" lines.
    let output = run_cmd("route", &["-n", "get", "-inet6", "default"])?;
    parse_macos_v6_route_get(&output).context("No IPv6 default gateway found on macOS")
}

#[cfg(target_os = "windows")]
fn get_default_gateway_v6_windows() -> Result<(Ipv6Addr, String)> {
    // PowerShell prints "<NextHop>|<InterfaceAlias>" for the best ::/0 route.
    let output = run_cmd(
        "powershell",
        &[
            "-NoProfile",
            "-Command",
            "(Get-NetRoute -DestinationPrefix '::/0' | Sort-Object RouteMetric | \
             Select-Object -First 1 | ForEach-Object { \"$($_.NextHop)|$($_.InterfaceAlias)\" })",
        ],
    )?;
    parse_windows_ps_v6_gateway(&output).context("No IPv6 default gateway found on Windows")
}

// ─── TUN device readiness check ───

/// Wait for a TUN device to appear and be operational.
#[cfg(target_os = "linux")]
fn wait_for_tun_device(tun_name: &str) -> Result<()> {
    let sys_path = format!("/sys/class/net/{}", tun_name);
    let operstate_path = format!("{}/operstate", sys_path);

    for i in 0..20 {
        if std::path::Path::new(&sys_path).exists() {
            // Check if device is up (operstate is "up" or "unknown" for TUN)
            if let Ok(state) = std::fs::read_to_string(&operstate_path) {
                let state = state.trim();
                if state == "up" || state == "unknown" {
                    tracing::debug!(
                        "TUN device {} ready (state: {}, attempt {})",
                        tun_name,
                        state,
                        i
                    );
                    return Ok(());
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    // If the device exists but operstate never became up, still try
    if std::path::Path::new(&sys_path).exists() {
        tracing::warn!(
            "TUN device {} exists but may not be fully ready, proceeding anyway",
            tun_name
        );
        return Ok(());
    }

    bail!("TUN device {} did not appear within 2 seconds", tun_name)
}

// ─── Utility functions ───

/// Run an external command with a timeout and return its output.
fn run_cmd(cmd: &str, args: &[&str]) -> Result<String> {
    run_cmd_timeout(cmd, args, std::time::Duration::from_secs(10))
}

/// Run an external command with a specific timeout.
fn run_cmd_timeout(cmd: &str, args: &[&str], timeout: std::time::Duration) -> Result<String> {
    let cmd_path = find_cmd(cmd).unwrap_or_else(|| cmd.to_string());
    let cmd_path_clone = cmd_path.clone();
    let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let args_display = args.join(" ");

    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let result = std::process::Command::new(&cmd_path_clone)
            .args(&args_owned)
            .output();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => {
            let _ = handle.join();
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                bail!("{} {} failed: {}", cmd, args_display, stderr.trim());
            }
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        }
        Ok(Err(e)) => {
            let _ = handle.join();
            bail!("Failed to run {} {}: {}", cmd, args_display, e);
        }
        Err(_) => {
            // Timeout - the thread may still be running but we proceed
            bail!(
                "{} {} timed out after {}s",
                cmd,
                args_display,
                timeout.as_secs()
            );
        }
    }
}

/// Find a command in PATH or common system directories.
fn find_cmd(cmd: &str) -> Option<String> {
    #[cfg(not(target_os = "windows"))]
    {
        if let Ok(output) = std::process::Command::new("which").arg(cmd).output()
            && output.status.success()
        {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(path);
            }
        }
        for dir in &["/usr/sbin", "/sbin", "/usr/bin", "/bin"] {
            let path = format!("{}/{}", dir, cmd);
            if std::path::Path::new(&path).exists() {
                return Some(path);
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(output) = std::process::Command::new("where").arg(cmd).output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                if !path.is_empty() {
                    return Some(path);
                }
            }
        }
    }
    None
}

// ─── Pure helpers: command builders and output parsers ───
//
// Everything in this section is platform-independent pure code so it can be
// unit-tested without root on any host. The cfg-gated platform code above
// only picks which builder's output to feed to `run_cmd*`.
//
// Builders for platforms other than the compilation target are intentionally
// kept (dead code on this target) so the tests exercise them everywhere.

/// Run a command with args built by the string builders below.
fn run_cmd_strs(cmd: &str, args: &[String]) -> Result<String> {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_cmd(cmd, &arg_refs)
}

/// Synchronous variant for the emergency restore path.
fn run_cmd_sync_strs(cmd: &str, args: &[String]) -> Result<String> {
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_cmd_sync(cmd, &arg_refs)
}

/// Split mixed IP addresses by family (order preserved).
fn split_by_family(ips: &[IpAddr]) -> (Vec<Ipv4Addr>, Vec<Ipv6Addr>) {
    let mut v4 = Vec::new();
    let mut v6 = Vec::new();
    for ip in ips {
        match ip {
            IpAddr::V4(a) => v4.push(*a),
            IpAddr::V6(a) => v6.push(*a),
        }
    }
    (v4, v6)
}

/// Format a v6 gateway for macOS `route`: link-local gateways need a
/// `%iface` scope or the kernel cannot resolve the next hop.
#[allow(dead_code)] // used on macOS only; tested everywhere
fn scoped_v6_gateway(gateway: &Ipv6Addr, iface: &str) -> String {
    if gateway.is_unicast_link_local() {
        format!("{}%{}", gateway, iface)
    } else {
        gateway.to_string()
    }
}

// — Linux (`ip -6`) —

#[allow(dead_code)]
fn linux_v6_addr_args(dev: &str) -> Vec<String> {
    vec![
        "-6".into(),
        "addr".into(),
        "add".into(),
        format!("{}/{}", TUN_IPV6, TUN_PREFIX_V6),
        "dev".into(),
        dev.into(),
    ]
}

#[allow(dead_code)]
fn linux_v6_default_add_args(tun_gw: &Ipv6Addr, dev: &str) -> Vec<String> {
    vec![
        "-6".into(),
        "route".into(),
        "add".into(),
        "default".into(),
        "via".into(),
        tun_gw.to_string(),
        "dev".into(),
        dev.into(),
        "metric".into(),
        TUN_V6_ROUTE_METRIC.to_string(),
    ]
}

#[allow(dead_code)]
fn linux_v6_default_del_args(tun_gw: &Ipv6Addr, dev: &str) -> Vec<String> {
    vec![
        "-6".into(),
        "route".into(),
        "del".into(),
        "default".into(),
        "via".into(),
        tun_gw.to_string(),
        "dev".into(),
        dev.into(),
    ]
}

#[allow(dead_code)]
fn linux_v6_bypass_add_args(ip: &Ipv6Addr, gw: &Ipv6Addr, dev: &str) -> Vec<String> {
    vec![
        "-6".into(),
        "route".into(),
        "add".into(),
        format!("{}/128", ip),
        "via".into(),
        gw.to_string(),
        "dev".into(),
        dev.into(),
    ]
}

#[allow(dead_code)]
fn linux_v6_bypass_del_args(ip: &Ipv6Addr) -> Vec<String> {
    vec![
        "-6".into(),
        "route".into(),
        "del".into(),
        format!("{}/128", ip),
    ]
}

// — macOS (`ifconfig` / `route -inet6`) —

#[allow(dead_code)]
fn macos_v6_addr_args(dev: &str) -> Vec<String> {
    vec![
        dev.into(),
        "inet6".into(),
        TUN_IPV6.to_string(),
        "prefixlen".into(),
        TUN_PREFIX_V6.to_string(),
    ]
}

#[allow(dead_code)]
fn macos_v6_change_default_args(scoped_gw: &str) -> Vec<String> {
    vec![
        "-n".into(),
        "change".into(),
        "-inet6".into(),
        "default".into(),
        scoped_gw.into(),
    ]
}

#[allow(dead_code)]
fn macos_v6_delete_default_args() -> Vec<String> {
    vec![
        "-n".into(),
        "delete".into(),
        "-inet6".into(),
        "default".into(),
    ]
}

#[allow(dead_code)]
fn macos_v6_add_default_args(scoped_gw: &str) -> Vec<String> {
    vec![
        "-n".into(),
        "add".into(),
        "-inet6".into(),
        "default".into(),
        scoped_gw.into(),
    ]
}

#[allow(dead_code)]
fn macos_v6_bypass_add_args(ip: &Ipv6Addr, scoped_gw: &str) -> Vec<String> {
    vec![
        "-n".into(),
        "add".into(),
        "-inet6".into(),
        "-host".into(),
        ip.to_string(),
        scoped_gw.into(),
    ]
}

#[allow(dead_code)]
fn macos_v6_bypass_del_args(ip: &Ipv6Addr) -> Vec<String> {
    vec![
        "-n".into(),
        "delete".into(),
        "-inet6".into(),
        "-host".into(),
        ip.to_string(),
    ]
}

// — Windows (`netsh interface ipv6`) —

#[allow(dead_code)]
fn windows_v6_addr_args(iface: &str) -> Vec<String> {
    vec![
        "interface".into(),
        "ipv6".into(),
        "set".into(),
        "address".into(),
        format!("interface={}", iface),
        format!("address={}/{}", TUN_IPV6, TUN_PREFIX_V6),
    ]
}

#[allow(dead_code)]
fn windows_v6_set_iface_metric_args(iface: &str) -> Vec<String> {
    vec![
        "interface".into(),
        "ipv6".into(),
        "set".into(),
        "interface".into(),
        format!("interface={}", iface),
        format!("metric={}", TUN_V6_IFACE_METRIC),
    ]
}

#[allow(dead_code)]
fn windows_v6_add_default_args(iface: &str, gw: &Ipv6Addr, metric: Option<u32>) -> Vec<String> {
    let mut args = vec![
        "interface".into(),
        "ipv6".into(),
        "add".into(),
        "route".into(),
        "prefix=::/0".into(),
        format!("interface={}", iface),
        format!("nexthop={}", gw),
    ];
    if let Some(m) = metric {
        args.push(format!("metric={}", m));
    }
    args
}

#[allow(dead_code)]
fn windows_v6_del_default_args(iface: &str) -> Vec<String> {
    vec![
        "interface".into(),
        "ipv6".into(),
        "delete".into(),
        "route".into(),
        "prefix=::/0".into(),
        format!("interface={}", iface),
    ]
}

#[allow(dead_code)]
fn windows_v6_bypass_add_args(ip: &Ipv6Addr, iface: &str, gw: &Ipv6Addr) -> Vec<String> {
    vec![
        "interface".into(),
        "ipv6".into(),
        "add".into(),
        "route".into(),
        format!("prefix={}/128", ip),
        format!("interface={}", iface),
        format!("nexthop={}", gw),
        format!("metric={}", TUN_V6_BYPASS_METRIC),
    ]
}

#[allow(dead_code)]
fn windows_v6_bypass_del_args(ip: &Ipv6Addr, iface: &str) -> Vec<String> {
    vec![
        "interface".into(),
        "ipv6".into(),
        "delete".into(),
        "route".into(),
        format!("prefix={}/128", ip),
        format!("interface={}", iface),
    ]
}

// — output parsers —

/// Parse `ip -6 route show default` output.
/// Example: "default via fe80::1 dev eth0 proto ra metric 1024"
#[allow(dead_code)] // used on Linux only; tested everywhere
fn parse_linux_v6_default_route(output: &str) -> Option<(Ipv6Addr, String)> {
    for line in output.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 5
            && parts[0] == "default"
            && parts[1] == "via"
            && parts[3] == "dev"
            && let Ok(gw) = parts[2].parse::<Ipv6Addr>()
            && !gw.is_unspecified()
        {
            return Some((gw, parts[4].to_string()));
        }
    }
    None
}

/// Parse `route -n get -inet6 default` output ("gateway:" / "interface:"
/// lines). The gateway may carry a `%iface` scope suffix.
#[allow(dead_code)] // used on macOS only; tested everywhere
fn parse_macos_v6_route_get(output: &str) -> Option<(Ipv6Addr, String)> {
    let mut gateway: Option<Ipv6Addr> = None;
    let mut iface: Option<String> = None;

    for line in output.lines() {
        let line = line.trim();
        if let Some(gw) = line.strip_prefix("gateway:") {
            let token = gw.trim();
            let bare = token.split('%').next().unwrap_or(token);
            gateway = bare.parse::<Ipv6Addr>().ok();
        }
        if let Some(if_name) = line.strip_prefix("interface:") {
            iface = Some(if_name.trim().to_string());
        }
    }

    match (gateway, iface) {
        (Some(gw), Some(ifc)) if !gw.is_unspecified() && !ifc.is_empty() => Some((gw, ifc)),
        _ => None,
    }
}

/// Parse the PowerShell `Get-NetRoute ::/0` output line "<NextHop>|<IfaceAlias>".
#[allow(dead_code)] // used on Windows only; tested everywhere
fn parse_windows_ps_v6_gateway(output: &str) -> Option<(Ipv6Addr, String)> {
    let line = output.lines().map(str::trim).find(|l| !l.is_empty())?;
    let (gw, iface) = line.split_once('|')?;
    let gw: Ipv6Addr = gw.trim().parse().ok()?;
    if gw.is_unspecified() {
        return None;
    }
    let iface = iface.trim();
    if iface.is_empty() {
        return None;
    }
    Some((gw, iface.to_string()))
}

/// Resolve a hostname to all IP addresses (IPv4 and IPv6) for route setup.
///
/// This is the TUN-bypass resolver: bypass routes are installed per family,
/// so an AAAA-only node yields v6 addresses here and TUN startup must not be
/// refused just because no A record exists.
pub fn resolve_to_ips(host: &str) -> Vec<IpAddr> {
    use std::net::ToSocketAddrs;
    format!("{}:0", host)
        .to_socket_addrs()
        .unwrap_or_else(|_| vec![].into_iter())
        .map(|addr| addr.ip())
        .collect()
}

/// Resolve a hostname to IPv4 addresses for route setup.
pub fn resolve_to_ipv4(host: &str) -> Vec<Ipv4Addr> {
    resolve_to_ips(host)
        .into_iter()
        .filter_map(|ip| match ip {
            IpAddr::V4(v4) => Some(v4),
            _ => None,
        })
        .collect()
}

/// Check if TUN mode is supported on the current platform.
pub fn tun_supported() -> bool {
    cfg!(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows"
    ))
}

/// Return a human-readable description of TUN requirements for the current OS.
pub fn tun_requirements() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "Requires root or CAP_NET_ADMIN capability"
    }
    #[cfg(target_os = "macos")]
    {
        "Requires root privileges (sudo)"
    }
    #[cfg(target_os = "windows")]
    {
        "Requires Administrator privileges"
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        "TUN mode is not supported on this platform"
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrivilegeStatus {
    Privileged,
    Missing,
    Unknown,
}

fn validate_tun_environment() -> Result<()> {
    if !tun_supported() {
        bail!("{}", tun_requirements());
    }

    #[cfg(target_os = "linux")]
    {
        if !std::path::Path::new("/dev/net/tun").exists() {
            bail!(
                "TUN device node /dev/net/tun is missing. Load the tun kernel module and ensure the device node exists."
            );
        }

        // Check that required routing commands are available.
        // (sysctl is intentionally NOT required: forwarding knobs are
        // written to /proc/sys directly, since router busybox builds often
        // lack the sysctl applet entirely.)
        let mut missing = Vec::new();
        if find_cmd("ip").is_none() {
            missing.push("ip (install: apt install iproute2 / dnf install iproute)");
        }
        if !missing.is_empty() {
            bail!(
                "TUN mode requires the following commands: {}",
                missing.join(", ")
            );
        }
    }

    #[cfg(target_os = "windows")]
    {
        if find_cmd("route").is_none() {
            bail!(
                "TUN mode requires the 'route' command (should be available by default on Windows)"
            );
        }
    }

    match current_privilege_status() {
        PrivilegeStatus::Missing if cfg!(any(target_os = "macos", target_os = "windows")) => {
            bail!("{}", tun_requirements());
        }
        _ => Ok(()),
    }
}

fn tun_creation_failure_message() -> String {
    #[cfg(target_os = "linux")]
    if !std::path::Path::new("/dev/net/tun").exists() {
        return "Failed to create TUN device: /dev/net/tun is missing. Load the tun kernel module and ensure the device node exists.".to_string();
    }

    match current_privilege_status() {
        PrivilegeStatus::Missing => format!("Failed to create TUN device. {}", tun_requirements()),
        PrivilegeStatus::Privileged => {
            #[cfg(target_os = "windows")]
            {
                "Failed to create TUN device even though the process has Administrator privileges\n\
                 (the underlying OS error is preserved in this error chain). The bundled WinTun\n\
                 driver may have failed to load, or a stale adapter from a previous run may\n\
                 be conflicting — check the log for details."
                    .to_string()
            }
            #[cfg(not(target_os = "windows"))]
            {
                "Failed to create TUN device. The TUN driver or adapter may be unavailable on this system.".to_string()
            }
        }
        PrivilegeStatus::Unknown => format!(
            "Failed to create TUN device. Ensure the TUN driver is installed and {}",
            tun_requirements().to_lowercase()
        ),
    }
}

fn tun_route_failure_hint() -> String {
    match current_privilege_status() {
        PrivilegeStatus::Missing => tun_requirements().to_string(),
        PrivilegeStatus::Privileged => {
            "The current process already has elevated privileges, so the TUN interface or routing command likely failed to initialize correctly.".to_string()
        }
        PrivilegeStatus::Unknown => format!(
            "Ensure the required routing tools are available and {}",
            tun_requirements().to_lowercase()
        ),
    }
}

fn current_privilege_status() -> PrivilegeStatus {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        if unix_is_root() {
            return PrivilegeStatus::Privileged;
        }
        PrivilegeStatus::Missing
    }

    #[cfg(target_os = "windows")]
    {
        return match windows_is_elevated() {
            Some(true) => PrivilegeStatus::Privileged,
            Some(false) => PrivilegeStatus::Missing,
            None => PrivilegeStatus::Unknown,
        };
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        PrivilegeStatus::Unknown
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
unsafe extern "C" {
    fn geteuid() -> u32;
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn unix_is_root() -> bool {
    unsafe { geteuid() == 0 }
}

#[cfg(target_os = "windows")]
fn windows_is_elevated() -> Option<bool> {
    // Use Win32 API — most reliable way to check admin privileges
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn IsUserAnAdmin() -> i32;
    }
    Some(unsafe { IsUserAnAdmin() != 0 })
}

/// Whether the current process holds the privileges TUN mode needs.
/// On Windows this is "running as Administrator"; on Unix, root.
pub fn tun_privileged() -> bool {
    matches!(current_privilege_status(), PrivilegeStatus::Privileged)
}

/// Relaunch the current executable with Administrator privileges via the UAC
/// prompt (`ShellExecuteW "runas"`), passing `--tun-admin` plus any extra args.
/// The UAC consent dialog is shown by the OS; on user approval the new
/// elevated process starts and this one should exit (see
/// [`relaunch_elevated_and_exit`]). Returns `Err` when the user cancels the
/// prompt or the relaunch fails.
#[cfg(target_os = "windows")]
pub fn relaunch_elevated(extra_args: &[&str]) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;

    let exe = std::env::current_exe()?;
    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    let file: Vec<u16> = exe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut params: Vec<u16> = Vec::new();
    for (i, arg) in std::iter::once("--tun-admin")
        .chain(extra_args.iter().copied())
        .enumerate()
    {
        if i > 0 {
            params.push(b' ' as u16);
        }
        params.extend(arg.encode_utf16());
    }
    params.push(0);

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(
            hwnd: *mut core::ffi::c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show_cmd: i32,
        ) -> *mut core::ffi::c_void;
    }
    const SW_SHOWNORMAL: i32 = 1;

    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW returns > 32 on success; ≤ 32 is an error code.
    // 1223 = ERROR_CANCELLED (user dismissed the UAC prompt).
    let code = result as usize;
    if code > 32 {
        Ok(())
    } else if code == 1223 {
        anyhow::bail!("UAC elevation cancelled by user")
    } else {
        anyhow::bail!("ShellExecuteW runas failed with code {}", code)
    }
}

/// Relaunch elevated and immediately exit the current (unprivileged) process.
/// The elevated child carries `--tun-admin` so the new instance knows to
/// auto-start TUN mode once it is up.
#[cfg(target_os = "windows")]
pub fn relaunch_elevated_and_exit(extra_args: &[&str]) -> ! {
    match relaunch_elevated(extra_args) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("UAC elevation failed: {e:#}");
            std::process::exit(1);
        }
    }
}

// === WinTun DLL auto-install (Windows only) ===

#[allow(dead_code)]
const WINTUN_DLL: &str = "wintun.dll";

// Embed the architecture-specific wintun.dll at compile time.
// The DLL files are from https://www.wintun.net/builds/wintun-0.14.1.zip
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const WINTUN_DLL_BYTES: &[u8] = include_bytes!("../../resources/wintun/wintun_amd64.dll");

#[cfg(all(target_os = "windows", target_arch = "aarch64"))]
const WINTUN_DLL_BYTES: &[u8] = include_bytes!("../../resources/wintun/wintun_arm64.dll");

#[cfg(all(target_os = "windows", target_arch = "x86"))]
const WINTUN_DLL_BYTES: &[u8] = include_bytes!("../../resources/wintun/wintun_x86.dll");

/// Check if wintun.dll is available (exists next to the executable or in PATH).
pub fn wintun_dll_available() -> bool {
    #[cfg(not(target_os = "windows"))]
    {
        true // not needed on non-Windows
    }
    #[cfg(target_os = "windows")]
    {
        wintun_dll_path().map(|p| p.exists()).unwrap_or(false)
    }
}

/// Path where wintun.dll should be placed (next to the executable).
#[allow(dead_code)]
fn wintun_dll_path() -> Option<std::path::PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.join(WINTUN_DLL)))
}

/// Ensure wintun.dll is available. Extracts the embedded DLL if missing, and
/// replaces it if the existing file does not match the bundled driver (wrong
/// architecture, stale or corrupted copy left by a previous version).
#[cfg(target_os = "windows")]
pub async fn ensure_wintun_dll() -> Result<std::path::PathBuf> {
    let dll_path = wintun_dll_path()
        .ok_or_else(|| anyhow::anyhow!("cannot determine executable directory"))?;

    if dll_path.exists() {
        match std::fs::read(&dll_path) {
            Ok(existing) if existing == WINTUN_DLL_BYTES => {
                tracing::info!(
                    "wintun.dll found at {} (matches bundled driver)",
                    dll_path.display()
                );
                return Ok(dll_path);
            }
            Ok(existing) => {
                tracing::warn!(
                    "wintun.dll at {} differs from the bundled driver ({} vs {} bytes), replacing it",
                    dll_path.display(),
                    existing.len(),
                    WINTUN_DLL_BYTES.len()
                );
            }
            Err(e) => {
                tracing::warn!(
                    "cannot read existing wintun.dll at {} ({}), re-extracting",
                    dll_path.display(),
                    e
                );
            }
        }
    } else {
        tracing::info!(
            "wintun.dll not found, extracting embedded DLL ({} bytes) to {}",
            WINTUN_DLL_BYTES.len(),
            dll_path.display()
        );
    }

    std::fs::write(&dll_path, WINTUN_DLL_BYTES).with_context(|| {
        format!(
            "failed to write wintun.dll to {} — the application directory may be read-only",
            dll_path.display()
        )
    })?;

    tracing::info!("wintun.dll installed at {}", dll_path.display());
    Ok(dll_path)
}

/// On non-Windows platforms, this is a no-op.
#[cfg(not(target_os = "windows"))]
pub async fn ensure_wintun_dll() -> Result<std::path::PathBuf> {
    Ok(std::path::PathBuf::from("/dev/net/tun"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tun_supported() {
        // On Linux/macOS/Windows, TUN should be reported as supported
        assert!(tun_supported());
    }

    #[test]
    fn test_tun_requirements_not_empty() {
        let req = tun_requirements();
        assert!(!req.is_empty());
    }

    #[test]
    fn test_resolve_to_ipv4() {
        let addrs = resolve_to_ipv4("localhost");
        assert!(
            addrs.contains(&Ipv4Addr::LOCALHOST),
            "localhost should resolve to 127.0.0.1"
        );
    }

    #[test]
    fn test_default_gateway_detection() {
        // This test verifies gateway detection works on the current system
        match get_default_gateway() {
            Ok((gw, iface)) => {
                assert!(!iface.is_empty(), "interface name should not be empty");
                // Gateway should not be 0.0.0.0 (that indicates no gateway)
                tracing::info!("Detected gateway: {} via {}", gw, iface);
            }
            Err(e) => {
                // Acceptable in some CI environments with no default route
                tracing::warn!("No default gateway (may be expected in CI): {}", e);
            }
        }
    }

    #[test]
    fn test_validate_tun_environment() {
        // Just verify it doesn't panic; result depends on the runtime environment
        let result = validate_tun_environment();
        tracing::info!("validate_tun_environment: {:?}", result);
    }

    #[test]
    fn test_addr_to_host_port() {
        let addr: SocketAddr = "1.2.3.4:443".parse().unwrap();
        let (host, port) = addr_to_host_port(&addr);
        assert_eq!(host, "1.2.3.4");
        assert_eq!(port, 443);
    }

    // ─── IPv6 support tests ───

    #[test]
    fn test_tun_tcp_mss() {
        // Must leave room for the larger IPv6 header (40) + TCP header (20).
        assert_eq!(tun_tcp_mss(1500), 1440);
        assert!(tun_tcp_mss(TUN_MTU) <= TUN_MTU - 60);
    }

    #[test]
    fn test_tun_ipv6_constants_are_ula() {
        // fc00::/7 marks Unique Local Addresses (RFC 4193).
        assert_eq!(TUN_IPV6.segments()[0] & 0xfe00, 0xfc00);
        assert_eq!(TUN_GATEWAY_V6.segments()[0] & 0xfe00, 0xfc00);
        // Both addresses share the configured /64 prefix and are distinct.
        assert_eq!(&TUN_IPV6.segments()[..4], &TUN_GATEWAY_V6.segments()[..4]);
        assert_ne!(TUN_IPV6, TUN_GATEWAY_V6);
        assert_eq!(TUN_PREFIX_V6, 64);
    }

    #[test]
    fn test_resolve_to_ips_localhost() {
        let ips = resolve_to_ips("localhost");
        assert!(
            ips.contains(&IpAddr::V4(Ipv4Addr::LOCALHOST)),
            "localhost should resolve to 127.0.0.1, got {:?}",
            ips
        );
    }

    #[test]
    fn test_split_by_family() {
        let mixed = vec![
            IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)),
            IpAddr::V6("2001:db8::1".parse().unwrap()),
            IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8)),
            IpAddr::V6("fd00::1".parse().unwrap()),
        ];
        let (v4, v6) = split_by_family(&mixed);
        assert_eq!(
            v4,
            vec![Ipv4Addr::new(1, 2, 3, 4), Ipv4Addr::new(5, 6, 7, 8)]
        );
        assert_eq!(
            v6,
            vec![
                "2001:db8::1".parse::<Ipv6Addr>().unwrap(),
                "fd00::1".parse::<Ipv6Addr>().unwrap()
            ]
        );
        assert!(split_by_family(&[]).0.is_empty());
    }

    #[test]
    fn test_scoped_v6_gateway() {
        let ll: Ipv6Addr = "fe80::1".parse().unwrap();
        assert_eq!(scoped_v6_gateway(&ll, "en0"), "fe80::1%en0");
        let global: Ipv6Addr = "2001:db8::1".parse().unwrap();
        assert_eq!(scoped_v6_gateway(&global, "en0"), "2001:db8::1");
    }

    #[test]
    fn test_parse_linux_v6_default_route() {
        let out = "default via fe80::1 dev eth0 proto ra metric 1024 pref medium\n";
        let (gw, iface) = parse_linux_v6_default_route(out).unwrap();
        assert_eq!(gw, "fe80::1".parse::<Ipv6Addr>().unwrap());
        assert_eq!(iface, "eth0");

        // Multiple defaults: first usable one wins.
        let out = "default dev tun0 metric 3\ndefault via 2001:db8::1 dev wlan0 proto static\n";
        let (gw, iface) = parse_linux_v6_default_route(out).unwrap();
        assert_eq!(gw, "2001:db8::1".parse::<Ipv6Addr>().unwrap());
        assert_eq!(iface, "wlan0");

        assert!(parse_linux_v6_default_route("").is_none());
        assert!(parse_linux_v6_default_route("default via :: dev eth0\n").is_none());
        assert!(parse_linux_v6_default_route("garbage\n").is_none());
    }

    #[test]
    fn test_parse_macos_v6_route_get() {
        let out = "   route to: ::\ndestination: ::\n       mask: ::\n\
                    gateway: fe80::1%en0\n  interface: en0\n";
        let (gw, iface) = parse_macos_v6_route_get(out).unwrap();
        assert_eq!(gw, "fe80::1".parse::<Ipv6Addr>().unwrap());
        assert_eq!(iface, "en0");

        assert!(parse_macos_v6_route_get("gateway: ::\ninterface: en0\n").is_none());
        assert!(parse_macos_v6_route_get("interface: en0\n").is_none());
        assert!(parse_macos_v6_route_get("").is_none());
    }

    #[test]
    fn test_parse_windows_ps_v6_gateway() {
        let (gw, iface) = parse_windows_ps_v6_gateway("fe80::1|Ethernet\r\n").unwrap();
        assert_eq!(gw, "fe80::1".parse::<Ipv6Addr>().unwrap());
        assert_eq!(iface, "Ethernet");

        let (gw, iface) = parse_windows_ps_v6_gateway("2001:db8::1|Wi-Fi\n").unwrap();
        assert_eq!(gw, "2001:db8::1".parse::<Ipv6Addr>().unwrap());
        assert_eq!(iface, "Wi-Fi");

        assert!(parse_windows_ps_v6_gateway("::|Ethernet").is_none());
        assert!(parse_windows_ps_v6_gateway("fe80::1|").is_none());
        assert!(parse_windows_ps_v6_gateway("").is_none());
        assert!(parse_windows_ps_v6_gateway("no-pipe-here").is_none());
    }

    #[test]
    fn test_linux_v6_command_builders() {
        let gw: Ipv6Addr = "fe80::1".parse().unwrap();
        let dns: Ipv6Addr = "2001:4860:4860::8888".parse().unwrap();

        assert_eq!(
            linux_v6_addr_args("tun0"),
            vec!["-6", "addr", "add", "fd9a:4c2e:17b0::2/64", "dev", "tun0"]
        );
        assert_eq!(
            linux_v6_default_add_args(&TUN_GATEWAY_V6, "tun0"),
            vec![
                "-6",
                "route",
                "add",
                "default",
                "via",
                "fd9a:4c2e:17b0::1",
                "dev",
                "tun0",
                "metric",
                "3"
            ]
        );
        assert_eq!(
            linux_v6_default_del_args(&TUN_GATEWAY_V6, "tun0"),
            vec![
                "-6",
                "route",
                "del",
                "default",
                "via",
                "fd9a:4c2e:17b0::1",
                "dev",
                "tun0"
            ]
        );
        assert_eq!(
            linux_v6_bypass_add_args(&dns, &gw, "eth0"),
            vec![
                "-6",
                "route",
                "add",
                "2001:4860:4860::8888/128",
                "via",
                "fe80::1",
                "dev",
                "eth0"
            ]
        );
        assert_eq!(
            linux_v6_bypass_del_args(&dns),
            vec!["-6", "route", "del", "2001:4860:4860::8888/128"]
        );
    }

    #[test]
    fn test_macos_v6_command_builders() {
        let dns: Ipv6Addr = "2001:4860:4860::8888".parse().unwrap();

        assert_eq!(
            macos_v6_addr_args("utun3"),
            vec!["utun3", "inet6", "fd9a:4c2e:17b0::2", "prefixlen", "64"]
        );
        assert_eq!(
            macos_v6_change_default_args("fd9a:4c2e:17b0::1"),
            vec!["-n", "change", "-inet6", "default", "fd9a:4c2e:17b0::1"]
        );
        assert_eq!(
            macos_v6_delete_default_args(),
            vec!["-n", "delete", "-inet6", "default"]
        );
        assert_eq!(
            macos_v6_add_default_args("fe80::1%en0"),
            vec!["-n", "add", "-inet6", "default", "fe80::1%en0"]
        );
        assert_eq!(
            macos_v6_bypass_add_args(&dns, "fe80::1%en0"),
            vec![
                "-n",
                "add",
                "-inet6",
                "-host",
                "2001:4860:4860::8888",
                "fe80::1%en0"
            ]
        );
        assert_eq!(
            macos_v6_bypass_del_args(&dns),
            vec!["-n", "delete", "-inet6", "-host", "2001:4860:4860::8888"]
        );
    }

    #[test]
    fn test_windows_v6_command_builders() {
        let gw: Ipv6Addr = "fe80::1".parse().unwrap();
        let dns: Ipv6Addr = "2001:4860:4860::8888".parse().unwrap();

        assert_eq!(
            windows_v6_addr_args("sockrocket-tun"),
            vec![
                "interface",
                "ipv6",
                "set",
                "address",
                "interface=sockrocket-tun",
                "address=fd9a:4c2e:17b0::2/64"
            ]
        );
        assert_eq!(
            windows_v6_set_iface_metric_args("sockrocket-tun"),
            vec![
                "interface",
                "ipv6",
                "set",
                "interface",
                "interface=sockrocket-tun",
                "metric=5"
            ]
        );
        assert_eq!(
            windows_v6_add_default_args("sockrocket-tun", &TUN_GATEWAY_V6, Some(3)),
            vec![
                "interface",
                "ipv6",
                "add",
                "route",
                "prefix=::/0",
                "interface=sockrocket-tun",
                "nexthop=fd9a:4c2e:17b0::1",
                "metric=3"
            ]
        );
        // Restore path re-adds the original route without forcing a metric.
        assert_eq!(
            windows_v6_add_default_args("Ethernet", &gw, None),
            vec![
                "interface",
                "ipv6",
                "add",
                "route",
                "prefix=::/0",
                "interface=Ethernet",
                "nexthop=fe80::1"
            ]
        );
        assert_eq!(
            windows_v6_del_default_args("sockrocket-tun"),
            vec![
                "interface",
                "ipv6",
                "delete",
                "route",
                "prefix=::/0",
                "interface=sockrocket-tun"
            ]
        );
        assert_eq!(
            windows_v6_bypass_add_args(&dns, "Ethernet", &gw),
            vec![
                "interface",
                "ipv6",
                "add",
                "route",
                "prefix=2001:4860:4860::8888/128",
                "interface=Ethernet",
                "nexthop=fe80::1",
                "metric=5"
            ]
        );
        assert_eq!(
            windows_v6_bypass_del_args(&dns, "Ethernet"),
            vec![
                "interface",
                "ipv6",
                "delete",
                "route",
                "prefix=2001:4860:4860::8888/128",
                "interface=Ethernet"
            ]
        );
    }

    #[test]
    fn test_icmpv6_echo_reply_roundtrip() {
        use etherparse::{IcmpEchoHeader, Icmpv6Type};

        let src: Ipv6Addr = "fd9a:4c2e:17b0::2".parse().unwrap();
        let dst: Ipv6Addr = "2001:4860:4860::8888".parse().unwrap();

        // Build a real echo request: header + payload, with a valid checksum.
        let echo = IcmpEchoHeader { id: 0x1234, seq: 7 };
        let mut req = Icmpv6Header::new(Icmpv6Type::EchoRequest(echo));
        let req_payload = b"sockrocket-ping";
        req.update_checksum(src.octets(), dst.octets(), req_payload)
            .unwrap();
        let mut req_bytes = req.to_bytes().to_vec();
        req_bytes.extend_from_slice(req_payload);

        // The reply is addressed dst -> src (the request's endpoints swapped).
        let reply = icmpv6_echo_reply(&req_bytes, dst, src).expect("echo reply");

        let (reply_header, reply_payload) = Icmpv6Header::from_slice(&reply).unwrap();
        match reply_header.icmp_type {
            Icmpv6Type::EchoReply(h) => {
                assert_eq!(h.id, 0x1234);
                assert_eq!(h.seq, 7);
            }
            other => panic!("expected EchoReply, got {:?}", other),
        }
        assert_eq!(reply_payload, req_payload);
        assert_ne!(reply_header.checksum, 0, "checksum must be filled in");
    }

    #[test]
    fn test_icmpv6_non_echo_dropped() {
        // NDP router solicitation (type 133) must not be answered.
        let rs = [133u8, 0, 0, 0, 0, 0, 0, 0];
        let a: Ipv6Addr = "fe80::1".parse().unwrap();
        let b: Ipv6Addr = "ff02::2".parse().unwrap();
        assert!(icmpv6_echo_reply(&rs, a, b).is_none());

        // Truncated garbage must not be answered either.
        assert!(icmpv6_echo_reply(&[128u8, 0], a, b).is_none());
        assert!(icmpv6_echo_reply(&[], a, b).is_none());
    }

    /// A UDP association must serve multiple DNS queries: the handler loops
    /// until the association closes instead of answering once and shutting
    /// down. Malformed (but ≥12-byte) packets get a FORMERR response without
    /// touching the network, which keeps this test offline.
    #[tokio::test]
    async fn test_tun_dns_multiple_queries_per_association() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let resolver = DnsResolver::new();
        let handle = tokio::spawn(async move { handle_tun_dns(&mut server, &resolver).await });

        // 13-byte packet: valid ID, then a compression pointer in the
        // question section — parse_dns_query rejects it.
        let mut bad = vec![0u8; 13];
        bad[0] = 0x12;
        bad[1] = 0x34;
        bad[12] = 0xFF;

        for _ in 0..2 {
            client.write_all(&bad).await.unwrap();
            let mut resp = [0u8; 64];
            let n = client.read(&mut resp).await.unwrap();
            assert!(n >= 12, "expected a DNS error response");
            assert_eq!(&resp[..2], &[0x12, 0x34], "response keeps the query ID");
            assert_eq!(resp[3] & 0x0F, 1, "rcode should be FORMERR");
        }

        // A short (<12-byte) packet is skipped, not answered, and the
        // association stays alive for the next query.
        client.write_all(&[0u8; 5]).await.unwrap();
        tokio::task::yield_now().await;
        client.write_all(&bad).await.unwrap();
        let mut resp = [0u8; 64];
        let n = client.read(&mut resp).await.unwrap();
        assert!(n >= 12, "association must survive a short packet");

        // Closing the association ends the handler cleanly.
        drop(client);
        handle.await.unwrap().unwrap();
    }

    #[test]
    fn test_find_cmd_known() {
        // `ls` should be findable on any Unix system
        #[cfg(not(target_os = "windows"))]
        {
            let result = find_cmd("ls");
            assert!(result.is_some(), "ls should be found on Unix");
        }
    }

    /// Smoke test: create a TUN device, verify name, then stop.
    /// Requires root and /dev/net/tun — skipped automatically if unavailable.
    #[tokio::test]
    async fn test_tun_create_and_stop() {
        if validate_tun_environment().is_err() {
            eprintln!("Skipping TUN smoke test (environment not suitable)");
            return;
        }

        let outbound = SharedOutbound::direct();
        let tun = match TunProxy::start(outbound, None, None) {
            Ok(t) => t,
            Err(e) => {
                // WinTun or similar driver may not be installed in CI — skip gracefully
                eprintln!("Skipping TUN smoke test (driver unavailable): {e}");
                return;
            }
        };
        let name = tun.tun_name().to_string();
        assert!(!name.is_empty(), "TUN device name should not be empty");

        let info = tun.route_info();
        assert_eq!(info.tun_gateway, TUN_GATEWAY);

        tun.stop().await;
    }
}
