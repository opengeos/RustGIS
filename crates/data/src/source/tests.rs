use super::*;
use crate::{FieldType, Geometry, Value, wkb};
use rustgis_geom::{Coord, lonlat_to_merc};
use std::path::PathBuf;
use std::sync::Arc;

/// 30 × 20 points over the world, named and numbered.
fn points() -> Vec<(Coord, String, i64)> {
    let mut v = Vec::new();
    for i in 0..30 {
        for j in 0..20 {
            let c = Coord::new(-174.0 + 12.0 * i as f64, -76.0 + 8.0 * j as f64);
            v.push((c, format!("p{i}_{j}"), (i * 20 + j) as i64));
        }
    }
    v
}

fn in_box(e: &Extent) -> usize {
    points().iter().filter(|(c, ..)| e.contains(*c)).count()
}

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("rustgis-source-tests-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_parquet(path: &Path, covering: bool) {
    use arrow_array::{BinaryArray, Float32Array, Int64Array, RecordBatch, StringArray, StructArray};
    use arrow_schema::{DataType, Field as AField, Schema};
    let pts = points();
    let wkbs: Vec<Vec<u8>> = pts.iter().map(|(c, ..)| wkb::encode(&Geometry::Point(*c))).collect();
    let bbox_fields = ["xmin", "ymin", "xmax", "ymax"].map(|n| Arc::new(AField::new(n, DataType::Float32, false)));
    let coord = |f: fn(&Coord) -> f64| Arc::new(Float32Array::from(pts.iter().map(|(c, ..)| f(c) as f32).collect::<Vec<_>>()));
    let bbox =
        StructArray::new(bbox_fields.iter().cloned().collect(), vec![coord(|c| c.x), coord(|c| c.y), coord(|c| c.x), coord(|c| c.y)], None);
    let mut fields = vec![
        AField::new("name", DataType::Utf8, false),
        AField::new("value", DataType::Int64, false),
        AField::new("geometry", DataType::Binary, false),
    ];
    let mut cols: Vec<arrow_array::ArrayRef> = vec![
        Arc::new(StringArray::from(pts.iter().map(|p| p.1.clone()).collect::<Vec<_>>())),
        Arc::new(Int64Array::from(pts.iter().map(|p| p.2).collect::<Vec<_>>())),
        Arc::new(BinaryArray::from(wkbs.iter().map(|w| w.as_slice()).collect::<Vec<_>>())),
    ];
    let geo = if covering {
        fields.push(AField::new("geometry_bbox", DataType::Struct(bbox_fields.iter().cloned().collect()), false));
        cols.push(Arc::new(bbox));
        r#"{"version":"1.1.0","primary_column":"geometry","columns":{"geometry":{"encoding":"WKB","geometry_types":["Point"],
            "covering":{"bbox":{"xmin":["geometry_bbox","xmin"],"ymin":["geometry_bbox","ymin"],"xmax":["geometry_bbox","xmax"],"ymax":["geometry_bbox","ymax"]}}}}}"#
    } else {
        r#"{"version":"1.0.0","primary_column":"geometry","columns":{"geometry":{"encoding":"WKB","geometry_types":["Point"]}}}"#
    };
    let schema = Arc::new(Schema::new(fields).with_metadata(std::collections::HashMap::from([("geo".to_string(), geo.to_string())])));
    let batch = RecordBatch::try_new(schema.clone(), cols).unwrap();
    let props = ::parquet::file::properties::WriterProperties::builder().set_max_row_group_row_count(Some(50)).build();
    let mut w = ::parquet::arrow::ArrowWriter::try_new(std::fs::File::create(path).unwrap(), schema, Some(props)).unwrap();
    w.write(&batch).unwrap();
    w.close().unwrap();
}

