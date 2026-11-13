use anyhow::{Context, Result};
use base64::Engine;
use base64::engine::general_purpose;
use serde::Deserialize;
use std::collections::HashMap;

use super::model::*;

/// V2Ray JSON outbound config (simplified)
#[derive(Debug, Deserialize)]
struct V2RayConfig {
    #[serde(default)]
    outbounds: Vec<V2RayOutbound>,
}

#[derive(Debug, Deserialize)]
struct V2RayOutbound {
    protocol: String,
    #[serde(default)]
    settings: serde_json::Value,
    #[serde(rename = "streamSettings")]
    stream_settings: Option<V2RayStreamSettings>,
    tag: Option<String>,
}

#[derive(Debug, Deserialize)]
struct V2RayStreamSettings {
    network: Option<String>,
    security: Option<String>,
    #[serde(rename = "tlsSettings")]
    tls_settings: Option<V2RayTlsSettings>,
    #[serde(rename = "httpupgradeSettings")]
    http_upgrade_settings: Option<V2RayWsSettings>,
    #[serde(rename = "grpcSettings")]
    grpc_settings: Option<GrpcConfig>,
    #[serde(rename = "wsSettings")]
    ws_settings: Option<V2RayWsSettings>,
    #[serde(rename = "xhttpSettings")]
    xhttp_settings: Option<XhttpConfig>,
    #[serde(rename = "realitySettings")]
    reality_settings: Option<V2RayRealitySettings>,
}

#[derive(Debug, Deserialize)]
struct V2RayTlsSettings {
    #[serde(rename = "serverName")]
    server_name: Option<String>,
    #[serde(default)]
    alpn: Vec<String>,
    fingerprint: Option<String>,
    #[serde(rename = "allowInsecure", default)]
    allow_insecure: bool,
}

#[derive(Debug, Deserialize)]
struct V2RayWsSettings {
    host: Option<String>,
    path: Option<String>,
    #[serde(default)]
    headers: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct V2RayRealitySettings {
    #[serde(rename = "publicKey", alias = "password")]
    public_key: Option<String>,
    #[serde(rename = "shortId")]
    short_id: Option<String>,
    #[serde(rename = "serverName")]
    server_name: Option<String>,
    fingerprint: Option<String>,
}

/// VMess share link JSON format
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct VMessShareLink {
    #[serde(default, alias = "global-padding")]
    global_padding: bool,
    #[serde(default, alias = "authenticated-length")]
    authenticated_length: bool,
    #[serde(default = "default_v")]
    v: String,
    ps: Option<String>,
    add: String,
    port: serde_json::Value,
    id: String,
    #[serde(default)]
    aid: serde_json::Value,
    #[serde(default = "default_cipher")]
    scy: String,
    net: Option<String>,
    #[serde(rename = "type")]
    header_type: Option<String>,
    host: Option<String>,
    path: Option<String>,
    tls: Option<String>,
    sni: Option<String>,
    alpn: Option<String>,
}

fn default_v() -> String {
    "2".to_string()
}

fn default_cipher() -> String {
    "auto".to_string()
}

/// Check if content looks like V2Ray JSON config
pub fn is_v2ray_config(content: &str) -> bool {
    content.contains("\"outbounds\"") || content.contains("\"inbounds\"")
}

/// Parse V2Ray JSON config into nodes
pub fn parse_v2ray_config(json_str: &str) -> Result<Vec<Node>> {
    let config: V2RayConfig =
        serde_json::from_slice(json_str.as_bytes()).context("Failed to parse V2Ray JSON")?;

    let mut nodes = Vec::new();
    for outbound in &config.outbounds {
        match convert_v2ray_outbound(outbound) {
            Ok(Some(node)) => nodes.push(node),
            Ok(None) => {} // skip non-proxy outbounds (direct, block, etc.)
            Err(_) => {
                tracing::warn!("Skipping unsupported or invalid V2Ray outbound");
            }
        }
    }

    Ok(nodes)
}

pub(crate) const UDP_ONLY: &str = "udp_only";
pub(crate) const VMESS_PADDING: &str = "vmess_global_padding";
pub(crate) const VMESS_AUTH_LEN: &str = "vmess_authenticated_length";
pub(crate) fn vmess_framing_options(
    extra: &mut HashMap<String, String>,
    padding: bool,
    auth_len: bool,
) {
    for (key, enabled) in [(VMESS_PADDING, padding), (VMESS_AUTH_LEN, auth_len)] {
        if enabled {
            extra.insert(key.into(), "true".into());
        }
    }
}

/// Parse a vmess:// share link
pub fn parse_vmess_uri(uri: &str) -> Result<Node> {
    let encoded = uri.strip_prefix("vmess://").context("Not a vmess:// URI")?;

    // Remove fragment (name after #)
    let encoded = encoded.split('#').next().unwrap_or(encoded);

    let decoded = general_purpose::STANDARD
        .decode(encoded.trim())
        .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(encoded.trim()))
        .context("Failed to Base64 decode vmess link")?;

    let json_str = String::from_utf8(decoded).context("Invalid UTF-8 in vmess link")?;
    let link: VMessShareLink =
        serde_json::from_slice(json_str.as_bytes()).context("Failed to parse vmess JSON")?;

    let port: u16 = match &link.port {
        serde_json::Value::Number(n) => n.as_u64().unwrap_or(443) as u16,
        serde_json::Value::String(s) => s.parse().unwrap_or(443),
        _ => 443,
    };

    let alter_id: u32 = match &link.aid {
        serde_json::Value::Number(n) => n.as_u64().unwrap_or(0) as u32,
        serde_json::Value::String(s) => s.parse().unwrap_or(0),
        _ => 0,
    };

    let name = link
        .ps
        .as_deref()
        .map(super::model::decode_display_name)
        .unwrap_or_else(|| format!("VMess-{}", link.add));

    let has_tls = link.tls.as_deref() == Some("tls");
    let network = link.net.as_deref().unwrap_or("tcp");

    anyhow::ensure!(
        matches!(network, "tcp" | "ws" | "grpc" | "httpupgrade"),
        "Unsupported VMess URI transport; configuration was not downgraded"
    );
    let transport = build_vmess_transport(network, has_tls, &link);

    let mut extra = HashMap::new();
    vmess_framing_options(&mut extra, link.global_padding, link.authenticated_length);
    Ok(Node {
        name,
        server: link.add,
        port,
        protocol: ProxyProtocol::VMess {
            uuid: link.id,
            alter_id,
            cipher: link.scy,
            udp: false,
        },
        transport,
        latency_ms: None,
        tags: vec![],
        extra,
    })
}

/// Parse a vless:// share link
pub fn parse_vless_uri(uri: &str) -> Result<Node> {
    let without_scheme = uri.strip_prefix("vless://").context("Not a vless:// URI")?;

    let parsed = url::Url::parse(&format!("scheme://{}", without_scheme))
        .context("Failed to parse VLESS URI")?;

    let uuid = parsed.username().to_string();
    let server = parsed
        .host_str()
        .context("Missing server host")?
        .to_string();
    let port = parsed.port().unwrap_or(443);
    let name =
        decode_fragment_name(parsed.fragment()).unwrap_or_else(|| format!("VLESS-{}", server));

    let params: HashMap<String, String> = parsed.query_pairs().into_owned().collect();

    let mut extra = HashMap::new();
    set_vless_encryption(&mut extra, params.get("encryption").map(String::as_str))?;
    let flow = params.get("flow").cloned();
    let security = params.get("security").map(|s| s.as_str()).unwrap_or("none");
    let transport_type_str = params.get("type").map(|s| s.as_str()).unwrap_or("tcp");

    let transport = build_uri_transport(security, transport_type_str, &params)?;

    Ok(Node {
        name,
        server,
        port,
        protocol: ProxyProtocol::VLess {
            uuid,
            flow,
            udp: true,
        },
        transport,
        latency_ms: None,
        tags: vec![],
        extra,
    })
}

pub use super::ss_uri::parse_ss_uri;

pub(crate) const VLESS_ENCRYPTION: &str = "vless_encryption";

pub(crate) fn set_vless_encryption(
    extra: &mut HashMap<String, String>,
    value: Option<&str>,
) -> Result<()> {
    if let Some(value) = value.filter(|v| !v.is_empty() && *v != "none") {
        crate::proxy::vless::encryption::parse_client_encryption(value)
            .map_err(anyhow::Error::msg)?;
        extra.insert(VLESS_ENCRYPTION.into(), value.into());
    }
    Ok(())
}
pub(crate) const TLS_CERT_PIN: &str = "tls_certificate_sha256";
pub(crate) const HY2_UP: &str = "hysteria2_up_mbps";
pub(crate) const HY2_DOWN: &str = "hysteria2_down_mbps";
pub(crate) const HY2_FAST_OPEN: &str = "hysteria2_fast_open";
pub(crate) const HY2_HOP_INTERVAL: &str = "hysteria2_hop_interval_ms";
pub(crate) const HY2_HOP_MAX: &str = "hysteria2_hop_interval_max_ms";
pub(crate) const HY2_MPORT: &str = "hysteria2_mport";
// Legacy storage key: keep reading it so saved imports gain the implementation.
pub(crate) const HY2_CLIENT_FP_HINT: &str = "hysteria2_client_fingerprint_hint";

pub(crate) fn hysteria2_fingerprint(node: &Node) -> Option<&str> {
    node.transport
        .as_ref()
        .and_then(|t| t.tls.as_ref())
        .and_then(|t| t.fingerprint.as_deref())
        .or_else(|| node.extra.get(HY2_CLIENT_FP_HINT).map(String::as_str))
}

