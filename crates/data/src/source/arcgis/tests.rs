use super::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::esripbf::tests::encode::{answer, feature, field, geometry, lonlat, number, text, transform, uint};

/// A points layer of 400 features on a 20 by 20 grid, 0.05° apart from (-100, 40): object id `i`
/// (from 1) at column `(i - 1) % 20`, row `(i - 1) / 20`.
const FEATURES: u64 = 400;

fn position(id: u64) -> (f64, f64) {
    (-100.0 + ((id - 1) % 20) as f64 * 0.05, 40.0 + ((id - 1) / 20) as f64 * 0.05)
}

/// What a fake service was asked.
#[derive(Default)]
struct Asked {
    count: AtomicUsize,
    urls: Mutex<Vec<String>>,
}

impl Asked {
    fn since(&self, n: usize) -> Vec<String> {
        self.urls.lock().unwrap().get(n..).unwrap_or_default().to_vec()
    }
}

/// How a fake service answers.
#[derive(Clone, Copy)]
struct Opts {
    /// Its pages' length.
    page: usize,
    /// Its queries take standardized SQL (`MOD`).
    sql: bool,
    /// Its description, when not the usual one.
    description: Option<&'static str>,
    /// Its points have Z values (100).
    z: bool,
    /// It answers every page from the start, whatever its offset.
    ignores_offset: bool,
    /// An empty page names no fields.
    empty_without_fields: bool,
    /// Each feature has a text field of 1.5 to 10.5 KB, by its object id.
    padded: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { page: 30, sql: true, description: None, z: false, ignores_offset: false, empty_without_fields: false, padded: false }
    }
}

/// Serves the layer at `https://<host>/arcgis/rest/services/P/FeatureServer/0` of [`FEATURES`]
/// points with [`service_with`]'s usual answers, its pages `page` long, its queries taking
/// standardized SQL when `sql`, its description `description` when given; returns its query and
/// what it was asked.
fn service(host: &str, page: usize, sql: bool, description: Option<&'static str>) -> (String, Arc<Asked>) {
    service_with(host, Opts { page, sql, description, ..Opts::default() })
}

