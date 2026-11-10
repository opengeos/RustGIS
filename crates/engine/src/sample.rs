//! The built-in sample project: a world map from Natural Earth (public domain) data.

use rustgis_data::io::read_geojson;
use rustgis_geom::{Coord, lonlat_to_merc};
use rustgis_map::{Bookmark, Camera, ClassMethod, Marker, Ramp, Renderer, Rgba, Symbol};

use crate::Project;

pub use rustgis_render::sample_data::{CITIES, COUNTRIES, LAKES, RIVERS, Worldview, countries, install, set_worldview, text, worldview};

/// The worldview this system calls for: `RUSTGIS_WORLDVIEW` (a worldview's code, such as `ind`)
/// when set, else China when the system's time zone or first locale is mainland China's. Nothing
/// is looked up over the network. Test builds of the engine don't look at the system.
pub fn system_worldview() -> Worldview {
    if let Some(w) = std::env::var("RUSTGIS_WORLDVIEW").ok().and_then(|v| Worldview::from_code(&v)) {
        return w;
    }
    if cfg!(test) {
        return Worldview::DEFAULT;
    }
    let tz = iana_time_zone::get_timezone().ok();
    let locales: Vec<String> = sys_locale::get_locales().take(4).collect();
    Worldview::detect(tz.as_deref(), locales.iter().map(String::as_str))
}

/// The worldview a preference asks for: a worldview's code, or the system's for `auto` (or
/// anything unknown).
pub fn wanted_worldview(pref: &str) -> Worldview {
    Worldview::from_code(pref).unwrap_or_else(system_worldview)
}

/// Applies a worldview preference (see [`wanted_worldview`]) and returns the worldview asked
/// for. It shows only once its countries are at hand ([`ready`]); until then the current one
/// stays, and the caller downloads it ([`download`], [`download_blocking`]).
pub fn apply_worldview(pref: &str) -> Worldview {
    let w = wanted_worldview(pref);
    if ready(w) {
        set_worldview(w);
    }
    w
}

/// Whether downloaded worldviews are kept in the cache folder and read back from it (not in the
/// engine's own tests, which mustn't touch the computer's cache).
static KEEP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(!cfg!(test));

/// Turns keeping downloaded worldviews in the cache folder on or off (off for tests).
pub fn keep_downloads(on: bool) {
    KEEP.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Where a downloaded worldview is kept: `worldviews/countries_<code>.geojson` in the cache folder
/// (none in the browser).
fn kept_path(w: Worldview) -> Option<std::path::PathBuf> {
    if w.built_in() || !KEEP.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    rustgis_data::dirs::cache_dir().map(|d| d.join("worldviews").join(format!("countries_{}.geojson", w.code())))
}

/// Whether a worldview's countries are at hand: built in, downloaded in this run, or kept from an
/// earlier download (read now). A kept file that isn't usable is deleted, so it's downloaded again.
pub fn ready(w: Worldview) -> bool {
    if text(w).is_some() {
        return true;
    }
    let Some(path) = kept_path(w) else { return false };
    let Ok(bytes) = std::fs::read(&path) else { return false };
    if bytes.len() <= rustgis_render::sample_data::MAX_WORLDVIEW_BYTES
        && let Ok(t) = String::from_utf8(bytes)
        && install(w, t).is_ok()
    {
        return true;
    }
    log::warn!("{}: not a usable worldview, removed", path.display());
    let _ = std::fs::remove_file(&path);
    false
}

/// Takes a worldview's downloaded countries: checks and keeps them for this run, and in the cache
/// folder for the next. Not keeping the file only costs a download next time, so it is logged.
pub fn accept(w: Worldview, bytes: Vec<u8>) -> Result<(), String> {
    let text = String::from_utf8(bytes).map_err(|_| format!("the {} worldview's countries aren't UTF-8 text", w.name()))?;
    let copy = kept_path(w).map(|p| (p, text.clone()));
    install(w, text)?;
    if let Some((path, text)) = copy {
        let saved =
            path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|_| rustgis_data::io::write_atomic(&path, text.as_bytes()));
        if let Err(e) = saved {
            log::warn!("couldn't keep the {} worldview in {}: {e}", w.name(), path.display());
        }
    }
    Ok(())
}

/// Downloads a worldview's countries off the calling thread (natively) or with the browser's
/// fetch, calling `done` with the bytes for [`accept`].
pub fn download(w: Worldview, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
    match w.url() {
        Some(url) => crate::render::rustgis_tiles::cache::download_limited(&url, rustgis_render::sample_data::MAX_WORLDVIEW_BYTES, done),
        None => done(Err("the default worldview is built in".into())),
    }
}

/// Makes a worldview's countries ready, downloading them and waiting if they aren't at hand (the
/// command line's `--worldview`).
#[cfg(not(target_arch = "wasm32"))]
pub fn download_blocking(w: Worldview) -> Result<(), String> {
    if ready(w) {
        return Ok(());
    }
    let url = w.url().ok_or("the default worldview is built in")?;
    let bytes = crate::render::rustgis_tiles::cache::download_blocking_limited(&url, rustgis_render::sample_data::MAX_WORLDVIEW_BYTES)
        .map_err(|e| format!("couldn't download the {} worldview: {e}", w.name()))?;
    accept(w, bytes)
}

