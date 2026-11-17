//! An ArcGIS feature layer read in place, a view at a time (#829): a layer too large to download
//! is asked for the features of the view, as a GeoPackage table or a PostGIS table is read.
//!
//! A view is asked for in cells of a grid fixed to longitude/latitude, at the level where the
//! view spans a few cells a side, so panning asks again only for the cells it reaches and a
//! service's own cache answers what was asked before. Each cell is counted first
//! (`returnCountOnly`); when the view holds more than a read takes, it reads every k-th feature by
//! object id (`MOD(<object id>, k) = 0`, k a power of two, so zooming in keeps what was drawn). A
//! service whose queries don't take that SQL gives its first features by object id instead, and
//! the read says it is incomplete. Pages come as protocol buffers ([`crate::esripbf`]) in object id
//! order, at full precision: geometry rounded to a pixel (`quantizationParameters`) drops the
//! features smaller than one, which left dense areas emptier than sparse ones in a sample.
//!
//! The answers are fetched as a file's bytes are ([`remote`]): a read fails with [`remote::NEED`]
//! until they are in, and [`remote::drive`] fetches them, on the desktop and in the browser. On
//! the desktop, [`FeatureSource::visit_chunks`] pages through every feature at full precision, a
//! page at a time, for tools and exports.
//!
//! A layer read this way needs pages in a fixed order, so it must say it can be paged
//! (`supportsPagination`), name its object id field and answer in protocol buffers. Others are
//! refused with [`NOT_LIVE`], to be downloaded instead.

use std::collections::HashSet;
use std::sync::Arc;

use rustgis_geom::Extent;

use super::remote::{self, Remote};
use super::{Batch, FeatureSource, Request, SourceInfo};
use crate::{DataError, Feature, Result, Value};

/// The format a layer read this way names.
pub const FORMAT: &str = "ArcGIS feature layer";

/// Starts the error of a layer that can't be read a view at a time (it is downloaded instead).
pub const NOT_LIVE: &str = "can't be read a view at a time";

/// The most cells a view spans a side.
const CELLS_A_SIDE: f64 = 4.0;
/// The finest grid level (cells about 2 cm wide).
const DEEPEST: i32 = 30;
/// The longest page asked for, whatever the layer allows.
const LONGEST_PAGE: usize = 4_000;
/// The most pages one read asks for in all: past it the view is sampled more thinly (or, where
/// the service can't sample, read in part).
const MAX_READ_PAGES: usize = if cfg!(target_arch = "wasm32") { 64 } else { 256 };
/// The largest sampling step (every 2^40th feature: a layer would need more features than any
/// service holds to need more).
const MAX_STEP: u64 = 1 << 40;
/// The most bytes of a count's answer read.
const MAX_COUNT_ANSWER: usize = 64 << 10;

/// An ArcGIS feature layer read a view at a time.
pub struct ArcgisSource {
    remote: Arc<Remote>,
    /// The layer's query as given: its `where` and `outFields` are kept.
    query: String,
    info: SourceInfo,
    /// The object id field, by name (it orders the pages and samples the features).
    oid: String,
    /// Its place among the fields read.
    oid_at: usize,
    /// The features a page asks for.
    page: usize,
    /// Whether the layer's queries take standardized SQL (`MOD`), to sample a view.
    sample: bool,
}

impl std::fmt::Debug for ArcgisSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArcgisSource").field("remote", &self.remote).field("page", &self.page).finish_non_exhaustive()
    }
}

/// Opens the layer an ArcGIS query (`…/FeatureServer/0/query?where=…`) reads, fetching its
/// description, count and first feature (off the caller's thread).
pub fn open(query: &str, done: Box<dyn FnOnce(Result<Box<dyn FeatureSource>>) + Send>) {
    let file = remote::file(query);
    let f = file.clone();
    remote::drive(file, Arc::new(move || open_now(f.clone()).map(|s| Box::new(s) as Box<dyn FeatureSource>)), done);
}

/// [`open`], waiting for it (the command line and tools).
#[cfg(not(target_arch = "wasm32"))]
pub fn open_blocking(query: &str) -> Result<ArcgisSource> {
    let file = remote::file(query);
    let f = file.clone();
    remote::drive_blocking(file, Arc::new(move || open_now(f.clone())))
}

