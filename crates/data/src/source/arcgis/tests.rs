use super::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::esripbf::tests::encode::{answer, feature, field, geometry, lonlat, number, transform, uint};

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

/// Serves the layer at `https://<host>/arcgis/rest/services/P/FeatureServer/0`, its pages
/// `page` long, its queries taking standardized SQL when `sql`, its description `description`
/// when given; returns its query and what it was asked.
fn service(host: &str, page: usize, sql: bool, description: Option<&'static str>) -> (String, Arc<Asked>) {
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
        let ids: Vec<u64> = (1..=FEATURES)
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
        let offset: usize = value("resultOffset").map_or(0, |o| o.parse().unwrap());
        let size: usize = value("resultRecordCount").map_or(page, |o| o.parse().unwrap()).min(page);
        let sent: Vec<u64> = ids.iter().skip(offset).take(size).copied().collect();
        let mut parts = vec![number(7, 0), lonlat(), transform(false, 0.01, 0.0, 0.0), field("OBJECTID", 6)];
        if offset + sent.len() < ids.len() {
            parts.push(number(9, 1));
        }
        for id in sent {
            let (x, y) = position(id);
            parts.push(feature(&[uint(id), geometry(&[], &[(x * 100.0).round() as i64, (y * 100.0).round() as i64])]));
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
        Extent::new(f64::NAN, 0.0, 1.0, 1.0),
        Extent::new(5.0, 5.0, -5.0, -5.0),
        Extent::new(-99.9, 40.1, -99.9, 40.1),
        Extent::new(179.9999999, 89.9999999, 180.0, 90.0),
    ];
    for v in views {
        let before = asked.count.load(Ordering::Relaxed);
        let b = super::super::read_blocking(&src, &Request { extent: Some(v), limit: 100 });
        assert!(b.is_ok(), "{v:?}: {b:?}");
        assert!(asked.count.load(Ordering::Relaxed) - before <= 64, "{v:?}");
    }
    for v in views.iter().filter_map(world_view) {
        assert!(cells(&v).len() <= 25, "{v:?}");
    }
}

#[test]
fn sampling_steps_are_powers_of_two() {
    assert_eq!([step_for(0, 10), step_for(10, 10), step_for(11, 10), step_for(400, 50), step_for(401, 50)], [1, 1, 2, 8, 16]);
    assert_eq!(step_for(u64::MAX, 1), MAX_STEP);
}
