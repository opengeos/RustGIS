//! A map in a streamed source's own system reads its window in map units, which the source tests
//! its features' own boxes against ([`Request::own`]); every other map reads in
//! longitude/latitude, as before.

use super::*;
use rustgis_data::raster::Crs;
use rustgis_data::{Dataset, Field, FieldType, Geometry, GeometryType, Value};

/// A source that keeps the requests it is asked.
struct Recording {
    inner: Box<dyn FeatureSource>,
    asked: Arc<Mutex<Vec<Request>>>,
}

impl FeatureSource for Recording {
    fn info(&self) -> &SourceInfo {
        self.inner.info()
    }
    fn coord_crs(&self) -> Crs {
        self.inner.coord_crs()
    }
    fn read(&self, req: &Request) -> rustgis_data::Result<Batch> {
        self.asked.lock().unwrap().push(*req);
        self.inner.read(req)
    }
}

fn utm31() -> Crs {
    Crs::from_epsg(32631).unwrap()
}

/// Lines in UTM zone 31N, as a FlatGeobuf: one at Paris, one from Paris to 1e9 m east (which has
/// no place in longitude/latitude), and one 400 km south.
fn lines() -> (Box<dyn FeatureSource>, Arc<Mutex<Vec<Request>>>) {
    let mut ds = Dataset::new("lines", GeometryType::Polyline, vec![Field::new("name", FieldType::Text)]);
    let line = |a: (f64, f64), b: (f64, f64)| Some(Geometry::Line(vec![vec![Coord::new(a.0, a.1), Coord::new(b.0, b.1)]]));
    ds.push(line((451_000.0, 5_411_000.0), (452_000.0, 5_412_000.0)), vec![Value::Text("paris".into())]);
    ds.push(line((451_500.0, 5_411_500.0), (1e9, 5_411_500.0)), vec![Value::Text("far east".into())]);
    ds.push(line((451_000.0, 5_011_000.0), (452_000.0, 5_012_000.0)), vec![Value::Text("south".into())]);
    ds.held_in = utm31();
    let (bytes, _) = rustgis_data::io::write_bytes_in(&ds, "fgb", utm31()).unwrap();
    let inner = rustgis_data::source::open_bytes("lines.fgb", bytes, None).unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    (Box::new(Recording { inner, asked: asked.clone() }), asked)
}

fn names(layer: &crate::Layer) -> Vec<String> {
    let mut out: Vec<String> = layer
        .dataset
        .features
        .iter()
        .filter_map(|f| match f.attrs.first() {
            Some(Value::Text(t)) => Some(t.clone()),
            _ => None,
        })
        .collect();
    out.sort();
    out
}

/// Around Paris: in longitude/latitude, and the same place in UTM 31N.
fn around_paris() -> (Extent, Extent) {
    (Extent::new(2.2, 48.75, 2.5, 48.95), Extent::new(440_000.0, 5_400_000.0, 470_000.0, 5_420_000.0))
}

fn sync(map: &mut crate::Map, lonlat: &Extent, own: &Extent) {
    map.sync_sources_in(lonlat, Some(own), 1000.0, true, &rustgis_data::query::Env::default());
}

#[test]
fn a_map_in_the_sources_own_system_reads_its_window_in_map_units() {
    let (lonlat, utm) = around_paris();
    let mut map = crate::Map::new("utm");
    map.set_projection(crate::Projection::Custom("EPSG:32631".into()));
    assert_eq!(map.own_system(), Some(utm31()));
    let (source, asked) = lines();
    let id = map.add_streamed("lines", SourceLayer::from_path("lines.fgb", None, source));
    sync(&mut map, &lonlat, &utm);
    let read = asked.lock().unwrap().last().copied().unwrap();
    let padded = utm.padded(0.5, 0.0);
    assert_eq!(read.own, Some((utm31(), padded)));
    // Its longitude/latitude extent is where that box lies, for a source reading by it.
    let e = read.extent.unwrap();
    assert!(e.min_x < 2.1 && e.max_x > 2.6 && e.min_y < 48.7 && e.max_y > 49.0, "{e:?}");
    let layer = map.layer(id).unwrap();
    assert_eq!(layer.source.as_ref().unwrap().window().unwrap().own, Some(padded));
    // Tested in its own system, the line past where longitude/latitude goes is read too.
    assert_eq!(names(layer), ["far east", "paris"]);
    // A view within the window, in map units, reads nothing more; one past it reads again.
    let reads = asked.lock().unwrap().len();
    let nudged = Extent::new(utm.min_x + 5_000.0, utm.min_y, utm.max_x + 5_000.0, utm.max_y);
    sync(&mut map, &lonlat, &nudged);
    assert_eq!(asked.lock().unwrap().len(), reads);
    let away = Extent::new(utm.min_x + 40_000.0, utm.min_y, utm.max_x + 40_000.0, utm.max_y);
    sync(&mut map, &lonlat, &away);
    assert_eq!(asked.lock().unwrap().len(), reads + 1);
}

