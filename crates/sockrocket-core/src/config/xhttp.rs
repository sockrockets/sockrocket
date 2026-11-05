use crate::config::model::XhttpConfig;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Range {
    pub from: u32,
    pub to: u32,
}
impl Range {
    pub fn fixed(n: u32) -> Self {
        Self { from: n, to: n }
    }
    pub fn sample(self) -> u32 {
        self.from + rand::random::<u32>() % (self.to - self.from + 1)
    }
}
impl<'de> Deserialize<'de> for Range {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        let parse = || -> Option<Self> {
            match value {
                serde_json::Value::Number(n) => {
                    let n = u32::try_from(n.as_u64()?).ok()?;
                    Some(Self::fixed(n))
                }
                serde_json::Value::String(s) => {
                    let (a, b) = s.split_once('-').unwrap_or((&s, &s));
                    Some(Self {
                        from: a.trim().parse().ok()?,
                        to: b.trim().parse().ok()?,
                    })
                }
                serde_json::Value::Object(m) => Some(Self {
                    from: u32::try_from(m.get("from")?.as_u64()?).ok()?,
                    to: u32::try_from(m.get("to")?.as_u64()?).ok()?,
                }),
                _ => None,
            }
        };
        parse()
            .filter(|r| r.from <= r.to && r.to < u32::MAX)
            .ok_or_else(|| serde::de::Error::custom("Invalid XHTTP range"))
    }
}
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Xmux {
    pub max_concurrency: Option<Range>,
    pub max_connections: Option<Range>,
    pub c_max_reuse_times: Option<Range>,
    pub h_max_request_times: Option<Range>,
    pub h_max_reusable_secs: Option<Range>,
    pub h_keep_alive_period: Option<i64>,
}
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Options {
    pub headers: BTreeMap<String, String>,
    pub x_padding_bytes: Option<Range>,
    #[serde(rename = "noGRPCHeader")]
    pub no_grpc_header: bool,
    #[serde(rename = "noSSEHeader")]
    pub no_sse_header: bool,
    pub sc_max_each_post_bytes: Option<Range>,
    pub sc_min_posts_interval_ms: Option<Range>,
    pub sc_max_buffered_posts: Option<u32>,
    pub sc_stream_up_server_secs: Option<Range>,
    pub xmux: Xmux,
    pub download_settings: Option<serde_json::Value>,
    pub x_padding_obfs_mode: bool,
    pub x_padding_key: Option<String>,
    pub x_padding_header: Option<String>,
    pub x_padding_placement: Option<String>,
    pub x_padding_method: Option<String>,
    #[serde(rename = "uplinkHTTPMethod")]
    pub uplink_http_method: Option<String>,
    #[serde(rename = "sessionIDPlacement")]
    pub session_id_placement: Option<String>,
    #[serde(rename = "sessionIDKey")]
    pub session_id_key: Option<String>,
    #[serde(rename = "sessionIDTable")]
    pub session_id_table: Option<String>,
    #[serde(rename = "sessionIDLength")]
    pub session_id_length: Option<Range>,
    pub seq_placement: Option<String>,
    pub seq_key: Option<String>,
    pub uplink_data_placement: Option<String>,
    pub uplink_data_key: Option<String>,
    pub uplink_chunk_size: Option<Range>,
    pub server_max_header_bytes: Option<u32>,
    pub h2_params: Option<serde_json::Value>,
}
#[derive(Debug, Clone)]
pub struct Settings {
    pub config: XhttpConfig,
    pub options: Options,
}
impl Settings {
    pub fn parse(config: &XhttpConfig) -> Result<Self> {
        ensure!(
            matches!(
                config.mode.as_str(),
                "" | "auto" | "stream-one" | "stream-up" | "packet-up"
            ),
            "Invalid XHTTP mode"
        );
        let mut values = serde_json::Map::from_iter(config.options.clone());
        if let Some(extra) = values.remove("extra") {
            let extra = extra
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("XHTTP extra must be an object"))?;
            values = extra.clone();
            // Host/path/mode always belong to the outer XHTTP object.
            for key in ["path", "host", "mode"] {
                values.remove(key);
            }
            ensure!(
                !values.contains_key("extra"),
                "Nested XHTTP extra options are not allowed"
            );
        }
        let mut options: Options = serde_json::from_value(values.into())
            .map_err(|_| anyhow::anyhow!("Invalid or unknown XHTTP options"))?;
        for r in [
            &mut options.xmux.max_concurrency,
            &mut options.xmux.max_connections,
            &mut options.xmux.c_max_reuse_times,
            &mut options.xmux.h_max_request_times,
            &mut options.xmux.h_max_reusable_secs,
        ] {
            if r.is_some_and(|r| r.to == 0) {
                *r = None;
            }
        }
        ensure!(
            options.xmux.h_keep_alive_period.is_none_or(|v| v <= 86400),
            "XHTTP keepalive interval exceeds budget"
        );
        ensure!(
            options
                .sc_min_posts_interval_ms
                .is_none_or(|r| r.to <= 60000),
            "XHTTP upload pacing exceeds budget"
        );
        for text in [
            &mut options.x_padding_key,
            &mut options.x_padding_header,
            &mut options.x_padding_placement,
            &mut options.x_padding_method,
            &mut options.uplink_http_method,
            &mut options.session_id_placement,
            &mut options.session_id_key,
            &mut options.session_id_table,
            &mut options.seq_placement,
            &mut options.seq_key,
            &mut options.uplink_data_placement,
            &mut options.uplink_data_key,
        ] {
            if text.as_ref().is_some_and(|s| s.is_empty()) {
                *text = None;
            }
        }
        let x = &mut options.xmux;
        if x.max_connections.is_none()
            && x.max_concurrency.is_none()
            && x.c_max_reuse_times.is_none()
            && x.h_max_request_times.is_none()
            && x.h_max_reusable_secs.is_none()
            && x.h_keep_alive_period.unwrap_or(0) == 0
        {
            x.max_connections = Some(Range::fixed(3));
            x.h_max_request_times = Some(Range { from: 600, to: 900 });
            x.h_max_reusable_secs = Some(Range {
                from: 1800,
                to: 3000,
            });
        }
        let path = config.path.as_deref().unwrap_or("/");
        ensure!(
            path.len() <= 4096 && !path.contains(['\r', '\n', '#']),
            "Invalid XHTTP path"
        );
        if let Some(host) = config.host.as_ref().filter(|s| !s.is_empty()) {
            let normalized = if host.parse::<std::net::Ipv6Addr>().is_ok() {
                format!("[{host}]")
            } else {
                host.clone()
            };
            let _: http::uri::Authority = normalized
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid XHTTP host"))?;
            ensure!(!host.contains('@'), "Invalid XHTTP host");
        }
        for (key, value) in &options.headers {
            let _: http::HeaderName = key
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid XHTTP header"))?;
            let _: http::HeaderValue = value
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid XHTTP header"))?;
            ensure!(
                !matches!(
                    key.to_ascii_lowercase().as_str(),
                    "host"
                        | "connection"
                        | "content-length"
                        | "transfer-encoding"
                        | "upgrade"
                        | "proxy-connection"
                ),
                "XHTTP header conflicts with streaming framing"
            );
        }
        ensure!(
            options
                .headers
                .iter()
                .map(|(k, v)| k.len() + v.len())
                .sum::<usize>()
                <= 16384,
            "XHTTP headers exceed budget"
        );
        for placement in [&options.session_id_placement, &options.seq_placement] {
            ensure!(
                matches!(
                    placement.as_deref().unwrap_or("path"),
                    "path" | "query" | "header" | "cookie"
                ),
                "Invalid XHTTP metadata placement"
            );
        }
        ensure!(
            matches!(
                options
                    .x_padding_placement
                    .as_deref()
                    .unwrap_or("queryInHeader"),
                "queryInHeader" | "query" | "header" | "cookie"
            ),
            "Invalid XHTTP padding placement"
        );
        ensure!(
            matches!(
                options.x_padding_method.as_deref().unwrap_or("repeat-x"),
                "repeat-x" | "tokenish"
            ),
            "Invalid XHTTP padding method"
        );
        ensure!(
            matches!(
                options.uplink_data_placement.as_deref().unwrap_or("auto"),
                "auto" | "body" | "header" | "cookie"
            ),
            "Invalid XHTTP payload placement"
        );
        if matches!(
            options.uplink_data_placement.as_deref(),
            Some("header" | "cookie")
        ) {
            ensure!(
                config.mode == "packet-up",
                "Header/cookie payload requires packet-up"
            );
        }
        let method = options
            .uplink_http_method
            .as_deref()
            .unwrap_or("POST")
            .to_ascii_uppercase();
        let _: http::Method = method
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid XHTTP method"))?;
        ensure!(
            !matches!(method.as_str(), "CONNECT" | "TRACE"),
            "Invalid XHTTP streaming method"
        );
        ensure!(
            method != "GET" || config.mode == "packet-up",
            "GET upload requires packet-up"
        );
        ensure!(
            options
                .x_padding_bytes
                .is_none_or(|r| r.to == 0 || (r.from > 0 && r.to <= 16384)),
            "XHTTP padding exceeds budget"
        );
        ensure!(
            options
                .sc_max_each_post_bytes
                .is_none_or(|r| r.from > 0 && r.to <= 4 * 1024 * 1024),
            "XHTTP post size exceeds budget"
        );
        ensure!(
            options
                .uplink_chunk_size
                .is_none_or(|r| r.from >= 64 && r.to <= 16384),
            "XHTTP header chunk exceeds budget"
        );
        ensure!(
            options
                .session_id_length
                .is_none_or(|r| r.from > 0 && r.to <= 256),
            "Invalid XHTTP session ID length"
        );
        if let Some(table) = &options.session_id_table {
            ensure!(
                !table.is_empty()
                    && table.len() <= 256
                    && table
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
                "Invalid XHTTP session ID table"
            );
            let table = predefined_table(table);
            let length = options
                .session_id_length
                .ok_or_else(|| anyhow::anyhow!("XHTTP custom session table needs a length"))?;
            let alphabet = table
                .bytes()
                .collect::<std::collections::HashSet<_>>()
                .len() as u64;
            let mut room = 0u64;
            let mut power = 1u64;
            for n in 0..=length.to {
                if n >= length.from {
                    room = room.saturating_add(power);
                }
                power = power.saturating_mul(alphabet);
            }
            ensure!(room >= (1u64 << 31), "XHTTP session ID space is too small");
        }
        for key in [
            &options.session_id_key,
            &options.seq_key,
            &options.uplink_data_key,
            &options.x_padding_key,
            &options.x_padding_header,
        ] {
            if let Some(key) = key {
                ensure!(
                    !key.is_empty()
                        && key.len() <= 128
                        && key
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)),
                    "Invalid XHTTP metadata key"
                );
            }
        }
        ensure!(
            options.xmux.max_concurrency.is_none() || options.xmux.max_connections.is_none(),
            "XHTTP xmux concurrency and connection limits are exclusive"
        );
        ensure!(
            options
                .xmux
                .max_connections
                .is_none_or(|r| r.from > 0 && r.to <= 32),
            "XHTTP connection count exceeds budget"
        );
        ensure!(
            options
                .xmux
                .max_concurrency
                .is_none_or(|r| r.from > 0 && r.to <= 256),
            "XHTTP concurrency exceeds budget"
        );
        if let Some(d) = &options.download_settings {
            let d = d
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("Invalid XHTTP download settings"))?;
            ensure!(
                d.keys().all(|k| matches!(
                    k.as_str(),
                    "address"
                        | "port"
                        | "network"
                        | "security"
                        | "tlsSettings"
                        | "realitySettings"
                        | "xhttpSettings"
                        | "splithttpSettings"
                )),
                "Unsupported XHTTP download transport setting"
            );
            for (key, allowed) in [
                (
                    "tlsSettings",
                    &["serverName", "allowInsecure", "alpn", "fingerprint"][..],
                ),
                (
                    "realitySettings",
                    &["serverName", "publicKey", "shortId", "fingerprint"][..],
                ),
            ] {
                if let Some(value) = d.get(key) {
                    let value = value
                        .as_object()
                        .ok_or_else(|| anyhow::anyhow!("Invalid XHTTP download TLS setting"))?;
                    ensure!(
                        value.keys().all(|k| allowed.contains(&k.as_str())),
                        "Unsupported XHTTP download TLS setting"
                    );
                }
            }
            if let Some(config) = d
                .get("xhttpSettings")
                .or_else(|| d.get("splithttpSettings"))
            {
                ensure!(
                    !has_nested_download(config),
                    "Nested XHTTP download settings are not allowed"
                );
                let config: XhttpConfig = serde_json::from_value(config.clone())
                    .map_err(|_| anyhow::anyhow!("Invalid XHTTP download options"))?;
                Self::parse(&config)?;
            }
        }
        if let Some(h2) = &options.h2_params {
            ensure!(h2.is_object(), "Invalid XHTTP h2Params");
        }
        Ok(Self {
            config: config.clone(),
            options,
        })
    }
    pub fn mode(&self, reality: bool) -> &str {
        match self.config.mode.as_str() {
            "" | "auto" => {
                if reality {
                    if self.options.download_settings.is_some() {
                        "stream-up"
                    } else {
                        "stream-one"
                    }
                } else {
                    "packet-up"
                }
            }
            s => s,
        }
    }
    pub fn session(&self) -> String {
        if let (Some(r), Some(table)) = (
            self.options.session_id_length,
            self.options.session_id_table.as_deref(),
        ) {
            let table = predefined_table(table).as_bytes();
            (0..r.sample())
                .map(|_| table[rand::random::<u32>() as usize % table.len()] as char)
                .collect()
        } else {
            uuid::Uuid::new_v4().to_string()
        }
    }
}

fn predefined_table(s: &str) -> &str {
    match s {
        "ALPHABET" => "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        "Alphabet" => "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
        "BASE36" => "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        "Base62" => "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
        "HEX" => "0123456789ABCDEF",
        "alphabet" => "abcdefghijklmnopqrstuvwxyz",
        "base36" => "0123456789abcdefghijklmnopqrstuvwxyz",
        "hex" => "0123456789abcdef",
        "number" => "0123456789",
        s => s,
    }
}

fn has_nested_download(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Object(m) => m
            .iter()
            .any(|(k, v)| k == "downloadSettings" || has_nested_download(v)),
        serde_json::Value::Array(a) => a.iter().any(has_nested_download),
        _ => false,
    }
}
