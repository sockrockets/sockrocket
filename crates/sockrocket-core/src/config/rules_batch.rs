//! Batch import/export for routing rules.
//!
//! Accepted line formats (blank lines and `#` comments ignored):
//! - Plain domain → `domain-suffix` (e.g. `example.com`)
//! - Plain IPv4 / CIDR → `ip-cidr` (bare IP becomes `/32`)
//! - Clash-style: `DOMAIN-SUFFIX,example.com,PROXY`
//! - Explicit: `domain-suffix example.com proxy` or `domain-suffix:example.com->proxy`
//! - Port: `dst-port 443 proxy` / `PORT,443,DIRECT`
//! - Final: `MATCH,PROXY` / `final proxy`

use crate::config::model::RoutingRule;

/// How to handle an incoming rule that already exists (same type + pattern).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BatchConflict {
    /// Keep the existing rule; count as skipped.
    #[default]
    Skip,
    /// Overwrite target / enabled / priority on the existing entry.
    Replace,
}

/// One failed line from a batch parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchLineError {
    /// 1-based source line number.
    pub line: usize,
    pub message: String,
}

/// Result of parsing a batch text block.
#[derive(Debug, Clone, Default)]
pub struct BatchParseResult {
    pub rules: Vec<RoutingRule>,
    pub errors: Vec<BatchLineError>,
}

/// Stats from merging parsed rules into an existing list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BatchMergeStats {
    pub added: usize,
    pub replaced: usize,
    pub skipped: usize,
}

/// Parse multi-line rule text into [`RoutingRule`]s.
///
/// `default_target` is used when a line does not specify an action.
/// `default_group` is assigned when a rule has no group yet; lines of the form
/// `# @group Name` switch the active group for following lines.
pub fn parse_batch_rules(text: &str, default_target: &str) -> BatchParseResult {
    parse_batch_rules_in_group(text, default_target, "")
}

/// Like [`parse_batch_rules`] with an initial group for lines before any
/// `# @group` directive.
pub fn parse_batch_rules_in_group(
    text: &str,
    default_target: &str,
    default_group: &str,
) -> BatchParseResult {
    let default_target = normalize_target(default_target).unwrap_or("proxy");
    let mut out = BatchParseResult::default();
    let mut active_group = default_group.trim().to_string();
    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(g) = parse_group_directive(trimmed) {
            active_group = g;
            continue;
        }
        let (line, name) = split_line_and_name(raw);
        if line.is_empty() {
            continue;
        }
        match parse_one_line(line, default_target) {
            Ok(mut rule) => {
                if !name.is_empty() {
                    rule.name = name;
                }
                if rule.group.is_empty() && !active_group.is_empty() {
                    rule.group = active_group.clone();
                }
                out.rules.push(rule);
            }
            Err(message) => out.errors.push(BatchLineError {
                line: line_no,
                message,
            }),
        }
    }
    out
}

/// Build one rule of a fixed type (used by single-add UI).
pub fn make_typed_rule(
    rule_type: &str,
    pattern: &str,
    target: &str,
    group: &str,
) -> Result<RoutingRule, String> {
    let target = normalize_target(target).unwrap_or("proxy");
    let typ = normalize_type(rule_type);
    let mut rule = build_rule(typ, pattern.trim(), target)?;
    rule.group = group.trim().to_string();
    Ok(rule)
}

/// Whether this rule type accepts many patterns pasted at once (one per line).
pub fn rule_type_supports_bulk(rule_type: &str) -> bool {
    matches!(
        rule_type,
        "domain" | "domain-suffix" | "domain-keyword" | "ip-cidr" | "geoip"
    )
}