fn cam(lon: f64, lat: f64, res: f64) -> Camera {
    Camera { center: lonlat_to_merc(Coord::new(lon, lat)), res, bearing: 0.0, pitch: 0.0 }
}

/// Swaps an untouched sample project's countries from worldview `from` to `to`: only in a project
/// that is unsaved, unchanged and has nothing to undo, and only a layer named `Countries` holding
/// exactly the built-in countries of `from`. Returns whether a layer was swapped.
pub fn swap_countries(s: &mut crate::Session, from: Worldview, to: Worldview) -> bool {
    if from == to || s.dirty() || s.path.is_some() || s.can_undo() {
        return false;
    }
    let read = |w| text(w).and_then(|t| read_geojson(t, "Countries").ok()).map(|(ds, _)| ds);
    let (Some(old), Some(new)) = (read(from), read(to)) else {
        return false;
    };
    let Some(l) = s.map_mut().layers.iter_mut().find(|l| l.name == "Countries" && l.dataset.features == old.features) else {
        return false;
    };
    l.dataset = new;
    l.selection.clear();
    l.touch_geometry();
    true
}

/// Slate gray country borders: visible against both the near-white lowest population class and the ocean.
const COUNTRY_BORDER: Rgba = Rgba::rgb(0x7D, 0x8C, 0x96);

/// "World Atlas": countries by population, cities sized by population, rivers and lakes.
pub fn world_project() -> Project {
    let mut p = Project::new("World Atlas");
    let map = p.map_mut();
    map.name = "World".into();
    map.basemap = rustgis_map::Basemap::Ocean;
    let mut add = |name: &str, text: &str| read_geojson(text, name).ok().map(|(ds, _)| map.add_dataset(ds));
    let countries = add("Countries", countries());
    let lakes = add("Lakes", LAKES);
    let rivers = add("Rivers", RIVERS);
    let cities = add("Cities", CITIES);
    if let Some(l) = countries.and_then(|id| map.layer_mut(id)) {
        let base = Symbol { fill: Rgba::rgb(200, 200, 200), stroke: COUNTRY_BORDER, stroke_width: 0.6, size: 0.0, marker: Marker::Circle };
        if let Ok(r) = Renderer::graduated(&l.dataset, &[], "pop_est", 5, ClassMethod::NaturalBreaks, Ramp::Lagoon, base) {
            l.renderer = r;
            l.renderer.map_symbols(|s| {
                s.stroke = COUNTRY_BORDER;
                s.stroke_width = 0.6;
            });
        }
        l.labels.enabled = true;
        l.labels.default.field = "name".into();
        l.labels.default.size = 8.5;
        l.labels.default.color = Rgba::rgb(0x33, 0x3A, 0x44);
    }
    if let Some(l) = lakes.and_then(|id| map.layer_mut(id)) {
        l.renderer = Renderer::Single(Symbol {
            fill: Rgba::rgb(0x9C, 0xC6, 0xE0),
            stroke: Rgba::rgb(0x6E, 0xA4, 0xC8),
            stroke_width: 0.5,
            size: 0.0,
            marker: Marker::Circle,
        });
    }
    if let Some(l) = rivers.and_then(|id| map.layer_mut(id)) {
        l.renderer = Renderer::Single(Symbol {
            fill: Rgba::rgb(0x4F, 0x8F, 0xC6),
            stroke: Rgba::rgb(0x4F, 0x8F, 0xC6),
            stroke_width: 1.2,
            size: 0.0,
            marker: Marker::Circle,
        });
    }
    if let Some(l) = cities.and_then(|id| map.layer_mut(id)) {
        let base = Symbol {
            fill: Rgba::new(0xE0, 0x6A, 0x3B, 220),
            stroke: Rgba::rgb(255, 255, 255),
            stroke_width: 0.8,
            size: 6.0,
            marker: Marker::Circle,
        };
        if let Ok(r) = Renderer::proportional(&l.dataset, &[], "pop_max", base) {
            l.renderer = r;
            if let Renderer::Proportional { min_size, max_size, .. } = &mut l.renderer {
                *min_size = 3.5;
                *max_size = 18.0;
            }
        }
        l.labels.default.field = "name".into();
        l.labels.default.size = 8.0;
        l.labels.default.bold = true;
        l.labels.default.color = Rgba::rgb(0x5A, 0x2A, 0x14);
        l.max_scale = None;
    }
    map.bookmarks = vec![
        Bookmark::new("World", cam(10.0, 15.0, 26_000.0)),
        Bookmark::new("Europe", cam(12.0, 50.0, 5_200.0)),
        Bookmark::new("Africa", cam(19.0, 2.0, 8_500.0)),
        Bookmark::new("Americas", cam(-80.0, 12.0, 15_000.0)),
        Bookmark::new("East Asia", cam(118.0, 32.0, 6_500.0)),
    ];
    map.camera = cam(10.0, 18.0, 26_000.0);
    p
}