pub(crate) fn validate_hysteria2_profile(tls: Option<&TlsConfig>) -> Result<()> {
    if let Some(tls) = tls
        && matches!(tls.fingerprint.as_deref(), Some("chrome" | "chrome115"))
    {
        anyhow::ensure!(
            tls.alpn.as_ref().is_none_or(|values| !values.is_empty()
                && values.iter().all(|value| value == "h3")),
            "Chrome QUIC fingerprint requires ALPN h3"
        );
    }
    Ok(())
}

fn import_tls_pin(
    params: &HashMap<String, String>,
    extra: &mut HashMap<String, String>,
) -> Result<()> {
    let mut selected = None;
    for key in ["pcs", "pinSHA256"] {
        if let Some(value) = params.get(key) {
            let pin = parse_tuic_certificate_pin(value)?;
            anyhow::ensure!(
                selected.is_none_or(|old| old == pin),
                "Conflicting certificate pins"
            );
            selected = Some(pin);
        }
    }
    if let Some(pin) = selected {
        extra.insert(
            TLS_CERT_PIN.into(),
            pin.iter().map(|b| format!("{b:02x}")).collect(),
        );
    }
    if let Some(security) = params.get("security") {
        anyhow::ensure!(security == "tls", "This protocol requires TLS security");
    }
    Ok(())
}

pub(crate) const TUIC_ZERO_RTT: &str = "tuic_zero_rtt";
pub(crate) const TUIC_CERT_PIN: &str = "tuic_certificate_sha256";

pub(crate) fn parse_tuic_certificate_pin(value: &str) -> Result<[u8; 32]> {
    let hex: String = value.chars().filter(|c| *c != ':' && *c != '-').collect();
    anyhow::ensure!(
        hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid certificate SHA-256 pin"
    );
    let mut pin = [0u8; 32];
    for (i, byte) in pin.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("validated hex");
    }
    Ok(pin)
}

/// Parse a tuic:// share link
pub fn parse_tuic_uri(uri: &str) -> Result<Node> {
    let without_scheme = uri.strip_prefix("tuic://").context("Not a tuic:// URI")?;

    let parsed = url::Url::parse(&format!("scheme://{}", without_scheme))
        .context("Failed to parse TUIC URI")?;

    let uuid = urlencoding::decode(parsed.username())
        .context("Invalid TUIC UUID encoding")?
        .into_owned();
    let password = urlencoding::decode(parsed.password().unwrap_or(""))
        .context("Invalid TUIC password encoding")?
        .into_owned();
    let server = parsed
        .host_str()
        .context("Missing server host")?
        .to_string();
    let port = parsed.port().unwrap_or(443);
    let name =
        decode_fragment_name(parsed.fragment()).unwrap_or_else(|| format!("TUIC-{}", server));

    let query_pairs: Vec<(String, String)> = parsed.query_pairs().into_owned().collect();
    let params: HashMap<String, String> = query_pairs.iter().cloned().collect();
    let congestion_control = params
        .get("congestion_control")
        .cloned()
        .unwrap_or_else(|| "bbr".to_string());
    let skip_cert_verify = query_truthy(&params, "insecure")
        || query_truthy(&params, "allow_insecure")
        || query_truthy(&params, "allowInsecure");
    let sni = params.get("sni").cloned();
    let alpn: Vec<String> = collect_alpn_values(&query_pairs)
        .iter()
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect();
    let mut extra = HashMap::new();
    if query_truthy(&params, "zero_rtt_handshake") {
        extra.insert(TUIC_ZERO_RTT.into(), "true".into());
    }
    if let Some(pin) = params.get("pcs") {
        let bytes = parse_tuic_certificate_pin(pin)?;
        extra.insert(
            TUIC_CERT_PIN.to_string(),
            bytes.iter().map(|b| format!("{b:02x}")).collect(),
        );
    }

    Ok(Node {
        name,
        server,
        port,
        protocol: ProxyProtocol::Tuic {
            uuid,
            password,
            congestion_control,
            heartbeat_ms: params
                .get("heartbeat_interval")
                .map(|v| v.parse::<u64>())
                .transpose()?,
            relay: TuicRelay::parse(
                params.get("udp_relay_mode").map(String::as_str),
                if query_truthy(&params, "udp_over_stream") {
                    Some(
                        params
                            .get("udp_over_stream_version")
                            .map(|s| s.parse::<u8>())
                            .transpose()?
                            .unwrap_or(2),
                    )
                } else {
                    None
                },
            )?,
            udp: true,
        },
        transport: Some(TransportConfig {
            grpc: None,
            xhttp: None,
            transport_type: TransportType::Quic,
            tls: Some(TlsConfig {
                sni,
                skip_cert_verify,
                alpn: if alpn.is_empty() { None } else { Some(alpn) },
                fingerprint: None,
            }),
            ws: None,
            reality: None,
        }),
        latency_ms: None,
        tags: vec![],
        extra,
    })
}

/// Parse a trojan:// share link
pub fn parse_trojan_uri(uri: &str) -> Result<Node> {
    let without_scheme = uri
        .strip_prefix("trojan://")
        .context("Not a trojan:// URI")?;

    let parsed = url::Url::parse(&format!("scheme://{}", without_scheme))
        .context("Failed to parse Trojan URI")?;

    let password = urlencoding::decode(parsed.username())
        .context("Invalid Trojan password encoding")?
        .into_owned();
    let server = parsed
        .host_str()
        .context("Missing server host")?
        .to_string();
    let port = parsed.port().unwrap_or(443);
    let name =
        decode_fragment_name(parsed.fragment()).unwrap_or_else(|| format!("Trojan-{}", server));

    let params: HashMap<String, String> = parsed.query_pairs().into_owned().collect();
    let security = params.get("security").map(|s| s.as_str()).unwrap_or("tls");
    let transport_type_str = params.get("type").map(|s| s.as_str()).unwrap_or("tcp");
    anyhow::ensure!(
        transport_type_str != "xhttp",
        "XHTTP is currently supported only for VLESS"
    );
    let transport = build_uri_transport(security, transport_type_str, &params)?;

    let mut extra = HashMap::new();
    let udp = match params.get("network").map(String::as_str) {
        None => true,
        Some("tcp") => false,
        Some("udp") => {
            extra.insert(UDP_ONLY.into(), "true".into());
            true
        }
        _ => anyhow::bail!("Invalid Trojan application network"),
    };
    Ok(Node {
        name,
        server,
        port,
        protocol: ProxyProtocol::Trojan { password, udp },
        transport,
        latency_ms: None,
        tags: vec![],
        extra,
    })
}

/// Parse any supported proxy URI
pub fn parse_proxy_uri(uri: &str) -> Result<Node> {
    let uri = uri.trim();
    if uri.starts_with("wireguard://") {
        super::wireguard::parse_uri(uri)
    } else if uri.starts_with("anytls://") {
        parse_anytls_uri(uri)
    } else if uri.starts_with("vmess://") {
        parse_vmess_uri(uri)
    } else if uri.starts_with("vless://") {
        parse_vless_uri(uri)
    } else if uri.starts_with("ss://") {
        parse_ss_uri(uri)
    } else if uri.starts_with("tuic://") {
        parse_tuic_uri(uri)
    } else if uri.starts_with("trojan://") {
        parse_trojan_uri(uri)
    } else if uri.starts_with("hysteria2://") || uri.starts_with("hy2://") {
        parse_hysteria2_uri(uri)
    } else {
        anyhow::bail!(
            "Unsupported URI scheme: {}",
            uri.split("://").next().unwrap_or("unknown")
        )
    }
}

/// Validate the advertised hopping range before using the URI's fixed port.
pub(crate) fn validate_hysteria2_mport(value: &str, fixed_port: u16) -> Result<()> {
    let mut includes_fixed_port = false;
    anyhow::ensure!(!value.is_empty(), "Invalid Hysteria2 mport range");
    for part in value.split(',') {
        let (start, end) = if let Some((start, end)) = part.split_once('-') {
            (start, end)
        } else {
            (part, part)
        };
        let start: u16 = start
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid Hysteria2 mport range"))?;
        let end: u16 = end
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid Hysteria2 mport range"))?;
        anyhow::ensure!(start > 0 && start <= end, "Invalid Hysteria2 mport range");
        includes_fixed_port |= (start..=end).contains(&fixed_port);
    }
    anyhow::ensure!(
        includes_fixed_port,
        "Hysteria2 fixed port is outside mport range"
    );
    Ok(())
}

