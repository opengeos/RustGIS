//! The tile cache: every tile and document fetched, decoded, by address.
//!
//! Drawing never waits. [`get`] answers what the cache holds and, for an address it doesn't hold
//! yet, starts fetching it; when the bytes arrive they are decoded, [`generation`] goes up and
//! the waker (the app's repaint) is called, so the map draws again with the new tile. Exports
//! and the command line, which can't draw a half-loaded map, [`wait`] for the fetches first.
//!
//! Natively, tiles are fetched on a few worker threads, the newest request first (the view the
//! user is looking at now, not the one they panned away from). The browser app installs its own
//! fetcher with [`set_fetcher`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tiny_skia::Pixmap;
use web_time::{Duration, Instant};

use crate::mvt;

/// What a fetched address is decoded as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// A PNG, JPEG or WebP tile.
    Image,
    /// A Mapbox Vector Tile.
    Vector,
    /// A JSON document (a style or TileJSON).
    Json,
}

/// A decoded fetch.
pub enum Payload {
    Image(Pixmap),
    Vector(mvt::Tile),
    Json(serde_json::Value),
    /// No content (a 204, or an empty body): nothing to draw.
    Empty,
}

/// What the cache knows about an address.
#[derive(Clone)]
pub enum State {
    Ready(Arc<Payload>),
    Pending,
    Failed(String),
}

/// Called once with a fetch's bytes or error.
pub type Done = Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>;
type Fetcher = Box<dyn Fn(String, Done) + Send + Sync>;

/// Bytes the cache keeps before dropping the least recently used entries.
const BUDGET: usize = if cfg!(target_arch = "wasm32") { 160 << 20 } else { 384 << 20 };
/// A failed address is tried again after this long.
const RETRY: Duration = Duration::from_secs(60);
/// The largest tile or document fetched.
pub const MAX_BYTES: usize = 16 << 20;
/// The error a fetcher gives a request it dropped unfetched (the view moved on): the address is
/// forgotten rather than failed, so it is fetched again when it is next wanted.
pub const DROPPED: &str = "dropped: the view moved on";

enum Entry {
    Ready(Arc<Payload>, usize),
    Pending,
    Failed(String, Instant),
}

struct Slot {
    entry: Entry,
    used: u64,
}

#[derive(Default)]
struct Cache {
    slots: HashMap<String, Slot>,
    bytes: usize,
    tick: u64,
}

fn cache() -> &'static Mutex<Cache> {
    static C: OnceLock<Mutex<Cache>> = OnceLock::new();
    C.get_or_init(Mutex::default)
}

fn lock() -> std::sync::MutexGuard<'static, Cache> {
    cache().lock().unwrap_or_else(|e| e.into_inner())
}

static GENERATION: AtomicU64 = AtomicU64::new(0);
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

fn fetcher() -> &'static Mutex<Option<Arc<Fetcher>>> {
    static F: OnceLock<Mutex<Option<Arc<Fetcher>>>> = OnceLock::new();
    F.get_or_init(Mutex::default)
}

fn waker() -> &'static Mutex<Option<Arc<dyn Fn() + Send + Sync>>> {
    static W: OnceLock<Mutex<Option<Arc<dyn Fn() + Send + Sync>>>> = OnceLock::new();
    W.get_or_init(Mutex::default)
}

/// Replaces how addresses are fetched (the browser's `fetch`). `done` must be called once.
pub fn set_fetcher(f: impl Fn(String, Done) + Send + Sync + 'static) {
    *fetcher().lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(Box::new(f)));
}

/// Sets what is called whenever a fetch finishes (the app asks for a repaint).
pub fn set_waker(f: impl Fn() + Send + Sync + 'static) {
    *waker().lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(f));
}

/// Goes up each time a fetch finishes: a map drawn at an older generation may now draw more.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// How many fetches are under way.
pub fn in_flight() -> usize {
    IN_FLIGHT.load(Ordering::Relaxed)
}