/// Parse a paste block as many patterns of a **fixed** `rule_type`.
///
/// Each non-empty, non-comment line becomes one rule. Comma-separated tokens
/// on the same line are also expanded (handy for domain lists).
pub fn parse_typed_patterns(
    text: &str,
    rule_type: &str,
    target: &str,
    group: &str,
) -> BatchParseResult {
    let target = normalize_target(target).unwrap_or("proxy");
    let typ = normalize_type(rule_type);
    let group = group.trim().to_string();
    let mut out = BatchParseResult::default();
    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Strip trailing "# comment" that is not part of the pattern.
        let (line, _) = split_line_and_name(raw);
        if line.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = if matches!(
            typ,
            "domain" | "domain-suffix" | "domain-keyword" | "geoip"
        ) {
            // geoip: allow `CN US JP` or `CN,US,JP` on one line
            line.split(|c: char| c == ',' || c == ';' || c.is_whitespace())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect()
        } else {
            // ip-cidr: one CIDR per line (commas rare); still allow comma split
            line.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect()
        };
        if tokens.is_empty() {
            continue;
        }
        for token in tokens {
            match build_rule(typ, token, target) {
                Ok(mut rule) => {
                    rule.group = group.clone();
                    out.rules.push(rule);
                }
                Err(message) => out.errors.push(BatchLineError {
                    line: line_no,
                    message,
                }),
            }
        }
    }
    out
}

fn parse_group_directive(trimmed: &str) -> Option<String> {
    let t = trimmed.trim();
    if !t.starts_with('#') {
        return None;
    }
    let rest = t.trim_start_matches('#').trim();
    let lower = rest.to_ascii_lowercase();
    let Some(after) = lower.strip_prefix("@group") else {
        return None;
    };
    let name = &rest[rest.len() - after.len()..];
    Some(name.trim().to_string())
}

/// Split `body  # optional name` — full-line comments become empty body.
fn split_line_and_name(raw: &str) -> (&str, String) {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return ("", String::new());
    }
    match trimmed.find('#') {
        Some(i) => (
            trimmed[..i].trim(),
            trimmed[i + 1..].trim().to_string(),
        ),
        None => (trimmed, String::new()),
    }
}

/// One Clash-style line: `TYPE,pattern,TARGET` or `MATCH,TARGET`.
pub fn format_rule_clash_line(rule: &RoutingRule) -> String {
    let typ = export_type(&rule.rule_type);
    if matches!(rule.rule_type.as_str(), "match" | "final") {
        format!("{},{}", typ, export_target(&rule.target))
    } else {
        format!(
            "{},{},{}",
            typ,
            rule.pattern,
            export_target(&rule.target)
        )
    }
}

/// Serialize rules to a Clash-compatible line format suitable for re-import.
pub fn format_rules_export(rules: &[RoutingRule]) -> String {
    use crate::config::rule_groups::group_rules_for_display;
    let mut lines = Vec::with_capacity(rules.len() + 4);
    lines.push("# Sockrocket rules export — one rule per line".to_string());
    lines.push("# @group Name switches group; trailing # is rule name".to_string());
    for bucket in group_rules_for_display(rules) {
        lines.push(format!("# @group {}", bucket.name));
        for &i in &bucket.indices {
            let r = &rules[i];
            let name_suffix = if r.name.is_empty() {
                String::new()
            } else {
                format!("  # {}", r.name.replace('\n', " "))
            };
            if !r.enabled {
                lines.push(format!(
                    "# disabled: {}{}",
                    format_rule_clash_line(r),
                    name_suffix
                ));
                continue;
            }
            lines.push(format!("{}{}", format_rule_clash_line(r), name_suffix));
        }
    }
    lines.join("\n")
}

/// Merge `incoming` into `existing` according to `conflict`.
///
/// New rules are prepended (higher list position) so they win over older peers
/// at the same priority, matching single-rule add behaviour.
pub fn merge_batch_rules(
    existing: &mut Vec<RoutingRule>,
    incoming: Vec<RoutingRule>,
    conflict: BatchConflict,
) -> BatchMergeStats {
    let mut stats = BatchMergeStats::default();
    // Insert newest-first among the batch so the first pasted line stays highest.
    for rule in incoming.into_iter().rev() {
        if let Some(pos) = existing
            .iter()
            .position(|e| e.rule_type == rule.rule_type && e.pattern == rule.pattern)
        {
            match conflict {
                BatchConflict::Skip => stats.skipped += 1,
                BatchConflict::Replace => {
                    existing[pos].target = rule.target;
                    existing[pos].enabled = rule.enabled;
                    existing[pos].priority = rule.priority;
                    if !rule.name.is_empty() {
                        existing[pos].name = rule.name;
                    }
                    if !rule.group.is_empty() {
                        existing[pos].group = rule.group;
                    }
                    stats.replaced += 1;
                }
            }
            continue;
        }
        let insert_at = existing
            .iter()
            .position(|r| r.priority <= rule.priority)
            .unwrap_or(existing.len());
        existing.insert(insert_at, rule);
        stats.added += 1;
    }
    stats
}