/// Parse a hysteria2:// or hy2:// share link.
pub fn parse_hysteria2_uri(uri: &str) -> Result<Node> {
    anyhow::ensure!(
        uri.starts_with("hysteria2://") || uri.starts_with("hy2://"),
        "Not a Hysteria2 URI"
    );
    // The Hysteria share format permits a list in the authority's port slot.
    // Parse the remainder with Url, which accepts only a single numeric port.
    let (_, rest) = uri.split_once("://").unwrap();
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    let port_list = host_port
        .rsplit_once(':')
        .map(|(_, p)| p)
        .filter(|p| p.contains([',', '-']) && !p.contains(']'));
    let normalized;
    let input = if let Some(list) = port_list {
        let first = list.split([',', '-']).next().unwrap_or("");
        let port = first
            .parse::<u16>()
            .map_err(|_| anyhow::anyhow!("Invalid Hysteria2 port range"))?;
        validate_hysteria2_mport(list, port)?;
        let end = uri.len() - rest.len() + authority.len();
        normalized = format!("{}{port}{}", &uri[..end - list.len()], &uri[end..]);
        &normalized
    } else {
        uri
    };
    let parsed = url::Url::parse(input).map_err(|_| anyhow::anyhow!("Invalid Hysteria2 URI"))?;
    anyhow::ensure!(
        matches!(parsed.path(), "" | "/"),
        "Unsupported Hysteria2 path"
    );
    let mut auth = parsed.username().to_owned();
    if let Some(password) = parsed.password() {
        auth.push(':');
        auth.push_str(password);
    }
    let password = urlencoding::decode(&auth)
        .map_err(|_| anyhow::anyhow!("Invalid Hysteria2 authentication encoding"))?
        .into_owned();
    anyhow::ensure!(!password.is_empty(), "Hysteria2 authentication is required");
    let mut params = HashMap::new();
    for (key, value) in parsed.query_pairs() {
        anyhow::ensure!(
            matches!(
                key.as_ref(),
                "sni"
                    | "insecure"
                    | "allowInsecure"
                    | "pinSHA256"
                    | "pcs"
                    | "security"
                    | "alpn"
                    | "upmbps"
                    | "downmbps"
                    | "fastopen"
                    | "fp"
                    | "obfs"
                    | "obfs-password"
                    | "min-packet-size"
                    | "max-packet-size"
                    | "hop_interval_ms"
                    | "hop_interval_max_ms"
                    | "mport"
            ),
            "Unsupported Hysteria2 option; obfuscation and unknown extensions are not ignored"
        );
        anyhow::ensure!(
            params
                .insert(key.into_owned(), value.into_owned())
                .is_none(),
            "Duplicate Hysteria2 option"
        );
    }
    let mut extra = HashMap::new();
    let port = parsed.port().unwrap_or(443);
    for (k, field) in [
        ("hop_interval_ms", HY2_HOP_INTERVAL),
        ("hop_interval_max_ms", HY2_HOP_MAX),
    ] {
        if let Some(v) = params.get(k) {
            extra.insert(field.into(), v.parse::<u64>()?.to_string());
        }
    }
    if let Some(list) = port_list {
        anyhow::ensure!(
            !params.contains_key("mport"),
            "Duplicate Hysteria2 port range"
        );
        params.insert("mport".into(), list.into());
    }
    if let Some(mport) = params.get("mport") {
        validate_hysteria2_mport(mport, port)?;
        extra.insert(HY2_MPORT.into(), mport.clone());
    }
    import_tls_pin(&params, &mut extra)?;
    // Browser profile and certificate pin are separate settings. Chrome uses
    // the versioned QUIC profile; other recognized hints retain an explicit
    // compatibility note until their own QUIC profiles are implemented.
    if let Some(fp) = params.get("fp").filter(|fp| !fp.is_empty()) {
        anyhow::ensure!(
            matches!(
                fp.as_str(),
                "chrome"
                    | "chrome115"
                    | "firefox"
                    | "safari"
                    | "ios"
                    | "edge"
                    | "random"
                    | "randomized"
                    | "none"
            ),
            "Unsupported Hysteria2 browser fingerprint hint; certificate pins require pinSHA256"
        );
        extra.insert(HY2_CLIENT_FP_HINT.into(), fp.clone());
    }
    for (query, field) in [("upmbps", HY2_UP), ("downmbps", HY2_DOWN)] {
        if let Some(value) = params.get(query) {
            extra.insert(field.into(), super::bandwidth::canonical_mbps(value)?);
        }
    }
    if let Some(value) = params.get("fastopen") {
        anyhow::ensure!(
            matches!(value.as_str(), "0" | "1" | "false" | "true"),
            "Invalid Hysteria2 fastopen"
        );
        extra.insert(
            HY2_FAST_OPEN.into(),
            query_truthy(&params, "fastopen").to_string(),
        );
    }
    if params
        .get("fp")
        .is_some_and(|fp| matches!(fp.as_str(), "chrome" | "chrome115"))
    {
        anyhow::ensure!(
            params
                .get("alpn")
                .is_none_or(|alpn| alpn.split(',').all(|p| p.trim() == "h3")),
            "Chrome QUIC fingerprint requires ALPN h3"
        );
    }
    let server = parsed
        .host_str()
        .context("Missing Hysteria2 host")?
        .trim_matches(['[', ']'])
        .to_owned();
    anyhow::ensure!(port != 0, "Invalid Hysteria2 port");
    Ok(Node {
        name: decode_fragment_name(parsed.fragment()).unwrap_or_else(|| "Hysteria2".into()),
        server,
        port,
        protocol: ProxyProtocol::Hysteria2 {
            obfs: Hy2Obfs::from_parts(
                params.get("obfs").map(String::as_str),
                params.get("obfs-password").map(String::as_str),
                params
                    .get("min-packet-size")
                    .map(|s| s.parse())
                    .transpose()?,
                params
                    .get("max-packet-size")
                    .map(|s| s.parse())
                    .transpose()?,
            )?,
            password,
            udp: true,
        },
        transport: Some(TransportConfig {
            grpc: None,
            xhttp: None,
            transport_type: TransportType::Tls,
            tls: Some(TlsConfig {
                sni: params.get("sni").cloned(),
                skip_cert_verify: query_truthy(&params, "insecure")
                    || query_truthy(&params, "allowInsecure"),
                alpn: params.get("alpn").map(|v| {
                    v.split(',')
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                        .map(str::to_owned)
                        .collect()
                }),
                fingerprint: params.get("fp").filter(|v| !v.is_empty()).cloned(),
            }),
            ws: None,
            reality: None,
        }),
        latency_ms: None,
        tags: vec![],
        extra,
    })
}

/// AnyTLS share URI. Password and TLS parameters are decoded once; errors omit input.
pub fn parse_anytls_uri(uri: &str) -> Result<Node> {
    let parsed = url::Url::parse(uri).map_err(|_| anyhow::anyhow!("Invalid AnyTLS URI"))?;
    anyhow::ensure!(
        parsed.scheme() == "anytls"
            && parsed.password().is_none()
            && matches!(parsed.path(), "" | "/"),
        "Invalid AnyTLS URI fields"
    );
    let password = urlencoding::decode(parsed.username())
        .map_err(|_| anyhow::anyhow!("Invalid password encoding"))?
        .into_owned();
    anyhow::ensure!(!password.is_empty(), "AnyTLS password is required");
    let server = parsed
        .host_str()
        .context("Missing AnyTLS host")?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let port = parsed.port().unwrap_or(443);
    anyhow::ensure!(port != 0, "Invalid AnyTLS port");
    let mut params = HashMap::new();
    for (key, value) in parsed.query_pairs() {
        anyhow::ensure!(
            params
                .insert(key.into_owned(), value.into_owned())
                .is_none(),
            "Duplicate AnyTLS URI option"
        );
    }
    for key in params.keys() {
        anyhow::ensure!(
            matches!(
                key.as_str(),
                "peer"
                    | "sni"
                    | "insecure"
                    | "allowInsecure"
                    | "alpn"
                    | "fp"
                    | "security"
                    | "pcs"
                    | "pinSHA256"
                    | "idle_session_check_interval_ms"
                    | "idle_session_timeout_ms"
                    | "min_idle_session"
                    | "disable_reuse"
            ),
            "Unsupported AnyTLS URI option"
        );
    }
    let mut extra = HashMap::new();
    import_tls_pin(&params, &mut extra)?;
    Ok(Node {
        name: decode_fragment_name(parsed.fragment()).unwrap_or_else(|| "AnyTLS".into()),
        server,
        port,
        protocol: ProxyProtocol::AnyTls {
            password,
            pool: AnyTlsPool {
                check_interval_ms: params
                    .get("idle_session_check_interval_ms")
                    .map(|s| s.parse())
                    .transpose()?,
                idle_timeout_ms: params
                    .get("idle_session_timeout_ms")
                    .map(|s| s.parse())
                    .transpose()?,
                min_idle: params
                    .get("min_idle_session")
                    .map(|s| s.parse())
                    .transpose()?
                    .unwrap_or(0),
                disable_reuse: params
                    .get("disable_reuse")
                    .map(|s| s.parse::<bool>())
                    .transpose()?
                    .unwrap_or(false),
            }
            .checked()?,
            udp: true,
        },
        transport: Some(TransportConfig {
            grpc: None,
            xhttp: None,
            transport_type: TransportType::Tls,
            tls: Some(TlsConfig {
                sni: params.get("peer").or_else(|| params.get("sni")).cloned(),
                skip_cert_verify: query_truthy(&params, "insecure")
                    || query_truthy(&params, "allowInsecure"),
                alpn: params
                    .get("alpn")
                    .map(|v| v.split(',').map(str::to_owned).collect()),
                fingerprint: params.get("fp").cloned(),
            }),
            ws: None,
            reality: None,
        }),
        latency_ms: None,
        tags: vec![],
        extra,
    })
}

fn decode_fragment_name(fragment: Option<&str>) -> Option<String> {
    fragment
        .map(super::model::decode_display_name)
        .filter(|value| !value.is_empty())
}

fn query_truthy(params: &HashMap<String, String>, key: &str) -> bool {
    params
        .get(key)
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn collect_alpn_values(query_pairs: &[(String, String)]) -> Vec<String> {
    let mut indexed = Vec::new();
    let mut plain = Vec::new();

    for (key, value) in query_pairs {
        if key == "alpn" {
            plain.push(value.clone());
            continue;
        }

        if let Some(index) = key
            .strip_prefix("alpn[")
            .and_then(|rest| rest.strip_suffix(']'))
            .and_then(|index| index.parse::<usize>().ok())
        {
            indexed.push((index, value.clone()));
        }
    }

    indexed.sort_by_key(|(index, _)| *index);
    let mut values: Vec<String> = indexed.into_iter().map(|(_, value)| value).collect();
    values.extend(plain);
    values
}

fn xray_endpoint<'a>(settings: &'a serde_json::Value, list: &str) -> Result<&'a serde_json::Value> {
    match settings.get(list) {
        Some(value) => {
            let entries = value.as_array().context("Invalid Xray endpoint list")?;
            anyhow::ensure!(entries.len() == 1, "Expected one Xray endpoint per node");
            Ok(&entries[0])
        }
        None => Ok(settings),
    }
}

