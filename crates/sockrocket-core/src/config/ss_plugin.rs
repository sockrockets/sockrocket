//! Native Shadowsocks TCP plugin options in Mihomo, SIP002 and sing-box.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

pub(crate) const KEY: &str = "ss_tcp_plugin";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct Plugin {
    pub mode: String,
    pub host: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub tls: bool,
    #[serde(default)]
    pub skip_cert_verify: bool,
    #[serde(default)]
    pub mux: Option<bool>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

impl Plugin {
    pub fn name(&self) -> &'static str {
        if self.mode == "websocket" {
            "v2ray-plugin"
        } else {
            "obfs"
        }
    }

    pub fn parse(
        name: &str,
        mut opts: serde_json::Map<String, serde_json::Value>,
        server: &str,
    ) -> Result<Self> {
        let obfs = matches!(name, "obfs" | "simple-obfs" | "obfs-local");
        ensure!(obfs || name == "v2ray-plugin", "Unsupported SS plugin");
        for (alias, field) in [("obfs", "mode"), ("obfs-host", "host")] {
            if let Some(v) = opts.remove(alias) {
                ensure!(
                    obfs && opts.insert(field.to_owned(), v).is_none(),
                    "Conflicting SS plugin option"
                );
            }
        }
        opts.entry("mode")
            .or_insert(if obfs { "http" } else { "websocket" }.into());
        opts.entry("host")
            .or_insert(if obfs { "www.bing.com" } else { server }.into());
        for field in ["tls", "skip-cert-verify", "mux"] {
            if let Some(value) = opts.get_mut(field) {
                let enabled = match &*value {
                    serde_json::Value::Bool(b) => *b,
                    v => {
                        let text = v
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| v.to_string());
                        match text.as_str() {
                            "true" | "1" => true,
                            "false" | "0" => false,
                            _ if field == "mux" => text.parse::<u16>()? != 0,
                            _ => anyhow::bail!("Invalid SS plugin flag"),
                        }
                    }
                };
                *value = enabled.into();
            }
        }
        let plugin: Self = serde_json::from_value(opts.into())?;
        ensure!(
            if obfs {
                matches!(plugin.mode.as_str(), "http" | "tls")
                    && !plugin.tls
                    && !plugin.skip_cert_verify
                    && plugin.path.is_none()
                    && plugin.mux.is_none()
                    && plugin.headers.is_empty()
            } else {
                plugin.mode == "websocket"
            },
            "Unsupported SS plugin mode or options"
        );
        ensure!(
            !plugin.host.is_empty()
                && plugin.host.len() <= 253
                && !plugin.host.bytes().any(|c| c <= 32 || c == 127),
            "Invalid SS plugin host"
        );
        if let Some(path) = &plugin.path {
            ensure!(
                path.starts_with('/') && !path.bytes().any(|c| c <= 32 || c == 127),
                "Invalid SS WebSocket path"
            );
        }
        for (k, v) in &plugin.headers {
            http::HeaderName::try_from(k.as_str())?;
            http::HeaderValue::try_from(v.as_str())?;
        }
        Ok(plugin)
    }

    pub fn save(&self, extra: &mut HashMap<String, String>) -> Result<()> {
        extra.insert(KEY.to_owned(), serde_json::to_string(self)?);
        Ok(())
    }
    pub fn load(extra: &HashMap<String, String>) -> Result<Option<Self>> {
        extra
            .get(KEY)
            .map(|s| serde_json::from_str(s).context("Invalid SS plugin settings"))
            .transpose()
    }
    pub fn options(&self) -> serde_json::Map<String, serde_json::Value> {
        let mut opts = serde_json::Map::new();
        opts.insert("mode".into(), self.mode.clone().into());
        opts.insert("host".into(), self.host.clone().into());
        if let Some(v) = &self.path {
            opts.insert("path".into(), v.clone().into());
        }
        if self.tls {
            opts.insert("tls".into(), true.into());
        }
        if self.skip_cert_verify {
            opts.insert("skip-cert-verify".into(), true.into());
        }
        if let Some(v) = self.mux {
            opts.insert("mux".into(), v.into());
        }
        if !self.headers.is_empty() {
            opts.insert("headers".into(), serde_json::json!(self.headers));
        }
        opts
    }
}
