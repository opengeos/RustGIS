//! Sprites: the icons and fill patterns a style draws, packed into one picture with an index.
//!
//! A style's `sprite` address names two files, `<address>.json` (where each image is in the
//! picture, its pixel ratio and whether it is an SDF icon) and `<address>.png`, and their `@2x`
//! versions for high-density screens. Both come through the tile cache; images are cut out of
//! the picture the first time they are asked for, and SDF icons are coloured as they are drawn.
//! A style may name several sprites, each with an id: one is its `default`, and the images of
//! the others are named `<id>:<name>`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tiny_skia::{IntRect, Pixmap};

use crate::cache::{self, Payload, State, Want};
use crate::style::Rgba;

/// Where one image is in the picture.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Entry {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    ratio: f32,
    sdf: bool,
}

/// A style's loaded sprites, by id.
pub struct Sprite {
    sheets: Vec<(String, Arc<Sheet>)>,
}

impl std::fmt::Debug for Sprite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.sheets.iter().map(|(id, s)| (id, s.index.len()))).finish()
    }
}

/// One sprite: its picture and index.
struct Sheet {
    sheet: Pixmap,
    index: HashMap<String, Entry>,
    /// Images cut out so far, by name and (for SDF icons) colour: looked up by `&str`, so a
    /// hit allocates nothing.
    cut: Mutex<HashMap<String, HashMap<Option<Rgba>, Arc<Pixmap>>>>,
}

/// An image from a sprite.
#[derive(Clone)]
pub struct Image {
    pub pixmap: Arc<Pixmap>,
    /// Pixels per logical point it was drawn for (2 in an `@2x` sprite).
    pub ratio: f32,
}

/// The most images a sprite index lists.
const MAX_IMAGES: usize = 10_000;
/// The largest side of one image (icons and patterns are tens of pixels).
const MAX_SIDE: f64 = 1024.0;
/// The most bytes of cut images kept.
const MAX_CUT_BYTES: usize = 32 << 20;

impl Sprite {
    /// A style's one sprite, from its index and picture.
    pub fn new(index: &serde_json::Value, sheet: Pixmap) -> Sprite {
        Sprite { sheets: vec![("default".to_owned(), Arc::new(Sheet::new(index, sheet)))] }
    }

    /// The sheet with the image `name`, and its name there, as MapLibre looks it up: `<id>:<name>`
    /// in the sprite with that id only, any other name in the `default` sprite only. Looking in
    /// the others too would draw another sprite's image of that name, and keep `coalesce` from
    /// going on to its next choice.
    fn find<'a>(&self, name: &'a str) -> Option<(&Sheet, &'a str)> {
        let sheet = |id: &str| self.sheets.iter().find(|(i, _)| i == id).map(|(_, s)| &**s);
        let (s, name) = match name.split_once(':') {
            Some((id, bare)) if sheet(id).is_some() => (sheet(id)?, bare),
            _ => (sheet("default")?, name),
        };
        s.usable(name).then_some((s, name))
    }

    /// Whether a sprite has the image `name`, one that can be drawn.
    pub fn has(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// The image `name`, if a sprite has it. An SDF icon is drawn in `color` (black when none
    /// is given).
    pub fn image(&self, name: &str, color: Option<Rgba>) -> Option<Image> {
        let (sheet, name) = self.find(name)?;
        sheet.image(name, color)
    }
}

impl Sheet {
    fn new(index: &serde_json::Value, sheet: Pixmap) -> Sheet {
        let num = |v: &serde_json::Value, k: &str| v.get(k).and_then(serde_json::Value::as_f64);
        let index = index
            .as_object()
            .into_iter()
            .flatten()
            .take(MAX_IMAGES)
            .filter_map(|(name, v)| {
                let (x, y, w, h) = (num(v, "x")?, num(v, "y")?, num(v, "width")?, num(v, "height")?);
                if [x, y].iter().any(|n| !(n.is_finite() && *n >= 0.0 && *n <= 16_384.0))
                    || ![w, h].iter().all(|n| n.is_finite() && *n >= 1.0 && *n <= MAX_SIDE)
                {
                    return None;
                }
                let ratio = num(v, "pixelRatio").filter(|r| r.is_finite() && *r > 0.0 && *r <= 8.0).unwrap_or(1.0) as f32;
                let sdf = v.get("sdf").and_then(serde_json::Value::as_bool).unwrap_or(false);
                Some((name.clone(), Entry { x: x as u32, y: y as u32, w: w as u32, h: h as u32, ratio, sdf }))
            })
            .collect();
        Sheet { sheet, index, cut: Mutex::default() }
    }

    /// Whether the index lists `name` wholly inside the picture: an entry reaching past it can't
    /// be cut (cutting would otherwise clip it to a part).
    fn usable(&self, name: &str) -> bool {
        self.index.get(name).is_some_and(|e| self.fits(e))
    }

