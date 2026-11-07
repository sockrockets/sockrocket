//! Read-only downloads for importing a public rule list into one selected group.
use std::{net::SocketAddr, time::Duration};

use anyhow::{Context, Result, ensure};

use super::rules_batch::{format_rule_clash_line, parse_batch_rules_in_group};

// Stay below the 1 MiB JSON request limit when the preview is submitted later.
pub const MAX_RULE_LIST_BYTES: usize = 256 * 1024;
pub const MAX_RULE_LIST_RULES: usize = 4096;
pub const RULE_LIST_TIMEOUT_SECS: u64 = 30;

#[derive(Debug)]
pub struct RuleListPreview {
    pub text: String,
    pub count: usize,
}

pub fn validate_rule_list_url(input: &str) -> Result<()> {
    ensure!(input.len() <= 4096, "规则链接过长");
    super::subscription::validate_subscription_url(input)
        .map_err(|_| anyhow::anyhow!("请填写有效的 HTTP 或 HTTPS 规则链接"))?;
    let url = url::Url::parse(input)?;
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "规则链接不能包含用户名或密码"
    );
    Ok(())
}

/// Produce explicit actions and remove group directives. Remote text must not
/// redirect an import to another group. Nothing is persisted here.
pub fn preview_rule_list(text: &str, target: &str, group: &str) -> Result<RuleListPreview> {
    ensure!(
        text.len() <= MAX_RULE_LIST_BYTES,
        "规则列表超过 256 KiB 限制"
    );
    let text = text.trim_start_matches('\u{feff}');
    ensure!(
        text.lines()
            .filter(|line| {
                let line = line.trim();
                !line.is_empty() && !line.starts_with('#')
            })
            .take(MAX_RULE_LIST_RULES + 1)
            .count()
            <= MAX_RULE_LIST_RULES,
        "一次最多导入 4096 条规则"
    );
    let parsed = parse_batch_rules_in_group(text, target, group);
    if let Some(error) = parsed.errors.first() {
        anyhow::bail!(
            "第 {} 行无法解析：{}；未导入任何规则",
            error.line,
            error.message
        );
    }
    ensure!(!parsed.rules.is_empty(), "链接中没有可导入的规则");
    ensure!(
        parsed.rules.len() <= MAX_RULE_LIST_RULES,
        "一次最多导入 4096 条规则"
    );
    let text = parsed
        .rules
        .iter()
        .map(format_rule_clash_line)
        .collect::<Vec<_>>()
        .join("\n");
    ensure!(
        text.len() <= MAX_RULE_LIST_BYTES,
        "展开后的规则列表超过 256 KiB 限制"
    );
    Ok(RuleListPreview {
        text,
        count: parsed.rules.len(),
    })
}

pub async fn fetch_rule_list(
    url: &str,
    target: &str,
    group: &str,
    socks: Option<SocketAddr>,
) -> Result<RuleListPreview> {
    validate_rule_list_url(url)?;
    let bytes = tokio::time::timeout(Duration::from_secs(RULE_LIST_TIMEOUT_SECS), async {
        match download(url, None).await {
            Ok(bytes) => Ok(bytes),
            Err(direct) => match socks {
                Some(address) => download(url, Some(address)).await,
                None => Err(direct),
            },
        }
    })
    .await
    .context("读取规则超时，请稍后重试")??;
    let text = String::from_utf8(bytes).context("规则列表须为 UTF-8 文本")?;
    preview_rule_list(&text, target, group)
}

async fn download(url: &str, socks: Option<SocketAddr>) -> Result<Vec<u8>> {
    // Reuse router DNS handling, verified TLS, bounded redirects and SOCKS support.
    let client = super::subscription::subscription_client(socks, true)?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|e| e.without_url())
        .context("读取规则链接失败")?;
    ensure!(
        response.status().is_success(),
        "规则链接返回 HTTP {}",
        response.status()
    );
    let html = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|s| s.to_ascii_lowercase().contains("text/html"));
    ensure!(!html, "链接返回的是网页，请使用原始规则文本链接");
    ensure!(
        response
            .content_length()
            .is_none_or(|n| n <= MAX_RULE_LIST_BYTES as u64),
        "规则列表超过 256 KiB 限制"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| e.without_url())
        .context("读取规则内容失败")?
    {
        ensure!(
            chunk.len() <= MAX_RULE_LIST_BYTES.saturating_sub(bytes.len()),
            "规则列表超过 256 KiB 限制"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_preview_keeps_actions_but_cannot_switch_groups() {
        let result = preview_rule_list("\u{feff}# > service\nDOMAIN,example.invalid\n#DOMAIN,disabled.invalid\n# @group Other\nGEOIP,CN,DIRECT\nDOMAIN,example.invalid", "group:Japan", "Emby").unwrap();
        assert_eq!(result.count, 3);
        assert_eq!(
            result.text,
            "DOMAIN,example.invalid,group:Japan\nGEOIP,CN,DIRECT\nDOMAIN,example.invalid,group:Japan"
        );
        let reparsed = parse_batch_rules_in_group(&result.text, "proxy", "Emby");
        assert!(reparsed.rules.iter().all(|r| r.group == "Emby"));
    }

    #[test]
    fn invalid_empty_and_oversized_lists_are_atomic() {
        for text in [
            "# comment\n",
            "DOMAIN,ok.invalid\nNOT-A-TYPE,bad",
            "<html>error</html>",
        ] {
            assert!(preview_rule_list(text, "proxy", "Emby").is_err());
        }
        assert!(preview_rule_list(&"#".repeat(MAX_RULE_LIST_BYTES + 1), "proxy", "Emby").is_err());
        assert!(
            preview_rule_list(
                &"DOMAIN,x.invalid\n".repeat(MAX_RULE_LIST_RULES + 1),
                "proxy",
                "Emby"
            )
            .is_err()
        );
        for url in [
            "file:///etc/passwd",
            "https://",
            "https://user:secret@example.invalid/list",
            "https://example.invalid/white space",
        ] {
            assert!(validate_rule_list_url(url).is_err());
        }
    }

    async fn response_server(response: Vec<u8>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            let _ = stream.read(&mut request).await;
            let _ = stream.write_all(&response).await;
        });
        format!("http://{address}/list")
    }

    #[tokio::test]
    async fn http_lists_and_failures_are_bounded() {
        let body = "# comment\nDOMAIN,example.invalid\nGEOIP,CN,DIRECT";
        let url = response_server(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .into_bytes(),
        )
        .await;
        let preview = fetch_rule_list(&url, "group:Japan", "Emby", None)
            .await
            .unwrap();
        assert_eq!(preview.count, 2);
        assert!(preview.text.contains("group:Japan"));
        for response in [
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 0\r\n\r\n".to_vec(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 9999999\r\n\r\n".to_vec(),
            [
                b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".as_slice(),
                &vec![b'#'; MAX_RULE_LIST_BYTES + 1],
            ]
            .concat(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\n\r\n\xff".to_vec(),
        ] {
            let url = response_server(response).await;
            assert!(fetch_rule_list(&url, "proxy", "Emby", None).await.is_err());
        }
    }
}