fn write_fgb(path: &Path) {
    use flatgeobuf::geozero::{ColumnValue, PropertyProcessor};
    use flatgeobuf::{ColumnType, FgbWriter, GeometryType as G};
    let mut w = FgbWriter::create("grid", G::Point).unwrap();
    w.add_column("name", ColumnType::String, |_, _| {});
    w.add_column("value", ColumnType::Long, |_, _| {});
    for (c, name, v) in points() {
        let g = flatgeobuf::geozero::wkb::Wkb(wkb::encode(&Geometry::Point(c)));
        w.add_feature_geom(g, |f| {
            f.property(0, "name", &ColumnValue::String(&name)).unwrap();
            f.property(1, "value", &ColumnValue::Long(v)).unwrap();
        })
        .unwrap();
    }
    w.write(std::fs::File::create(path).unwrap()).unwrap();
}

/// A GeoPackage of the points in Web Mercator, with an R-tree index (or without).
fn write_gpkg(path: &Path, rtree: bool) {
    let _ = std::fs::remove_file(path);
    let c = rusqlite::Connection::open(path).unwrap();
    c.execute_batch(
        "CREATE TABLE gpkg_spatial_ref_sys (srs_name TEXT, srs_id INTEGER PRIMARY KEY, organization TEXT, organization_coordsys_id INTEGER, definition TEXT, description TEXT);
         INSERT INTO gpkg_spatial_ref_sys VALUES ('Web Mercator', 3857, 'EPSG', 3857, '', '');
         CREATE TABLE gpkg_contents (table_name TEXT PRIMARY KEY, data_type TEXT, identifier TEXT, description TEXT, last_change TEXT, min_x DOUBLE, min_y DOUBLE, max_x DOUBLE, max_y DOUBLE, srs_id INTEGER);
         CREATE TABLE gpkg_geometry_columns (table_name TEXT, column_name TEXT, geometry_type_name TEXT, srs_id INTEGER, z TINYINT, m TINYINT);
         INSERT INTO gpkg_contents (table_name, data_type, srs_id) VALUES ('grid \"q\"', 'features', 3857);
         INSERT INTO gpkg_geometry_columns VALUES ('grid \"q\"', 'geom', 'POINT', 3857, 0, 0);
         CREATE TABLE \"grid \"\"q\"\"\" (fid INTEGER PRIMARY KEY AUTOINCREMENT, geom BLOB, name TEXT, value MEDIUMINT, ok BOOLEAN);",
    )
    .unwrap();
    if rtree {
        c.execute_batch("CREATE VIRTUAL TABLE \"rtree_grid \"\"q\"\"_geom\" USING rtree(id, minx, maxx, miny, maxy);").unwrap();
    }
    for (i, (p, name, v)) in points().into_iter().enumerate() {
        let m = lonlat_to_merc(p);
        // A header with an xy envelope, then WKB.
        let mut blob = b"GP\x00\x03".to_vec();
        blob.extend_from_slice(&3857i32.to_le_bytes());
        for x in [m.x, m.x, m.y, m.y] {
            blob.extend_from_slice(&x.to_le_bytes());
        }
        blob.extend(wkb::encode(&Geometry::Point(m)));
        let fid = i as i64 + 1;
        c.execute("INSERT INTO \"grid \"\"q\"\"\" VALUES (?1, ?2, ?3, ?4, ?5)", rusqlite::params![fid, blob, name, v, v % 2]).unwrap();
        if rtree {
            c.execute("INSERT INTO \"rtree_grid \"\"q\"\"_geom\" VALUES (?1, ?2, ?3, ?4, ?5)", rusqlite::params![fid, m.x, m.x, m.y, m.y])
                .unwrap();
        }
    }
}

