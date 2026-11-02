//! A cloud-optimised GeoTIFF as a raster layer's pixels, read in place: the same raster a file
//! gives, its symbology, statistics and tools alike, without downloading it.
//!
//! A layer holds an [`overview`]: every band of the finest overview that fits a budget, which its
//! statistics, histograms and tools read, as a file too large to hold whole is opened from an
//! overview. Drawn zoomed in past it, the part in view is read at the level the view needs as a
//! [`window`], a level's tiles at a time, from the tiles kept or fetched by range; the map draws
//! again as they land.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use rustgis_data::raster::{ColorHint, Raster};
use web_time::Instant;

use super::{BUDGET, Bands, Info, RETRY, TileState, fetch_all, get_tile, split, tiles, with_info};

/// The most samples (every band) of the overview a layer holds.
const OVERVIEW_SAMPLES: usize = if cfg!(target_arch = "wasm32") { 1 << 20 } else { 2 << 20 };
/// The most chunks read for it.
const OVERVIEW_CHUNKS: usize = 512;
/// The most chunks of the coarsest level, held when no level fits [`OVERVIEW_SAMPLES`] (a file
/// with few or no overviews); its samples are bounded by half the tiles kept.
const FALLBACK_CHUNKS: usize = 4096;
/// The most samples of one band of a view's window (a large screen's worth, and more).
pub const WINDOW_SAMPLES: usize = 16 << 20;

/// The level a layer holds as its overview: the finest whose every band fits [`OVERVIEW_SAMPLES`]
/// in at most [`OVERVIEW_CHUNKS`] chunks, else the coarsest when it fits half the tiles kept in at
/// most [`FALLBACK_CHUNKS`]; `None` when not even that does (a large image without overviews).
pub fn overview_level(info: &Info) -> Option<usize> {
    let layout = &info.layout;
    let bands = layout.bands.max(1);
    let planes = if layout.planar { bands } else { 1 };
    // Samples decoded, whole tiles (an edge tile's padding too), and chunks read.
    let size = |l: &rustgis_data::raster::cog::Level| {
        let tile = (l.tile_width as usize).saturating_mul(l.tile_height as usize);
        let samples = (l.tiles() as usize).saturating_mul(tile).saturating_mul(bands);
        (samples, (l.tiles() as usize).saturating_mul(planes))
    };
    layout
        .levels
        .iter()
        .position(|l| {
            let (samples, chunks) = size(l);
            samples <= OVERVIEW_SAMPLES && chunks <= OVERVIEW_CHUNKS
        })
        .or_else(|| {
            let last = layout.levels.len().checked_sub(1)?;
            let (samples, chunks) = size(layout.levels.get(last)?);
            (samples <= BUDGET / 2 && chunks <= FALLBACK_CHUNKS).then_some(last)
        })
}

/// Reads a file's overview (see [`overview_level`]) as a raster, every band, then calls `done`.
/// The raster says it is an overview of the full image when it is one.
pub fn overview(url: &str, done: Box<dyn FnOnce(Result<Raster, String>) + Send>) {
    let file = split(url).0.to_string();
    let f = file.clone();
    with_info(
        &file,
        Box::new(move |r| match r {
            Ok(info) => read_overview(info, f, done),
            Err(e) => done(Err(e)),
        }),
    );
}

/// [`overview`], waited for (natively; at most two minutes).
#[cfg(not(target_arch = "wasm32"))]
pub fn overview_blocking(url: &str) -> Result<Raster, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    overview(url, Box::new(move |r| tx.send(r).unwrap_or_default()));
    rx.recv_timeout(std::time::Duration::from_secs(120)).map_err(|_| format!("{}: no answer in two minutes", split(url).0))?
}

