//! QR code rendering component.
//!
//! Encodes arbitrary text (typically a share URI or a whole subscription
//! payload) into a QR module matrix via `sockrocket_core::qr_module_matrix` and
//! paints it with a single `gpui::canvas` element — one painted quad per
//! dark module. (The previous version built one div per module, i.e.
//! ~2,000–3,600 layout nodes per QR code, rebuilt on every repaint while
//! visible. Canvas quads skip layout and batch on the GPU instead.)
//! (The matrix logic itself lives in sockrocket-core so it can be unit-tested
//! without compiling this GPUI app in test mode.)

use gpui::*;
use sockrocket_core::qr_module_matrix;
use std::sync::Arc;

/// Number of quiet-zone (blank margin) modules around the QR code,
/// as required by the QR specification.
const QUIET_ZONE_MODULES: usize = 4;

/// Foreground (dark module) color: pure black for maximum scan contrast.
const QR_FOREGROUND: u32 = 0x000000;
/// Background / quiet zone color: pure white.
const QR_BACKGROUND: u32 = 0xffffff;

/// Cached QR matrix: (payload, module width, shared cell grid).
type QrCacheEntry = (String, usize, Arc<Vec<bool>>);

/// Cache of the last encoded QR matrix. Encoding the payload is the expensive
/// part of render_qr, and the matrix only changes when the payload does — so
/// re-encoding it on every repaint is pure waste. The `Arc` makes cache hits
/// allocation-free (the previous version cloned the whole matrix per render).
static QR_MATRIX_CACHE: std::sync::Mutex<Option<QrCacheEntry>> = std::sync::Mutex::new(None);

/// Render `data` as a QR code painted by a single canvas element.
///
/// The canvas is sized `(width + 2 * quiet zone) * module_px` with a white
/// background (painted via the element style); the paint callback then draws
/// one black quad per dark module. Returns `None` when the data cannot be
/// encoded (e.g. too long).
pub fn render_qr(data: &str, module_px: f32) -> Option<Canvas<()>> {
    let (width, cells) = {
        let mut cache = QR_MATRIX_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        match cache.as_ref() {
            Some((cached_data, w, cells)) if cached_data == data => (*w, cells.clone()),
            _ => {
                let (w, cells) = qr_module_matrix(data)?;
                let cells = Arc::new(cells);
                *cache = Some((data.to_string(), w, cells.clone()));
                (w, cells)
            }
        }
    };
    let module_px = module_px.max(1.0);
    let quiet = QUIET_ZONE_MODULES as f32 * module_px;
    let total = px((width + 2 * QUIET_ZONE_MODULES) as f32 * module_px);

    let el = canvas(
        move |_, _, _| (),
        move |bounds, _, window, _| {
            let dark: Background = rgb(QR_FOREGROUND).into();
            let module = size(px(module_px), px(module_px));
            for row in 0..width {
                for col in 0..width {
                    if cells[row * width + col] {
                        let origin = bounds.origin
                            + point(
                                px(quiet + col as f32 * module_px),
                                px(quiet + row as f32 * module_px),
                            );
                        window.paint_quad(fill(Bounds::new(origin, module), dark));
                    }
                }
            }
        },
    );
    Some(el.w(total).h(total).bg(rgb(QR_BACKGROUND)))
}