/// [`service`], answering as `opts` says.
fn service_with(host: &str, opts: Opts) -> (String, Arc<Asked>) {
    let Opts { page, sql, description, z, ignores_offset, empty_without_fields, padded } = opts;
    let layer = format!("https://{host}/arcgis/rest/services/P/FeatureServer/0");
    let asked = Arc::new(Asked::default());
    let a = asked.clone();
    remote::testing::handle(&format!("https://{host}/"), move |url| {
        a.count.fetch_add(1, Ordering::Relaxed);
        a.urls.lock().unwrap().push(url.to_string());
        let value = |k: &str| crate::ogc::query_value(url, k).map(crate::address::percent_decode);
        if !url.contains("/query") {
            return Some(match description {
                Some(d) => d.as_bytes().to_vec(),
                None => format!(
                    r#"{{"name":"Points","objectIdField":"OBJECTID","maxRecordCount":{page},"supportedQueryFormats":"JSON, geoJSON, PBF",
                    "useStandardizedQueries":{sql},"advancedQueryCapabilities":{{"supportsPagination":true}}}}"#
                )
                .into_bytes(),
            });
        }
        let within = value("geometry").map(|g| g.split(',').map(|v| v.parse::<f64>().unwrap()).collect::<Vec<_>>());
        let step = value("where")
            .and_then(|w| w.split("MOD(OBJECTID,").nth(1).map(|r| r.split(')').next().unwrap().parse::<u64>().unwrap()))
            .unwrap_or(1);
        if step > 1 && !sql {
            return Some(br#"{"error":{"code":400,"message":"Unable to complete operation."}}"#.to_vec());
        }
        let none = value("where").is_some_and(|w| w.contains("1=0"));
        let ids: Vec<u64> = (1..=FEATURES)
            .filter(|_| !none)
            .filter(|id| id % step == 0)
            .filter(|id| {
                let (x, y) = position(*id);
                within.as_ref().is_none_or(|g| x >= g[0] && y >= g[1] && x <= g[2] && y <= g[3])
            })
            .collect();
        if value("returnCountOnly").as_deref() == Some("true") {
            let extent = r#","extent":{"xmin":-100,"ymin":40,"xmax":-99.05,"ymax":40.95,"spatialReference":{"wkid":4326}}"#;
            let extent = if value("returnExtentOnly").is_some() { extent } else { "" };
            return Some(format!(r#"{{"count":{}{extent}}}"#, ids.len()).into_bytes());
        }
        let offset: usize = if ignores_offset { 0 } else { value("resultOffset").map_or(0, |o| o.parse().unwrap()) };
        let size: usize = value("resultRecordCount").map_or(page, |o| o.parse().unwrap()).min(page);
        let sent: Vec<u64> = ids.iter().skip(offset).take(size).copied().collect();
        let geometry_too = value("returnGeometry").as_deref() != Some("false");
        let mut parts = vec![number(7, 0), lonlat(), transform(false, 0.01, 0.0, 0.0)];
        if !(sent.is_empty() && empty_without_fields) {
            parts.push(field("OBJECTID", 6));
            if padded {
                parts.push(field("NOTE", 4));
            }
        }
        if z {
            parts.insert(1, number(10, 1));
        }
        if ignores_offset || offset + sent.len() < ids.len() {
            parts.push(number(9, 1));
        }
        for id in sent {
            let (x, y) = position(id);
            let mut at = vec![(x * 100.0).round() as i64, (y * 100.0).round() as i64];
            if z {
                at.push(100);
            }
            let mut values = vec![uint(id)];
            if padded {
                values.push(text(&"x".repeat((id % 7 + 1) as usize * 1500)));
            }
            if geometry_too {
                values.push(geometry(&[], &at));
            }
            parts.push(feature(&values));
        }
        Some(answer(&parts))
    });
    (format!("{layer}/query?where=1%3D1&outFields=*&f=geojson"), asked)
}

fn ids(batch: &Batch) -> Vec<u64> {
    let mut ids: Vec<u64> = batch.features.iter().map(|f| f.id).collect();
    ids.sort_unstable();
    ids
}

fn read(src: &ArcgisSource, req: Request) -> Result<Batch> {
    let src: Arc<dyn FeatureSource> = Arc::new(open_blocking(src.remote.url()).unwrap());
    super::super::read_blocking(&src, &req)
}

/// The view of columns 2..=5 and rows 3..=6: 16 features.
fn small_view() -> Extent {
    Extent::new(-99.91, 40.14, -99.74, 40.31)
}

#[test]
fn a_view_is_read_whole_over_several_pages_with_object_ids_as_ids() {
    let (query, _) = service("live-view.test", 3, true, None);
    let src = open_blocking(&query).unwrap();
    assert_eq!((src.info.format, src.info.count, src.info.crs.as_str()), (FORMAT, Some(FEATURES), "EPSG:4326"));
    assert_eq!(src.info.extent, Extent::new(-100.0, 40.0, -99.05, 40.95));
    let b = read(&src, Request { extent: Some(small_view()), limit: 1000 }).unwrap();
    let want: Vec<u64> = (3..=6).flat_map(|row| (2..=5).map(move |col| row * 20 + col + 1)).collect();
    assert_eq!(ids(&b), want);
    assert!(b.complete && b.matched == 16);
}

#[test]
fn a_view_holding_more_than_the_limit_reads_every_kth_feature_spread_over_it() {
    let (query, _) = service("live-sample.test", 30, true, None);
    let src = open_blocking(&query).unwrap();
    let b = read(&src, Request { extent: Some(Extent::new(-101.0, 39.0, -99.0, 41.0)), limit: 50 }).unwrap();
    assert!(!b.complete && b.matched == FEATURES);
    // 400 over 50 is 8: every 8th object id, across the whole grid.
    assert!(b.features.len() == 50 && b.features.iter().all(|f| f.id % 8 == 0), "{:?}", ids(&b));
    let rows: HashSet<u64> = b.features.iter().map(|f| (f.id - 1) / 20).collect();
    assert_eq!(rows.len(), 20);
    // Every feature, without an extent, is read in object id order.
    let all = read(&src, Request::all()).unwrap();
    assert!(all.complete && all.features.iter().map(|f| f.id).eq(1..=FEATURES));
}

#[test]
fn a_view_read_again_asks_the_service_nothing() {
    let (query, asked) = service("live-again.test", 5, true, None);
    let src: Arc<dyn FeatureSource> = Arc::new(open_blocking(&query).unwrap());
    let req = Request { extent: Some(small_view()), limit: 1000 };
    let first = super::super::read_blocking(&src, &req).unwrap();
    let before = asked.count.load(Ordering::Relaxed);
    let again = super::super::read_blocking(&src, &req).unwrap();
    assert_eq!(asked.since(before), Vec::<String>::new());
    assert_eq!(ids(&first), ids(&again));
}

#[test]
fn a_service_that_cant_sample_gives_its_first_features_and_says_so() {
    let (query, _) = service("live-nosql.test", 30, false, None);
    let src = open_blocking(&query).unwrap();
    assert!(src.info.notes.iter().any(|n| n.contains("standardized SQL")), "{:?}", src.info.notes);
    let b = read(&src, Request { extent: Some(Extent::new(-101.0, 39.0, -99.0, 41.0)), limit: 50 }).unwrap();
    assert!(!b.complete && b.matched == FEATURES && !b.features.is_empty() && b.features.len() <= 50);
}

#[test]
fn every_feature_is_visited_a_page_at_a_time_in_object_id_order() {
    let (query, _) = service("live-visit.test", 30, true, None);
    let src = open_blocking(&query).unwrap();
    let mut seen = Vec::new();
    let mut chunks = 0;
    let b = src
        .visit_chunks(None, 4 << 10, &mut |c| {
            chunks += 1;
            seen.extend(c.iter().map(|f| f.id));
            Ok(())
        })
        .unwrap();
    assert!(b.complete && b.matched == FEATURES && chunks > 1);
    assert!(seen.into_iter().eq(1..=FEATURES));
    // Within an extent, only the features in it.
    let mut inside = Vec::new();
    src.visit_chunks(Some(small_view()), 4 << 10, &mut |c| {
        inside.extend(c.iter().map(|f| f.id));
        Ok(())
    })
    .unwrap();
    assert_eq!(inside.len(), 16);
}

#[test]
fn layers_that_cant_be_read_a_view_at_a_time_are_refused_to_be_downloaded() {
    let unpaged = r#"{"objectIdField":"OBJECTID","maxRecordCount":5,"supportedQueryFormats":"JSON, PBF"}"#;
    let geojson_only = r#"{"objectIdField":"OBJECTID","maxRecordCount":5,"supportedQueryFormats":"JSON, geoJSON",
        "advancedQueryCapabilities":{"supportsPagination":true}}"#;
    for (host, d) in [("live-unpaged.test", unpaged), ("live-geojson.test", geojson_only)] {
        let (query, _) = service(host, 5, true, Some(d));
        let e = open_blocking(&query).unwrap_err();
        assert!(matches!(&e, DataError::Unsupported(m) if m.contains(NOT_LIVE)), "{e}");
    }
    let (query, _) = service("live-window.test", 5, true, None);
    let e = open_blocking(&format!("{query}&resultOffset=10")).unwrap_err();
    assert!(e.to_string().contains(NOT_LIVE), "{e}");
}