/// What the cache holds for `url`, fetching it when it holds nothing (or a failure old enough
/// to try again).
pub fn get(url: &str, want: Want) -> State {
    let mut c = lock();
    c.tick += 1;
    let tick = c.tick;
    if let Some(slot) = c.slots.get_mut(url) {
        slot.used = tick;
        match &slot.entry {
            Entry::Ready(p, _) => return State::Ready(p.clone()),
            Entry::Pending => return State::Pending,
            Entry::Failed(e, at) if at.elapsed() < RETRY => return State::Failed(e.clone()),
            Entry::Failed(..) => {}
        }
    }
    c.slots.insert(url.to_string(), Slot { entry: Entry::Pending, used: tick });
    drop(c);
    start(url.to_string(), want);
    State::Pending
}

/// What the cache holds for `url`, without fetching it.
pub fn peek(url: &str) -> Option<Arc<Payload>> {
    let mut c = lock();
    c.tick += 1;
    let tick = c.tick;
    let slot = c.slots.get_mut(url)?;
    match &slot.entry {
        Entry::Ready(p, _) => {
            slot.used = tick;
            Some(p.clone())
        }
        _ => None,
    }
}

/// Forgets everything (and failures, so they are tried again).
pub fn clear() {
    let mut c = lock();
    c.slots.retain(|_, s| matches!(s.entry, Entry::Pending));
    c.bytes = 0;
    drop(c);
    // Styles and TileJSON documents are read again too.
    crate::source::forget();
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// Waits (up to `timeout`) for the fetches under way to finish. Native only: in the browser
/// fetches finish between frames, so this returns at once.
pub fn wait(timeout: Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let start = Instant::now();
        while in_flight() > 0 && start.elapsed() < timeout {
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }
    #[cfg(target_arch = "wasm32")]
    let _ = timeout;
}

fn start(url: String, want: Want) {
    IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
    let key = url.clone();
    let done: Done = Box::new(move |bytes| {
        if matches!(&bytes, Err(e) if e == DROPPED) {
            lock().slots.remove(&key);
            finished();
            return;
        }
        let entry = match bytes.and_then(|b| decode(&b, want)) {
            Ok(p) => {
                let size = payload_bytes(&p);
                Entry::Ready(Arc::new(p), size)
            }
            Err(e) => {
                log::debug!("{key}: {e}");
                Entry::Failed(e, Instant::now())
            }
        };
        store(&key, entry);
        finished();
    });
    let f = fetcher().lock().unwrap_or_else(|e| e.into_inner()).clone();
    match f {
        Some(f) => f(url, done),
        None => native::fetch(url, done),
    }
}

/// A fetch is over: the map can draw again.
fn finished() {
    IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
    GENERATION.fetch_add(1, Ordering::Relaxed);
    let w = waker().lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(w) = w {
        w();
    }
}

fn store(url: &str, entry: Entry) {
    let mut c = lock();
    let added = if let Entry::Ready(_, n) = &entry { *n } else { 0 };
    // Dropped while it was fetched (cleared, or evicted): kept anyway, it was asked for.
    let tick = c.tick;
    let old = c.slots.insert(url.to_string(), Slot { entry, used: tick });
    if let Some(Slot { entry: Entry::Ready(_, n), .. }) = old {
        c.bytes = c.bytes.saturating_sub(n);
    }
    c.bytes += added;
    if c.bytes > BUDGET {
        evict(&mut c);
    }
}

/// Drops the least recently used entries until the cache is at three quarters of its budget.
fn evict(c: &mut Cache) {
    let mut ready: Vec<(u64, String, usize)> = c
        .slots
        .iter()
        .filter_map(|(k, s)| match s.entry {
            Entry::Ready(_, n) => Some((s.used, k.clone(), n)),
            Entry::Failed(..) => Some((s.used, k.clone(), 0)),
            Entry::Pending => None,
        })
        .collect();
    ready.sort_unstable_by_key(|r| r.0);
    for (_, k, n) in ready {
        if c.bytes <= BUDGET / 4 * 3 {
            break;
        }
        c.slots.remove(&k);
        c.bytes = c.bytes.saturating_sub(n);
    }
}

fn payload_bytes(p: &Payload) -> usize {
    match p {
        Payload::Image(pm) => pm.data().len(),
        Payload::Vector(t) => t
            .layers
            .iter()
            .flat_map(|l| &l.features)
            .map(|f| 64 + f.geometry.iter().map(|g| g.len() * 8 + 24).sum::<usize>() + f.props.len() * 48)
            .sum(),
        Payload::Json(_) => 64 << 10,
        Payload::Empty => 64,
    }
}

/// Decodes fetched bytes.
pub fn decode(bytes: &[u8], want: Want) -> Result<Payload, String> {
    if bytes.is_empty() {
        return Ok(Payload::Empty);
    }
    if bytes.len() > MAX_BYTES {
        return Err(format!("{} bytes is too large for a tile", bytes.len()));
    }
    match want {
        Want::Image => decode_image(bytes).map(Payload::Image),
        Want::Vector => mvt::decode(bytes).map(Payload::Vector),
        Want::Json => serde_json::from_slice(bytes).map(Payload::Json).map_err(|e| format!("not JSON: {e}")),
    }
}

/// Decodes a PNG, JPEG or WebP picture into a premultiplied pixmap (at most 4096 pixels a side).
pub fn decode_image(bytes: &[u8]) -> Result<Pixmap, String> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(96 << 20);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| e.to_string())?.into_rgba8();
    let (w, h) = img.dimensions();
    let mut data = img.into_raw();
    for px in data.as_chunks_mut::<4>().0 {
        let a = u16::from(px[3]);
        if a < 255 {
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
    Pixmap::from_vec(data, tiny_skia::IntSize::from_wh(w, h).ok_or("an empty picture")?).ok_or_else(|| "a picture of the wrong size".into())
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    //! Worker threads fetching with `ureq`, newest request first.

    use std::sync::{Condvar, Mutex, OnceLock};

    use super::{DROPPED, Done, MAX_BYTES};

    const WORKERS: usize = 8;
    /// Requests waiting beyond this many are dropped, oldest first: the view moved on.
    const QUEUE: usize = 512;

    struct Queue {
        jobs: Mutex<Vec<(String, Done)>>,
        ready: Condvar,
    }

    fn queue() -> &'static Queue {
        static Q: OnceLock<Queue> = OnceLock::new();
        Q.get_or_init(|| {
            for i in 0..WORKERS {
                let spawned = std::thread::Builder::new().name(format!("rustgis-tiles-{i}")).spawn(work);
                if let Err(e) = spawned {
                    log::error!("could not start a tile fetcher: {e}");
                }
            }
            Queue { jobs: Mutex::new(Vec::new()), ready: Condvar::new() }
        })
    }

    pub(super) fn fetch(url: String, done: Done) {
        let q = queue();
        let dropped = {
            let mut jobs = q.jobs.lock().unwrap_or_else(|e| e.into_inner());
            jobs.push((url, done));
            (jobs.len() > QUEUE).then(|| jobs.remove(0))
        };
        q.ready.notify_one();
        // Answered once the queue is unlocked: `done` takes the cache's lock and wakes the app.
        if let Some((_, done)) = dropped {
            done(Err(DROPPED.into()));
        }
    }

    fn work() {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(30)))
            .user_agent(concat!("RustGIS/", env!("CARGO_PKG_VERSION"), " (+https://rustgis.app)"))
            // Redirects are followed by `get`, which checks where each one leads.
            .max_redirects(0)
            .build()
            .into();
        let q = queue();
        loop {
            let job = {
                let mut jobs = q.jobs.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if let Some(j) = jobs.pop() {
                        break j;
                    }
                    jobs = q.ready.wait(jobs).unwrap_or_else(|e| e.into_inner());
                }
            };
            let (url, done) = job;
            done(get(&agent, &url));
        }
    }

    fn get(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, String> {
        use std::io::Read;
        let mut at = url.to_string();
        let mut redirects = 0;
        let mut resp = loop {
            let resp = agent.get(&at).call().map_err(|e| e.to_string())?;
            if !resp.status().is_redirection() {
                break resp;
            }
            let location = resp.headers().get("location").and_then(|v| v.to_str().ok()).ok_or("a redirect with no Location")?;
            let next = crate::template::join(&at, location);
            // Where a redirect leads is the server's choice, not the user's.
            crate::address::check(&next, Some(&at))?;
            // Never from HTTPS down to plain HTTP.
            if at.starts_with("https://") && !next.starts_with("https://") {
                return Err(format!("{next}: a redirect from https to http"));
            }
            redirects += 1;
            if redirects > 5 {
                return Err("too many redirects".into());
            }
            at = next;
        };
        if resp.status().as_u16() == 204 {
            return Ok(Vec::new());
        }
        let mut buf = Vec::new();
        resp.body_mut().with_config().limit(MAX_BYTES as u64 + 1).reader().read_to_end(&mut buf).map_err(|e| e.to_string())?;
        Ok(buf)
    }
}

