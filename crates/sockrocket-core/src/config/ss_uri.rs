use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose};
use std::collections::HashMap;

use super::model::{Node, ProxyProtocol, ShadowTlsConfig};

fn decode_base64(value: &str) -> Result<String> {
    for encoding in [
        general_purpose::URL_SAFE_NO_PAD,
        general_purpose::URL_SAFE,
        general_purpose::STANDARD_NO_PAD,
        general_purpose::STANDARD,
    ] {
        if let Ok(bytes) = encoding.decode(value.trim()) {
            return String::from_utf8(bytes).context("Invalid SS credential encoding");
        }
    }
    anyhow::bail!("Invalid SS base64")
}

fn parse_plugin(value: &str) -> Result<ShadowTlsConfig> {
    let (name, options) = value.split_once(';').unwrap_or((value, ""));
    ensure!(name == "shadow-tls", "Unsupported SS plugin");
    let mut chars = options.chars();
    let mut fields = HashMap::new();
    while !chars.as_str().is_empty() {
        let mut key = None;
        let mut field = String::new();
        while let Some(c) = chars.next() {
            match c {
                '\\' => field.push(chars.next().context("Incomplete SS plugin escape")?),
                ';' => break,
                '=' if key.is_none() => key = Some(std::mem::take(&mut field)),
                _ => field.push(c),
            }
        }
        let key = key.context("Missing shadow-tls option value")?;
        ensure!(
            fields.insert(key, field).is_none(),
            "Duplicate shadow-tls option"
        );
    }
    if let Some(version) = fields.remove("version") {
        ensure!(version == "3", "Only shadow-tls v3 is supported");
    }
    let host = fields
        .remove("host")
        .filter(|v| !v.is_empty())
        .context("Missing shadow-tls host")?;
    let password = fields
        .remove("password")
        .filter(|v| !v.is_empty())
        .context("Missing shadow-tls password")?;
    let skip_cert_verify = fields
        .remove("skip-cert-verify")
        .map(|v| v.parse())
        .transpose()
        .context("Invalid shadow-tls skip-cert-verify")?
        .unwrap_or(false);
    ensure!(fields.is_empty(), "Unsupported shadow-tls option");
    Ok(ShadowTlsConfig {
        host,
        password,
        skip_cert_verify,
    })
}

pub fn parse_ss_uri(uri: &str) -> Result<Node> {
    let body = uri.strip_prefix("ss://").context("Not an ss:// URI")?;
    let (body, label) = body
        .split_once('#')
        .map_or((body, None), |(s, n)| (s, Some(n)));
    let (authority, query) = body.split_once('?').unwrap_or((body, ""));
    let (cipher, password, endpoint) =
        if let Some((userinfo, endpoint)) = authority.rsplit_once('@') {
            let (cipher, password) = match userinfo.split_once(':') {
                Some((method, password)) => (
                    urlencoding::decode(method)?.into_owned(),
                    urlencoding::decode(password)?.into_owned(),
                ),
                None => {
                    let credentials = decode_base64(&urlencoding::decode(userinfo)?)?;
                    let (method, password) = credentials
                        .split_once(':')
                        .context("Invalid SS credentials")?;
                    (method.to_owned(), password.to_owned())
                }
            };
            (cipher, password, endpoint.to_owned())
        } else {
            let legacy = decode_base64(authority)?;
            let (credentials, endpoint) = legacy
                .rsplit_once('@')
                .context("Invalid legacy SS endpoint")?;
            let (method, password) = credentials
                .split_once(':')
                .context("Invalid legacy SS credentials")?;
            (method.to_owned(), password.to_owned(), endpoint.to_owned())
        };
    let addr = url::Url::parse(&format!("scheme://{endpoint}")).context("Invalid SS endpoint")?;
    ensure!(
        addr.username().is_empty() && addr.password().is_none() && matches!(addr.path(), "" | "/"),
        "Invalid SS endpoint"
    );
    let server = addr
        .host_str()
        .context("Missing SS host")?
        .trim_matches(['[', ']'])
        .to_owned();
    let mut shadow_tls = None;
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        if key == "plugin" {
            ensure!(shadow_tls.is_none(), "Duplicate SS plugin");
            shadow_tls = Some(parse_plugin(&value)?);
        }
    }
    Ok(Node {
        name: label
            .map(super::model::decode_display_name)
            .unwrap_or_else(|| format!("SS-{server}")),
        server,
        port: addr.port().unwrap_or(8388),
        protocol: ProxyProtocol::Shadowsocks {
            cipher,
            password,
            udp: true,
            shadow_tls,
        },
        transport: None,
        latency_ms: None,
        tags: vec![],
        extra: HashMap::new(),
    })
}

