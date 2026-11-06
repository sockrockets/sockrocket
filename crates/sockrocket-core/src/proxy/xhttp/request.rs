use super::settings::{Range, Settings};
use anyhow::{Result, ensure};
use base64::Engine;
use bytes::Bytes;
use http::{HeaderMap, Request, Uri};
fn encoded(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}
fn cookie(headers: &mut HeaderMap, key: &str, value: &str) -> Result<()> {
    let old = headers
        .get("cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let value = if old.is_empty() {
        format!("{key}={value}")
    } else {
        format!("{old}; {key}={value}")
    };
    headers.insert(
        "cookie",
        value
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid XHTTP cookie"))?,
    );
    Ok(())
}
fn header(headers: &mut HeaderMap, key: &str, value: &str) -> Result<()> {
    let key: http::HeaderName = key
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid XHTTP header"))?;
    let value = value
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid XHTTP header"))?;
    headers.insert(key, value);
    Ok(())
}
fn metadata(
    path: &mut String,
    query: &mut Vec<(String, String)>,
    headers: &mut HeaderMap,
    place: &str,
    key: Option<&str>,
    default: &str,
    value: &str,
) -> Result<()> {
    if value.is_empty() {
        return Ok(());
    }
    let key = key.filter(|s| !s.is_empty()).unwrap_or(default);
    match place {
        "path" => {
            if !path.ends_with('/') {
                path.push('/');
            }
            path.push_str(&encoded(value));
        }
        "query" => {
            query.retain(|(k, _)| k != key);
            query.push((key.into(), value.into()));
        }
        "header" => header(headers, key, value)?,
        "cookie" => cookie(headers, key, value)?,
        _ => anyhow::bail!("Invalid XHTTP metadata placement"),
    }
    Ok(())
}
fn padding(length: usize, method: &str) -> String {
    if method != "tokenish" {
        return "X".repeat(length);
    }
    const TABLE: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    fn bits(c: u8) -> usize {
        match c {
            b'0'..=b'2' | b'a' | b'c' | b'e' | b'i' | b'o' | b's' | b't' => 5,
            b'3'..=b'9'
            | b'A'
            | b'b'
            | b'd'
            | b'f'
            | b'g'
            | b'h'
            | b'l'
            | b'm'
            | b'n'
            | b'p'
            | b'r'
            | b'u' => 6,
            b'X' | b'Z' => 8,
            _ => 7,
        }
    }
    let mut s = String::new();
    let mut n: usize = 0;
    while n.div_ceil(8) < length {
        let c = TABLE[rand::random::<u32>() as usize % TABLE.len()];
        s.push(c as char);
        n += bits(c);
    }
    s
}
fn make_uri(secure: bool, authority: &str, path: &str, query: &[(String, String)]) -> Result<Uri> {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(query.iter().map(|(a, b)| (a, b)))
        .finish();
    let target = if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{query}")
    };
    let authority = if authority.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{authority}]")
    } else {
        authority.to_string()
    };
    Uri::builder()
        .scheme(if secure { "https" } else { "http" })
        .authority(authority)
        .path_and_query(target)
        .build()
        .map_err(|_| anyhow::anyhow!("Invalid XHTTP request target"))
}
impl Settings {
    pub fn request(
        &self,
        host: &str,
        secure: bool,
        session: &str,
        sequence: Option<u64>,
        payload: Option<Bytes>,
        download: bool,
    ) -> Result<(Request<()>, Option<Bytes>)> {
        let o = &self.options;
        let original = self.config.path.as_deref().unwrap_or("/");
        let (path, raw_query) = original.split_once('?').unwrap_or((original, ""));
        // Go url.URL.Path treats percent signs and UTF-8 as literal path
        // characters. Encode them here rather than accidentally decoding /%2F.
        let mut normalized = String::new();
        if !path.starts_with('/') {
            normalized.push('/');
        }
        for b in path.bytes() {
            if b.is_ascii_alphanumeric() || b"/-._~!$&'()*+,;=:@".contains(&b) {
                normalized.push(b as char);
            } else {
                normalized.push_str(&format!("%{b:02X}"));
            }
        }
        let mut path = normalized;
        if (o.session_id_placement.as_deref().unwrap_or("path") == "path"
            || o.seq_placement.as_deref().unwrap_or("path") == "path")
            && !path.ends_with('/')
        {
            path.push('/');
        }
        let mut query: Vec<(String, String)> = url::form_urlencoded::parse(raw_query.as_bytes())
            .into_owned()
            .collect();
        let authority = self
            .config
            .host
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(host);
        let mut headers = HeaderMap::new();
        for (k, v) in &o.headers {
            if !k.eq_ignore_ascii_case("host") {
                header(&mut headers, k, v)?;
            }
        }
        let method = if download {
            "GET".to_string()
        } else {
            o.uplink_http_method
                .as_deref()
                .unwrap_or("POST")
                .to_ascii_uppercase()
        };
        let mut body = payload;
        if sequence.is_some() {
            let placement = o.uplink_data_placement.as_deref().unwrap_or("auto");
            if matches!(placement, "header" | "cookie") {
                let bytes = body.take().unwrap_or_default();
                let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
                let chunk = o
                    .uplink_chunk_size
                    .unwrap_or(if placement == "cookie" {
                        Range {
                            from: 2048,
                            to: 3072,
                        }
                    } else {
                        Range {
                            from: 3000,
                            to: 4000,
                        }
                    })
                    .sample() as usize;
                let key = o
                    .uplink_data_key
                    .as_deref()
                    .unwrap_or(if placement == "cookie" {
                        "x_data"
                    } else {
                        "X-Data"
                    });
                for (i, c) in encoded.as_bytes().chunks(chunk).enumerate() {
                    let value = std::str::from_utf8(c).unwrap();
                    let key = format!("{key}{}{i}", if placement == "cookie" { "_" } else { "-" });
                    if placement == "cookie" {
                        cookie(&mut headers, &key, value)?;
                    } else {
                        header(&mut headers, &key, value)?;
                    }
                }
            }
        } else if !download && !o.no_grpc_header {
            header(&mut headers, "content-type", "application/grpc")?;
        }
        let pad = padding(
            o.x_padding_bytes
                .filter(|r| r.to > 0)
                .unwrap_or(Range {
                    from: 100,
                    to: 1000,
                })
                .sample() as usize,
            o.x_padding_method.as_deref().unwrap_or("repeat-x"),
        );
        let place = if o.x_padding_obfs_mode {
            o.x_padding_placement.as_deref().unwrap_or("queryInHeader")
        } else {
            "queryInHeader"
        };
        let key = if o.x_padding_obfs_mode {
            o.x_padding_key.as_deref().unwrap_or("x_padding")
        } else {
            "x_padding"
        };
        let pad_header = if o.x_padding_obfs_mode {
            o.x_padding_header.as_deref().unwrap_or("X-Padding")
        } else {
            "Referer"
        };
        match place {
            "query" => {
                query.retain(|(k, _)| k != key);
                query.push((key.into(), pad));
            }
            "cookie" => cookie(&mut headers, key, &pad)?,
            "header" => header(&mut headers, pad_header, &pad)?,
            "queryInHeader" => {
                let reference = make_uri(secure, authority, &path, &[(key.into(), pad)])?;
                header(&mut headers, pad_header, &reference.to_string())?;
            }
            _ => anyhow::bail!("Invalid XHTTP padding placement"),
        }
        let place = o.session_id_placement.as_deref().unwrap_or("path");
        let default = if place == "header" {
            "X-Session"
        } else {
            "x_session"
        };
        metadata(
            &mut path,
            &mut query,
            &mut headers,
            place,
            o.session_id_key.as_deref(),
            default,
            session,
        )?;
        if let Some(sequence) = sequence {
            let place = o.seq_placement.as_deref().unwrap_or("path");
            let default = if place == "header" { "X-Seq" } else { "x_seq" };
            metadata(
                &mut path,
                &mut query,
                &mut headers,
                place,
                o.seq_key.as_deref(),
                default,
                &sequence.to_string(),
            )?;
        }
        ensure!(
            headers
                .iter()
                .map(|(k, v)| k.as_str().len() + v.len())
                .sum::<usize>()
                <= 128 * 1024,
            "XHTTP request headers exceed budget"
        );
        let mut request = Request::builder()
            .method(method.as_str())
            .uri(make_uri(secure, authority, &path, &query)?)
            .body(())
            .map_err(|_| anyhow::anyhow!("Invalid XHTTP request"))?;
        *request.headers_mut() = headers;
        Ok((request, body))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::model::XhttpConfig;
    #[test]
    fn placements_and_padding_preserve_original_query() {
        let mut c = XhttpConfig {
            path: Some("/a%2Fb?keep=yes".into()),
            ..Default::default()
        };
        c.options.insert("extra".into(),serde_json::json!({"sessionIDPlacement":"cookie","seqPlacement":"query","xPaddingObfsMode":true,"xPaddingPlacement":"header","xPaddingMethod":"tokenish","xPaddingBytes":128,"headers":{"x-test":"synthetic"}}));
        let s = Settings::parse(&c).unwrap();
        let (r, _) = s
            .request(
                "example.invalid",
                true,
                "abc",
                Some(7),
                Some(Bytes::from_static(b"body")),
                false,
            )
            .unwrap();
        assert_eq!(r.uri().path(), "/a%252Fb");
        assert!(r.uri().query().unwrap().contains("keep=yes"));
        assert!(r.uri().query().unwrap().contains("x_seq=7"));
        assert_eq!(r.headers()["cookie"], "x_session=abc");
        assert!(r.headers()["x-padding"].len() > 128);
    }
}
