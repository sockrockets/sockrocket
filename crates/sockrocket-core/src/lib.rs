pub mod config;
pub mod dns;
pub mod proxy;
pub mod qr;
pub mod router;
pub mod share_server;
pub mod system_proxy;

pub use config::clash::parse_clash_config;
pub use config::dedup::{dedup_nodes, node_fingerprint, uniquify_node_names};
pub use config::export::{SubscriptionFormat, export_clash_config, export_v2ray_base64};
pub use config::group::{ProbeResult, fallback_pick, resolve_members, url_test_pick};
pub use config::model::{
    AppConfig, GroupType, HealthCheckConfig, Node, ProxyGroupConfig, ProxyMode, ProxyProtocol,
    RoutingRule, ShadowTlsConfig, Subscription, TransportConfig, TransportType,
    decode_display_name, normalize_node_names,
};
pub use config::singbox::{parse_singbox_config, parse_singbox_outbound};
pub use config::subscription::{detect_format, fetch_subscription, parse_subscription_content};
pub use config::v2ray::{
    node_to_share_uri, parse_hysteria2_uri, parse_proxy_uri, parse_ss_uri, parse_trojan_uri,
    parse_tuic_uri, parse_v2ray_config, parse_vless_uri, parse_vmess_uri,
};
pub use config::watch::{
    ConfigWatchEvent, ConfigWatchHandle, ConfigWatcher, ReloadedConfig, config_content_hash,
};
pub use qr::qr_module_matrix;

pub use proxy::connector::{
    BoxProxyStream, DirectOutbound, Outbound, ProxyStream, RetryOutbound, SharedOutbound,
    SwappableOutbound,
};
pub use proxy::factory::{create_outbound, create_outbound_unwarmed};
pub use proxy::health::{HealthCheckSetup, HealthEvent, HealthMonitor, OutboundFactory};
pub use proxy::http::HttpProxyServer;
pub use proxy::hysteria2::Hysteria2Outbound;
pub use proxy::relay::relay_bidirectional;
pub use proxy::routing::RoutingOutbound;
pub use proxy::service::ProxyService;
pub use proxy::socks5::Socks5Server;
pub use proxy::speedtest::{
    ProbeFailureKind, SpeedTestResult, classify_probe_error, http_latency_test, latency_test_node,
    speed_test_node, tcp_latency_test,
};
pub use proxy::ss::SsOutbound;
pub use proxy::trojan::TrojanOutbound;
pub use proxy::tuic::TuicOutbound;
pub use proxy::tun::{
    TunProxy, TunRouteGuard, TunRouteInfo, emergency_restore_routes, ensure_wintun_dll,
    resolve_to_ips, resolve_to_ipv4, setup_tun_routes, tun_privileged, tun_requirements,
    tun_supported, update_tun_bypass_ips, wintun_dll_available,
};
#[cfg(target_os = "windows")]
pub use proxy::tun::{relaunch_elevated, relaunch_elevated_and_exit};
pub use proxy::vless::VlessOutbound;
pub use proxy::vmess::VMessOutbound;
pub use proxy::{ProxyState, ProxyStats};

pub use dns::fakeip::{FakeIpPool, is_fake_ip};
pub use dns::{
    DnsConfig, DnsGroup, DnsResolver, DnsServer, answer_raw_dns_query, build_dns_error_response,
    build_dns_response, china_domain_suffixes, china_split_dns_config,
    china_split_dns_config_proxy_tls, parse_dns_query, serve_dns_udp,
};

pub use router::{
    GeoIpDb, MatchRule, RouteAction, Router, RuleSet, china_direct_ruleset, china_geoip_db,
    china_ipv4_cidrs, rule_mode_ruleset,
};
pub use share_server::{
    DEFAULT_SHARE_PORT, ShareServer, ShareState, SharedShareState, generate_token, local_lan_ip,
    new_shared_state, start_share_server,
};
pub use system_proxy::{
    DEFAULT_BYPASS, SystemProxyConfig, clear_system_proxy, get_system_proxy, set_system_proxy,
    set_system_proxy_with_bypass, system_proxy_supported,
};