/// One attempt at opening the layer `remote`'s address queries, from what is fetched so far:
/// fails with [`remote::NEED`] until its description, count and first feature are in.
pub fn open_now(remote: Arc<Remote>) -> Result<ArcgisSource> {
    let query = remote.url().to_string();
    let refuse = |why: &str| DataError::Unsupported(crate::diagnostics::message_with_address(&query, &format!("{NOT_LIVE}: {why}")));
    if !crate::ogc::is_arcgis_query(&query) {
        return Err(refuse("it isn't an ArcGIS layer's query"));
    }
    const WINDOWS: [&str; 9] = [
        "resultOffset",
        "resultRecordCount",
        "geometry",
        "objectIds",
        "returnCountOnly",
        "returnIdsOnly",
        "returnExtentOnly",
        "groupByFieldsForStatistics",
        "outStatistics",
    ];
    if WINDOWS.iter().any(|k| crate::ogc::query_value(&query, k).is_some()) {
        return Err(refuse("the query asks for a part of the layer of its own"));
    }
    let description = crate::ogc::arcgis_layer_url(&query);
    let counted = with(&query, &[("returnCountOnly", "true"), ("returnExtentOnly", "true"), ("outSR", "4326"), ("f", "json")]);
    let first = with(&features_query(&query), &[("resultRecordCount", "1")]);
    // Asked for together: one round.
    let (d, c, f) = (remote.answer(&description)?, remote.answer(&counted)?, remote.answer(&first)?);
    let (Some(d), Some(c), Some(f)) = (d, c, f) else { return Err(remote::need()) };

    let d = json(&remote, &description, &d, crate::ogc::DESCRIPTION_MAX, "description")?;
    let paging = crate::ogc::arcgis_paging(&d).ok_or_else(|| refuse("the layer doesn't say it can be read in pages"))?;
    let oid = paging.order.ok_or_else(|| refuse("the layer names no object id field"))?;
    if !crate::ogc::supports_pbf(&d) {
        return Err(refuse("the layer doesn't answer in protocol buffers"));
    }
    let page = paging.size.unwrap_or(1_000).clamp(1, LONGEST_PAGE);
    let sample = d.get("useStandardizedQueries").and_then(serde_json::Value::as_bool) == Some(true);

    let c = json(&remote, &counted, &c, MAX_COUNT_ANSWER, "count")?;
    let count = count_of(&c).ok_or_else(|| answer_error(&remote, &counted, "the count isn't a number"))?;
    let extent = c.get("extent").and_then(lonlat_extent).unwrap_or_default();

    let name = d.get("name").and_then(serde_json::Value::as_str).filter(|n| !n.trim().is_empty()).unwrap_or("layer").to_string();
    let first = crate::esripbf::read_page(&f, &name).inspect_err(|_| remote.forget_answer(&first))?;
    let oid_at = first.object_id_field.ok_or_else(|| refuse("its answers hold no object ids"))?;
    let mut info = SourceInfo {
        name,
        format: FORMAT,
        geometry_type: first.dataset.geometry_type,
        fields: first.dataset.fields.clone(),
        extent,
        count: Some(count),
        crs: "EPSG:4326".into(),
        indexed: true,
        notes: Vec::new(),
        zm_dropped: false,
    };
    if first.losses.with_zm > 0 {
        info.drop_zm(None);
    }
    // What the first feature's answer left out or assumed (fields it couldn't read, the coordinate
    // system); each read says what its own features lost ([`Batch::notes`]).
    let mut lost = first.losses;
    lost.with_zm = 0;
    info.notes.extend(first.report.messages.iter().cloned().chain(lost.messages()));
    if !sample {
        info.notes.push("its queries don't take standardized SQL, so a view holding more features than are drawn shows the first ones by object id rather than a sample of all of them".into());
    }
    Ok(ArcgisSource { remote, query, info, oid, oid_at, page, sample })
}

/// `query` with each value of `values` set, in order (so the same read asks for the same address,
/// which a cache answers).
fn with(query: &str, values: &[(&str, &str)]) -> String {
    values.iter().fold(query.to_string(), |q, (k, v)| crate::ogc::with_query_value(&q, k, v))
}

/// The layer's query asking for features in protocol buffers, in longitude/latitude, with every
/// field unless it names its own.
fn features_query(query: &str) -> String {
    // With geometry, whatever the query said: a view is chosen by it.
    let q = with(query, &[("outSR", "4326"), ("f", "pbf"), ("returnGeometry", "true")]);
    if crate::ogc::query_value(&q, "outFields").is_some_and(|f| !f.is_empty()) { q } else { with(&q, &[("outFields", "*")]) }
}

