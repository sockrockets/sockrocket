//! WireGuard profile, Clash, sing-box and URI conversion. Never executes hooks.
use super::model::*;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
fn text(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| v.get(k).and_then(Value::as_str).map(str::to_owned))
}
fn required(v: &Value, keys: &[&str]) -> Result<String> {
    text(v, keys)
        .filter(|s| !s.is_empty())
        .context("Missing required WireGuard field")
}
fn number(v: &Value, keys: &[&str]) -> Result<Option<u64>> {
    keys.iter()
        .find_map(|k| v.get(k))
        .map(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
                .context("Invalid WireGuard numeric field")
        })
        .transpose()
}
fn strings(v: &Value, keys: &[&str]) -> Result<Option<Vec<String>>> {
    let Some(value) = keys.iter().find_map(|k| v.get(k)) else {
        return Ok(None);
    };
    if let Some(array) = value.as_array() {
        return Ok(Some(
            array
                .iter()
                .map(|s| {
                    s.as_str()
                        .map(str::to_owned)
                        .context("Expected WireGuard string list")
                })
                .collect::<Result<_>>()?,
        ));
    }
    Ok(Some(
        value
            .as_str()
            .context("Expected WireGuard string list")?
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect(),
    ))
}
fn finish(name: String, server: String, port: u16, config: WireguardConfig) -> Result<Node> {
    let _ = crate::proxy::wireguard::WireguardOutbound::new(&server, port, &config)?;
    Ok(Node {
        name,
        server,
        port,
        protocol: ProxyProtocol::Wireguard { config },
        transport: None,
        latency_ms: None,
        tags: vec![],
        extra: Default::default(),
    })
}
fn reject_reserved(v: &Value) -> Result<()> {
    if let Some(value) = v.get("reserved") {
        ensure!(
            value
                .as_array()
                .is_some_and(|a| a.len() == 3 && a.iter().all(|b| b.as_u64() == Some(0))),
            "Non-standard WireGuard reserved bytes are not supported"
        );
    }
    Ok(())
}
pub(crate) fn from_clash(value: &serde_yaml::Value) -> Result<Node> {
    let v = serde_json::to_value(value)?;
    reject_reserved(&v)?;
    ensure!(
        v.get("amnezia-wg-option").is_none(),
        "AmneziaWG is not standard WireGuard"
    );
    let mut local_addresses = Vec::new();
    for (field, prefix) in [("ip", 32), ("ipv6", 128)] {
        if let Some(ip) = text(&v, &[field]) {
            local_addresses.push(if ip.contains('/') {
                ip
            } else {
                format!("{ip}/{prefix}")
            });
        }
    }
    let mut config = WireguardConfig {
        private_key: required(&v, &["private-key"])?,
        public_key: required(&v, &["public-key"])?,
        preshared_key: text(&v, &["pre-shared-key", "preshared-key"]),
        local_addresses,
        ..Default::default()
    };
    common(&v, &mut config)?;
    let port = u16::try_from(number(&v, &["port"])?.context("Missing WireGuard port")?)
        .context("Invalid WireGuard port")?;
    finish(
        text(&v, &["name"])
            .map(|s| decode_display_name(&s))
            .unwrap_or_else(|| "WireGuard".into()),
        required(&v, &["server"])?,
        port,
        config,
    )
}
pub(crate) fn from_singbox(v: &Value) -> Result<Node> {
    reject_reserved(v)?;
    ensure!(
        !v.get("system")
            .or_else(|| v.get("system_interface"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        "WireGuard system interfaces are not created by the userspace endpoint"
    );
    if let Some(system) = v.get("system").or_else(|| v.get("system_interface")) {
        ensure!(
            system.as_bool() == Some(false),
            "WireGuard requires the userspace endpoint"
        );
    }
    ensure!(
        number(v, &["listen_port"])?.unwrap_or(0) == 0,
        "A fixed local WireGuard listen port is not supported"
    );
    let peer = if let Some(peers) = v.get("peers") {
        let peers = peers.as_array().context("Invalid WireGuard peers")?;
        ensure!(peers.len() == 1, "Import one WireGuard peer per node");
        &peers[0]
    } else {
        v
    };
    reject_reserved(peer)?;
    let config = WireguardConfig {
        private_key: required(v, &["private_key"])?,
        public_key: required(peer, &["public_key", "peer_public_key"])?,
        preshared_key: text(peer, &["pre_shared_key", "preshared_key"]),
        local_addresses: strings(v, &["address", "local_address"])?
            .context("Missing WireGuard local address")?,
        ..Default::default()
    };
    let mut config = config;
    common(v, &mut config)?;
    common(peer, &mut config)?;
    let port = u16::try_from(
        number(peer, &["port", "server_port"])?.context("Missing WireGuard peer port")?,
    )
    .context("Invalid WireGuard peer port")?;
    finish(
        text(v, &["tag"]).unwrap_or_else(|| "WireGuard".into()),
        required(peer, &["address", "server"])?,
        port,
        config,
    )
}
fn common(v: &Value, c: &mut WireguardConfig) -> Result<()> {
    if let Some(allowed) = strings(v, &["allowed_ips", "allowed-ips"])? {
        c.allowed_ips = allowed;
    }
    if let Some(dns) = strings(v, &["dns"])? {
        c.dns = dns
            .iter()
            .map(|s| {
                s.parse()
                    .map_err(|_| anyhow::anyhow!("WireGuard DNS must use IP addresses"))
            })
            .collect::<Result<_>>()?;
    }
    if let Some(mtu) = number(v, &["mtu"])? {
        c.mtu = usize::try_from(mtu).context("Invalid WireGuard MTU")?;
    }
    if let Some(keepalive) = number(
        v,
        &[
            "persistent_keepalive_interval",
            "persistent_keepalive",
            "persistent-keepalive",
        ],
    )? {
        c.persistent_keepalive = u16::try_from(keepalive).context("Invalid WireGuard keepalive")?;
    }
    if let Some(udp) = v.get("udp") {
        c.udp = udp.as_bool().context("WireGuard UDP must be boolean")?;
    }
    Ok(())
}
pub fn parse_uri(uri: &str) -> Result<Node> {
    let parsed = url::Url::parse(uri).map_err(|_| anyhow::anyhow!("Invalid WireGuard URI"))?;
    ensure!(
        parsed.scheme() == "wireguard"
            && parsed.password().is_none()
            && matches!(parsed.path(), "" | "/"),
        "Invalid WireGuard URI fields"
    );
    let mut params = std::collections::HashMap::new();
    for (key, value) in parsed.query_pairs() {
        ensure!(
            matches!(
                key.as_ref(),
                "publickey"
                    | "presharedkey"
                    | "address"
                    | "allowedips"
                    | "dns"
                    | "mtu"
                    | "keepalive"
                    | "udp"
            ),
            "Unsupported WireGuard URI option"
        );
        ensure!(
            params
                .insert(key.into_owned(), value.into_owned())
                .is_none(),
            "Duplicate WireGuard URI option"
        );
    }
    let list = |key: &str| {
        params.get(key).map(|v| {
            v.split(',')
                .map(|s| s.trim().to_owned())
                .collect::<Vec<_>>()
        })
    };
    let mut config = WireguardConfig {
        private_key: urlencoding::decode(parsed.username())
            .map_err(|_| anyhow::anyhow!("Invalid WireGuard key encoding"))?
            .into_owned(),
        public_key: params
            .get("publickey")
            .context("Missing WireGuard public key")?
            .clone(),
        preshared_key: params.get("presharedkey").cloned(),
        local_addresses: list("address").context("Missing WireGuard local address")?,
        ..Default::default()
    };
    if let Some(v) = list("allowedips") {
        config.allowed_ips = v;
    }
    if let Some(v) = list("dns") {
        config.dns = v
            .iter()
            .map(|s| {
                s.parse()
                    .map_err(|_| anyhow::anyhow!("Invalid WireGuard DNS"))
            })
            .collect::<Result<_>>()?;
    }
    if let Some(v) = params.get("mtu") {
        config.mtu = v.parse().context("Invalid WireGuard MTU")?;
    }
    if let Some(v) = params.get("keepalive") {
        config.persistent_keepalive = v.parse().context("Invalid WireGuard keepalive")?;
    }
    if let Some(v) = params.get("udp") {
        ensure!(
            matches!(v.as_str(), "0" | "1"),
            "Invalid WireGuard UDP option"
        );
        config.udp = v == "1";
    }
    finish(
        parsed
            .fragment()
            .map(decode_display_name)
            .unwrap_or_else(|| "WireGuard".into()),
        parsed
            .host_str()
            .context("Missing WireGuard host")?
            .trim_start_matches('[')
            .trim_end_matches(']')
            .into(),
        parsed.port().context("Missing WireGuard port")?,
        config,
    )
}
pub fn share_uri(node: &Node, c: &WireguardConfig) -> String {
    let mut params = vec![
        ("publickey", c.public_key.clone()),
        ("address", c.local_addresses.join(",")),
        ("allowedips", c.allowed_ips.join(",")),
        (
            "dns",
            c.dns
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
        ),
        ("mtu", c.mtu.to_string()),
        ("keepalive", c.persistent_keepalive.to_string()),
        ("udp", if c.udp { "1" } else { "0" }.into()),
    ];
    if let Some(key) = &c.preshared_key {
        params.push(("presharedkey", key.clone()));
    }
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(params)
        .finish();
    format!(
        "wireguard://{}@{}?{query}#{}",
        urlencoding::encode(&c.private_key),
        crate::proxy::transport::format_host_port(&node.server, node.port),
        urlencoding::encode(&node.name)
    )
}
pub fn parse_profile(text: &str) -> Result<Node> {
    ensure!(text.len() <= 64 * 1024, "WireGuard profile too large");
    let mut section = "";
    let mut interface = serde_json::Map::new();
    let mut peer = serde_json::Map::new();
    let mut saw_peer = false;
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line == "[Interface]" {
            ensure!(
                section.is_empty(),
                "Duplicate or misplaced WireGuard interface"
            );
            section = "interface";
            continue;
        }
        if line == "[Peer]" {
            ensure!(
                section == "interface" && !saw_peer,
                "Import one WireGuard peer per profile"
            );
            saw_peer = true;
            section = "peer";
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .context("Invalid WireGuard profile line")?;
        let key = key.trim();
        let value = value.trim();
        let (map, target) = match (section, key) {
            ("interface", "PrivateKey") => (&mut interface, "private_key"),
            ("interface", "Address") => (&mut interface, "local_address"),
            ("interface", "DNS") => (&mut interface, "dns"),
            ("interface", "MTU") => (&mut interface, "mtu"),
            ("peer", "PublicKey") => (&mut peer, "public_key"),
            ("peer", "PresharedKey") => (&mut peer, "pre_shared_key"),
            ("peer", "AllowedIPs") => (&mut peer, "allowed_ips"),
            ("peer", "PersistentKeepalive") => (&mut peer, "persistent_keepalive"),
            ("peer", "Endpoint") => (&mut peer, "endpoint"),
            _ => anyhow::bail!(
                "Unsupported WireGuard profile field; hooks and host routing directives are never executed"
            ),
        };
        ensure!(
            map.insert(target.into(), json!(value)).is_none(),
            "Duplicate WireGuard profile field"
        );
    }
    let endpoint = peer
        .remove("endpoint")
        .and_then(|v| v.as_str().map(str::to_owned))
        .context("WireGuard peer endpoint is required")?;
    let endpoint = url::Url::parse(&format!("wireguard://{endpoint}"))
        .map_err(|_| anyhow::anyhow!("Invalid WireGuard peer endpoint"))?;
    ensure!(
        endpoint.username().is_empty()
            && endpoint.query().is_none()
            && endpoint.fragment().is_none()
            && matches!(endpoint.path(), "" | "/"),
        "Invalid WireGuard peer endpoint fields"
    );
    peer.insert(
        "address".into(),
        json!(
            endpoint
                .host_str()
                .context("Missing WireGuard host")?
                .trim_start_matches('[')
                .trim_end_matches(']')
        ),
    );
    peer.insert(
        "port".into(),
        json!(endpoint.port().context("Missing WireGuard port")?),
    );
    interface.insert("peers".into(), json!([peer]));
    from_singbox(&Value::Object(interface))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    fn node() -> Node {
        finish(
            "Synthetic node".into(),
            "vpn.example.invalid".into(),
            51820,
            WireguardConfig {
                private_key: STANDARD.encode([11; 32]),
                public_key: STANDARD.encode([22; 32]),
                preshared_key: Some(STANDARD.encode([33; 32])),
                local_addresses: vec!["192.0.2.2/32".into(), "2001:db8::2/128".into()],
                ..Default::default()
            },
        )
        .unwrap()
    }
    #[test]
    fn uri_clash_and_persistence_roundtrip_preserve_wireguard_configuration() {
        let node = node();
        let ProxyProtocol::Wireguard { config } = &node.protocol else {
            unreachable!()
        };
        let uri = share_uri(&node, config);
        let parsed = parse_uri(&uri).unwrap();
        assert_eq!(
            serde_json::to_value(&node.protocol).unwrap(),
            serde_json::to_value(parsed.protocol).unwrap()
        );
        let yaml = super::super::export::nodes_to_clash_yaml(std::slice::from_ref(&node)).unwrap();
        let parsed = super::super::clash::parse_clash_config(&yaml).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(
            serde_json::to_value(&node.protocol).unwrap(),
            serde_json::to_value(&parsed[0].protocol).unwrap()
        );
        let persisted = serde_yaml::to_string(&node).unwrap();
        let restored: Node = serde_yaml::from_str(&persisted).unwrap();
        assert_eq!(
            serde_json::to_value(&node.protocol).unwrap(),
            serde_json::to_value(restored.protocol).unwrap()
        );
        assert!(!format!("{config:?}").contains(&config.private_key));
    }
    #[test]
    fn profile_and_singbox_endpoint_import_without_system_changes() {
        let node = node();
        let ProxyProtocol::Wireguard { config } = node.protocol else {
            unreachable!()
        };
        let profile = format!(
            "[Interface]\nPrivateKey={}\nAddress=192.0.2.2/32\nDNS=1.1.1.1\n[Peer]\nPublicKey={}\nPresharedKey={}\nAllowedIPs=0.0.0.0/0\nEndpoint=vpn.example.invalid:51820\nPersistentKeepalive=25\n",
            config.private_key,
            config.public_key,
            config.preshared_key.unwrap()
        );
        assert!(parse_profile(&profile).is_ok());
        assert!(
            parse_profile(&profile.replace("Address=", "PostUp=arbitrary command\nAddress="))
                .is_err()
        );
        assert!(
            parse_profile(&profile.replace("PersistentKeepalive=25", "PersistentKeepalive=-1"))
                .is_err()
        );
        let value = json!({"type":"wireguard","private_key":config.private_key,"address":["192.0.2.2/32"],"peers":[{"public_key":config.public_key,"address":"vpn.example.invalid","port":51820,"allowed_ips":["0.0.0.0/0"]}]});
        assert!(from_singbox(&value).is_ok());
        let mut invalid = value;
        invalid["mtu"] = json!(-1);
        assert!(from_singbox(&invalid).is_err());
    }
    #[test]
    fn duplicate_parameters_and_nonstandard_reserved_bytes_fail_closed() {
        let node = node();
        let ProxyProtocol::Wireguard { config } = &node.protocol else {
            unreachable!()
        };
        let uri = share_uri(&node, config).replace("#", "&mtu=1280#");
        assert!(parse_uri(&uri).is_err());
        assert!(reject_reserved(&json!({"reserved":[1,0,0]})).is_err());
        assert!(reject_reserved(&json!({"reserved":[0,0,0]})).is_ok());
    }
}