fn read_overview(info: Arc<Info>, url: String, done: Box<dyn FnOnce(Result<Raster, String>) + Send>) {
    let Some(coarsest) = info.layout.levels.last() else {
        done(Err(format!("{url}: the GeoTIFF has no image")));
        return;
    };
    let Some((level, l)) = overview_level(&info).and_then(|i| Some((i, info.layout.levels.get(i)?))) else {
        done(Err(format!(
            "{url}: its smallest image, {} × {} with {} band(s), is too large to hold as the layer's overview: \
             a GeoTIFF with overviews (as GDAL's COG driver or gdaladdo make) is read in place",
            coarsest.width, coarsest.height, info.layout.bands
        )));
        return;
    };
    let bands: Vec<usize> = (0..info.layout.bands.max(1)).collect();
    let planes = if info.layout.planar { bands.len() } else { 1 };
    let Some(n) = l.tiles().checked_mul(u32::try_from(planes).unwrap_or(u32::MAX)) else {
        done(Err(format!("{url}: too many tiles")));
        return;
    };
    let (w, h) = (l.width, l.height);
    let i = info.clone();
    fetch_all(
        info,
        url.clone(),
        level,
        (0..n).collect(),
        Box::new(move |r| {
            done(r.and_then(|got| {
                let (data, _) = assemble(&i, level, (0, w), (0, h), &bands, |c| got.get(c as usize).cloned());
                let l = i.layout.levels.get(level).ok_or("no such overview")?;
                let mut r = Raster::new(
                    file_name(&url),
                    w,
                    h,
                    data,
                    i.layout.transform_of(l),
                    i.layout.crs,
                    i.layout.nodata,
                    i.layout.color.clone(),
                    i.layout.data_type,
                )
                .map_err(|e| format!("{url}: {e}"))?;
                if let Some(full) = i.layout.levels.first().filter(|_| level > 0) {
                    r.overview = Some((full.width, full.height));
                }
                r.palette_missing = i.layout.palette_missing;
                r.source = Some(url.clone());
                Ok(r)
            }))
        }),
    );
}

/// A file's name, from its address (without its query).
fn file_name(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url).trim_end_matches('/');
    path.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("raster").to_string()
}

/// The chunks a window of a level needs, of the bands given (each band of a tile its own chunk
/// when they are stored apart, else one chunk a tile).
fn chunks_of(info: &Info, level: usize, cols: (u32, u32), rows: (u32, u32), bands: &[usize]) -> HashSet<u32> {
    let mut chunks = HashSet::new();
    let Some(l) = info.layout.levels.get(level) else { return chunks };
    let (tw, th) = (l.tile_width.max(1), l.tile_height.max(1));
    for ty in rows.0 / th..rows.1.min(l.height).div_ceil(th) {
        for tx in cols.0 / tw..cols.1.min(l.width).div_ceil(tw) {
            let tile = ty * l.across() + tx;
            for &b in bands {
                if let Some((c, _)) = info.layout.chunk(l, tile, b) {
                    chunks.insert(c);
                }
            }
        }
    }
    chunks
}

/// Whether a window of a level (see [`window`]) fits the tiles kept: at most half of them, so the
/// tiles of the level before stay while its own arrive. One that doesn't is read from a coarser
/// level.
pub fn fits(url: &str, level: usize, cols: (u32, u32), rows: (u32, u32), bands: &[usize]) -> bool {
    let Ok(Some(info)) = super::info(split(url).0) else { return true };
    let Some(l) = info.layout.levels.get(level) else { return false };
    let per_chunk = (l.tile_width as usize) * (l.tile_height as usize) * info.layout.chunk_bands();
    let n = chunks_of(&info, level, cols, rows, bands).len();
    ((cols.1.saturating_sub(cols.0)) as usize).saturating_mul(rows.1.saturating_sub(rows.0) as usize) <= WINDOW_SAMPLES
        && n.saturating_mul(per_chunk) <= BUDGET / 2
}

/// The part of a level in view, as [`window`] has it now.
#[derive(Debug, Default)]
pub struct Window {
    /// Its pixels (NaN where a tile hasn't arrived); `None` before the file's header has.
    pub raster: Option<Raster>,
    /// Tiles (or the header) are still on their way: the map draws again as they land.
    pub pending: bool,
    /// Tiles in it couldn't be read lately (other levels stand in for them, and they are asked
    /// for again when their wait is over).
    pub failed: bool,
}