#[test]
fn every_other_map_reads_in_longitude_latitude_as_before() {
    let (lonlat, utm) = around_paris();
    // Web Mercator, and a map in another projected system: the box in map units isn't the
    // source's, so it isn't given.
    for projection in [crate::Projection::WebMercator, crate::Projection::Custom("EPSG:2154".into())] {
        let mut map = crate::Map::new("other");
        map.set_projection(projection.clone());
        let (source, asked) = lines();
        let id = map.add_streamed("lines", SourceLayer::from_path("lines.fgb", None, source));
        sync(&mut map, &lonlat, &utm);
        let read = asked.lock().unwrap().last().copied().unwrap();
        assert_eq!(read.own, None, "{projection:?}");
        assert_eq!(read.extent, Some(clamp_world(&lonlat.padded(0.5, 0.0))), "{projection:?}");
        let layer = map.layer(id).unwrap();
        assert_eq!(layer.source.as_ref().unwrap().window().unwrap().own, None, "{projection:?}");
        assert_eq!(names(layer), ["paris"], "{projection:?}");
    }
    // Without the view in map units (the globe, a caller that has none), the same.
    let mut map = crate::Map::new("utm");
    map.set_projection(crate::Projection::Custom("EPSG:32631".into()));
    let (source, asked) = lines();
    map.add_streamed("lines", SourceLayer::from_path("lines.fgb", None, source));
    map.sync_sources(&lonlat, 1000.0, true);
    assert_eq!(asked.lock().unwrap().last().unwrap().own, None);
}

/// A source of points in `crs`, as a FlatGeobuf.
fn points(crs: Crs, at: &[(f64, f64, &str)]) -> (Box<dyn FeatureSource>, Arc<Mutex<Vec<Request>>>) {
    let mut ds = Dataset::new("points", GeometryType::Point, vec![Field::new("name", FieldType::Text)]);
    for (x, y, name) in at {
        ds.push(Some(Geometry::Point(Coord::new(*x, *y))), vec![Value::Text((*name).into())]);
    }
    ds.held_in = crs;
    let (bytes, _) = rustgis_data::io::write_bytes_in(&ds, "fgb", crs).unwrap();
    let inner = rustgis_data::source::open_bytes("points.fgb", bytes, None).unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    (Box::new(Recording { inner, asked: asked.clone() }), asked)
}