/// The same checks for every provider.
fn check(src: &dyn FeatureSource, expect_indexed: bool) {
    let info = src.info();
    assert_eq!(info.geometry_type, GeometryType::Point, "{}", info.format);
    assert_eq!(info.count, Some(600), "{}", info.format);
    assert_eq!(info.indexed, expect_indexed, "{}", info.format);
    assert!(info.fields.iter().any(|f| f.name == "name" && f.ty == FieldType::Text), "{:?}", info.fields);
    let vi = info.fields.iter().position(|f| f.name == "value").unwrap();
    assert_eq!(info.fields[vi].ty, FieldType::Integer);

    // Everything.
    let all = src.read(&Request::all()).unwrap();
    assert_eq!(all.features.len(), 600, "{}", info.format);
    assert!(all.complete);
    let f = all.features.iter().find(|f| f.attrs.contains(&Value::Text("p3_4".into()))).unwrap();
    let c = match f.geometry {
        Some(Geometry::Point(c)) => c,
        _ => panic!("not a point"),
    };
    assert!((c.x - (-138.0)).abs() < 1e-4 && (c.y - (-44.0)).abs() < 1e-4, "{c:?} in {}", info.format);
    assert_eq!(f.attrs[vi], Value::Int(64));
    // Ids stay put between reads.
    let near = src.read(&Request { extent: Some(Extent::new(-140.0, -46.0, -130.0, -40.0)), limit: 100 }).unwrap();
    let g = near.features.iter().find(|g| g.attrs.contains(&Value::Text("p3_4".into()))).unwrap();
    assert_eq!(g.id, f.id, "{}", info.format);

    // An extent: exactly the points in it.
    let e = Extent::new(-60.0, -10.0, 40.0, 50.0);
    let got = src.read(&Request { extent: Some(e), limit: 10_000 }).unwrap();
    assert_eq!(got.features.len(), in_box(&e), "{}", info.format);
    assert!(got.complete);
    assert!(got.features.iter().all(|f| f.geometry.as_ref().is_some_and(|g| e.contains(g.extent().center()))));

    // A limit: a sample spread over the whole extent, not the first rows.
    let s = src.read(&Request { extent: None, limit: 60 }).unwrap();
    assert!(!s.complete);
    assert!(s.features.len() <= 60 && s.features.len() >= 50, "{} of {}", s.features.len(), info.format);
    let xs: Vec<f64> = s.features.iter().filter_map(|f| f.geometry.as_ref()).map(|g| g.extent().min_x).collect();
    assert!(xs.iter().any(|x| *x < -100.0) && xs.iter().any(|x| *x > 100.0), "{} sample isn't spread", info.format);
    assert!(s.matched >= 600);

    // An extent outside the data.
    let none = src.read(&Request { extent: Some(Extent::new(-179.9, 80.0, -179.0, 84.0)), limit: 100 }).unwrap();
    assert!(none.features.is_empty());

    // A whole read into a dataset.
    let (ds, report) = materialize(src).unwrap();
    assert_eq!(ds.features.len(), 600);
    assert!(report.messages.is_empty());
}

#[test]
fn geoparquet_with_and_without_a_covering() {
    let d = dir();
    let p = d.join("grid.parquet");
    write_parquet(&p, true);
    let src = open_path(&p, None).unwrap();
    check(src.as_ref(), true);
    // The covering's row-group statistics give the extent.
    assert!(src.info().extent.min_x <= -174.0 && src.info().extent.max_x >= 174.0);
    let bytes = std::fs::read(&p).unwrap();
    check(open_bytes("grid.parquet", bytes, None).unwrap().as_ref(), true);

    let p2 = d.join("plain.parquet");
    write_parquet(&p2, false);
    check(open_path(&p2, None).unwrap().as_ref(), false);
}

#[test]
fn flatgeobuf() {
    let p = dir().join("grid.fgb");
    write_fgb(&p);
    let src = open_path(&p, None).unwrap();
    check(src.as_ref(), true);
    assert_eq!(src.info().name, "grid");
    check(open_bytes("grid.fgb", std::fs::read(&p).unwrap(), None).unwrap().as_ref(), true);
}