fn parse_one_line(line: &str, default_target: &str) -> Result<RoutingRule, String> {
    // Clash CSV: TYPE,value,action  or MATCH,action
    if line.contains(',') {
        return parse_clash_line(line);
    }
    // Explicit arrow: type:pattern->target  /  type pattern -> target
    if let Some((left, right)) = line.split_once("->") {
        let target = normalize_target(right.trim()).ok_or_else(|| {
            format!("Unknown target '{}'", right.trim())
        })?;
        return parse_typed_left(left.trim(), target);
    }
    // Whitespace: type pattern [target]
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 2 && looks_like_type(parts[0]) {
        let target = if parts.len() >= 3 {
            normalize_target(parts[2]).ok_or_else(|| format!("Unknown target '{}'", parts[2]))?
        } else {
            default_target
        };
        return build_rule(normalize_type(parts[0]), parts[1], target);
    }
    // Colon form without arrow: type:pattern
    if let Some((typ, pat)) = line.split_once(':') {
        if looks_like_type(typ) {
            return build_rule(normalize_type(typ), pat.trim(), default_target);
        }
    }
    // Auto-detect plain value
    auto_detect(line, default_target)
}

fn parse_clash_line(line: &str) -> Result<RoutingRule, String> {
    let parts: Vec<&str> = line.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return Err("Empty rule".into());
    }
    let typ_raw = parts[0];
    if !looks_like_type(typ_raw) {
        return Err(format!("Unknown rule type '{typ_raw}'"));
    }
    let typ = normalize_type(typ_raw);
    if matches!(typ, "match" | "final") {
        let target = parts
            .get(1)
            .copied()
            .and_then(normalize_target)
            .ok_or_else(|| "MATCH/FINAL requires a target".to_string())?;
        return build_rule(typ, "*", target);
    }
    if parts.len() < 2 {
        return Err(format!("{typ} requires a pattern"));
    }
    let pattern = parts[1];
    let target = parts
        .get(2)
        .copied()
        .and_then(normalize_target)
        .unwrap_or("proxy");
    build_rule(typ, pattern, target)
}

fn parse_typed_left(left: &str, target: &str) -> Result<RoutingRule, String> {
    if let Some((typ, pat)) = left.split_once(':') {
        if looks_like_type(typ) {
            return build_rule(normalize_type(typ), pat.trim(), target);
        }
    }
    let parts: Vec<&str> = left.split_whitespace().collect();
    if parts.len() >= 2 && looks_like_type(parts[0]) {
        return build_rule(normalize_type(parts[0]), parts[1], target);
    }
    if parts.len() == 1 && looks_like_type(parts[0]) {
        return build_rule(normalize_type(parts[0]), "*", target);
    }
    Err(format!("Cannot parse typed rule '{left}'"))
}

fn auto_detect(value: &str, target: &str) -> Result<RoutingRule, String> {
    let v = value.trim();
    if looks_like_cidr(v) {
        let cidr = normalize_cidr(v)?;
        return build_rule("ip-cidr", &cidr, target);
    }
    if looks_like_ipv4(v) {
        let cidr = format!("{v}/32");
        return build_rule("ip-cidr", &cidr, target);
    }
    if looks_like_domain(v) {
        let s = v.trim_start_matches('.').to_lowercase();
        return build_rule("domain-suffix", &s, target);
    }
    Err(format!(
        "Unrecognized pattern '{v}' (use domain, CIDR, or TYPE,pattern,TARGET)"
    ))
}