/// Tiles whose read failed lately, and until when they aren't asked for again ([`RETRY`] after, or
/// as long as their server asked to be left), so a map drawn again and again doesn't ask for them
/// each time. The map is drawn again when that time comes.
fn failed() -> &'static Mutex<HashMap<(String, usize, u32), Instant>> {
    static F: OnceLock<Mutex<HashMap<(String, usize, u32), Instant>>> = OnceLock::new();
    F.get_or_init(Mutex::default)
}

/// The most failed tiles remembered.
const MAX_FAILED: usize = 4096;

/// Remembers a failed tile until `until`. With [`MAX_FAILED`] already, those whose wait is over go
/// first; failing that, the one whose wait ends soonest (asked for again a little early, once)
/// makes room, rather than the new one, which would be asked for again at each draw, whatever its
/// server asked for.
fn remember_failed(f: &mut HashMap<(String, usize, u32), Instant>, key: (String, usize, u32), until: Instant) {
    if f.len() >= MAX_FAILED && !f.contains_key(&key) {
        let now = Instant::now();
        f.retain(|_, at| now < *at);
        if f.len() >= MAX_FAILED
            && let Some(soonest) = f.iter().min_by_key(|(_, at)| **at).map(|(k, _)| k.clone())
        {
            f.remove(&soonest);
        }
    }
    f.insert(key, until);
}

/// The columns `cols` and rows `rows` (half-open, in the level's pixels) of level `level` of a
/// file, bands `bands` (0-based, in that order, as the raster's bands), coloured as `color` says:
/// from the tiles kept, without waiting. Those missing are fetched (each once at a time); the
/// tile cache's generation goes up as each lands, so the map is drawn again.
pub fn window(url: &str, level: usize, cols: (u32, u32), rows: (u32, u32), bands: &[usize], color: ColorHint) -> Result<Window, String> {
    let file = split(url).0;
    let Some(info) = super::info(file)? else { return Ok(Window { raster: None, pending: true, failed: false }) };
    let l = info.layout.levels.get(level).ok_or("no such overview")?;
    let (c0, c1) = (cols.0.min(l.width), cols.1.min(l.width));
    let (r0, r1) = (rows.0.min(l.height), rows.1.min(l.height));
    if c0 >= c1 || r0 >= r1 || bands.is_empty() {
        return Ok(Window::default());
    }
    if ((c1 - c0) as usize).saturating_mul((r1 - r0) as usize) > WINDOW_SAMPLES {
        return Err("the view is too large a part of the GeoTIFF to read at once".into());
    }
    if let Some(b) = bands.iter().find(|b| **b >= info.layout.bands.max(1)) {
        return Err(format!("the GeoTIFF has {} bands, not {}", info.layout.bands, b + 1));
    }
    // The chunks the window needs: those kept (marked as used, so they stay), and the others
    // asked for.
    let chunks = chunks_of(&info, level, (c0, c1), (r0, r1), bands);
    let mut kept: HashMap<u32, Bands> = HashMap::new();
    let mut ask = Vec::new();
    {
        let mut t = tiles().lock().unwrap_or_else(|e| e.into_inner());
        for &c in &chunks {
            let key = (file.to_string(), level, c);
            match t.used(&key) {
                Some(b) => {
                    kept.insert(c, b);
                }
                None if t.map.contains_key(&key) => {}
                None => ask.push(c),
            }
        }
    }
    let mut pending = kept.len() < chunks.len();
    let waiting: bool;
    {
        let mut f = failed().lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        f.retain(|_, until| now < *until);
        // Those failed lately wait; the map draws again when the first may be asked for.
        let mut next: Option<Instant> = None;
        ask.retain(|c| match f.get(&(file.to_string(), level, *c)) {
            Some(until) => {
                next = Some(next.map_or(*until, |n| n.min(*until)));
                false
            }
            None => true,
        });
        if let Some(at) = next {
            crate::cache::notify_at(at);
        }
        waiting = next.is_some();
    }
    // Tiles that failed lately aren't waited for (the map would wait for them for nothing).
    if ask.is_empty() && pending {
        let t = tiles().lock().unwrap_or_else(|e| e.into_inner());
        pending = chunks.iter().any(|c| matches!(t.map.get(&(file.to_string(), level, *c)), Some(TileState::Loading(_))));
    }
    for c in ask {
        let key = (file.to_string(), level, c);
        get_tile(
            info.clone(),
            file.to_string(),
            level,
            c,
            Box::new(move |r| {
                if let Err(e) = r {
                    let until = Instant::now() + crate::cache::retry_wait(&e, RETRY);
                    remember_failed(&mut failed().lock().unwrap_or_else(|e| e.into_inner()), key, until);
                    crate::cache::notify_at(until);
                }
                crate::cache::notify();
            }),
        );
        pending = true;
    }
    let (mut data, gaps) = assemble(&info, level, (c0, c1), (r0, r1), bands, |c| kept.get(&c).cloned());
    // Until they arrive, its missing tiles show the levels kept (the one zoomed from), not holes.
    if !gaps.is_empty() {
        // The tiles kept, looked up one at a time: the cache isn't held while the gaps fill.
        let kept = |o: usize, c: u32| tiles().lock().unwrap_or_else(|e| e.into_inner()).used(&(file.to_string(), o, c));
        stand_in(&info, level, (c0, c1), (r0, r1), bands, &mut data, &gaps, kept);
    }
    let t = info.layout.transform_of(l);
    let transform = rustgis_data::raster::GeoTransform {
        origin_x: t.origin_x + f64::from(c0) * t.pixel_width,
        origin_y: t.origin_y + f64::from(r0) * t.pixel_height,
        ..t
    };
    let raster = Raster::unmeasured(
        file_name(file),
        c1 - c0,
        r1 - r0,
        data,
        transform,
        info.layout.crs,
        info.layout.nodata,
        color,
        info.layout.data_type,
    )
    .map_err(|e| e.to_string())?;
    Ok(Window { raster: Some(raster), pending, failed: waiting })
}

