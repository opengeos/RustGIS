//! Datum grids RustGIS downloads itself (#2032 A4), when its user turns that on ([`set_enabled`]):
//! the grids a coordinate system's most accurate datum transformation needs, from PROJ's grid
//! server (`https://cdn.proj.org/`), as `projsync` would get them. On a computer they are kept in
//! RustGIS's grid folder ([`crs::grid_dir`]) and read from there on later runs; in the browser they
//! are registered for the run ([`crs::register_grid`]). terrio-crs's own network stays off: a grid
//! is only ever asked for here, never while a frame is drawn.
//!
//! A file read on a thread of its own waits for its system's grids before its features are
//! converted ([`crate::cmd::reads::ReadAhead::run_on_its_thread`]), so they land where the grid puts
//! them. Elsewhere a download starts in the background, and the session says what it brought:
//! rasters, streamed layers and the map follow at once, while a layer read before keeps the places
//! it was given until it is opened again (it is held in longitude/latitude).

use rustgis_data::raster::Crs;
use rustgis_data::raster::crs::{self, MissingGrid};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};

/// The largest grid downloaded: PROJ's largest are a few hundred megabytes (geoid models).
const MAX_GRID: usize = 512 << 20;

/// How long a download may take before it is given up.
#[cfg(not(target_arch = "wasm32"))]
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// How long a file's read waits for its system's grids before it goes on without them (and says so).
#[cfg(not(target_arch = "wasm32"))]
const WAIT: std::time::Duration = std::time::Duration::from_secs(120);

/// Whether the user downloads datum grids (off unless turned on: it reaches a server).
static ENABLED: AtomicBool = AtomicBool::new(false);

/// What a download did, for the session to say ([`take_news`]).
#[derive(Debug, Clone, PartialEq)]
pub enum News {
    /// A grid arrived, with its size in bytes.
    Got { name: String, bytes: usize },
    /// A grid couldn't be had, and why.
    Failed { name: String, error: String },
}

#[derive(Default)]
struct State {
    /// Grids being downloaded now.
    busy: HashSet<String>,
    /// Grids whose download failed this run: not tried again until the setting is turned on again.
    failed: HashSet<String>,
    /// Grids downloaded this run: never downloaded again, even when still listed as missing.
    got: HashSet<String>,
    news: Vec<News>,
}

fn state() -> &'static (Mutex<State>, Condvar) {
    static STATE: OnceLock<(Mutex<State>, Condvar)> = OnceLock::new();
    STATE.get_or_init(Default::default)
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    state().0.lock().unwrap_or_else(|e| e.into_inner())
}

/// How a grid is downloaded natively: its address, the most bytes, how long it may take.
#[cfg(not(target_arch = "wasm32"))]
type Download = fn(&str, usize, std::time::Duration) -> Result<Vec<u8>, String>;

#[cfg(not(target_arch = "wasm32"))]
fn download() -> &'static std::sync::RwLock<Download> {
    static DOWNLOAD: std::sync::RwLock<Download> = std::sync::RwLock::new(crate::render::rustgis_tiles::cache::download_blocking_within);
    &DOWNLOAD
}

/// Downloads grids with `f` instead of from the server: for tests, which reach no network.
#[doc(hidden)]
#[cfg(not(target_arch = "wasm32"))]
pub fn set_download(f: Download) {
    *download().write().unwrap_or_else(|e| e.into_inner()) = f;
}

/// Turns downloading datum grids on or off (the "Download datum grids" setting). Turning it on
/// forgets this run's failures, so they are tried again.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
    if on {
        lock().failed.clear();
    }
    #[cfg(not(target_arch = "wasm32"))]
    crs::set_grid_waiter(if on { Some(wait_for) } else { None });
}

/// Whether datum grids are downloaded.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::SeqCst)
}

/// What the downloads did since this was last asked.
pub fn take_news() -> Vec<News> {
    std::mem::take(&mut lock().news)
}