/// An answer read as JSON (at most `max` bytes): the service's error, or why it isn't JSON, said.
/// One that can't be read is dropped, so that a later read asks for it again.
fn json(remote: &Remote, url: &str, bytes: &[u8], max: usize, what: &str) -> Result<serde_json::Value> {
    if bytes.len() > max {
        return Err(answer_error(remote, url, &format!("the layer's {what} is larger than {} KB", max >> 10)));
    }
    let v: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| answer_error(remote, url, &format!("the layer's {what} isn't JSON")))?;
    if let Some(e) = crate::ogc::arcgis_error_text(&v) {
        return Err(answer_error(remote, url, &e));
    }
    Ok(v)
}

/// The error of an answer that can't be read, dropping it so a later read asks again.
fn answer_error(remote: &Remote, url: &str, why: &str) -> DataError {
    remote.forget_answer(url);
    DataError::Format(crate::diagnostics::message_with_address(remote.url(), why))
}

/// A count's answer (`{"count": n}`).
fn count_of(v: &serde_json::Value) -> Option<u64> {
    v.get("count")?.as_u64()
}

/// An extent the service gave in longitude/latitude (`outSR=4326`), kept within the world;
/// `None` when it isn't one.
fn lonlat_extent(v: &serde_json::Value) -> Option<Extent> {
    let n = |k: &str| v.get(k).and_then(serde_json::Value::as_f64).filter(|x| x.is_finite());
    let e = Extent::new(n("xmin")?.max(-180.0), n("ymin")?.max(-90.0), n("xmax")?.min(180.0), n("ymax")?.min(90.0));
    (e.min_x <= e.max_x && e.min_y <= e.max_y).then_some(e)
}

/// The cells of the grid a view is asked for in: those of the level where it spans at most
/// [`CELLS_A_SIDE`] a side, as `(level, column, row)`, with their extents.
fn cells(view: &Extent) -> Vec<Extent> {
    let side = view.width().max(view.height());
    let level = if side > 0.0 { (360.0 * CELLS_A_SIDE / side).log2().floor() as i32 } else { DEEPEST };
    let size = 360.0 / f64::from(1u32 << level.clamp(0, DEEPEST));
    let span = |lo: f64, hi: f64, origin: f64, most: f64| {
        let first = ((lo - origin) / size).floor().max(0.0);
        let last = ((hi - origin) / size).floor().min(((most - origin) / size).ceil() - 1.0).max(first);
        (first as u64, last as u64)
    };
    let (x0, x1) = span(view.min_x, view.max_x, -180.0, 180.0);
    let (y0, y1) = span(view.min_y, view.max_y, -90.0, 90.0);
    let mut out = Vec::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (min_x, min_y) = (-180.0 + x as f64 * size, -90.0 + y as f64 * size);
            out.push(Extent::new(min_x, min_y, (min_x + size).min(180.0), (min_y + size).min(90.0)));
        }
    }
    out
}

/// A view kept within the world; `None` when it lies outside it (nothing is in it). A view that
/// isn't finite, or whose minimum is past its maximum, is refused: it is no view at all.
fn world_view(e: &Extent) -> Result<Option<Extent>> {
    if [e.min_x, e.min_y, e.max_x, e.max_y].iter().any(|v| !v.is_finite()) || e.min_x > e.max_x || e.min_y > e.max_y {
        return Err(DataError::Format(format!("the view {e:?} isn't an extent of finite longitudes and latitudes")));
    }
    let v = Extent::new(e.min_x.max(-180.0), e.min_y.max(-90.0), e.max_x.min(180.0), e.max_y.min(90.0));
    Ok((v.min_x <= v.max_x && v.min_y <= v.max_y).then_some(v))
}

/// The smallest power of two `k` for which every k-th of `matched` features fits in `limit`.
fn step_for(matched: u64, limit: usize) -> u64 {
    let limit = (limit.max(1)) as u64;
    matched.div_ceil(limit).max(1).checked_next_power_of_two().unwrap_or(MAX_STEP).min(MAX_STEP)
}