#[test]
fn hostile_answers_are_errors_and_other_services_are_never_asked() {
    for (host, d) in [("live-junk.test", "not json"), ("live-refused.test", r#"{"error":{"code":499,"message":"Token Required"}}"#)] {
        let (query, _) = service(host, 5, true, Some(d));
        assert!(open_blocking(&query).is_err());
    }
    let (query, _) = service("live-origin.test", 5, true, None);
    let file = remote::file(&query);
    assert!(file.answer("https://elsewhere.test/arcgis/rest/services/P/FeatureServer/0/query?f=pbf").is_err());
    assert!(file.answer("http://live-origin.test/arcgis/rest/services/P/FeatureServer/0/query?f=pbf").is_err());
}

#[test]
fn views_anywhere_never_panic_and_ask_for_a_few_cells() {
    let (query, asked) = service("live-views.test", 30, true, None);
    let src: Arc<dyn FeatureSource> = Arc::new(open_blocking(&query).unwrap());
    let views = [
        Extent::new(-180.0, -90.0, 180.0, 90.0),
        Extent::new(-1e300, -1e300, 1e300, 1e300),
        Extent::new(-99.9, 40.1, -99.9, 40.1),
        Extent::new(179.9999999, 89.9999999, 180.0, 90.0),
        Extent::new(200.0, 0.0, 210.0, 10.0),
    ];
    for v in views {
        let before = asked.count.load(Ordering::Relaxed);
        let b = super::super::read_blocking(&src, &Request { extent: Some(v), limit: 100 });
        assert!(b.is_ok(), "{v:?}: {b:?}");
        assert!(asked.count.load(Ordering::Relaxed) - before <= 64, "{v:?}");
    }
    for v in views.iter().filter_map(|v| world_view(v).unwrap()) {
        assert!(cells(&v).len() <= 25, "{v:?}");
    }
    // Outside the world: nothing, said to be all of it.
    let outside = super::super::read_blocking(&src, &Request { extent: Some(Extent::new(200.0, 0.0, 210.0, 10.0)), limit: 100 }).unwrap();
    assert!(outside.complete && outside.features.is_empty());
    // No view at all: an error, never an empty answer, and nothing asked.
    let before = asked.count.load(Ordering::Relaxed);
    for v in [Extent::new(f64::NAN, 0.0, 1.0, 1.0), Extent::new(0.0, f64::INFINITY, 1.0, 1.0), Extent::new(5.0, 5.0, -5.0, -5.0)] {
        assert!(super::super::read_blocking(&src, &Request { extent: Some(v), limit: 100 }).is_err(), "{v:?}");
        let mut visit = |_: Vec<Feature>| Ok(());
        assert!(src.visit_chunks(Some(v), 4 << 10, &mut visit).is_err(), "{v:?}");
    }
    assert_eq!(asked.count.load(Ordering::Relaxed), before);
}

#[test]
fn what_pages_lose_is_said_by_each_read_and_none_when_nothing_is() {
    let (query, _) = service_with("live-z.test", Opts { z: true, ..Opts::default() });
    let src = open_blocking(&query).unwrap();
    assert!(src.info.zm_dropped);
    let b = read(&src, Request { extent: Some(small_view()), limit: 1000 }).unwrap();
    // The pages of the view's cells: at least its 16 features.
    assert!(b.notes.len() == 1 && b.notes[0].contains("features had Z or M values"), "{:?}", b.notes);
    let mut visit = |_: Vec<Feature>| Ok(());
    let all = src.visit_chunks(None, 4 << 10, &mut visit).unwrap();
    assert_eq!(all.notes, vec![crate::zm_note(Some(FEATURES))]);
    // Clean pages: no notes.
    let (query, _) = service("live-clean.test", 30, true, None);
    let src = open_blocking(&query).unwrap();
    assert!(src.info.notes.is_empty(), "{:?}", src.info.notes);
    assert!(read(&src, Request { extent: Some(small_view()), limit: 1000 }).unwrap().notes.is_empty());
}

#[test]
fn a_service_that_ignores_offsets_stops_a_visit_with_an_error() {
    let (query, _) = service_with("live-offsets.test", Opts { ignores_offset: true, ..Opts::default() });
    let src = open_blocking(&query).unwrap();
    let mut pages = 0;
    let e = src
        .visit_chunks(None, 4 << 10, &mut |_| {
            pages += 1;
            Ok(())
        })
        .unwrap_err();
    assert!(e.to_string().contains("past the 400 features"), "{e}");
    assert!(pages < 100);
}

#[test]
fn sampling_steps_are_powers_of_two() {
    assert_eq!([step_for(0, 10), step_for(10, 10), step_for(11, 10), step_for(400, 50), step_for(401, 50)], [1, 1, 2, 8, 16]);
    assert_eq!(step_for(u64::MAX, 1), MAX_STEP);
}

#[test]
fn a_query_without_geometry_still_reads_its_views() {
    let (query, _) = service("live-nogeometry.test", 30, true, None);
    let src = open_blocking(&format!("{query}&returnGeometry=false")).unwrap();
    let b = read(&src, Request { extent: Some(small_view()), limit: 1000 }).unwrap();
    assert!(b.complete && b.features.len() == 16 && b.features.iter().all(|f| f.geometry.is_some()));
}

#[test]
fn empty_pages_naming_no_fields_are_read_as_empty() {
    let (query, _) = service_with("live-emptypages.test", Opts { empty_without_fields: true, ..Opts::default() });
    let src = open_blocking(&query).unwrap();
    // Every 256th: one feature (256) in all, so most cells' sampled pages are empty.
    let b = read(&src, Request { extent: Some(Extent::new(-101.0, 39.0, -99.0, 41.0)), limit: 2 }).unwrap();
    assert_eq!(ids(&b), vec![256]);
    assert!(!b.complete);
}

#[test]
fn a_budgeted_view_stays_within_its_bytes_as_it_is_read_and_spread_over_it() {
    let (query, _) = service("live-budget.test", 30, true, None);
    let src = open_blocking(&query).unwrap();
    let whole = Request { extent: Some(Extent::new(-101.0, 39.0, -99.0, 41.0)), limit: 1000 };
    let all = read(&src, Request { extent: whole.extent, limit: 1000 }).unwrap();
    let bytes = super::super::memory::features_bytes(&all.features) / 5;
    let shared: Arc<dyn FeatureSource> = Arc::new(open_blocking(&query).unwrap());
    // Fetched first, as the map's reads are, then read within the budget.
    super::super::read_blocking(&shared, &whole).unwrap();
    let b = src.read_budgeted(&whole, bytes).unwrap();
    assert!(!b.complete && !b.features.is_empty());
    assert!(
        super::super::memory::features_bytes(&b.features) <= bytes,
        "{} features, {} bytes of {bytes}; all {} in {}",
        b.features.len(),
        super::super::memory::features_bytes(&b.features),
        all.features.len(),
        super::super::memory::features_bytes(&all.features)
    );
    let rows: HashSet<u64> = b.features.iter().map(|f| (f.id - 1) / 20).collect();
    assert!(rows.len() >= 10, "{rows:?}");
}

#[test]
fn a_layer_matching_nothing_opens_and_reads_empty() {
    let (query, _) = service_with("live-empty.test", Opts { empty_without_fields: true, ..Opts::default() });
    let src = open_blocking(&query.replace("where=1%3D1", "where=1%3D0")).unwrap();
    assert_eq!(src.info.count, Some(0));
    let b = read(&src, Request { extent: Some(small_view()), limit: 1000 }).unwrap();
    assert!(b.complete && b.features.is_empty() && b.matched == 0);
    let mut visited = 0;
    let all = src
        .visit_chunks(None, 4 << 10, &mut |c| {
            visited += c.len();
            Ok(())
        })
        .unwrap();
    assert!(all.complete && visited == 0);
}

#[test]
fn a_view_larger_than_the_answers_kept_is_read_in_part_without_running_out_of_rounds() {
    // 400 features of 1.5 to 10.5 KB: about 2.4 MB of uneven answers, past what a service keeps
    // in tests (2 MB). The service offers pages of 30, up to 315 KB; they are asked for shorter,
    // under the largest answer (64 KB).
    let (query, _) = service_with("live-large.test", Opts { page: 30, padded: true, ..Opts::default() });
    let src = open_blocking(&query).unwrap();
    assert!(src.page < 30 && src.page * 10_600 < remote::MAX_ANSWER as usize, "{}", src.page);
    let b = read(&src, Request { extent: Some(Extent::new(-101.0, 39.0, -99.0, 41.0)), limit: 1000 }).unwrap();
    assert!(!b.complete && b.matched == FEATURES && !b.features.is_empty() && b.features.len() < FEATURES as usize);
    assert_eq!(b.features.iter().map(|f| f.id).collect::<HashSet<_>>().len(), b.features.len());
    // Read again, it asks for what it read before.
    let again = read(&src, Request { extent: Some(Extent::new(-101.0, 39.0, -99.0, 41.0)), limit: 1000 }).unwrap();
    assert_eq!(ids(&again), ids(&b));
}