/// Starts downloading, in the background, the grids `crs` needs that RustGIS doesn't have, and
/// gives the names of those it started or that were already on their way: none when downloads are
/// off, and none for a grid that failed or arrived this run.
pub fn want(crs: Crs) -> Vec<String> {
    if !enabled() {
        return Vec::new();
    }
    let mut coming = Vec::new();
    for grid in crs::missing_grids(crs) {
        match claim(&grid.name) {
            Claim::Mine => {
                coming.push(grid.name.clone());
                start(grid);
            }
            Claim::Coming => coming.push(grid.name),
            Claim::Settled => {}
        }
    }
    coming
}

/// What [`claim`] found of a grid.
enum Claim {
    /// It is this caller's to download.
    Mine,
    /// Another is downloading it.
    Coming,
    /// It arrived or failed this run.
    Settled,
}

/// Marks `name` as being downloaded by the caller, unless it is already, or it arrived or failed
/// this run.
fn claim(name: &str) -> Claim {
    let mut s = lock();
    if s.busy.contains(name) {
        return Claim::Coming;
    }
    if s.failed.contains(name) || s.got.contains(name) {
        return Claim::Settled;
    }
    s.busy.insert(name.to_string());
    Claim::Mine
}

/// Records how `name`'s download went, and wakes those waiting for it.
fn finish(name: &str, outcome: Result<usize, String>) {
    let mut s = lock();
    s.busy.remove(name);
    match outcome {
        Ok(bytes) => {
            s.got.insert(name.to_string());
            s.news.push(News::Got { name: name.to_string(), bytes });
        }
        Err(error) => {
            s.failed.insert(name.to_string());
            s.news.push(News::Failed { name: name.to_string(), error });
        }
    }
    drop(s);
    state().1.notify_all();
    rustgis_map::source::wake();
}

/// Whether a grid's name can be a file name in the grid folder: PROJ's grid names are plain
/// (`us_noaa_conus.tif`), and nothing else is written there.
fn plain(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// Whether `bytes` are a datum grid file, as PROJ's grid server gives them: a GeoTIFF (classic or
/// BigTIFF, either byte order), an NTv2 file or a CTable2 one. Anything else (an error page a
/// network answered with) isn't kept, or it would count as the grid from then on.
fn a_grid(bytes: &[u8]) -> bool {
    [&b"II*\0"[..], b"MM\0*", b"II+\0", b"MM\0+", b"NUM_OREC", b"CTABLE V2"].iter().any(|m| bytes.starts_with(m))
}

/// Makes the downloaded bytes of `name` available: kept in the grid folder on a computer, then
/// the transformers are built again with it.
#[cfg(not(target_arch = "wasm32"))]
fn keep(name: &str, bytes: &[u8]) -> Result<(), String> {
    if !plain(name) {
        return Err("its name isn't a plain file name".into());
    }
    if !a_grid(bytes) {
        return Err("the server's answer isn't a grid file".into());
    }
    let dir = crs::grid_dir().ok_or("RustGIS has no folder to keep it in")?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("the folder {} can't be made: {e}", dir.display()))?;
    rustgis_data::io::write_atomic(&dir.join(name), bytes).map_err(|e| format!("it can't be written to {}: {e}", dir.display()))?;
    crs::grids_changed();
    Ok(())
}

/// Downloads `grid` on a thread of its own.
#[cfg(not(target_arch = "wasm32"))]
fn start(grid: MissingGrid) {
    let name = grid.name.clone();
    let spawned = std::thread::Builder::new().name("datum grid".into()).spawn(move || fetch_now(&grid));
    if let Err(e) = spawned {
        finish(&name, Err(format!("couldn't start a thread: {e}")));
    }
}

/// Downloads `grid` here, and keeps it.
#[cfg(not(target_arch = "wasm32"))]
fn fetch_now(grid: &MissingGrid) {
    let got = std::panic::catch_unwind(|| {
        let download = *download().read().unwrap_or_else(|e| e.into_inner());
        let bytes = download(&grid.url, MAX_GRID, DOWNLOAD_TIMEOUT)?;
        keep(&grid.name, &bytes).map(|()| bytes.len())
    })
    .unwrap_or_else(|_| Err("the download stopped unexpectedly".into()));
    finish(&grid.name, got);
}