#[cfg(target_arch = "wasm32")]
mod native {
    use super::Done;

    pub(super) fn fetch(url: String, done: Done) {
        done(Err(format!("{url}: this build has no way to fetch tiles")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_pictures_and_documents() {
        let mut pm = Pixmap::new(2, 2).unwrap_or_else(|| panic!("pixmap"));
        pm.fill(tiny_skia::Color::from_rgba8(200, 100, 50, 128));
        let png = pm.encode_png().unwrap_or_default();
        let Payload::Image(back) = decode(&png, Want::Image).unwrap_or(Payload::Empty) else { panic!("not an image") };
        assert_eq!((back.width(), back.height()), (2, 2));
        assert!(matches!(decode(b"{\"a\": 1}", Want::Json), Ok(Payload::Json(_))));
        assert!(matches!(decode(b"", Want::Image), Ok(Payload::Empty)));
        assert!(decode(b"not a picture", Want::Image).is_err());
        assert!(decode(b"{", Want::Json).is_err());
    }

    #[test]
    fn fetches_through_the_fetcher_once() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        set_fetcher(|url, done| {
            CALLS.fetch_add(1, Ordering::Relaxed);
            if url.ends_with("ok.json") { done(Ok(b"[1, 2]".to_vec())) } else { done(Err("404".into())) }
        });
        let g = generation();
        assert!(matches!(get("test://cache/ok.json", Want::Json), State::Pending));
        assert!(generation() > g);
        assert!(matches!(get("test://cache/ok.json", Want::Json), State::Ready(_)));
        assert!(peek("test://cache/ok.json").is_some());
        assert!(matches!(get("test://cache/missing.json", Want::Json), State::Pending));
        assert!(matches!(get("test://cache/missing.json", Want::Json), State::Failed(_)));
        assert!(peek("test://cache/never.json").is_none());
        // A request the fetcher dropped is forgotten, not failed: asked for again, it is fetched.
        set_fetcher(|url, done| {
            CALLS.fetch_add(1, Ordering::Relaxed);
            if url.ends_with("first.json") { done(Err(DROPPED.into())) } else { done(Ok(b"1".to_vec())) }
        });
        assert!(matches!(get("test://cache/first.json", Want::Json), State::Pending));
        assert!(matches!(get("test://cache/first.json", Want::Json), State::Pending), "fetched again");
        assert_eq!(CALLS.load(Ordering::Relaxed), 4);
    }
}