fn build_rule(rule_type: &str, pattern: &str, target: &str) -> Result<RoutingRule, String> {
    let pattern = match rule_type {
        "match" | "final" => "*".to_string(),
        "geoip" => {
            let p = pattern.trim();
            if p.len() == 2 && p.chars().all(|c| c.is_ascii_alphabetic()) {
                p.to_uppercase()
            } else {
                return Err(format!("Invalid GeoIP code '{pattern}'"));
            }
        }
        "ip-cidr" => normalize_cidr(pattern)?,
        "domain" | "domain-suffix" => {
            let s = pattern.trim().trim_start_matches('.').to_lowercase();
            if !looks_like_domain(&s) {
                return Err(format!("Invalid domain '{pattern}'"));
            }
            s
        }
        "domain-keyword" => {
            let s = pattern.trim().to_lowercase();
            if s.is_empty() || s.chars().any(char::is_whitespace) {
                return Err("Keyword must be non-empty without spaces".into());
            }
            s
        }
        "dst-port" | "port" => {
            let s = pattern.trim();
            if !valid_port_pattern(s) {
                return Err(format!("Invalid port '{pattern}' (e.g. 443 or 1000-2000)"));
            }
            s.to_string()
        }
        other => return Err(format!("Unsupported rule type '{other}'")),
    };
    Ok(RoutingRule {
        name: String::new(),
       group: String::new(), rule_type: if rule_type == "port" {
            "dst-port".into()
        } else if rule_type == "final" {
            "match".into()
        } else {
            rule_type.into()
        },
        pattern,
        target: target.to_string(),
        enabled: true,
        priority: 0,
    })
}

fn looks_like_type(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "domain"
            | "domain-suffix"
            | "domain_suffix"
            | "domainsuffix"
            | "domain-keyword"
            | "domain_keyword"
            | "domainkeyword"
            | "ip-cidr"
            | "ip_cidr"
            | "ipcidr"
            | "geoip"
            | "dst-port"
            | "dst_port"
            | "port"
            | "match"
            | "final"
    )
}

fn normalize_type(s: &str) -> &str {
    match s.to_ascii_lowercase().as_str() {
        "domain" => "domain",
        "domain-suffix" | "domain_suffix" | "domainsuffix" => "domain-suffix",
        "domain-keyword" | "domain_keyword" | "domainkeyword" => "domain-keyword",
        "ip-cidr" | "ip_cidr" | "ipcidr" => "ip-cidr",
        "geoip" => "geoip",
        "dst-port" | "dst_port" | "port" => "dst-port",
        "match" | "final" => "match",
        _ => "domain-suffix",
    }
}

fn normalize_target(s: &str) -> Option<&'static str> {
    match s.trim().to_ascii_lowercase().as_str() {
        "direct" | "dir" => Some("direct"),
        "proxy" | "proxied" => Some("proxy"),
        "reject" | "block" | "deny" => Some("reject"),
        _ => None,
    }
}

fn export_type(t: &str) -> &'static str {
    match t {
        "domain" => "DOMAIN",
        "domain-suffix" => "DOMAIN-SUFFIX",
        "domain-keyword" => "DOMAIN-KEYWORD",
        "ip-cidr" => "IP-CIDR",
        "geoip" => "GEOIP",
        "dst-port" | "port" => "DST-PORT",
        "match" | "final" => "MATCH",
        _ => "DOMAIN-SUFFIX",
    }
}

fn export_target(t: &str) -> &'static str {
    match t {
        "direct" => "DIRECT",
        "reject" => "REJECT",
        _ => "PROXY",
    }
}