    fn fits(&self, e: &Entry) -> bool {
        e.x.checked_add(e.w).is_some_and(|r| r <= self.sheet.width()) && e.y.checked_add(e.h).is_some_and(|b| b <= self.sheet.height())
    }

    fn image(&self, name: &str, color: Option<Rgba>) -> Option<Image> {
        let (key, e) = self.index.get_key_value(name).map(|(k, e)| (k.as_str(), *e))?;
        let color = e.sdf.then(|| color.unwrap_or([0, 0, 0, 255]));
        if let Some(p) = self.cut.lock().unwrap_or_else(|p| p.into_inner()).get(key).and_then(|by_color| by_color.get(&color)) {
            return Some(Image { pixmap: p.clone(), ratio: e.ratio });
        }
        // Cut outside the lock.
        if !self.fits(&e) {
            return None;
        }
        let mut p = self.sheet.clone_rect(IntRect::from_xywh(e.x as i32, e.y as i32, e.w, e.h)?)?;
        if let Some(c) = color {
            sdf_fill(&mut p, c);
        }
        let p = Arc::new(p);
        let mut cut = self.cut.lock().unwrap_or_else(|p| p.into_inner());
        // Kept within a byte budget: an SDF icon cut in many colours, or a style naming
        // thousands of images, starts the cache again rather than growing it.
        let bytes: usize = cut.values().flat_map(HashMap::values).map(|p| p.data().len()).sum();
        if bytes + p.data().len() > MAX_CUT_BYTES {
            cut.clear();
        }
        cut.entry(key.to_string()).or_default().insert(color, p.clone());
        Some(Image { pixmap: p, ratio: e.ratio })
    }
}

/// Colours an SDF icon: its alpha is a distance from the edge (0.75 at the edge, more inside),
/// which becomes `c`, smoothed over a pixel.
fn sdf_fill(p: &mut Pixmap, c: Rgba) {
    const EDGE: f32 = 0.75;
    const SMOOTH: f32 = 0.06;
    for px in p.data_mut().as_chunks_mut::<4>().0 {
        let d = f32::from(px[3]) / 255.0;
        let t = ((d - (EDGE - SMOOTH)) / (2.0 * SMOOTH)).clamp(0.0, 1.0);
        let a = t * t * (3.0 - 2.0 * t) * f32::from(c[3]) / 255.0;
        for (i, ch) in c.iter().take(3).enumerate() {
            px[i] = (f32::from(*ch) * a).round() as u8;
        }
        px[3] = (a * 255.0).round() as u8;
    }
}

pub enum Load {
    Ready(Arc<Sprite>),
    Pending,
    Failed,
}

fn sprites() -> &'static Mutex<HashMap<String, Arc<Sheet>>> {
    static S: OnceLock<Mutex<HashMap<String, Arc<Sheet>>>> = OnceLock::new();
    S.get_or_init(Mutex::default)
}

/// Forgets the decoded sprite sheets, so they are read again (with [`crate::cache::clear`]).
pub(crate) fn forget() {
    sprites().lock().unwrap_or_else(|p| p.into_inner()).clear();
}

/// `<address><suffix>`, the suffix going before any query string.
fn with_suffix(base: &str, suffix: &str) -> String {
    match base.split_once('?') {
        Some((path, query)) => format!("{path}{suffix}?{query}"),
        None => format!("{base}{suffix}"),
    }
}

/// A sheet on its way.
enum SheetLoad {
    Ready(Arc<Sheet>),
    Pending,
    Failed,
}

/// A style's sprites, as (id, address without `.json`/`.png`), at high density when `hi_dpi`
/// and a sprite has it, else at 1×. Ready once none is on its way, with those that could be
/// read; one that can't only loses its images.
pub fn load(sprites: &[(String, String)], hi_dpi: bool) -> Load {
    let mut sheets = Vec::new();
    let mut pending = false;
    for (id, base) in sprites {
        match load_sheet(base, hi_dpi) {
            SheetLoad::Ready(s) => sheets.push((id.clone(), s)),
            SheetLoad::Pending => pending = true,
            SheetLoad::Failed => {}
        }
    }
    if pending {
        Load::Pending
    } else if sheets.is_empty() {
        Load::Failed
    } else {
        Load::Ready(Arc::new(Sprite { sheets }))
    }
}

fn load_sheet(base: &str, hi_dpi: bool) -> SheetLoad {
    if hi_dpi {
        match load_one(base, "@2x") {
            SheetLoad::Failed => {}
            other => return other,
        }
    }
    load_one(base, "")
}

