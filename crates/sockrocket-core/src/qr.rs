//! Pure QR module-matrix encoding (no UI dependencies).
//!
//! Lives in sockrocket-core so the logic can be unit-tested without compiling the
//! GPUI app in test mode (whose macro expansion overflows rustc's stack).

use qrcode::QrCode;

/// Encode `data` into a QR module matrix.
///
/// Returns `(width, cells)` where `cells` is a row-major `width * width`
/// vector (`true` = dark module), or `None` when the data is too long for
/// a QR code or encoding otherwise fails.
pub fn qr_module_matrix(data: &str) -> Option<(usize, Vec<bool>)> {
    if data.is_empty() {
        return None;
    }
    let code = QrCode::new(data.as_bytes()).ok()?;
    let width = code.width();
    let cells = code
        .to_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();
    Some((width, cells))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_for_short_payload() {
        // A version-1 QR code is 21x21; short payloads must fit in it.
        let (width, cells) = qr_module_matrix("hello").expect("short data must encode");
        assert_eq!(width, 21);
        assert_eq!(cells.len(), width * width);
        // QR codes always have dark modules (finder patterns) and light ones.
        assert!(cells.iter().any(|&c| c));
        assert!(cells.iter().any(|&c| !c));
    }

    #[test]
    fn matrix_scales_with_payload_size() {
        let (small, _) = qr_module_matrix("vmess://a").unwrap();
        // A realistic share URI is a few hundred bytes -> larger version.
        let long_uri = format!("vmess://{}", "a".repeat(300));
        let (big, _) = qr_module_matrix(&long_uri).unwrap();
        assert!(big > small, "larger payload needs a larger QR version");
        // Widths of consecutive versions step by 4 modules.
        assert_eq!((big - small) % 4, 0);
    }

    #[test]
    fn empty_input_rejected() {
        assert!(qr_module_matrix("").is_none());
    }

    #[test]
    fn oversized_input_rejected() {
        // Version 40-L tops out at 2953 bytes; 10 KiB must fail.
        let huge = "x".repeat(10 * 1024);
        assert!(qr_module_matrix(&huge).is_none());
    }

    #[test]
    fn matrix_succeeds_for_typical_share_uri() {
        let uri = "trojan://password@example.com:443?sni=example.com#node-1";
        let (width, cells) = qr_module_matrix(uri).expect("share URI must encode");
        assert_eq!(cells.len(), width * width);
    }
}