impl ArcgisSource {
    /// The query of the features in `cell` (all of them for none), every `step`-th by object id.
    fn query_of(&self, cell: Option<&Extent>, step: u64) -> String {
        let q = features_query(&self.query);
        let q = match cell {
            Some(c) => with(
                &q,
                &[
                    ("geometry", &format!("{},{},{},{}", c.min_x, c.min_y, c.max_x, c.max_y)),
                    ("geometryType", "esriGeometryEnvelope"),
                    ("inSR", "4326"),
                    ("spatialRel", "esriSpatialRelIntersects"),
                ],
            ),
            None => q,
        };
        if step <= 1 {
            return q;
        }
        let given = crate::ogc::query_value(&q, "where")
            .map(|w| crate::address::percent_decode(&w.replace('+', " ")))
            .filter(|w| !w.trim().is_empty());
        let mut clause = format!("MOD({},{step})=0", self.oid);
        if let Some(given) = given {
            clause = format!("({given}) AND {clause}");
        }
        with(&q, &[("where", &clause)])
    }

    /// The count of the features in `cell` (or all of them).
    fn count_url(&self, cell: Option<&Extent>) -> String {
        with(&self.query_of(cell, 1), &[("returnCountOnly", "true"), ("f", "json")])
    }

    /// A page of the features in `cell`, every `step`-th, from `offset`.
    fn page_url(&self, cell: Option<&Extent>, step: u64, offset: usize) -> String {
        with(
            &self.query_of(cell, step),
            &[("orderByFields", &self.oid), ("resultOffset", &offset.to_string()), ("resultRecordCount", &self.page.to_string())],
        )
    }

    /// [`FeatureSource::read`], keeping the features read within `bytes` as they come.
    fn read_within(&self, req: &Request, bytes: usize) -> Result<Batch> {
        let view = match req.extent {
            Some(e) => match world_view(&e)? {
                Some(v) => Some(v),
                None => return Ok(Batch { complete: true, ..Batch::default() }),
            },
            None => None,
        };
        let cells: Vec<Option<Extent>> = match &view {
            Some(v) => cells(v).into_iter().map(Some).collect(),
            None => vec![None],
        };
        // The view's own count (what it matched), and each cell's (what reading it takes),
        // asked for in one round.
        let matched = self.count(view.as_ref())?;
        let mut counts = Vec::with_capacity(cells.len());
        for c in &cells {
            counts.push(self.count(c.as_ref())?);
        }
        let (Some(matched), Some(counts)) = (matched, counts.into_iter().collect::<Option<Vec<u64>>>()) else {
            return Err(remote::need());
        };
        if req.limit == 0 || matched == 0 {
            return Ok(Batch { matched, complete: matched == 0, ..Batch::default() });
        }
        let limit = super::clamp_limit(req.limit);
        let page = self.page as u64;
        let pages_at = |step: u64| counts.iter().map(|n| n.div_ceil(step).div_ceil(page)).fold(0u64, u64::saturating_add);
        let mut step = if self.sample { step_for(counts.iter().sum::<u64>(), limit) } else { 1 };
        while self.sample && step < MAX_STEP && pages_at(step) > MAX_READ_PAGES as u64 {
            step *= 2;
        }
        // Where the service can't sample: each cell's first features, its share of the limit.
        let share = if self.sample { usize::MAX } else { limit.div_ceil(cells.len()) };
        let mut budget = MAX_READ_PAGES;
        // The answers a read holds at once stay within half of what a service keeps, or the
        // pages it needs would push each other out: past it, the view is read in part.
        let room = remote::KEEP_ANSWERS / 2;
        let (mut held, mut present, mut present_bytes) = (0usize, 0usize, 0usize);
        // Features kept within `bytes` as they come: every other one dropped, and every other
        // one after it, each time they pass it.
        let (mut stride, mut counted, mut kept_bytes, mut thinned) = (1u64, 0u64, 0usize, false);
        // A kept feature's memory, its place in the list included (as the list holds it once
        // it is fitted to its length).
        let cost = |f: &Feature| std::mem::size_of::<Feature>().saturating_add(super::memory::feature_bytes(f));
        let mut seen = HashSet::new();
        let mut features = Vec::new();
        let (mut skipped, mut waiting, mut cut) = (0, false, false);
        let mut losses = crate::esripbf::Losses::default();
        for (cell, n) in cells.iter().zip(&counts) {
            if *n == 0 {
                continue;
            }
            let wanted = usize::try_from(n.div_ceil(step)).unwrap_or(usize::MAX).min(share);
            let mut planned = wanted.div_ceil(self.page).max(1);
            let (mut offset, mut asked, mut kept) = (0usize, 0usize, 0usize);
            // A page of this cell isn't in yet: the offsets of those after it are guesses.
            let mut gap = false;
            while asked < planned {
                // A page not in yet is taken to be as large as those that are.
                let guess = present_bytes.checked_div(present).unwrap_or(256 << 10);
                if budget == 0 || held.saturating_add(guess) > room {
                    budget = 0;
                    cut = true;
                    break;
                }
                budget -= 1;
                asked += 1;
                let url = self.page_url(cell.as_ref(), step, offset);
                let Some(answer) = self.remote.answer(&url)? else {
                    // Asked for with the pages after it, as if it were full: when it isn't, those
                    // are asked for again at the right offsets once it is in.
                    held = held.saturating_add(guess);
                    waiting = true;
                    gap = true;
                    offset = offset.saturating_add(self.page);
                    continue;
                };
                held = held.saturating_add(answer.len());
                present += 1;
                present_bytes = present_bytes.saturating_add(answer.len());
                if gap {
                    // An earlier page isn't in: this one's offset may be wrong, read it later.
                    offset = offset.saturating_add(self.page);
                    continue;
                }
                let (got, more, sent) = self.page_of(&url, &answer, &mut skipped, &mut losses)?;
                offset = offset.saturating_add(sent);
                for f in got {
                    let inside = match (&view, &f.geometry) {
                        (Some(v), Some(g)) => g.extent().intersects(v),
                        (Some(_), None) => false,
                        (None, _) => true,
                    };
                    if !(inside && seen.insert(f.id)) {
                        continue;
                    }
                    kept += 1;
                    counted += 1;
                    if counted % stride != 0 {
                        continue;
                    }
                    kept_bytes = kept_bytes.saturating_add(cost(&f));
                    features.push(f);
                    while kept_bytes > bytes && features.len() > 1 {
                        super::memory::halve(&mut features);
                        kept_bytes = features.iter().map(cost).fold(0, usize::saturating_add);
                        stride = stride.saturating_mul(2);
                        thinned = true;
                    }
                }
                if !more {
                    break;
                }
                if sent == 0 || kept >= share {
                    cut = true;
                    break;
                }
                if asked == planned {
                    planned += 1;
                }
            }
        }
        if waiting {
            return Err(remote::need());
        }
        let mut complete = step == 1 && !cut && !thinned;
        if features.len() > limit {
            // A sample comes out a little over its share now and then: thinned evenly.
            let (n, limit) = (features.len() as u64, limit as u64);
            let kept = |i: u64| i.saturating_mul(limit) / n != (i + 1).saturating_mul(limit) / n;
            features = features.into_iter().enumerate().filter(|(i, _)| kept(*i as u64)).map(|(_, f)| f).collect();
            complete = false;
        }
        let matched = if complete { features.len() as u64 } else { matched };
        Ok(Batch { features, matched, complete, skipped, notes: losses.messages(), ..Batch::default() })
    }