#[test]
fn geopackage_reprojects_and_uses_its_rtree() {
    let d = dir();
    let p = d.join("grid.gpkg");
    write_gpkg(&p, true);
    assert_eq!(layers(&p).unwrap(), vec!["grid \"q\"".to_string()]);
    let src = open_path(&p, Some("GRID \"Q\"")).unwrap();
    assert_eq!(src.info().crs, "EPSG:3857");
    check(src.as_ref(), true);
    // Booleans from integers.
    let all = src.read(&Request::all()).unwrap();
    let ok = src.info().fields.iter().position(|f| f.name == "ok").unwrap();
    assert!(all.features.iter().any(|f| f.attrs[ok] == Value::Bool(true)));
    check(open_bytes("grid.gpkg", std::fs::read(&p).unwrap(), None).unwrap().as_ref(), true);

    let p2 = d.join("scan.gpkg");
    write_gpkg(&p2, false);
    check(open_path(&p2, None).unwrap().as_ref(), false);
    assert!(open_path(&p2, Some("missing")).is_err());
}

#[test]
fn hostile_files_are_errors_not_panics() {
    let mut junk = vec![b"PAR1".to_vec(), b"fgb\x03fgb\x00".to_vec(), b"SQLite format 3\0".to_vec(), vec![0xff; 4096], Vec::new()];
    // Every truncation of a real file.
    let d = dir();
    let p = d.join("t.parquet");
    write_parquet(&p, true);
    let pq = std::fs::read(&p).unwrap();
    let f = d.join("t.fgb");
    write_fgb(&f);
    let fgb = std::fs::read(&f).unwrap();
    for n in (0..pq.len()).step_by(97) {
        junk.push(pq[..n].to_vec());
    }
    for n in (0..fgb.len()).step_by(89) {
        junk.push(fgb[..n].to_vec());
    }
    for bytes in junk {
        for name in ["x.parquet", "x.fgb", "x.gpkg"] {
            if let Ok(s) = open_bytes(name, bytes.clone(), None) {
                let _ = s.read(&Request::all());
                let _ = s.read(&Request { extent: Some(Extent::new(-10.0, -10.0, 10.0, 10.0)), limit: 3 });
            }
        }
    }
    assert!(open_path(Path::new("nope.shp"), None).is_err());
}

/// Reads every vector file in `RUSTGIS_SAMPLE_VECTORS` (the opengeos sample data, e.g.
/// <https://source.coop/giswqs/opengeos>): `cargo test -p rustgis-data -- --ignored vector_samples --nocapture`.
#[test]
#[ignore]
fn vector_samples() {
    let Ok(dir) = std::env::var("RUSTGIS_SAMPLE_VECTORS") else { return };
    let mut paths: Vec<_> =
        std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| is_source_name(&p.to_string_lossy())).collect();
    paths.sort();
    assert!(!paths.is_empty());
    for p in paths {
        let names = match layers(&p) {
            Ok(n) => n,
            Err(e) => {
                println!("{}: {e}", p.display());
                continue;
            }
        };
        for layer in names {
            let t = std::time::Instant::now();
            let src = match open_path(&p, Some(&layer)) {
                Ok(s) => s,
                Err(e) => {
                    println!("{} [{layer}]: {e}", p.display());
                    continue;
                }
            };
            let i = src.info();
            let open = t.elapsed();
            let e = i.extent;
            let view = Extent::new(
                e.center().x - e.width() / 8.0,
                e.center().y - e.height() / 8.0,
                e.center().x + e.width() / 8.0,
                e.center().y + e.height() / 8.0,
            );
            let t2 = std::time::Instant::now();
            let b = match src.read(&Request { extent: Some(view), limit: 200_000 }) {
                Ok(b) => b,
                Err(e) => {
                    println!("{} [{layer}]: {e}", p.display());
                    continue;
                }
            };
            println!(
                "{} [{}] {} {:?} {} fields, {:?} features, {} indexed={} extent={:?} open {:?}; view: {} of {} (complete={}, skipped {}) in {:?}",
                p.file_name().unwrap().to_string_lossy(),
                i.name,
                i.format,
                i.geometry_type,
                i.fields.len(),
                i.count,
                i.crs,
                i.indexed,
                i.extent,
                open,
                b.features.len(),
                b.matched,
                b.complete,
                b.skipped,
                t2.elapsed()
            );
        }
    }
}