fn xray_port(endpoint: &serde_json::Value, default: u16) -> Result<u16> {
    let Some(value) = endpoint.get("port") else {
        return Ok(default);
    };
    value
        .as_u64()
        .and_then(|n| u16::try_from(n).ok())
        .filter(|n| *n != 0)
        .context("Invalid Xray port")
}

fn xray_user_id(user: &serde_json::Value) -> Result<String> {
    use sha1::{Digest, Sha1};

    let id = user
        .get("id")
        .and_then(|v| v.as_str())
        .context("Missing user id")?;
    if (1..=30).contains(&id.len()) {
        // Xray maps short UTF-8 identifiers to UUIDv5 in the nil namespace.
        let digest = Sha1::new()
            .chain_update([0; 16])
            .chain_update(id.as_bytes())
            .finalize();
        let mut bytes = [0; 16];
        bytes.copy_from_slice(&digest[..16]);
        return Ok(uuid::Builder::from_sha1_bytes(bytes)
            .into_uuid()
            .to_string());
    }
    Ok(uuid::Uuid::parse_str(id)
        .context("Invalid Xray user id")?
        .to_string())
}

fn convert_v2ray_outbound(outbound: &V2RayOutbound) -> Result<Option<Node>> {
    anyhow::ensure!(
        !outbound
            .stream_settings
            .as_ref()
            .is_some_and(|s| s.network.as_deref() == Some("xhttp"))
            || outbound.protocol == "vless",
        "XHTTP is currently supported only for VLESS"
    );
    match outbound.protocol.as_str() {
        "wireguard" => {
            super::wireguard::from_xray(&outbound.settings, outbound.tag.as_deref()).map(Some)
        }
        "vmess" => {
            let vnext = xray_endpoint(&outbound.settings, "vnext")?;
            let server = vnext
                .get("address")
                .and_then(|v| v.as_str())
                .context("Missing address")?
                .to_string();
            let port = xray_port(vnext, 443)?;

            let user = xray_endpoint(vnext, "users")?;
            let uuid = xray_user_id(user)?;
            let alter_id = match user.get("alterId") {
                Some(v) => v
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .context("Invalid VMess alterId")?,
                None => 0,
            };
            let cipher = user
                .get("security")
                .and_then(|v| v.as_str())
                .unwrap_or("auto")
                .to_string();

            let name = outbound
                .tag
                .clone()
                .unwrap_or_else(|| format!("VMess-{}", server));

            let transport = outbound
                .stream_settings
                .as_ref()
                .map(build_v2ray_transport)
                .transpose()?
                .flatten();

            Ok(Some(Node {
                name,
                server,
                port,
                protocol: ProxyProtocol::VMess {
                    uuid,
                    alter_id,
                    cipher,
                    udp: true,
                },
                transport,
                latency_ms: None,
                tags: vec![],
                extra: HashMap::new(),
            }))
        }
        "vless" => {
            let vnext = xray_endpoint(&outbound.settings, "vnext")?;
            let server = vnext
                .get("address")
                .and_then(|v| v.as_str())
                .context("Missing address")?
                .to_string();
            let port = xray_port(vnext, 443)?;

            let user = xray_endpoint(vnext, "users")?;
            let uuid = xray_user_id(user)?;
            let mut extra = HashMap::new();
            let encryption = user
                .get("encryption")
                .map(|v| v.as_str().context("Invalid VLESS encryption field"))
                .transpose()?;
            set_vless_encryption(&mut extra, encryption)?;
            let flow = user
                .get("flow")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let name = outbound
                .tag
                .clone()
                .unwrap_or_else(|| format!("VLESS-{}", server));

            let transport = outbound
                .stream_settings
                .as_ref()
                .map(build_v2ray_transport)
                .transpose()?
                .flatten();

            anyhow::ensure!(
                !transport
                    .as_ref()
                    .is_some_and(|t| t.transport_type == TransportType::Xhttp)
                    || flow.as_deref().unwrap_or("").is_empty(),
                "XHTTP does not support Vision flow"
            );
            Ok(Some(Node {
                name,
                server,
                port,
                protocol: ProxyProtocol::VLess {
                    uuid,
                    flow,
                    udp: true,
                },
                transport,
                latency_ms: None,
                tags: vec![],
                extra,
            }))
        }
        "trojan" => {
            let endpoint = match outbound.settings.get("servers") {
                Some(list) => {
                    let servers = list.as_array().context("Invalid Trojan servers")?;
                    anyhow::ensure!(
                        servers.len() == 1,
                        "Expected one Trojan server per outbound"
                    );
                    &servers[0]
                }
                None => &outbound.settings,
            };
            let server = endpoint
                .get("address")
                .and_then(|v| v.as_str())
                .context("Missing Trojan address")?;
            let port = endpoint
                .get("port")
                .and_then(|v| v.as_u64())
                .and_then(|v| u16::try_from(v).ok())
                .filter(|v| *v != 0)
                .context("Invalid Trojan port")?;
            let password = endpoint
                .get("password")
                .and_then(|v| v.as_str())
                .context("Missing Trojan password")?;
            let stream = outbound
                .stream_settings
                .as_ref()
                .context("Trojan requires TLS or REALITY")?;
            anyhow::ensure!(
                matches!(stream.security.as_deref(), Some("tls" | "reality")),
                "Trojan requires TLS or REALITY"
            );
            Ok(Some(Node {
                name: outbound
                    .tag
                    .clone()
                    .unwrap_or_else(|| format!("Trojan-{server}")),
                server: server.into(),
                port,
                protocol: ProxyProtocol::Trojan {
                    password: password.into(),
                    udp: true,
                },
                transport: build_v2ray_transport(stream)?,
                latency_ms: None,
                tags: vec![],
                extra: HashMap::new(),
            }))
        }
        "shadowsocks" => {
            let servers = xray_endpoint(&outbound.settings, "servers")?;
            let server = servers
                .get("address")
                .and_then(|v| v.as_str())
                .context("Missing address")?
                .to_string();
            let port = xray_port(servers, 8388)?;
            let cipher = servers
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("aes-256-gcm")
                .to_string();
            let password = servers
                .get("password")
                .and_then(|v| v.as_str())
                .context("Missing password")?
                .to_string();

            let name = outbound
                .tag
                .clone()
                .unwrap_or_else(|| format!("SS-{}", server));

            Ok(Some(Node {
                name,
                server,
                port,
                protocol: ProxyProtocol::Shadowsocks {
                    cipher,
                    password,
                    udp: true,
                    shadow_tls: None,
                },
                transport: None,
                latency_ms: None,
                tags: vec![],
                extra: HashMap::new(),
            }))
        }
        // Skip non-proxy protocols
        "freedom" | "blackhole" | "dns" | "loopback" => Ok(None),
        other => {
            tracing::debug!("Unsupported V2Ray protocol: {}", other);
            Ok(None)
        }
    }
}

fn build_v2ray_transport(ss: &V2RayStreamSettings) -> Result<Option<TransportConfig>> {
    let security = ss.security.as_deref().unwrap_or("none");
    let network = ss.network.as_deref().unwrap_or("tcp");

    anyhow::ensure!(
        matches!(
            network,
            "tcp" | "raw" | "ws" | "grpc" | "httpupgrade" | "xhttp"
        ),
        "Unsupported Xray transport; configuration was not downgraded"
    );
    anyhow::ensure!(
        matches!(security, "none" | "tls" | "reality"),
        "Unsupported Xray transport security"
    );
    anyhow::ensure!(
        security != "reality"
            || (matches!(network, "tcp" | "raw" | "xhttp")
                && ss
                    .reality_settings
                    .as_ref()
                    .and_then(|r| r.public_key.as_ref())
                    .is_some_and(|k| !k.is_empty())),
        "REALITY requires TCP and a public key"
    );
    if security == "none" && matches!(network, "tcp" | "raw") {
        return Ok(None);
    }

    let xhttp = if network == "xhttp" {
        let config = ss.xhttp_settings.clone().unwrap_or_default();
        config.validate()?;
        Some(config)
    } else {
        None
    };
    if security == "reality"
        && let Some(ref rs) = ss.reality_settings
    {
        return Ok(Some(TransportConfig {
            grpc: None,
            xhttp,
            transport_type: if network == "xhttp" {
                TransportType::Xhttp
            } else {
                TransportType::Reality
            },
            tls: None,
            ws: None,
            reality: Some(RealityConfig {
                public_key: rs.public_key.clone().unwrap_or_default(),
                short_id: rs.short_id.clone().unwrap_or_default(),
                sni: rs.server_name.clone(),
                client_fingerprint: rs.fingerprint.clone(),
                skip_cert_verify: false,
            }),
        }));
    }

    let tls = if security == "tls" {
        Some(TlsConfig {
            sni: ss.tls_settings.as_ref().and_then(|t| t.server_name.clone()),
            skip_cert_verify: ss
                .tls_settings
                .as_ref()
                .map(|t| t.allow_insecure)
                .unwrap_or(false),
            alpn: ss.tls_settings.as_ref().map(|t| t.alpn.clone()),
            fingerprint: ss.tls_settings.as_ref().and_then(|t| t.fingerprint.clone()),
        })
    } else {
        None
    };

    if xhttp.is_some() {
        anyhow::ensure!(
            tls.as_ref()
                .and_then(|t| t.alpn.as_ref())
                .is_none_or(|a| a.is_empty()
                    || a.iter()
                        .all(|s| matches!(s.as_str(), "h2" | "http/1.1" | "h3"))),
            "Unsupported XHTTP ALPN"
        );
    }
    let ws = if matches!(network, "ws" | "httpupgrade") {
        let settings = if network == "httpupgrade" {
            &ss.http_upgrade_settings
        } else {
            &ss.ws_settings
        };
        settings.as_ref().map(|w| {
            let host = w.host.clone().or_else(|| w.headers.get("Host").cloned());
            WsConfig {
                path: w.path.clone(),
                host,
                headers: Some(w.headers.clone()),
            }
        })
    } else {
        None
    };

    let transport_type = match network {
        "ws" => TransportType::WebSocket,
        "httpupgrade" => TransportType::HttpUpgrade,
        "grpc" => TransportType::Grpc,
        "xhttp" => TransportType::Xhttp,
        _ if security == "tls" => TransportType::Tls,
        _ => TransportType::Tcp,
    };

    Ok(Some(TransportConfig {
        grpc: (network == "grpc").then(|| ss.grpc_settings.clone().unwrap_or_default()),
        xhttp,
        transport_type,
        tls,
        ws,
        reality: None,
    }))
}

