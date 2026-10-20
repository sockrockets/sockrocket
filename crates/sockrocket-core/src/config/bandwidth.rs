//! Exact decimal bandwidth conversion shared by URI, YAML and outbounds.
use anyhow::{Result, bail, ensure};
pub(crate) fn parse_mbps(value: &str) -> Result<u64> {
    let value = value.trim().to_ascii_lowercase();
    let (number, unit) = if let Some(v) = value.strip_suffix("gbps") {
        (v, 1_000_000_000u128)
    } else if let Some(v) = value.strip_suffix("mbps") {
        (v, 1_000_000)
    } else if let Some(v) = value.strip_suffix("kbps") {
        (v, 1_000)
    } else if let Some(v) = value.strip_suffix("bps") {
        (v, 1)
    } else {
        (value.as_str(), 1_000_000)
    };
    let number = number.trim();
    let mut parts = number.split('.');
    let whole = parts.next().unwrap_or("");
    let fraction = parts.next().unwrap_or("");
    ensure!(
        parts.next().is_none()
            && (!whole.is_empty() || !fraction.is_empty())
            && fraction.len() <= 9
            && whole.bytes().all(|b| b.is_ascii_digit())
            && fraction.bytes().all(|b| b.is_ascii_digit()),
        "Invalid bandwidth"
    );
    let scale = 10u128.pow(fraction.len() as u32);
    let whole = if whole.is_empty() {
        0
    } else {
        whole
            .parse::<u128>()
            .map_err(|_| anyhow::anyhow!("Bandwidth out of range"))?
    };
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u128>()?
    };
    let Some(bits) = whole
        .checked_mul(scale)
        .and_then(|n| n.checked_add(fraction))
        .and_then(|n| n.checked_mul(unit))
    else {
        bail!("Bandwidth out of range")
    };
    let divisor = scale * 8;
    let bytes = bits.div_ceil(divisor);
    ensure!(bytes <= 125_000_000_000, "Bandwidth out of range");
    Ok(bytes as u64)
}
pub(crate) fn canonical_mbps(value: &str) -> Result<String> {
    let bytes = parse_mbps(value)?;
    let whole = bytes / 125_000;
    let fraction = (bytes % 125_000) * 8;
    Ok(if fraction == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{fraction:06}")
            .trim_end_matches('0')
            .to_string()
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decimal_units_and_bounds_are_exact() {
        for (value, bytes) in [
            ("10", 1_250_000),
            ("0.5 Mbps", 62_500),
            ("100 Kbps", 12_500),
            ("1 Gbps", 125_000_000),
            ("1 bps", 1),
            ("0", 0),
        ] {
            assert_eq!(parse_mbps(value).unwrap(), bytes);
            assert_eq!(parse_mbps(&canonical_mbps(value).unwrap()).unwrap(), bytes);
        }
        for value in [
            "-1",
            "NaN",
            "inf",
            "10 bogus",
            "1000001 Mbps",
            "1.2.3",
            "9999999999999999999999999999999999999999999999",
        ] {
            assert!(parse_mbps(value).is_err());
        }
    }
}