#[test]
fn a_window_read_in_map_units_holds_everything_only_when_its_box_does() {
    // Around the South Pole in Antarctic Polar Stereographic, the window's longitude/latitude
    // extent reaches every longitude and down to its corners' latitude: points 2,500 km out on
    // every side are inside that, but outside the box read (2,000 km out at its edges' middles).
    let polar = Crs::from_epsg(3031).unwrap();
    let mut map = crate::Map::new("polar");
    map.set_projection(crate::Projection::Custom("EPSG:3031".into()));
    assert_eq!(map.own_system(), Some(polar));
    // A GeoJSON file read in place, whose extent is where its features are, not its box's corners.
    let out = 2_500_000.0;
    let at = [(0.0, 0.0, "pole"), (out, 0.0, "east"), (-out, 0.0, "west"), (0.0, out, "north"), (0.0, -out, "south")];
    let features: Vec<String> = at
        .iter()
        .map(|(x, y, name)| {
            format!(r#"{{"type":"Feature","properties":{{"name":"{name}"}},"geometry":{{"type":"Point","coordinates":[{x},{y}]}}}}"#)
        })
        .collect();
    let path = std::env::temp_dir().join(format!("rustgis-own-polar-{}.geojson", std::process::id()));
    std::fs::write(
        &path,
        format!(
            r#"{{"type":"FeatureCollection","crs":{{"type":"name","properties":{{"name":"urn:ogc:def:crs:EPSG::3031"}}}},"features":[{}]}}"#,
            features.join(",")
        ),
    )
    .unwrap();
    let inner = Box::new(rustgis_data::source::geojson::GeoJsonSource::open_path(&path, "points").unwrap());
    let asked = Arc::new(Mutex::new(Vec::new()));
    let source: Box<dyn FeatureSource> = Box::new(Recording { inner, asked: asked.clone() });
    let id = map.add_streamed("points", SourceLayer::from_path("points.fgb", None, source));
    let view = Extent::new(-1_000_000.0, -1_000_000.0, 1_000_000.0, 1_000_000.0);
    let lonlat = Extent::new(-180.0, -90.0, 180.0, -80.0);
    sync(&mut map, &lonlat, &view);
    let layer = map.layer(id).unwrap();
    assert_eq!(names(layer), ["pole"]);
    assert!(layer.source.as_ref().unwrap().window().unwrap().complete);
    // Panned onto it, it is read: the window didn't hold everything.
    let reads = asked.lock().unwrap().len();
    let there = Extent::new(1_500_000.0, -1_000_000.0, 3_500_000.0, 1_000_000.0);
    sync(&mut map, &lonlat, &there);
    assert_eq!(asked.lock().unwrap().len(), reads + 1);
    assert_eq!(names(map.layer(id).unwrap()), ["east"]);
}

#[test]
fn a_view_past_the_world_in_map_units_reads_no_more() {
    // Points sampled on a map in their own system: zoomed out past the world, a view reaches only
    // as far as the world does there, so zooming further out doesn't read the sample again.
    for (projection, crs) in [(crate::Projection::WebMercator, Crs::WebMercator), (crate::Projection::Custom("EPSG:32631".into()), utm31())]
    {
        let mut map = crate::Map::new("world");
        map.set_projection(projection.clone());
        assert_eq!(map.own_system(), Some(crs));
        let (source, asked) = points(crs, &[(0.0, 0.0, "a"), (100_000.0, 0.0, "b"), (0.0, 100_000.0, "c")]);
        let mut layer = SourceLayer::from_path("points.fgb", None, source);
        layer.limit = 1;
        let id = map.add_streamed("points", layer);
        let world = Extent::new(-180.0, -85.0, 180.0, 85.0);
        sync(&mut map, &world, &Extent::new(-4e7, -4e7, 4e7, 4e7));
        assert!(!map.layer(id).unwrap().source.as_ref().unwrap().window().unwrap().complete, "{projection:?}");
        let reads = asked.lock().unwrap().len();
        sync(&mut map, &world, &Extent::new(-9e7, -9e7, 9e7, 9e7));
        sync(&mut map, &world, &Extent::new(-3e7, -1e7, 5e7, 2e7));
        assert_eq!(asked.lock().unwrap().len(), reads, "{projection:?}");
    }
    // On Web Mercator's map, the world's box to 85°, padded by half.
    let mut map = crate::Map::new("mercator");
    let (source, asked) = points(Crs::WebMercator, &[(0.0, 0.0, "a")]);
    map.add_streamed("points", SourceLayer::from_path("points.fgb", None, source));
    sync(&mut map, &Extent::new(-180.0, -85.0, 180.0, 85.0), &Extent::new(-4e7, -4e7, 4e7, 4e7));
    let (_, read) = asked.lock().unwrap().last().unwrap().own.unwrap();
    let corner = rustgis_geom::lonlat_to_merc(Coord::new(180.0, 85.0));
    let (w, h) = (corner.x * 2.0, corner.y * 2.0);
    for (got, want) in [(read.min_x, -w), (read.min_y, -h), (read.max_x, w), (read.max_y, h)] {
        assert!((got - want).abs() < 1.0, "{read:?}");
    }
}

#[test]
fn a_stated_extent_too_small_hides_nothing_in_view() {
    // A source whose stated extent misses features (a GeoPackage's stale contents row, a line
    // reaching past where longitude/latitude goes): the view is read as it is.
    let (lonlat, utm) = around_paris();
    let mut map = crate::Map::new("utm");
    map.set_projection(crate::Projection::Custom("EPSG:32631".into()));
    let (source, asked) = lines();
    map.add_streamed("lines", SourceLayer::from_path("lines.fgb", None, source));
    let east = Extent::new(utm.min_x + 100_000.0, utm.min_y, utm.max_x + 100_000.0, utm.max_y);
    sync(&mut map, &lonlat, &east);
    assert_eq!(asked.lock().unwrap().last().unwrap().own, Some((utm31(), east.padded(0.5, 0.0))));
}

/// Lines in UTM 31N as a FlatGeobuf: one at Paris and one from 1,200 km east to 1e9 m east, whose
/// far end has no place in longitude/latitude, so the file's stated extent covers Paris only.
fn far_lines() -> (Box<dyn FeatureSource>, Arc<Mutex<Vec<Request>>>) {
    let mut ds = Dataset::new("lines", GeometryType::Polyline, vec![Field::new("name", FieldType::Text)]);
    let line = |a: (f64, f64), b: (f64, f64)| Some(Geometry::Line(vec![vec![Coord::new(a.0, a.1), Coord::new(b.0, b.1)]]));
    ds.push(line((451_000.0, 5_411_000.0), (452_000.0, 5_412_000.0)), vec![Value::Text("paris".into())]);
    ds.push(line((1_200_000.0, 5_411_000.0), (1e9, 5_411_000.0)), vec![Value::Text("far".into())]);
    ds.held_in = utm31();
    let (bytes, _) = rustgis_data::io::write_bytes_in(&ds, "fgb", utm31()).unwrap();
    let inner = rustgis_data::source::open_bytes("far.fgb", bytes, None).unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    (Box::new(Recording { inner, asked: asked.clone() }), asked)
}

#[test]
fn a_complete_window_in_map_units_still_reads_a_feature_past_the_stated_extent() {
    let (source, asked) = far_lines();
    let stated = source.info().extent;
    assert!(stated.max_x < 3.0, "the stated extent covers Paris only: {stated:?}");
    let mut map = crate::Map::new("utm");
    map.set_projection(crate::Projection::Custom("EPSG:32631".into()));
    let id = map.add_streamed("lines", SourceLayer::from_path("far.fgb", None, source));
    let (lonlat, utm) = around_paris();
    sync(&mut map, &lonlat, &utm);
    let layer = map.layer(id).unwrap();
    assert_eq!(names(layer), ["paris"]);
    assert!(layer.source.as_ref().unwrap().window().unwrap().complete);
    // Panned 750 km east, onto the far line: read, though the window read everything the stated
    // extent holds.
    let reads = asked.lock().unwrap().len();
    let east = Extent::new(1_190_000.0, 5_400_000.0, 1_220_000.0, 5_420_000.0);
    sync(&mut map, &Extent::new(12.0, 48.5, 13.0, 49.5), &east);
    assert_eq!(asked.lock().unwrap().len(), reads + 1);
    assert_eq!(names(map.layer(id).unwrap()), ["far"]);
}

#[test]
fn a_window_read_in_longitude_latitude_isnt_reused_for_a_view_in_map_units() {
    // Read first without the view in map units (the far line is left out there, as its far end
    // has no place in longitude/latitude), then with it on the same map: read again, the far line
    // with it.
    let mut map = crate::Map::new("utm");
    map.set_projection(crate::Projection::Custom("EPSG:32631".into()));
    let (source, asked) = lines();
    let id = map.add_streamed("lines", SourceLayer::from_path("lines.fgb", None, source));
    let (lonlat, utm) = around_paris();
    map.sync_sources(&lonlat, 1000.0, true);
    assert_eq!(names(map.layer(id).unwrap()), ["paris"]);
    let reads = asked.lock().unwrap().len();
    sync(&mut map, &lonlat, &utm);
    assert_eq!(asked.lock().unwrap().len(), reads + 1);
    assert_eq!(names(map.layer(id).unwrap()), ["far east", "paris"]);
}