fn load_one(base: &str, density: &str) -> SheetLoad {
    let key = format!("{base}|{density}");
    if let Some(s) = sprites().lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
        return SheetLoad::Ready(s.clone());
    }
    let index = cache::get(&with_suffix(base, &format!("{density}.json")), Want::Json);
    let sheet = cache::get(&with_suffix(base, &format!("{density}.png")), Want::Image);
    match (index, sheet) {
        (State::Ready(i), State::Ready(p)) => match (&*i, &*p) {
            (Payload::Json(index), Payload::Image(sheet)) => {
                let s = Arc::new(Sheet::new(index, sheet.clone()));
                let mut all = sprites().lock().unwrap_or_else(|p| p.into_inner());
                if all.len() > 16 {
                    all.clear();
                }
                all.insert(key, s.clone());
                SheetLoad::Ready(s)
            }
            _ => SheetLoad::Failed,
        },
        (State::Failed(_), _) | (_, State::Failed(_)) => SheetLoad::Failed,
        _ => SheetLoad::Pending,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet() -> Pixmap {
        let mut p = Pixmap::new(8, 4).unwrap();
        p.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 255));
        p
    }

    #[test]
    fn forgetting_drops_the_decoded_sheets() {
        let key = "forget-test|".to_string();
        let sprite = Sheet { sheet: sheet(), index: Default::default(), cut: Default::default() };
        sprites().lock().unwrap().insert(key.clone(), Arc::new(sprite));
        forget();
        assert!(!sprites().lock().unwrap().contains_key(&key));
    }

    #[test]
    fn cuts_images_out() {
        let index = serde_json::json!({
            "dot": {"x": 0, "y": 0, "width": 4, "height": 4, "pixelRatio": 2},
            "sdf": {"x": 4, "y": 0, "width": 4, "height": 4, "pixelRatio": 1, "sdf": true},
            "bad": {"x": 6, "y": 0, "width": 40, "height": 4},
            "huge": {"x": 0, "y": 0, "width": 4096, "height": 4},
            "junk": {"x": -1}
        });
        let s = Sprite::new(&index, sheet());
        let dot = s.image("dot", None).unwrap();
        assert_eq!((dot.pixmap.width(), dot.ratio), (4, 2.0));
        // Several sprites: `<id>:<name>`.
        // A style's one sprite is `default`: its images are named without an id.
        assert!(s.image("base:dot", None).is_none());
        assert!(s.image("bad", None).is_none() && s.image("junk", None).is_none() && s.image("nope", None).is_none());
        // Listed, but reaching past the picture: not there for `image` either, so `coalesce`
        // goes on to the next.
        assert!(!s.has("bad") && s.has("dot"));
        assert!(s.image("huge", None).is_none(), "over the largest side");
        // An SDF icon takes the colour asked for (full alpha is well inside the edge).
        let blue = s.image("sdf", Some([0, 0, 255, 255])).unwrap();
        let px = blue.pixmap.pixel(1, 1).unwrap();
        assert_eq!((px.red(), px.blue(), px.alpha()), (0, 255, 255));
    }

    #[test]
    fn several_sprites_name_their_images_by_id() {
        let sheet_of = |names: &[&str]| {
            let index: serde_json::Map<_, _> =
                names.iter().map(|n| (n.to_string(), serde_json::json!({"x": 0, "y": 0, "width": 4, "height": 4}))).collect();
            Arc::new(Sheet::new(&serde_json::Value::Object(index), sheet()))
        };
        let s = Sprite {
            sheets: vec![
                ("base".to_owned(), sheet_of(&["icon-bus", "anchor"])),
                ("nautical".to_owned(), sheet_of(&["anchor", "light/red"])),
            ],
        };
        assert!(s.has("nautical:light/red") && s.has("base:icon-bus"));
        // The id picks the sprite when both have the name.
        let (picked, name) = s.find("nautical:anchor").unwrap();
        assert!(std::ptr::eq(picked, &*s.sheets[1].1) && name == "anchor");
        // Only in the sprite named: not in another that has an image of that name.
        assert!(!s.has("base:light/red") && !s.has("nautical:icon-bus") && !s.has("nautical:") && !s.has(""));
        // Without an id, only in the `default` sprite, which this style hasn't got.
        assert!(!s.has("icon-bus") && !s.has("other:icon-bus"));
        let with_default = Sprite { sheets: vec![("default".to_owned(), sheet_of(&["pin"])), ("nautical".to_owned(), sheet_of(&["pin"]))] };
        let (d, name) = with_default.find("pin").unwrap();
        assert!(std::ptr::eq(d, &*with_default.sheets[0].1) && name == "pin");
        assert!(with_default.has("nautical:pin") && !with_default.has("default:missing"));
        // Nothing to read is no sprite.
        assert!(matches!(load(&[], false), Load::Failed));
    }

    #[test]
    fn suffixes_go_before_the_query() {
        assert_eq!(with_suffix("https://a/sprite?key=1", "@2x.png"), "https://a/sprite@2x.png?key=1");
        assert_eq!(with_suffix("https://a/sprite", ".json"), "https://a/sprite.json");
    }
}