fn build_vmess_transport(
    network: &str,
    has_tls: bool,
    link: &VMessShareLink,
) -> Option<TransportConfig> {
    if !has_tls && network == "tcp" {
        return None;
    }

    let tls = if has_tls {
        Some(TlsConfig {
            sni: link.sni.clone().or_else(|| link.host.clone()),
            skip_cert_verify: false,
            alpn: link
                .alpn
                .as_ref()
                .map(|a| a.split(',').map(|s| s.trim().to_string()).collect()),
            fingerprint: None,
        })
    } else {
        None
    };

    let ws = if matches!(network, "ws" | "httpupgrade") {
        Some(WsConfig {
            path: link.path.clone(),
            host: link.host.clone(),
            headers: None,
        })
    } else {
        None
    };

    let transport_type = match network {
        "ws" => TransportType::WebSocket,
        "httpupgrade" => TransportType::HttpUpgrade,
        "grpc" => TransportType::Grpc,
        _ if has_tls => TransportType::Tls,
        _ => TransportType::Tcp,
    };

    Some(TransportConfig {
        grpc: (network == "grpc").then(|| GrpcConfig {
            service_name: link.path.clone().unwrap_or_default(),
            multi_mode: link.header_type.as_deref() == Some("multi"),
            authority: link.host.clone(),
        }),
        xhttp: None,
        transport_type,
        tls,
        ws,
        reality: None,
    })
}

fn build_uri_transport(
    security: &str,
    transport_type_str: &str,
    params: &HashMap<String, String>,
) -> Result<Option<TransportConfig>> {
    anyhow::ensure!(
        matches!(
            transport_type_str,
            "tcp" | "raw" | "ws" | "grpc" | "httpupgrade" | "xhttp"
        ),
        "Unsupported URI transport; configuration was not downgraded"
    );
    anyhow::ensure!(
        matches!(security, "none" | "tls" | "reality"),
        "Unsupported URI security"
    );
    anyhow::ensure!(
        security != "reality"
            || (matches!(transport_type_str, "tcp" | "raw" | "xhttp")
                && params.get("pbk").is_some_and(|k| !k.is_empty())),
        "REALITY requires TCP and a public key"
    );
    if security == "none" && matches!(transport_type_str, "tcp" | "raw") {
        return Ok(None);
    }

    if security == "reality" {
        return Ok(Some(TransportConfig {
            grpc: None,
            xhttp: if transport_type_str == "xhttp" {
                Some(uri_xhttp(params)?)
            } else {
                None
            },
            transport_type: if transport_type_str == "xhttp" {
                TransportType::Xhttp
            } else {
                TransportType::Reality
            },
            tls: None,
            ws: None,
            reality: Some(RealityConfig {
                public_key: params.get("pbk").cloned().unwrap_or_default(),
                short_id: params.get("sid").cloned().unwrap_or_default(),
                sni: params.get("sni").cloned(),
                client_fingerprint: params.get("fp").cloned(),
                skip_cert_verify: params
                    .get("allowInsecure")
                    .map(|v| v == "1")
                    .unwrap_or(false),
            }),
        }));
    }

    let tls = if security == "tls" {
        Some(TlsConfig {
            sni: params.get("sni").cloned(),
            skip_cert_verify: params
                .get("allowInsecure")
                .map(|v| v == "1")
                .unwrap_or(false),
            alpn: params
                .get("alpn")
                .map(|a| a.split(',').map(|s| s.to_string()).collect()),
            fingerprint: params.get("fp").cloned(),
        })
    } else {
        None
    };

    let ws = if matches!(transport_type_str, "ws" | "httpupgrade") {
        Some(WsConfig {
            path: params.get("path").cloned(),
            host: params.get("host").cloned(),
            headers: None,
        })
    } else {
        None
    };

    let xhttp = if transport_type_str == "xhttp" {
        anyhow::ensure!(
            params.get("flow").is_none_or(|f| f.is_empty()),
            "XHTTP does not support Vision flow"
        );
        let config = uri_xhttp(params)?;
        config.validate()?;
        anyhow::ensure!(
            tls.as_ref()
                .and_then(|t| t.alpn.as_ref())
                .is_none_or(|a| a.is_empty()
                    || a.iter()
                        .all(|s| matches!(s.as_str(), "h2" | "http/1.1" | "h3"))),
            "Unsupported XHTTP ALPN"
        );
        Some(config)
    } else {
        None
    };
    let transport_type = match transport_type_str {
        "ws" => TransportType::WebSocket,
        "httpupgrade" => TransportType::HttpUpgrade,
        "grpc" => TransportType::Grpc,
        "xhttp" => TransportType::Xhttp,
        _ if security == "tls" => TransportType::Tls,
        _ => TransportType::Tcp,
    };

    Ok(Some(TransportConfig {
        grpc: (transport_type_str == "grpc").then(|| GrpcConfig {
            service_name: params.get("serviceName").cloned().unwrap_or_default(),
            multi_mode: params.get("mode").is_some_and(|v| v == "multi"),
            authority: params.get("authority").cloned(),
        }),
        xhttp,
        transport_type,
        tls,
        ws,
        reality: None,
    }))
}

