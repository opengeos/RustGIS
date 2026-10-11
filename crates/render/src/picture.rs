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

/// Decoded pictures, and their keys oldest first (for eviction).
#[derive(Default)]
struct Cache {
    images: HashMap<u64, Option<Pixmap>>,
    order: std::collections::VecDeque<u64>,
}

thread_local! {
    static DECODED: RefCell<Cache> = RefCell::new(Cache::default());
}

/// A cheap key for a picture: its length and its first and last bytes (hashing megabytes of
/// base64 every frame would be slow).
fn key(b64: &str) -> u64 {
    let bytes = b64.as_bytes();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.len().hash(&mut h);
    bytes.get(..bytes.len().min(512)).hash(&mut h);
    bytes.get(bytes.len().saturating_sub(512)..).hash(&mut h);
    h.finish()
}

/// The pixel size of base64 PNG or JPEG bytes, read from the header without decoding the image.
pub fn size(b64: &str) -> Option<(u32, u32)> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()?;
    image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?.into_dimensions().ok()
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
    let key = key(b64);
    DECODED.with(|cache| {
        let mut cache = cache.borrow_mut();
        if !cache.images.contains_key(&key) {
            // Evict the oldest picture rather than all of them.
            while cache.images.len() >= CACHE {
                let Some(old) = cache.order.pop_front() else { break };
                cache.images.remove(&old);
            }
            cache.images.insert(key, decode(b64));
            cache.order.push_back(key);
        }
        let Some(Some(img)) = cache.images.get(&key) else { return };
        let t = Transform::from_row(w / img.width() as f32, 0.0, 0.0, h / img.height() as f32, x0.min(x1), y0.min(y1));
        let paint = PixmapPaint { opacity, quality: FilterQuality::Bilinear, ..PixmapPaint::default() };
        pm.draw_pixmap(0, 0, img.as_ref(), &paint, t, None);
    });
}