fn looks_like_domain(s: &str) -> bool {
    let s = s.trim().trim_start_matches('.');
    if s.is_empty() || s.len() > 253 {
        return false;
    }
    if s.starts_with('-') || s.ends_with('-') || s.starts_with('.') || s.ends_with('.') {
        return false;
    }
    if !s.contains('.') {
        // Allow single-label keywords used as suffixes (e.g. "cn") — still ok as domain-suffix.
        return s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-');
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && s.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

fn looks_like_ipv4(s: &str) -> bool {
    s.parse::<std::net::Ipv4Addr>().is_ok()
}

fn looks_like_cidr(s: &str) -> bool {
    s.contains('/') && normalize_cidr(s).is_ok()
}

fn normalize_cidr(s: &str) -> Result<String, String> {
    let (addr, prefix) = s.split_once('/').ok_or_else(|| {
        format!("CIDR needs a prefix (e.g. {s}/32)")
    })?;
    let ip: std::net::IpAddr = addr
        .parse()
        .map_err(|_| format!("Invalid IP address '{addr}'"))?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let n: u8 = prefix
        .parse()
        .map_err(|_| format!("Invalid prefix '{prefix}'"))?;
    if n > max {
        return Err(format!("Prefix length must be 0-{max}"));
    }
    Ok(format!("{addr}/{n}"))
}

fn valid_port_pattern(s: &str) -> bool {
    if let Some((a, b)) = s.split_once('-') {
        let (Ok(min), Ok(max)) = (a.parse::<u16>(), b.parse::<u16>()) else {
            return false;
        };
        return min <= max;
    }
    s.parse::<u16>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_domains_and_cidrs() {
        let r = parse_batch_rules(
            "example.com\n10.0.0.0/8\n1.2.3.4\n# comment\n",
            "proxy",
        );
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.rules.len(), 3);
        assert_eq!(r.rules[0].rule_type, "domain-suffix");
        assert_eq!(r.rules[0].pattern, "example.com");
        assert_eq!(r.rules[0].target, "proxy");
        assert_eq!(r.rules[1].rule_type, "ip-cidr");
        assert_eq!(r.rules[1].pattern, "10.0.0.0/8");
        assert_eq!(r.rules[2].pattern, "1.2.3.4/32");
    }

    #[test]
    fn parses_clash_and_reports_bad_lines() {
        let r = parse_batch_rules(
            "DOMAIN-SUFFIX,google.com,DIRECT\nMATCH,PROXY\n@@@\nGEOIP,CN,DIRECT\n",
            "proxy",
        );
        assert_eq!(r.rules.len(), 3);
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.errors[0].line, 3);
        assert_eq!(r.rules[0].target, "direct");
        assert_eq!(r.rules[1].rule_type, "match");
        assert_eq!(r.rules[2].rule_type, "geoip");
        assert_eq!(r.rules[2].pattern, "CN");
    }

    #[test]
    fn merge_skips_duplicates() {
        let mut existing = vec![RoutingRule { name: String::new(),group: String::new(), rule_type: "domain-suffix".into(),
            pattern: "a.com".into(),
            target: "direct".into(),
            enabled: true,
            priority: 1,
        }];
        let incoming = parse_batch_rules("a.com\nb.com\n", "proxy").rules;
        let stats = merge_batch_rules(&mut existing, incoming, BatchConflict::Skip);
        assert_eq!(stats.added, 1);
        assert_eq!(stats.skipped, 1);
        assert_eq!(existing.len(), 2);
        assert!(existing.iter().any(|r| r.pattern == "b.com"));
        assert_eq!(
            existing
                .iter()
                .find(|r| r.pattern == "a.com")
                .unwrap()
                .target,
            "direct"
        );
    }

    #[test]
    fn parses_trailing_name_comment() {
        let r = parse_batch_rules(
            "DOMAIN-SUFFIX,futunn.com,PROXY  # Futu\nexample.com # My App\n",
            "proxy",
        );
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.rules[0].name, "Futu");
        assert_eq!(r.rules[1].name, "My App");
        assert_eq!(r.rules[1].pattern, "example.com");
    }

    #[test]
    fn format_clash_line_variants() {
        let domain = RoutingRule {
            name: String::new(),
            group: String::new(),
            rule_type: "geoip".into(),
            pattern: "CN".into(),
            target: "direct".into(),
            enabled: true,
            priority: 0,
        };
        assert_eq!(format_rule_clash_line(&domain), "GEOIP,CN,DIRECT");
        let fin = RoutingRule {
            name: String::new(),
            group: String::new(),
            rule_type: "match".into(),
            pattern: "*".into(),
            target: "proxy".into(),
            enabled: true,
            priority: 0,
        };
        assert_eq!(format_rule_clash_line(&fin), "MATCH,PROXY");
    }

    #[test]
    fn export_roundtrip_clash() {
        let rules = vec![
            RoutingRule { name: String::new(),group: String::new(), rule_type: "domain-suffix".into(),
                pattern: "x.com".into(),
                target: "proxy".into(),
                enabled: true,
                priority: 0,
            },
            RoutingRule { name: String::new(),group: String::new(), rule_type: "match".into(),
                pattern: "*".into(),
                target: "direct".into(),
                enabled: true,
                priority: 0,
            },
        ];
        let text = format_rules_export(&rules);
        let parsed = parse_batch_rules(&text, "proxy");
        assert!(parsed.errors.is_empty());
        assert_eq!(parsed.rules.len(), 2);
        assert_eq!(parsed.rules[0].pattern, "x.com");
        assert_eq!(parsed.rules[1].rule_type, "match");
        assert_eq!(parsed.rules[1].target, "direct");
    }

    #[test]
    fn typed_bulk_paste_in_group() {
        let r = parse_typed_patterns(
            "google.com\nfacebook.com, x.com\n# skip\n",
            "domain-suffix",
            "proxy",
            "跨境",
        );
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.rules.len(), 3);
        assert!(r.rules.iter().all(|x| x.group == "跨境"));
        assert!(r.rules.iter().all(|x| x.rule_type == "domain-suffix"));
        assert_eq!(r.rules[2].pattern, "x.com");
        assert!(rule_type_supports_bulk("domain-suffix"));
        assert!(rule_type_supports_bulk("geoip"));
        let geo = parse_typed_patterns("CN\nUS JP\nHK,TW\n", "geoip", "direct", "地区");
        assert!(geo.errors.is_empty(), "{:?}", geo.errors);
        assert_eq!(geo.rules.len(), 5);
        assert_eq!(geo.rules[0].pattern, "CN");
        assert_eq!(geo.rules[1].pattern, "US");
        assert_eq!(geo.rules[4].pattern, "TW");
    }

    #[test]
    fn single_add_path_matches_gui() {
        // Mirrors GUI single-add: type + one pattern + group + target.
        let r = make_typed_rule("domain-suffix", "google.com", "proxy", "跨境").unwrap();
        assert_eq!(r.rule_type, "domain-suffix");
        assert_eq!(r.pattern, "google.com");
        assert_eq!(r.target, "proxy");
        assert_eq!(r.group, "跨境");
        assert!(r.enabled);

        let geo = make_typed_rule("geoip", "cn", "direct", "").unwrap();
        assert_eq!(geo.pattern, "CN");
        assert_eq!(geo.target, "direct");

        let port = make_typed_rule("dst-port", "443", "reject", "ports").unwrap();
        assert_eq!(port.pattern, "443");
        assert_eq!(port.group, "ports");

        assert!(make_typed_rule("domain-suffix", "", "proxy", "").is_err());
        assert!(make_typed_rule("geoip", "USA", "proxy", "").is_err());
    }

    #[test]
    fn merge_single_add_into_list() {
        let mut rules = Vec::new();
        let incoming = vec![
            make_typed_rule("domain-suffix", "a.com", "proxy", "G").unwrap(),
            make_typed_rule("domain-suffix", "b.com", "direct", "G").unwrap(),
        ];
        let stats = merge_batch_rules(&mut rules, incoming, BatchConflict::Skip);
        assert_eq!(stats.added, 2);
        assert_eq!(rules.len(), 2);
        // duplicate skipped
        let again = vec![make_typed_rule("domain-suffix", "a.com", "reject", "G").unwrap()];
        let stats = merge_batch_rules(&mut rules, again, BatchConflict::Skip);
        assert_eq!(stats.skipped, 1);
        assert_eq!(rules.len(), 2);
        assert_eq!(
            rules.iter().find(|r| r.pattern == "a.com").unwrap().target,
            "proxy"
        );
    }
}