fn escape_option(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for c in value.chars() {
        if matches!(c, '\\' | ';' | '=' | ':') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

pub(super) fn share_uri(
    node: &Node,
    cipher: &str,
    password: &str,
    shadow: Option<&ShadowTlsConfig>,
    name: &str,
) -> String {
    let userinfo = if cipher.starts_with("2022-") {
        format!(
            "{}:{}",
            urlencoding::encode(cipher),
            urlencoding::encode(password)
        )
    } else {
        general_purpose::URL_SAFE_NO_PAD.encode(format!("{cipher}:{password}"))
    };
    let endpoint = crate::proxy::transport::format_host_port(&node.server, node.port);
    let mut uri = format!("ss://{userinfo}@{endpoint}");
    if let Some(shadow) = shadow {
        let mut plugin = format!(
            "shadow-tls;version=3;host={};password={}",
            escape_option(&shadow.host),
            escape_option(&shadow.password)
        );
        if shadow.skip_cert_verify {
            plugin.push_str(";skip-cert-verify=true");
        }
        uri.push_str("/?plugin=");
        uri.push_str(&urlencoding::encode(&plugin));
    }
    uri.push('#');
    uri.push_str(name);
    uri
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::v2ray::node_to_share_uri;
    use shadowsocks::ServerConfig;

    #[test]
    fn sip002_shadow_tls_and_2022_interoperate_with_shadowsocks_rust() {
        let password = general_purpose::STANDARD.encode([0xfb; 32]);
        let mut node = parse_ss_uri(&format!(
            "ss://2022-blake3-aes-256-gcm:{}@[2001:db8::2]:443#fixture",
            urlencoding::encode(&password)
        ))
        .unwrap();
        let ProxyProtocol::Shadowsocks { shadow_tls, .. } = &mut node.protocol else {
            unreachable!()
        };
        *shadow_tls = Some(ShadowTlsConfig {
            host: "front.example.invalid".into(),
            password: r"a;:b=c\d+%".into(),
            skip_cert_verify: true,
        });
        let url = node_to_share_uri(&node);
        let external = ServerConfig::from_url(&url).unwrap();
        assert_eq!(external.password(), password);
        let plugin = external.plugin().unwrap();
        assert_eq!(plugin.plugin, "shadow-tls");
        assert!(
            plugin
                .plugin_opts
                .as_deref()
                .unwrap()
                .contains(r"password=a\;\:b\=c\\d+%")
        );
        let restored = parse_ss_uri(&external.to_url()).unwrap();
        assert_eq!(restored.server, "2001:db8::2");
        assert_eq!(node_to_share_uri(&restored), url);
    }

    #[test]
    fn legacy_and_url_safe_credentials_preserve_delimiters() {
        let password = "???>@x";
        for engine in [general_purpose::STANDARD, general_purpose::URL_SAFE_NO_PAD] {
            let user = engine.encode(format!("aes-128-gcm:{password}"));
            let legacy = engine.encode(format!("aes-128-gcm:{password}@localhost:8388"));
            for body in [format!("{user}@localhost:8388"), legacy] {
                let node = parse_ss_uri(&format!("ss://{body}")).unwrap();
                let external = ServerConfig::from_url(&node_to_share_uri(&node)).unwrap();
                assert_eq!(external.password(), password);
            }
        }
    }

    #[test]
    fn unsupported_or_malformed_plugin_never_becomes_plain_ss() {
        for query in [
            "plugin=unrecognized",
            "plugin=shadow-tls%3Bhost%3Dfoo",
            "plugin=shadow-tls%3Bhost%3Dfoo%3Bpassword%3Dbar%3Bversion%3D2",
            "plugin=shadow-tls%3Bhost%3Dfoo%3Bpassword%3Dbar&plugin=unrecognized",
        ] {
            assert!(
                parse_ss_uri(&format!(
                    "ss://aes-128-gcm:test@example.invalid:8388/?{query}"
                ))
                .is_err()
            );
        }
    }
}
