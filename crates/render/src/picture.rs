//! Picture graphics: PNG or JPEG images (base64 in the graphic's `image` attribute) stretched over
//! the graphic's rectangle.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use base64::Engine;
use rustgis_data::{Dataset, Value};
use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

/// Largest picture decoded, in pixels per side.
const MAX_SIDE: u32 = 4096;
/// Decoded pictures kept between frames.
const CACHE: usize = 32;

thread_local! {
    static DECODED: RefCell<HashMap<u64, Option<Pixmap>>> = RefCell::new(HashMap::new());
}

/// Decodes base64 PNG or JPEG bytes into a premultiplied pixmap.
pub fn decode(b64: &str) -> Option<Pixmap> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    let img = if img.width() > MAX_SIDE || img.height() > MAX_SIDE { img.thumbnail(MAX_SIDE, MAX_SIDE) } else { img };
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut pm = Pixmap::new(w, h)?;
    for (dst, src) in pm.pixels_mut().iter_mut().zip(rgba.pixels()) {
        let [r, g, b, a] = src.0;
        let c = tiny_skia::ColorU8::from_rgba(r, g, b, a);
        *dst = c.premultiply();
    }
    Some(pm)
}

/// Draws row `row`'s picture into the pixel rectangle (x0, y0)–(x1, y1).
pub fn draw(pm: &mut Pixmap, ds: &Dataset, row: usize, (x0, y0, x1, y1): (f32, f32, f32, f32), opacity: f32) {
    let Some(Value::Text(b64)) = ds.field_index("image").map(|fi| ds.value(row, fi)) else { return };
    let (w, h) = ((x1 - x0).abs(), (y1 - y0).abs());
    if !(w.is_finite() && h.is_finite()) || w < 1.0 || h < 1.0 || w > 1e5 || h > 1e5 {
        return;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    b64.hash(&mut hasher);
    let key = hasher.finish();
    DECODED.with(|cache| {
        let mut cache = cache.borrow_mut();
        if !cache.contains_key(&key) {
            if cache.len() >= CACHE {
                cache.clear();
            }
            cache.insert(key, decode(b64));
        }
        let Some(Some(img)) = cache.get(&key) else { return };
        let t = Transform::from_row(w / img.width() as f32, 0.0, 0.0, h / img.height() as f32, x0.min(x1), y0.min(y1));
        let paint = PixmapPaint { opacity, quality: FilterQuality::Bilinear, ..PixmapPaint::default() };
        pm.draw_pixmap(0, 0, img.as_ref(), &paint, t, None);
    });
}