    /// A count's answer, or `None` when it isn't fetched yet (noted as missing).
    fn count(&self, cell: Option<&Extent>) -> Result<Option<u64>> {
        let url = self.count_url(cell);
        let Some(bytes) = self.remote.answer(&url)? else { return Ok(None) };
        let v = json(&self.remote, &url, &bytes, MAX_COUNT_ANSWER, "count")?;
        count_of(&v).map(Some).ok_or_else(|| answer_error(&self.remote, &url, "the count isn't a number"))
    }

    /// A page read: its features with their object ids as their ids (one without is skipped),
    /// whether the service holds more, and how many features it sent. What its features lost is
    /// added to `losses`.
    fn page_of(
        &self,
        url: &str,
        bytes: &[u8],
        skipped: &mut usize,
        losses: &mut crate::esripbf::Losses,
    ) -> Result<(Vec<Feature>, bool, usize)> {
        let page = crate::esripbf::read_page(bytes, &self.info.name).inspect_err(|_| self.remote.forget_answer(url))?;
        // An empty page may name no fields: there is nothing in it to misread.
        if !page.dataset.features.is_empty() && (page.dataset.fields != self.info.fields || page.object_id_field != Some(self.oid_at)) {
            self.remote.forget_answer(url);
            return Err(DataError::Format(crate::diagnostics::message_with_address(
                &self.query,
                "the layer's fields changed since it was added; add it again",
            )));
        }
        *skipped = skipped.saturating_add(page.report.skipped);
        losses.add(&page.losses);
        let sent = page.dataset.features.len();
        let mut out = Vec::with_capacity(sent);
        for mut f in page.dataset.features.into_vec() {
            match f.attrs.get(self.oid_at) {
                Some(Value::Int(id)) if *id >= 0 => {
                    f.id = *id as u64;
                    out.push(f);
                }
                _ => *skipped = skipped.saturating_add(1),
            }
        }
        Ok((out, page.exceeded_transfer_limit, sent))
    }
}