/// Encode a Node back to its share URI (best-effort, simplified transport params).
pub fn node_to_share_uri(node: &Node) -> String {
    let name_enc = urlencoding::encode(&node.name);

    let mut qparams: Vec<String> = Vec::new();
    if let Some(ref tc) = node.transport {
        use TransportType::*;
        let net = match tc.transport_type {
            Tcp => "tcp",
            Tls => "tcp",
            WebSocket => "ws",
            HttpUpgrade => "httpupgrade",
            Grpc => "grpc",
            Xhttp => "xhttp",
            Quic => "quic",
            Reality => "tcp",
        };
        qparams.push(format!("type={}", net));
        if let Some(grpc) = &tc.grpc {
            qparams.push(format!(
                "serviceName={}",
                urlencoding::encode(&grpc.service_name)
            ));
            if grpc.multi_mode {
                qparams.push("mode=multi".into());
            }
            if let Some(host) = &grpc.authority {
                qparams.push(format!("authority={}", urlencoding::encode(host)));
            }
        }

        let security = if tc.reality.is_some() {
            "reality"
        } else {
            match tc.transport_type {
                Tls | WebSocket => "tls",
                Reality => "reality",
                Quic => "quic",
                _ => {
                    if tc.tls.is_some() {
                        "tls"
                    } else {
                        "none"
                    }
                }
            }
        };
        qparams.push(format!("security={}", security));

        if let Some(ref tls) = tc.tls {
            if let Some(ref sni) = tls.sni {
                qparams.push(format!("sni={}", urlencoding::encode(sni)));
            }
            if tls.skip_cert_verify {
                qparams.push("allowInsecure=1".to_string());
            }
        }
        if let Some(xhttp) = &tc.xhttp {
            for (key, value) in [
                ("path", xhttp.path.as_deref()),
                ("host", xhttp.host.as_deref()),
                ("mode", Some(xhttp.mode.as_str())),
            ] {
                if let Some(value) = value {
                    qparams.push(format!("{key}={}", urlencoding::encode(value)));
                }
            }
            if let Some(alpn) = tc.tls.as_ref().and_then(|t| t.alpn.as_ref()) {
                qparams.push(format!("alpn={}", urlencoding::encode(&alpn.join(","))));
            }
            let mut options = xhttp.options.clone();
            if let Some(serde_json::Value::Object(extra)) = options.remove("extra") {
                options = extra.into_iter().collect();
                for key in ["host", "path", "mode"] {
                    options.remove(key);
                }
            }
            if !options.is_empty() {
                qparams.push(format!(
                    "extra={}",
                    urlencoding::encode(&serde_json::to_string(&options).unwrap_or_default())
                ));
            }
        }
        if let Some(ref ws) = tc.ws {
            if let Some(ref path) = ws.path {
                qparams.push(format!("path={}", urlencoding::encode(path)));
            }
            if let Some(ref host) = ws.host {
                qparams.push(format!("host={}", urlencoding::encode(host)));
            }
        }
        if let Some(ref reality) = tc.reality {
            if let Some(sni) = &reality.sni {
                qparams.push(format!("sni={}", urlencoding::encode(sni)));
            }
            qparams.push(format!("pbk={}", urlencoding::encode(&reality.public_key)));
            qparams.push(format!("sid={}", urlencoding::encode(&reality.short_id)));
            if let Some(ref fp) = reality.client_fingerprint {
                qparams.push(format!("fp={}", urlencoding::encode(fp)));
            }
            if reality.skip_cert_verify {
                qparams.push("allowInsecure=1".to_string());
            }
        }
    }

    match &node.protocol {
        ProxyProtocol::Wireguard { config } => super::wireguard::share_uri(node, config),
        ProxyProtocol::AnyTls { password, pool, .. } => {
            let mut params = Vec::new();
            if let Some(ms) = pool.check_interval_ms {
                params.push(format!("idle_session_check_interval_ms={ms}"));
            }
            if let Some(ms) = pool.idle_timeout_ms {
                params.push(format!("idle_session_timeout_ms={ms}"));
            }
            if pool.min_idle != 0 {
                params.push(format!("min_idle_session={}", pool.min_idle));
            }
            if pool.disable_reuse {
                params.push("disable_reuse=true".into());
            }
            if let Some(tls) = node.transport.as_ref().and_then(|t| t.tls.as_ref()) {
                if let Some(sni) = &tls.sni {
                    params.push(format!("peer={}", urlencoding::encode(sni)));
                }
                if tls.skip_cert_verify {
                    params.push("insecure=1".into());
                }
                if let Some(alpn) = &tls.alpn {
                    params.push(format!("alpn={}", urlencoding::encode(&alpn.join(","))));
                }
                if let Some(fp) = &tls.fingerprint {
                    params.push(format!("fp={}", urlencoding::encode(fp)));
                }
            }
            if let Some(pin) = node.extra.get(TLS_CERT_PIN) {
                params.push(format!("pcs={pin}"));
            }
            format!(
                "anytls://{}@{}?{}#{}",
                urlencoding::encode(password),
                crate::proxy::transport::format_host_port(&node.server, node.port),
                params.join("&"),
                name_enc
            )
        }
        ProxyProtocol::Shadowsocks {
            cipher,
            password,
            shadow_tls,
            ..
        } => super::ss_uri::share_uri(node, cipher, password, shadow_tls.as_ref(), &name_enc),
        ProxyProtocol::VMess {
            uuid,
            alter_id,
            cipher,
            ..
        } => {
            let (net, tls_str, host_val, path_val) = if let Some(ref tc) = node.transport {
                let n = match tc.transport_type {
                    TransportType::WebSocket => "ws",
                    TransportType::HttpUpgrade => "httpupgrade",
                    TransportType::Grpc => "grpc",
                    TransportType::Quic => "quic",
                    _ => "tcp",
                };
                let t = if tc.tls.is_some() { "tls" } else { "" };
                let h = tc
                    .ws
                    .as_ref()
                    .and_then(|w| w.host.clone())
                    .unwrap_or_default();
                let p = tc
                    .ws
                    .as_ref()
                    .and_then(|w| w.path.clone())
                    .unwrap_or_default();
                (n, t, h, p)
            } else {
                ("tcp", "", String::new(), String::new())
            };
            let sni = node
                .transport
                .as_ref()
                .and_then(|tc| tc.tls.as_ref())
                .and_then(|t| t.sni.clone())
                .unwrap_or_default();
            let mut json = serde_json::json!({
                "v": "2",
                "ps": node.name,
                "add": node.server,
                "port": node.port,
                "id": uuid,
                "aid": alter_id,
                "scy": cipher,
                "net": net,
                "type": "none",
                "host": host_val,
                "path": path_val,
                "tls": tls_str,
                "sni": sni,
            });
            if let Some(grpc) = node.transport.as_ref().and_then(|t| t.grpc.as_ref()) {
                json["path"] = grpc.service_name.clone().into();
                json["type"] = if grpc.multi_mode { "multi" } else { "gun" }.into();
                if let Some(host) = &grpc.authority {
                    json["host"] = host.clone().into();
                }
            }
            for (key, name) in [
                (VMESS_PADDING, "global_padding"),
                (VMESS_AUTH_LEN, "authenticated_length"),
            ] {
                if node.extra.get(key).is_some_and(|v| v == "true") {
                    json[name] = true.into();
                }
            }
            let encoded = general_purpose::STANDARD.encode(json.to_string().as_bytes());
            format!("vmess://{}", encoded)
        }
        ProxyProtocol::VLess { uuid, flow, .. } => {
            if let Some(value) = node.extra.get(VLESS_ENCRYPTION) {
                qparams.push(format!("encryption={}", urlencoding::encode(value)));
            }
            if let Some(f) = flow {
                qparams.push(format!("flow={}", f));
            }
            let qs = if qparams.is_empty() {
                String::new()
            } else {
                format!("?{}", qparams.join("&"))
            };
            format!(
                "vless://{}@{}:{}{}#{}",
                uuid, node.server, node.port, qs, name_enc
            )
        }
        ProxyProtocol::Tuic {
            uuid,
            password,
            congestion_control,
            relay,
            heartbeat_ms,
            ..
        } => {
            let sni = node
                .transport
                .as_ref()
                .and_then(|tc| tc.tls.as_ref())
                .and_then(|t| t.sni.clone())
                .unwrap_or_default();
            let skip = node
                .transport
                .as_ref()
                .and_then(|tc| tc.tls.as_ref())
                .map(|t| t.skip_cert_verify)
                .unwrap_or(false);
            let mut extra = format!("?congestion_control={}", congestion_control);
            if node.extra.get(TUIC_ZERO_RTT).is_some_and(|v| v == "true") {
                extra.push_str("&zero_rtt_handshake=1");
            }
            if let Some(ms) = heartbeat_ms {
                extra.push_str(&format!("&heartbeat_interval={ms}"));
            }
            match relay {
                TuicRelay::Native => {}
                TuicRelay::Quic => extra.push_str("&udp_relay_mode=quic"),
                TuicRelay::UotV1 => extra.push_str("&udp_over_stream=1&udp_over_stream_version=1"),
                TuicRelay::UotV2 => extra.push_str("&udp_over_stream=1&udp_over_stream_version=2"),
            }
            if !sni.is_empty() {
                extra.push_str(&format!("&sni={}", urlencoding::encode(&sni)));
            }
            if skip {
                extra.push_str("&allowInsecure=1");
            }
            if let Some(pin) = node.extra.get(TUIC_CERT_PIN) {
                extra.push_str(&format!("&pcs={}", urlencoding::encode(pin)));
            }
            if let Some(alpn) = node
                .transport
                .as_ref()
                .and_then(|t| t.tls.as_ref())
                .and_then(|t| t.alpn.as_ref())
                && !alpn.is_empty()
            {
                extra.push_str(&format!("&alpn={}", urlencoding::encode(&alpn.join(","))));
            }
            let host = if node.server.contains(':') {
                format!("[{}]", node.server.trim_matches(['[', ']']))
            } else {
                node.server.clone()
            };
            format!(
                "tuic://{}:{}@{}:{}{}#{}",
                urlencoding::encode(uuid),
                urlencoding::encode(password),
                host,
                node.port,
                extra,
                name_enc
            )
        }
        ProxyProtocol::Trojan { password, udp } => {
            if node.extra.get(UDP_ONLY).is_some_and(|v| v == "true") {
                qparams.push("network=udp".into());
            } else if !udp {
                qparams.push("network=tcp".into());
            }
            let qs = if qparams.is_empty() {
                String::new()
            } else {
                format!("?{}", qparams.join("&"))
            };
            format!(
                "trojan://{}@{}{}#{}",
                urlencoding::encode(password),
                crate::proxy::transport::format_host_port(&node.server, node.port),
                qs,
                name_enc
            )
        }
        ProxyProtocol::Hysteria2 { password, obfs, .. } => {
            let sni = node
                .transport
                .as_ref()
                .and_then(|tc| tc.tls.as_ref())
                .and_then(|t| t.sni.clone())
                .unwrap_or_default();
            let skip = node
                .transport
                .as_ref()
                .and_then(|tc| tc.tls.as_ref())
                .map(|t| t.skip_cert_verify)
                .unwrap_or(false);
            let mut extra_params = Vec::new();
            if let Some(o) = obfs {
                extra_params.push(format!("obfs={}", o.kind));
                extra_params.push(format!(
                    "obfs-password={}",
                    urlencoding::encode(&o.password)
                ));
                for (k, v) in [
                    ("min-packet-size", o.min_packet_size),
                    ("max-packet-size", o.max_packet_size),
                ] {
                    if let Some(v) = v {
                        extra_params.push(format!("{k}={v}"));
                    }
                }
            }

            if !sni.is_empty() {
                extra_params.push(format!("sni={}", urlencoding::encode(&sni)));
            }
            if skip {
                extra_params.push("insecure=1".to_string());
            }
            for (field, query) in [
                (TLS_CERT_PIN, "pinSHA256"),
                (HY2_UP, "upmbps"),
                (HY2_DOWN, "downmbps"),
                (HY2_FAST_OPEN, "fastopen"),
                (HY2_MPORT, "mport"),
                (HY2_HOP_INTERVAL, "hop_interval_ms"),
                (HY2_HOP_MAX, "hop_interval_max_ms"),
            ] {
                if let Some(value) = node.extra.get(field) {
                    extra_params.push(format!("{query}={}", urlencoding::encode(value)));
                }
            }
            if let Some(fp) = hysteria2_fingerprint(node) {
                extra_params.push(format!("fp={}", urlencoding::encode(fp)));
            }
            if let Some(alpn) = node
                .transport
                .as_ref()
                .and_then(|t| t.tls.as_ref())
                .and_then(|t| t.alpn.as_ref())
            {
                extra_params.push(format!("alpn={}", urlencoding::encode(&alpn.join(","))));
            }
            let qs = if extra_params.is_empty() {
                String::new()
            } else {
                format!("?{}", extra_params.join("&"))
            };
            format!(
                "hy2://{}@{}{}#{}",
                urlencoding::encode(password),
                crate::proxy::transport::format_host_port(&node.server, node.port),
                qs,
                name_enc
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hysteria_and_anytls_parameters_survive_share_yaml_clash_and_dedup() {
        let pin = "ab".repeat(32);
        for link in [
            format!(
                "hy2://synthetic@vpn.example.invalid:443?security=tls&insecure=1&pinSHA256={pin}&sni=tls.example.invalid&alpn=h3&upmbps=10&downmbps=20&fastopen=1"
            ),
            format!(
                "anytls://synthetic@vpn.example.invalid:443?security=tls&insecure=1&pcs={pin}&sni=tls.example.invalid&alpn=h2"
            ),
        ] {
            let node = parse_proxy_uri(&link).unwrap();
            assert_eq!(node.extra[TLS_CERT_PIN], pin);
            let shared = parse_proxy_uri(&node_to_share_uri(&node)).unwrap();
            let persisted: Node =
                serde_yaml::from_str(&serde_yaml::to_string(&node).unwrap()).unwrap();
            let clash = super::super::clash::parse_clash_config(&crate::export_clash_config(&[
                node.clone(),
            ]))
            .unwrap();
            assert_eq!(clash.len(), 1);
            for restored in [shared, persisted, clash[0].clone()] {
                assert_eq!(restored.extra, node.extra);
                assert_eq!(
                    serde_json::to_value(restored.transport).unwrap(),
                    serde_json::to_value(&node.transport).unwrap()
                );
            }
            let mut changed = node.clone();
            changed.extra.insert(TLS_CERT_PIN.into(), "cd".repeat(32));
            assert_ne!(
                super::super::dedup::node_fingerprint(&node),
                super::super::dedup::node_fingerprint(&changed)
            );
            assert!(parse_proxy_uri(&link.replace(&pin, "invalid")).is_err());
            assert!(parse_proxy_uri(&link.replace("security=tls", "security=none")).is_err());
        }
    }

    #[test]
    fn hysteria_parameters_reject_invalid_and_conflicting_values() {
        let prefix = "hy2://synthetic@vpn.example.invalid:443?";
        for query in [
            "upmbps=-1",
            "downmbps=nan",
            "upmbps=1000001",
            "fastopen=maybe",
            "alpn=h3&alpn=hysteria",
        ] {
            assert!(parse_hysteria2_uri(&format!("{prefix}{query}")).is_err());
        }
        assert!(
            parse_hysteria2_uri(&format!(
                "{prefix}pcs={}&pinSHA256={}",
                "ab".repeat(32),
                "cd".repeat(32)
            ))
            .is_err()
        );
    }

    #[test]
    fn hysteria_quic_profile_preserves_security_and_roundtrips() {
        let uri = "hy2://synthetic@vpn.example.invalid:443?security=tls&sni=tls.example.invalid&alpn=h3&fp=chrome";
        let node = parse_hysteria2_uri(uri).unwrap();
        assert_eq!(
            node.extra.get(HY2_CLIENT_FP_HINT).map(String::as_str),
            Some("chrome")
        );
        assert!(
            !node
                .transport
                .as_ref()
                .unwrap()
                .tls
                .as_ref()
                .unwrap()
                .skip_cert_verify
        );
        let exported = node_to_share_uri(&node);
        let imported = parse_hysteria2_uri(&exported).unwrap();
        assert_eq!(
            imported.extra.get(HY2_CLIENT_FP_HINT),
            node.extra.get(HY2_CLIENT_FP_HINT)
        );
        assert!(
            parse_hysteria2_uri(&uri.replace("fp=chrome", &format!("fp={}", "ab".repeat(32))))
                .is_err()
        );
        assert!(parse_hysteria2_uri(&format!("{uri}&obfs=salamander")).is_err());
        assert!(parse_hysteria2_uri(&uri.replace("alpn=h3", "alpn=hysteria")).is_err());
        assert!(parse_hysteria2_uri(&uri.replace("fp=chrome", "fp=chrome115")).is_ok());
        let exported = super::super::export::export_clash_config(std::slice::from_ref(&node));
        let restored = super::super::clash::parse_clash_config(&exported).unwrap();
        assert_eq!(hysteria2_fingerprint(&restored[0]), Some("chrome"));
        let incompatible = exported.replace("h3", "hysteria");
        assert!(
            super::super::clash::parse_clash_config(&incompatible)
                .unwrap()
                .is_empty()
        );
        let mut legacy = node.clone();
        legacy
            .transport
            .as_mut()
            .unwrap()
            .tls
            .as_mut()
            .unwrap()
            .fingerprint = None;
        let legacy_export = super::super::export::export_clash_config(&[legacy]);
        let restored = super::super::clash::parse_clash_config(&legacy_export).unwrap();
        assert_eq!(hysteria2_fingerprint(&restored[0]), Some("chrome"));
    }

    #[test]
    fn test_parse_vmess_uri() {
        let json = serde_json::json!({
            "v": "2",
            "ps": "Test-VMess",
            "add": "example.com",
            "port": "443",
            "id": "b0e80a62-8a51-47f0-91f1-f0f7faf8d9d4",
            "aid": "0",
            "scy": "auto",
            "net": "ws",
            "host": "example.com",
            "path": "/v2ray",
            "tls": "tls",
            "sni": "example.com"
        });
        let encoded = general_purpose::STANDARD.encode(json.to_string());
        let uri = format!("vmess://{}", encoded);

        let node = parse_vmess_uri(&uri).unwrap();
        assert_eq!(node.name, "Test-VMess");
        assert_eq!(node.server, "example.com");
        assert_eq!(node.port, 443);
        match &node.protocol {
            ProxyProtocol::VMess { uuid, .. } => {
                assert_eq!(uuid, "b0e80a62-8a51-47f0-91f1-f0f7faf8d9d4");
            }
            _ => panic!("Expected VMess"),
        }
        let transport = node.transport.as_ref().unwrap();
        assert_eq!(transport.transport_type, TransportType::WebSocket);
    }

    #[test]
    fn test_parse_vmess_uri_with_percent_encoded_name() {
        let json = serde_json::json!({
            "v": "2",
            "ps": "%2545%2578%2561%256D%2570%256C%2565",
            "add": "example.com",
            "port": "443",
            "id": "b0e80a62-8a51-47f0-91f1-f0f7faf8d9d4",
            "aid": "0"
        });
        let encoded = general_purpose::STANDARD.encode(json.to_string());
        let uri = format!("vmess://{}", encoded);

        let node = parse_vmess_uri(&uri).unwrap();
        assert_eq!(node.name, "Example");
    }

    #[test]
    fn test_parse_vless_uri() {
        let uri = "vless://uuid-here@example.com:443?encryption=none&security=tls&sni=example.com&type=ws&path=/v2ray#My-VLESS";
        let node = parse_vless_uri(uri).unwrap();
        assert_eq!(node.name, "My-VLESS");
        assert_eq!(node.server, "example.com");
        assert_eq!(node.port, 443);
    }

    #[test]
    fn test_parse_vless_uri_with_double_encoded_name() {
        let uri =
            "vless://uuid-here@example.com:443?encryption=none#%2545%2578%2561%256D%2570%256C%2565";
        let node = parse_vless_uri(uri).unwrap();
        assert_eq!(node.name, "Example");
    }

    #[test]
    fn test_parse_ss_uri_sip002() {
        let userinfo = general_purpose::STANDARD.encode("aes-256-gcm:password123");
        let uri = format!("ss://{}@1.2.3.4:8388#My-SS", userinfo);
        let node = parse_ss_uri(&uri).unwrap();
        assert_eq!(node.name, "My-SS");
        assert_eq!(node.server, "1.2.3.4");
        assert_eq!(node.port, 8388);
        match &node.protocol {
            ProxyProtocol::Shadowsocks {
                cipher, password, ..
            } => {
                assert_eq!(cipher, "aes-256-gcm");
                assert_eq!(password, "password123");
            }
            _ => panic!("Expected SS"),
        }
    }

    #[test]
    fn test_parse_tuic_uri() {
        let uri = "tuic://uuid-here:password@example.com:443?congestion_control=bbr&sni=www.example.com&allow_insecure=1&alpn%5B0%5D=h3#TUIC-SG";
        let node = parse_tuic_uri(uri).unwrap();
        assert_eq!(node.name, "TUIC-SG");
        assert_eq!(node.server, "example.com");
        match &node.protocol {
            ProxyProtocol::Tuic {
                congestion_control, ..
            } => {
                assert_eq!(congestion_control, "bbr");
            }
            _ => panic!("Expected TUIC"),
        }
        let transport = node.transport.as_ref().expect("TUIC transport");
        assert_eq!(transport.transport_type, TransportType::Quic);
        let tls = transport.tls.as_ref().expect("TUIC TLS");
        assert_eq!(tls.sni.as_deref(), Some("www.example.com"));
        assert!(tls.skip_cert_verify);
        assert_eq!(tls.alpn.as_ref().unwrap(), &vec!["h3".to_string()]);
    }

    #[test]
    fn tuic_insecure_pin_credentials_and_alpn_roundtrip() {
        let pin = "ab".repeat(32);
        let link = format!(
            "tuic://00000000-0000-0000-0000-000000000001:pass%40word%3Avalue@vpn.example.invalid:443?insecure=1&pcs={pin}&sni=tls.example.invalid&alpn=h3%2Ctuic"
        );
        let node = parse_tuic_uri(&link).unwrap();
        let tls = node.transport.as_ref().unwrap().tls.as_ref().unwrap();
        assert!(tls.skip_cert_verify);
        assert_eq!(tls.alpn.as_ref().unwrap(), &["h3", "tuic"]);
        assert_eq!(node.extra[TUIC_CERT_PIN], pin);
        if let ProxyProtocol::Tuic { password, .. } = &node.protocol {
            assert_eq!(password, "pass@word:value");
        } else {
            panic!("TUIC expected");
        }
        let exported = node_to_share_uri(&node);
        let restored = parse_tuic_uri(&exported).unwrap();
        assert_eq!(restored.extra[TUIC_CERT_PIN], pin);
        assert_eq!(
            restored
                .transport
                .as_ref()
                .unwrap()
                .tls
                .as_ref()
                .unwrap()
                .alpn,
            tls.alpn
        );
        let saved = serde_yaml::to_string(&node).unwrap();
        let persisted: Node = serde_yaml::from_str(&saved).unwrap();
        assert_eq!(persisted.extra[TUIC_CERT_PIN], pin);
        let yaml = crate::export_clash_config(&[node.clone()]);
        let clash = super::super::clash::parse_clash_config(&yaml).unwrap();
        assert_eq!(clash[0].extra[TUIC_CERT_PIN], pin);
        let mut changed = node.clone();
        changed.extra.insert(TUIC_CERT_PIN.into(), "cd".repeat(32));
        assert_ne!(
            super::super::dedup::node_fingerprint(&node),
            super::super::dedup::node_fingerprint(&changed)
        );
        assert!(parse_tuic_uri(&link.replace(&pin, "invalid")).is_err());
    }

    #[test]
    fn test_parse_tuic_uri_with_percent_encoded_name() {
        let uri = "tuic://uuid-here:password@example.com:443?congestion_control=bbr#%45%78%61%6D%70%6C%65";
        let node = parse_tuic_uri(uri).unwrap();
        assert_eq!(node.name, "Example");
    }

    #[test]
    fn test_parse_tuic_uri_with_double_encoded_name() {
        let uri = "tuic://uuid-here:password@example.com:443?congestion_control=bbr#%2545%2578%2561%256D%2570%256C%2565";
        let node = parse_tuic_uri(uri).unwrap();
        assert_eq!(node.name, "Example");
    }

    #[test]
    fn test_parse_trojan_uri() {
        let uri = "trojan://password@example.com:443?sni=example.com#Trojan-JP";
        let node = parse_trojan_uri(uri).unwrap();
        assert_eq!(node.name, "Trojan-JP");
        assert_eq!(node.server, "example.com");
    }

    #[test]
    fn test_parse_hysteria2_uri_with_percent_encoded_name() {
        let uri = "hy2://password@example.com:443#%45%78%61%6D%70%6C%65";
        let node = parse_hysteria2_uri(uri).unwrap();
        assert_eq!(node.name, "Example");
    }

    #[test]
    fn flat_xray_endpoints_preserve_user_settings() {
        let settings = serde_json::json!({"address":"example.invalid","port":8443,"id":"我爱🍉老师1314","security":"chacha20-poly1305"});
        for kind in ["vmess", "vless"] {
            let config = serde_json::json!({"outbounds":[{"protocol":kind,"settings":settings}]});
            let nodes = parse_v2ray_config(&config.to_string()).unwrap();
            assert_eq!(nodes.len(), 1);
            assert_eq!(nodes[0].port, 8443);
            let id = match &nodes[0].protocol {
                ProxyProtocol::VMess { uuid, cipher, .. } => {
                    assert_eq!(cipher, "chacha20-poly1305");
                    uuid
                }
                ProxyProtocol::VLess { uuid, .. } => uuid,
                _ => panic!("unexpected protocol"),
            };
            assert_eq!(id, "5783a3e7-e373-51cd-8642-c83782b807c5");
        }
        let config = serde_json::json!({"outbounds":[{"protocol":"shadowsocks","settings":{"address":"example.invalid","port":8388,"method":"aes-128-gcm","password":"local"}}]});
        let nodes = parse_v2ray_config(&config.to_string()).unwrap();
        assert!(
            matches!(&nodes[0].protocol, ProxyProtocol::Shadowsocks { cipher, .. } if cipher == "aes-128-gcm")
        );
    }

    #[test]
    fn test_parse_v2ray_config() {
        let json = r#"{
            "outbounds": [
                {
                    "protocol": "vmess",
                    "tag": "proxy",
                    "settings": {
                        "vnext": [{
                            "address": "example.com",
                            "port": 443,
                            "users": [{
                                "id": "test-uuid",
                                "alterId": 0,
                                "security": "auto"
                            }]
                        }]
                    },
                    "streamSettings": {
                        "network": "ws",
                        "security": "tls",
                        "tlsSettings": {
                            "serverName": "example.com"
                        },
                        "wsSettings": {
                            "path": "/path",
                            "headers": {"Host": "example.com"}
                        }
                    }
                },
                {
                    "protocol": "freedom",
                    "tag": "direct"
                }
            ]
        }"#;

        let nodes = parse_v2ray_config(json).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "proxy");
    }
}

#[cfg(test)]
mod native_protocol_regressions {
    use super::*;
    #[test]
    fn anytls_password_tls_and_name_roundtrip() {
        let node=parse_anytls_uri("anytls://synthetic%3Apassword%40value@vpn.example.invalid:443?sni=tls.example.invalid&alpn=h2%2Chttp%2F1.1#Synthetic%20node").unwrap();
        let uri = node_to_share_uri(&node);
        let restored = parse_anytls_uri(&uri).unwrap();
        assert_eq!(
            serde_json::to_value(&node).unwrap(),
            serde_json::to_value(&restored).unwrap()
        );
        let yaml = super::super::export::nodes_to_clash_yaml(std::slice::from_ref(&node)).unwrap();
        let parsed = super::super::clash::parse_clash_config(&yaml).unwrap();
        assert_eq!(parsed.len(), 1);
        let ProxyProtocol::AnyTls { password, .. } = &parsed[0].protocol else {
            unreachable!()
        };
        assert_eq!(password, "synthetic:password@value");
        assert!(
            !parsed[0]
                .transport
                .as_ref()
                .unwrap()
                .tls
                .as_ref()
                .unwrap()
                .skip_cert_verify
        );
        assert!(parse_anytls_uri("anytls://synthetic@vpn.example.invalid?sni=a&sni=b").is_err());
    }
    #[test]
    fn hysteria_auth_sni_and_security_options_are_not_lost() {
        let node=parse_hysteria2_uri("hy2://synthetic%40auth:secret@vpn.example.invalid:443?sni=tls.example.invalid&insecure=1").unwrap();
        let ProxyProtocol::Hysteria2 { password, .. } = node.protocol else {
            unreachable!()
        };
        assert_eq!(password, "synthetic@auth:secret");
        let tls = node.transport.unwrap().tls.unwrap();
        assert_eq!(tls.sni.as_deref(), Some("tls.example.invalid"));
        assert!(tls.skip_cert_verify);
        assert!(
            parse_hysteria2_uri(
                "hy2://auth@vpn.example.invalid?obfs=salamander&obfs-password=secret"
            )
            .is_ok()
        );
    }
    #[test]
    fn hysteria_mport_imports_on_fixed_port_with_explicit_metadata() {
        let node = parse_hysteria2_uri(
            "hy2://synthetic@example.invalid:35000?mport=35000-39000&sni=tls.example.invalid",
        )
        .unwrap();
        assert_eq!(node.port, 35000);
        assert_eq!(
            node.extra.get(HY2_MPORT).map(String::as_str),
            Some("35000-39000")
        );
        let shared = parse_hysteria2_uri(&node_to_share_uri(&node)).unwrap();
        assert_eq!(shared.extra.get(HY2_MPORT), node.extra.get(HY2_MPORT));
        let clash = crate::export_clash_config(&[node]);
        let restored = super::super::clash::parse_clash_config(&clash).unwrap();
        assert_eq!(restored[0].port, 35000);
        assert_eq!(
            restored[0].extra.get(HY2_MPORT).map(String::as_str),
            Some("35000-39000")
        );
        for mport in ["", "35000-", "39000-35000", "35001-39000", "0-35000"] {
            let uri = format!("hy2://synthetic@example.invalid:35000?mport={mport}");
            assert!(parse_hysteria2_uri(&uri).is_err(), "{mport}");
        }
        assert!(
            parse_hysteria2_uri(
                "hy2://synthetic@example.invalid:35000?mport=35000-39000&obfs=salamander"
            )
            .is_err()
        );
    }
    #[test]
    fn unsupported_uri_transports_fail_closed() {
        for transport in ["h2", "kcp"] {
            assert!(parse_vless_uri(&format!("vless://00000000-0000-0000-0000-000000000001@vpn.example.invalid:443?security=tls&type={transport}")).is_err());
        }
    }
}

fn uri_xhttp(params: &HashMap<String, String>) -> Result<XhttpConfig> {
    let mut config = XhttpConfig {
        path: params.get("path").cloned(),
        host: params.get("host").cloned(),
        mode: params.get("mode").cloned().unwrap_or_else(|| "auto".into()),
        options: Default::default(),
    };
    if let Some(extra) = params.get("extra") {
        let extra: serde_json::Value = serde_json::from_slice(extra.as_bytes())
            .map_err(|_| anyhow::anyhow!("Invalid XHTTP extra options"))?;
        config.options.insert("extra".into(), extra);
    }
    config.validate()?;
    Ok(config)
}