/// A part of a window a chunk not kept leaves empty: the band (its place among those read), and
/// its columns and rows (half-open, in the level's pixels).
struct Gap {
    k: usize,
    cols: (u32, u32),
    rows: (u32, u32),
}

/// Fills the `gaps` of a window of `level` from the other levels' tiles kept, the nearest first
/// (the finer, then the coarser, then further): the level zoomed from stands in for the one zoomed
/// to until its tiles arrive. Only the gaps are filled: a tile's own no-data pixels stay empty.
/// `kept` gives a level's chunk when it is kept (marking it used); nothing is fetched.
#[allow(clippy::too_many_arguments)]
fn stand_in(
    info: &Info,
    level: usize,
    cols: (u32, u32),
    rows: (u32, u32),
    bands: &[usize],
    data: &mut [Vec<f32>],
    gaps: &[Gap],
    mut kept: impl FnMut(usize, u32) -> Option<Bands>,
) {
    let levels = &info.layout.levels;
    let Some(l) = levels.get(level) else { return };
    let ww = (cols.1 - cols.0) as usize;
    // The gaps a level kept hasn't filled yet: walked a row at a time, nothing kept per pixel.
    let mut open: Vec<&Gap> = gaps.iter().collect();
    // The other levels, nearest first.
    let others: Vec<usize> =
        (1..levels.len()).flat_map(|d| [level.checked_sub(d), Some(level + d)]).flatten().filter(|o| *o < levels.len()).collect();
    for o in others {
        let Some(ol) = levels.get(o) else { continue };
        let (sx, sy) = (f64::from(ol.width) / f64::from(l.width.max(1)), f64::from(ol.height) / f64::from(l.height.max(1)));
        let (tw, th) = (ol.tile_width.max(1), ol.tile_height.max(1));
        // The chunks of that level found so far (none: not kept).
        let mut found: HashMap<u32, Option<Bands>> = HashMap::new();
        open.retain(|g| {
            let (Some(&b), Some(dst)) = (bands.get(g.k), data.get_mut(g.k)) else { return false };
            // Whether a pixel still empty found that level's tile not kept.
            let mut short = false;
            // The chunk the last pixel came from: its neighbours mostly share it.
            let mut last: Option<(u32, Option<Bands>)> = None;
            for y in g.rows.0..g.rows.1 {
                let oy = (((f64::from(y) + 0.5) * sy) as u32).min(ol.height.saturating_sub(1));
                let row = (y - rows.0) as usize * ww;
                for x in g.cols.0..g.cols.1 {
                    let Some(v) = dst.get_mut(row + (x - cols.0) as usize).filter(|v| v.is_nan()) else { continue };
                    // The pixel of that level under this one's centre.
                    let ox = (((f64::from(x) + 0.5) * sx) as u32).min(ol.width.saturating_sub(1));
                    let tile = (oy / th) * ol.across() + ox / tw;
                    let Some((c, at)) = info.layout.chunk(ol, tile, b) else {
                        short = true;
                        continue;
                    };
                    if last.as_ref().is_none_or(|(lc, _)| *lc != c) {
                        let chunk = found.entry(c).or_insert_with(|| kept(o, c)).clone();
                        last = Some((c, chunk));
                    }
                    let value = last
                        .as_ref()
                        .and_then(|(_, ch)| ch.as_ref())
                        .and_then(|ch| ch.get(at))
                        .and_then(|p| p.get(((oy % th) * tw + ox % tw) as usize));
                    match value {
                        // That level's value, no data included: it is what the image has there.
                        Some(s) => *v = *s,
                        None => short = true,
                    }
                }
            }
            short
        });
        if open.is_empty() {
            return;
        }
    }
}