/// Downloads `grid` with the browser's fetch, and registers it for the run.
#[cfg(target_arch = "wasm32")]
fn start(grid: MissingGrid) {
    let name = grid.name.clone();
    crate::render::rustgis_tiles::cache::download_limited(
        &grid.url,
        MAX_GRID,
        Box::new(move |r| {
            let got = r.and_then(|bytes| {
                if !plain(&name) {
                    return Err("its name isn't a plain file name".to_string());
                }
                if !a_grid(&bytes) {
                    return Err("the server's answer isn't a grid file".to_string());
                }
                let n = bytes.len();
                crs::register_grid(&name, bytes);
                Ok(n)
            });
            finish(&name, got);
        }),
    );
}

/// Gets the grids `crs` needs before a file in it is read ([`crs::set_grid_waiter`]): each one
/// missing is downloaded on a thread of its own, or was already on its way, and the read waits for
/// them, then asks again, as the next operation's grids come up where the first's don't reach. It
/// waits at most [`WAIT`] in all, and stops waiting when its read is cancelled; the downloads go
/// on, and what they bring is said and used from then on.
#[cfg(not(target_arch = "wasm32"))]
fn wait_for(crs: Crs) {
    let deadline = web_time::Instant::now() + WAIT;
    let control = rustgis_analysis::control::current();
    // A few rounds at most: each brings the grids the operation chosen after the last needs.
    for _ in 0..4 {
        let coming = want(crs);
        if coming.is_empty() {
            return;
        }
        let (lock, ready) = (&state().0, &state().1);
        let mut s = lock.lock().unwrap_or_else(|e| e.into_inner());
        while coming.iter().any(|name| s.busy.contains(name)) {
            let left = deadline.saturating_duration_since(web_time::Instant::now());
            if left.is_zero() || control.as_ref().is_some_and(|c| c.is_cancelled()) {
                return;
            }
            // In slices, to see Cancel.
            s = ready.wait_timeout(s, left.min(std::time::Duration::from_millis(100))).unwrap_or_else(|e| e.into_inner()).0;
        }
        if coming.iter().all(|name| s.failed.contains(name)) {
            return;
        }
    }
}

/// What the session says of a download: the grid, its size and what it changes; or why it failed.
pub fn news_text(news: &News) -> (String, bool) {
    match news {
        News::Got { name, bytes } => (
            format!(
                "Downloaded the datum grid {name} ({}) from PROJ's grid server: places in the coordinate systems that use it are now where PROJ puts them with it. Layers opened before keep their places until they are opened again.",
                size_text(*bytes)
            ),
            false,
        ),
        News::Failed { name, error } => (
            format!(
                "The datum grid {name} couldn't be downloaded ({error}), so the next transformation is used without it; turn Download Datum Grids off and on again to try again."
            ),
            true,
        ),
    }
}

fn size_text(bytes: usize) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 1.0 { format!("{mb:.1} MB") } else { format!("{} KB", bytes.div_ceil(1000)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_names_are_kept() {
        assert!(plain("us_noaa_conus.tif"));
        assert!(plain("ch_swisstopo_CHENyx06_ETRS.tif"));
        for bad in ["", "../a.tif", "a/b.tif", "a\\b.tif", ".hidden", "c:a.tif", "a b.tif"] {
            assert!(!plain(bad), "{bad}");
        }
    }

    #[test]
    fn news_says_the_grid_its_size_and_what_it_changes() {
        let (text, error) = news_text(&News::Got { name: "us_noaa_conus.tif".into(), bytes: 173_029 });
        assert!(!error && text.contains("us_noaa_conus.tif (174 KB)") && text.contains("opened again"), "{text}");
        let (text, error) = news_text(&News::Failed { name: "a.tif".into(), error: "HTTP 404".into() });
        assert!(error && text.contains("a.tif") && text.contains("HTTP 404"), "{text}");
        assert_eq!(size_text(12_345_678), "12.3 MB");
    }
}
