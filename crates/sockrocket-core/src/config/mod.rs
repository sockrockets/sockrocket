pub(crate) mod bandwidth;
pub mod clash;
pub mod dedup;
pub mod dns;
pub mod export;
pub mod group;
pub mod model;
pub mod node_identity;
pub mod rule_groups;
pub mod rule_list;
pub mod rule_scenes;
pub mod rules_batch;
pub mod singbox;
pub(crate) mod ss_plugin;
mod ss_uri;
pub mod subscription;
pub mod v2ray;
pub mod watch;

pub mod storage;

pub mod wireguard;

pub mod server_dns;
pub(crate) mod xhttp;

/// Importers accept either Base64 alphabet, with or without canonical padding.
fn decode_base64(encoded: &[u8]) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::{Engine, engine::general_purpose};
    let url_safe = encoded.iter().any(|&b| matches!(b, b'-' | b'_'));
    let padded = encoded.last() == Some(&b'=');
    let engine = match (url_safe, padded) {
        (false, false) => &general_purpose::STANDARD_NO_PAD,
        (false, true) => &general_purpose::STANDARD,
        (true, false) => &general_purpose::URL_SAFE_NO_PAD,
        (true, true) => &general_purpose::URL_SAFE,
    };
    engine.decode(encoded)
}