/// The pixels of columns `cols` and rows `rows` (half-open) of a level, bands `bands` in that
/// order, each row by row, from the chunks `chunk` gives (NaN where it gives none), and the gaps
/// the chunks it doesn't give leave.
fn assemble(
    info: &Info,
    level: usize,
    cols: (u32, u32),
    rows: (u32, u32),
    bands: &[usize],
    chunk: impl Fn(u32) -> Option<Bands>,
) -> (Vec<Vec<f32>>, Vec<Gap>) {
    let (ww, wh) = ((cols.1 - cols.0) as usize, (rows.1 - rows.0) as usize);
    let mut out = vec![vec![f32::NAN; ww * wh]; bands.len()];
    let mut gaps = Vec::new();
    let Some(l) = info.layout.levels.get(level) else { return (out, gaps) };
    let (tw, th) = (l.tile_width.max(1), l.tile_height.max(1));
    for ty in rows.0 / th..rows.1.div_ceil(th) {
        for tx in cols.0 / tw..cols.1.div_ceil(tw) {
            let tile = ty * l.across() + tx;
            // The tile's part of the window, in the level's pixels.
            let (x0, x1) = ((tx * tw).max(cols.0), ((tx + 1) * tw).min(cols.1));
            let (y0, y1) = ((ty * th).max(rows.0), ((ty + 1) * th).min(rows.1));
            for (k, &b) in bands.iter().enumerate() {
                let Some((c, at)) = info.layout.chunk(l, tile, b) else { continue };
                let Some(data) = chunk(c) else {
                    gaps.push(Gap { k, cols: (x0, x1), rows: (y0, y1) });
                    continue;
                };
                let (Some(plane), Some(dst)) = (data.get(at), out.get_mut(k)) else { continue };
                for y in y0..y1 {
                    let src = ((y - ty * th) * tw + (x0 - tx * tw)) as usize;
                    let to = (y - rows.0) as usize * ww + (x0 - cols.0) as usize;
                    let n = (x1 - x0) as usize;
                    if let (Some(s), Some(d)) = (plane.get(src..src + n), dst.get_mut(to..to + n)) {
                        d.copy_from_slice(s);
                    }
                }
            }
        }
    }
    (out, gaps)
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::Tiles;
    use rustgis_data::raster::cog::{Example, Level, Parse, Sparse, example_with, layout};

    /// A file's info, as reading its header gives it.
    fn info_of(file: Vec<u8>) -> Info {
        let Ok(Parse::Done(layout)) = layout(&Sparse::default().with(0, file)) else { panic!("not a COG") };
        let transformer = layout.crs.transformer().unwrap();
        Info {
            layout: *layout,
            header: Sparse::default(),
            transformer,
            bounds: [0.0; 4],
            merc: [0.0; 4],
            min_zoom: 0,
            max_zoom: 0,
            stretch: Mutex::default(),
            stretching: Mutex::default(),
            stretch_failed: Mutex::default(),
            alpha: None,
        }
    }

    /// The overview held is the finest small one; without one, the coarsest if it fits half the
    /// tiles kept; else none, rather than reading a large image whole.
    #[test]
    fn the_overview_held_is_bounded() {
        let mut info = info_of(example_with(&Example { size: 4096, tile: 256, ..Example::default() }));
        // 4096, 2048, 1024 … a side: 1024 a side is a million values.
        assert_eq!(overview_level(&info).and_then(|l| info.layout.levels.get(l)).map(|l| l.width), Some(1024));
        let first = info.layout.levels[0].clone();
        let level = |side: u32| Level { width: side, height: side, ..first.clone() };
        // Only the full image, 4096 a side (16 million values in 256 tiles): held.
        info.layout.levels = vec![level(4096)];
        assert_eq!(overview_level(&info), Some(0));
        // One 20 000 a side (400 million values in 6241 tiles): none.
        info.layout.levels = vec![level(20_000)];
        assert_eq!(overview_level(&info), None);
    }

    /// A window's missing tiles are filled from the level kept, but not its own no-data pixels.
    #[test]
    fn stand_ins_fill_only_the_tiles_missing() {
        let info = info_of(example_with(&Example { size: 64, tile: 16, ..Example::default() }));
        let file = "test://stand-in.tif";
        // Level 1 (32 a side) kept: its tile 0 is level 0's pixels 0 to 32, all 7.
        let mut t = Tiles::default();
        let _ = t.keep((file.to_string(), 1, 0), Arc::new(vec![vec![7.0; 16 * 16]]), 256);
        // Level 0's tile 0 here, one pixel of it no data; tile 1, right of it, not yet.
        let mut own = vec![3.0; 16 * 16];
        own[16 + 1] = f32::NAN;
        let own: Bands = Arc::new(vec![own]);
        let (cols, rows) = ((0, 32), (0, 16));
        let (mut data, gaps) = assemble(&info, 0, cols, rows, &[0], |c| (c == 0).then(|| own.clone()));
        assert_eq!(gaps.len(), 1);
        stand_in(&info, 0, cols, rows, &[0], &mut data, &gaps, |o, c| t.used(&(file.to_string(), o, c)));
        let at = |x: usize, y: usize| data[0][y * 32 + x];
        assert_eq!((at(0, 0), at(20, 5), at(31, 15)), (3.0, 7.0, 7.0), "tile 1 from level 1");
        assert!(at(1, 1).is_nan(), "tile 0's no-data pixel stays empty");
    }

    /// A full memory of failed tiles makes room for a new one: the one whose wait ends soonest
    /// goes, so every tile failing waits its turn.
    #[test]
    fn failed_tiles_make_room_for_the_newest() {
        let mut f = HashMap::new();
        let now = Instant::now();
        let key = |i: u32| ("https://x.org/a.tif".to_string(), 0usize, i);
        for i in 0..MAX_FAILED as u32 {
            remember_failed(&mut f, key(i), now + std::time::Duration::from_secs(60 + u64::from(i)));
        }
        remember_failed(&mut f, key(u32::MAX), now + std::time::Duration::from_secs(30));
        assert_eq!(f.len(), MAX_FAILED);
        assert!(f.contains_key(&key(u32::MAX)) && !f.contains_key(&key(0)), "the soonest gone, the newest kept");
        // One already remembered is waited for again, nothing else goes.
        remember_failed(&mut f, key(5), now + std::time::Duration::from_secs(90));
        assert!(f.contains_key(&key(1)) && f.len() == MAX_FAILED);
    }

    #[test]
    fn names_from_addresses() {
        assert_eq!(file_name("https://x.org/a/scene.tif?sig=1"), "scene.tif");
        assert_eq!(file_name("https://api.maxar.com/analytics/v1/raster/ortho/geotiff?function=f"), "geotiff");
        assert_eq!(file_name("https://x.org/"), "x.org");
    }
}