impl FeatureSource for ArcgisSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn remote(&self) -> Option<Arc<Remote>> {
        Some(self.remote.clone())
    }

    /// The features of the view, as the module says: complete when every one was read, otherwise
    /// every k-th by object id (or the first, where the service can't sample), `matched` the count
    /// of the view. Without an extent, the whole layer is the view: every feature in object id
    /// order (as [`FeatureSource::visit_chunks`] visits them) when they fit the limit, otherwise a
    /// sample as above. What the features read lost is said in [`Batch::notes`].
    fn read(&self, req: &Request) -> Result<Batch> {
        self.read_within(req, usize::MAX)
    }

    /// A read, kept within `bytes` as it is read: a sample is asked for once, at the size the view
    /// takes, not probed first, as each size asks the service for other pages.
    fn read_budgeted(&self, req: &Request, bytes: usize) -> Result<Batch> {
        let mut batch = self.read_within(req, bytes)?;
        batch.features.shrink_to_fit();
        if batch.features.len() == 1 && super::memory::features_bytes(&batch.features) > bytes {
            return Err(DataError::Format("one feature exceeds the view memory budget; load it explicitly for full processing".into()));
        }
        Ok(batch)
    }

    /// On the desktop: tools and exports page through the layer.
    fn visits_chunks(&self) -> bool {
        cfg!(not(target_arch = "wasm32"))
    }

    /// Every feature (in `extent`, when given) in object id order, a page at a
    /// time: each page is fetched while the visit waits, read, visited and let go. Checks Cancel
    /// before each page; fetching one page is a step without a check (two minutes at most).
    fn visit_chunks(&self, extent: Option<Extent>, bytes: usize, visit: &mut dyn FnMut(Vec<Feature>) -> Result<()>) -> Result<Batch> {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (extent, bytes, visit);
            Err(DataError::Unsupported("an ArcGIS layer read a chunk at a time in the browser".into()))
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut chunks = super::chunk_memory::Chunker::new(bytes, visit)?;
            let view = match extent {
                Some(e) => match world_view(&e)? {
                    Some(v) => Some(v),
                    None => return Ok(Batch { complete: true, ..Batch::default() }),
                },
                None => None,
            };
            let fetch = |url: String| {
                let r = self.remote.clone();
                remote::drive_blocking(self.remote.clone(), Arc::new(move || r.answer(&url)?.ok_or_else(remote::need)))
            };
            let counted = self.count_url(view.as_ref());
            let total = fetch(counted.clone()).and_then(|b| {
                let v = json(&self.remote, &counted, &b, MAX_COUNT_ANSWER, "count")?;
                count_of(&v).ok_or_else(|| answer_error(&self.remote, &counted, "the count isn't a number"))
            })?;
            crate::progress::begin(usize::try_from(total).unwrap_or(usize::MAX));
            let (mut offset, mut skipped, mut visited) = (0usize, 0usize, 0u64);
            let mut losses = crate::esripbf::Losses::default();
            loop {
                crate::progress::check()?;
                let url = self.page_url(view.as_ref(), 1, offset);
                let answer = fetch(url.clone())?;
                // Not kept: the layer is read once, through.
                self.remote.forget_answer(&url);
                let (got, more, sent) = self.page_of(&url, &answer, &mut skipped, &mut losses)?;
                drop(answer);
                for f in got {
                    if view.as_ref().is_none_or(|v| f.geometry.as_ref().is_some_and(|g| g.extent().intersects(v))) {
                        visited += 1;
                        chunks.push(f)?;
                    }
                }
                crate::progress::step(sent)?;
                if !more {
                    break;
                }
                if sent == 0 {
                    return Err(DataError::Format(crate::diagnostics::message_with_address(
                        &self.query,
                        "the service said it held more features but sent none",
                    )));
                }
                offset = offset.saturating_add(sent);
                // A service that ignores the offset would send pages for ever.
                if offset as u64 > total.saturating_add(self.page as u64) {
                    return Err(DataError::Format(crate::diagnostics::message_with_address(
                        &self.query,
                        &format!("the service kept sending pages past the {total} features it counted"),
                    )));
                }
            }
            chunks.flush()?;
            Ok(Batch { matched: visited, complete: true, skipped, notes: losses.messages(), ..Batch::default() })
        }
    }
}

#[cfg(test)]
mod tests;
