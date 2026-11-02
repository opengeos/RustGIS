use super::*;
use crate::raster::Crs;

// Encoding, as `FeatureCollection.proto` lays answers out.

fn varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8 & 0x7f) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

fn zz(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn number(field: u64, v: u64) -> Vec<u8> {
    let mut out = Vec::new();
    varint(&mut out, field << 3);
    varint(&mut out, v);
    out
}

fn message(field: u64, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    varint(&mut out, (field << 3) | 2);
    varint(&mut out, bytes.len() as u64);
    out.extend_from_slice(bytes);
    out
}

fn double(field: u64, v: f64) -> Vec<u8> {
    let mut out = Vec::new();
    varint(&mut out, (field << 3) | 1);
    out.extend_from_slice(&v.to_le_bytes());
    out
}

fn float(field: u64, v: f32) -> Vec<u8> {
    let mut out = Vec::new();
    varint(&mut out, (field << 3) | 5);
    out.extend_from_slice(&v.to_le_bytes());
    out
}

fn packed(values: impl IntoIterator<Item = u64>) -> Vec<u8> {
    let mut out = Vec::new();
    for v in values {
        varint(&mut out, v);
    }
    out
}

/// An answer: a version, then a query result holding `result`'s fields as its feature result.
fn answer(result: &[Vec<u8>]) -> Vec<u8> {
    [message(1, b"10.0"), message(2, &message(1, &result.concat()))].concat()
}

fn field(name: &str, ty: u64) -> Vec<u8> {
    message(13, &[message(1, name.as_bytes()), number(2, ty)].concat())
}

/// A transform: lower-left or upper-left origin, X and Y scale and translate.
fn transform(upper_left: bool, scale: f64, x0: f64, y0: f64) -> Vec<u8> {
    message(
        12,
        &[
            number(1, if upper_left { 0 } else { 1 }),
            message(2, &[double(1, scale), double(2, scale)].concat()),
            message(3, &[double(1, x0), double(2, y0)].concat()),
        ]
        .concat(),
    )
}

fn text(s: &str) -> Vec<u8> {
    message(1, &message(1, s.as_bytes()))
}
fn sint64(v: i64) -> Vec<u8> {
    message(1, &number(8, zz(v)))
}
fn uint(v: u64) -> Vec<u8> {
    message(1, &number(5, v))
}
fn uint64(v: u64) -> Vec<u8> {
    message(1, &number(7, v))
}
fn single(v: f32) -> Vec<u8> {
    message(1, &float(2, v))
}
fn real(v: f64) -> Vec<u8> {
    message(1, &double(3, v))
}
fn null() -> Vec<u8> {
    message(1, &number(10, 1))
}

/// A geometry of parts `lengths` (none: one part) and coordinates, X and Y as differences.
fn geometry(lengths: &[u64], coords: &[i64]) -> Vec<u8> {
    let mut g = Vec::new();
    if !lengths.is_empty() {
        g.extend(message(2, &packed(lengths.iter().copied())));
    }
    g.extend(message(3, &packed(coords.iter().map(|&c| zz(c)))));
    message(2, &g)
}

fn feature(parts: &[Vec<u8>]) -> Vec<u8> {
    message(15, &parts.concat())
}

fn c(x: f64, y: f64) -> Coord {
    Coord::new(x, y)
}

fn err(bytes: &[u8]) -> String {
    match read(bytes, "t") {
        Ok(_) => panic!("read an answer that should be refused"),
        Err(e) => e.to_string(),
    }
}

/// Two points with every kind of field, as the services of the date test answered (2026-11-02):
/// dates as `sint64` milliseconds, object ids as `uint` or `uint64`, date-only values as text.
fn points() -> Vec<u8> {
    answer(&[
        number(7, POINT),
        transform(false, 0.5, 10.0, 20.0),
        field("OBJECTID", OID),
        field("NAME", 4),
        field("MAG", 2),
        field("DEPTH", 3),
        field("WHEN", 5),
        field("DAY", 14),
        field("BIG", 13),
        feature(&[
            uint64(7),
            text("Bogotá"),
            single(6.3),
            real(-2.5),
            sint64(1_785_903_246_000),
            text("1991-01-01"),
            sint64(10_000_000_000),
            geometry(&[], &[4, 6]),
        ]),
        feature(&[uint(8), null(), null(), null(), null(), null(), null(), geometry(&[], &[-2, 0])]),
    ])
}

#[test]
fn fields_keep_the_values_the_same_query_gives_as_geojson() {
    let page = read_page(&points(), "quakes").unwrap();
    let ds = &page.dataset;
    let types: Vec<(&str, FieldType)> = ds.fields.iter().map(|f| (f.name.as_str(), f.ty)).collect();
    assert_eq!(
        types,
        [
            ("OBJECTID", FieldType::Integer),
            ("NAME", FieldType::Text),
            ("MAG", FieldType::Double),
            ("DEPTH", FieldType::Double),
            ("WHEN", FieldType::Integer),
            ("DAY", FieldType::Text),
            ("BIG", FieldType::Integer),
        ]
    );
    let first = &ds.features[0];
    assert_eq!(
        first.attrs,
        [
            Value::Int(7),
            Value::Text("Bogotá".into()),
            // The single-precision value's shortest digits, not 6.300000190734863.
            Value::Float(6.3),
            Value::Float(-2.5),
            Value::Int(1_785_903_246_000),
            Value::Text("1991-01-01".into()),
            Value::Int(10_000_000_000),
        ]
    );
    assert_eq!(ds.features[1].attrs[0], Value::Int(8));
    assert!(ds.features[1].attrs[1..].iter().all(|v| *v == Value::Null));
    // Numbered by place, as GeoJSON is; the object id stays a field.
    assert_eq!(ds.features.iter().map(|f| f.id).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(page.object_id_field, Some(0));
    assert_eq!(ds.geometry_type, GeometryType::Point);
    assert_eq!(first.geometry, Some(Geometry::Point(c(12.0, 23.0))));
    assert_eq!(ds.features[1].geometry, Some(Geometry::Point(c(9.0, 20.0))));
    assert!(!page.exceeded_transfer_limit);
    assert!(page.report.messages.is_empty(), "{:?}", page.report.messages);
    assert!(!ds.zm_dropped && ds.crs.is_none());
}

#[test]
fn line_parts_start_over_and_an_upper_left_origin_counts_down() {
    let bytes = answer(&[number(7, POLYLINE), transform(true, 1.0, 0.0, 100.0), feature(&[geometry(&[2, 2], &[1, 1, 2, 0, 5, 5, 0, 1])])]);
    let (ds, report) = read(&bytes, "t").unwrap();
    assert_eq!(ds.geometry_type, GeometryType::Polyline);
    assert_eq!(ds.features[0].geometry, Some(Geometry::Line(vec![vec![c(1.0, 99.0), c(3.0, 99.0)], vec![c(5.0, 95.0), c(5.0, 94.0)]])));
    assert!(report.messages.is_empty());
}

#[test]
fn clockwise_rings_start_polygons_and_counterclockwise_ones_are_their_holes() {
    #[rustfmt::skip]
    let coords = [
        // Outer, clockwise: (0 0) (0 10) (10 10) (10 0) (0 0).
        0, 0, 0, 10, 10, 0, 0, -10, -10, 0,
        // Hole, counterclockwise: (2 2) (4 2) (4 4) (2 4) (2 2).
        2, 2, 2, 0, 0, 2, -2, 0, 0, -2,
        // A second outer ring: (20 0) (20 5) (25 5) (25 0) (20 0).
        20, 0, 0, 5, 5, 0, 0, -5, -5, 0,
    ];
    let bytes = answer(&[number(7, POLYGON), transform(false, 1.0, 0.0, 0.0), feature(&[geometry(&[5, 5, 5], &coords)])]);
    let (ds, report) = read(&bytes, "t").unwrap();
    let Some(Geometry::Polygon(polygons)) = &ds.features[0].geometry else { panic!("not a polygon") };
    assert_eq!(polygons.iter().map(Vec::len).collect::<Vec<_>>(), [2, 1]);
    assert_eq!(polygons[0][1][0], c(2.0, 2.0));
    assert_eq!(polygons[1][0][0], c(20.0, 0.0));
    assert!(report.messages.is_empty());
}

#[test]
fn a_geometry_without_coordinates_is_no_geometry() {
    // As `Water_Network/FeatureServer/3` sends a line without vertices: its type alone.
    let empty = message(2, &number(1, POLYLINE));
    let bytes = answer(&[number(7, POLYLINE), transform(false, 1.0, 0.0, 0.0), feature(&[empty])]);
    let (ds, report) = read(&bytes, "t").unwrap();
    assert_eq!(ds.features[0].geometry, None);
    assert_eq!(ds.geometry_type, GeometryType::Polyline);
    assert!(report.messages.is_empty());
}

#[test]
fn a_ring_without_area_is_left_out_and_said() {
    // A clockwise square, then three points on a line.
    let coords = [0, 0, 0, 10, 10, 0, 0, -10, -10, 0, 0, 0, 5, 0, 5, 0];
    let bytes = answer(&[number(7, POLYGON), transform(false, 1.0, 0.0, 0.0), feature(&[geometry(&[5, 3], &coords)])]);
    let (ds, report) = read(&bytes, "t").unwrap();
    let Some(Geometry::Polygon(polygons)) = &ds.features[0].geometry else { panic!("not a polygon") };
    assert_eq!(polygons.len(), 1);
    assert_eq!(report.messages, ["1 polygon rings have no area and are left out"]);
}

#[test]
fn z_and_m_are_dropped_and_said_even_when_they_are_all_alike() {
    let bytes = answer(&[
        number(10, 1),
        number(11, 1),
        transform(false, 1.0, 0.0, 0.0),
        feature(&[geometry(&[], &[4, 6, 100, 3])]),
        feature(&[geometry(&[], &[1, 1, 100, 3])]),
    ]);
    let (ds, report) = read(&bytes, "t").unwrap();
    assert_eq!(ds.features[0].geometry, Some(Geometry::Point(c(4.0, 6.0))));
    assert!(ds.zm_dropped);
    assert_eq!(report.messages, [crate::zm_note(Some(2))]);
}

#[test]
fn web_mercator_answers_are_turned_to_longitude_latitude() {
    let bytes = answer(&[
        message(8, &[number(1, 102_100), number(2, 3857)].concat()),
        transform(false, 1.0, 0.0, 0.0),
        feature(&[geometry(&[], &[1_113_195, 0])]),
    ]);
    let (ds, report) = read(&bytes, "t").unwrap();
    let Some(Geometry::Point(p)) = ds.features[0].geometry else { panic!("not a point") };
    assert!((p.x - 10.0).abs() < 1e-4 && p.y.abs() < 1e-9, "{p:?}");
    assert_eq!(ds.crs, Some(Crs::WebMercator));
    assert!(report.messages.is_empty());
}

#[test]
fn an_unknown_coordinate_system_is_said() {
    let bytes = answer(&[message(8, &number(1, 999_999)), transform(false, 1.0, 0.0, 0.0), feature(&[geometry(&[], &[1, 2])])]);
    let (ds, report) = read(&bytes, "t").unwrap();
    assert!(ds.crs.is_none());
    assert_eq!(report.messages, ["coordinate system 999999 can't be used; coordinates are read as longitude/latitude"]);
}

#[test]
fn repeated_names_and_unread_field_types_are_left_out_and_said() {
    let bytes = answer(&[
        field("NAME", 4),
        field("name", 4),
        field("PHOTO", 8),
        field("COUNT", 1),
        feature(&[text("a"), text("b"), text("blob"), text("many")]),
    ]);
    let (ds, report) = read(&bytes, "t").unwrap();
    assert_eq!(ds.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["NAME", "COUNT"]);
    assert_eq!(ds.features[0].attrs, [Value::Text("a".into()), Value::Null]);
    assert_eq!(
        report.messages,
        [
            "1 fields repeat an earlier field's name and are left out",
            "1 fields hold geometries, blobs or rasters, which aren't read, and are left out",
            "1 values don't fit their field's type and are left empty",
        ]
    );
    // A table: no geometry type, no geometry.
    assert_eq!(ds.geometry_type, GeometryType::None);
}

#[test]
fn a_page_cut_at_the_record_limit_says_so() {
    let bytes = answer(&[number(9, 1), field("OBJECTID", OID), feature(&[uint(1)])]);
    assert!(read_page(&bytes, "t").unwrap().exceeded_transfer_limit);
    let (_, report) = read(&bytes, "t").unwrap();
    assert_eq!(report.messages, ["the service sent its first 1 features only (its record limit); the rest aren't in this answer"]);
}

#[test]
fn refused_answers_say_why() {
    assert!(err(b"").contains("f=json"));
    let e = err(br#"{"error":{"message":"Invalid query","code":400,"details":[]}}"#);
    assert!(e.contains("(400): Invalid query"), "{e}");
    assert!(err(br#"{"type":"FeatureCollection","features":[]}"#).contains("JSON, not a protocol buffer"));
    // A count (`returnCountOnly`).
    assert!(err(&[message(1, b"10.0"), message(2, &message(2, &number(1, 5)))].concat()).contains("count"));
    assert!(err(&[message(1, b"10.0")].concat()).contains("no features"));
    let with = |part: Vec<u8>| answer(&[transform(false, 1.0, 0.0, 0.0), feature(&[part])]);
    assert!(err(&with(message(7, &[]))).contains("curve"));
    assert!(err(&with(message(3, &[]))).contains("shape-buffer"));
    assert!(err(&with(geometry(&[], &[1, 1, 1, 1]))).contains("more than one vertex"));
    assert!(err(&with(geometry(&[3], &[1, 1]))).contains("ends inside"));
    assert!(err(&with(geometry(&[1], &[1, 1, 1, 1]))).contains("more coordinates"));
    let multipatch = answer(&[number(7, MULTIPATCH), transform(false, 1.0, 0.0, 0.0), feature(&[geometry(&[], &[1, 1])])]);
    assert!(err(&multipatch).contains("multipatch"));
    let untransformed = answer(&[feature(&[geometry(&[], &[1, 1])])]);
    assert!(err(&untransformed).contains("no transform"));
    let e = err(&answer(&[field("A", 4), feature(&[text("x"), text("y")])]));
    assert!(e.contains("feature 1: it has more values"), "{e}");
}

#[test]
fn many_fields_times_many_empty_features_are_refused_before_they_take_memory() {
    let mut parts: Vec<Vec<u8>> = (0..3000).map(|i| field(&format!("f{i}"), 4)).collect();
    parts.extend((0..3000).map(|_| feature(&[])));
    let e = err(&answer(&parts));
    assert!(e.contains("3000 features of 3000 fields"), "{e}");
}

#[test]
fn cut_or_corrupted_answers_never_panic() {
    let whole = points();
    for n in 0..whole.len() {
        let _ = read(&whole[..n], "cut");
        let _ = is_esripbf(&whole[..n]);
    }
    for i in 0..whole.len() {
        for byte in [0x00, 0x01, 0x7f, 0x80, 0xff] {
            let mut bytes = whole.clone();
            bytes[i] = byte;
            let _ = read(&bytes, "corrupt");
        }
    }
    // A length past every byte there is, and a number of eleven bytes.
    let mut huge = vec![0x0a];
    varint(&mut huge, u64::MAX);
    assert!(read(&huge, "t").is_err());
    assert!(read(&[0x08, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01], "t").is_err());
}

#[test]
fn a_dropped_or_fetched_answer_is_read_by_its_content() {
    // A file saved as `.pbf`, and an address without an extension (`…/query?…&f=pbf`), which
    // `sniff_extension` names `.pbf`.
    assert_eq!(crate::io::sniff_extension(&points()), Some("pbf"));
    let (ds, report) = crate::io::read_bytes("quakes.pbf", &points()).unwrap();
    assert_eq!((ds.name.as_str(), ds.features.len(), ds.geometry_type), ("quakes", 2, GeometryType::Point));
    assert_eq!(ds.source.as_deref(), Some("quakes.pbf"));
    assert!(report.messages.is_empty(), "{:?}", report.messages);
    let layers = crate::io::read_layers("quakes.pbf", &points()).unwrap();
    assert_eq!(layers.len(), 1);
    // OpenStreetMap stays OpenStreetMap's, and another `.pbf` says what it isn't.
    let osm = crate::io::read_layers("city.osm.pbf", &crate::osm::tests::sample()).unwrap();
    assert!(osm.iter().any(|l| l.dataset.name.ends_with("points")), "{:?}", osm.iter().map(|l| &l.dataset.name).collect::<Vec<_>>());
    let e = crate::io::read_bytes("tile.pbf", &message(3, &message(1, b"roads"))).unwrap_err().to_string();
    assert!(e.contains("isn't OpenStreetMap data or an ArcGIS feature service's answer"), "{e}");
    // A refusal names the format, not OpenStreetMap.
    let e = crate::io::read_bytes("x.pbf", &answer(&[feature(&[message(7, &[])])])).unwrap_err().to_string();
    assert!(e.contains("ArcGIS protocol buffer") && e.contains("curve"), "{e}");
}

#[test]
fn answers_are_told_from_other_protocol_buffers_and_json() {
    assert!(is_esripbf(&points()));
    // Without its version too.
    assert!(is_esripbf(&message(2, &message(1, &number(7, POINT)))));
    assert!(!is_esripbf(&[message(1, b"10.0"), message(2, &message(2, &number(1, 5)))].concat()));
    // OpenStreetMap: a big-endian header length, then its `OSMHeader` blob header.
    assert!(!is_esripbf(b"\x00\x00\x00\x0d\x0a\x09OSMHeader\x18\x40"));
    // A vector tile: a layer (field 3).
    assert!(!is_esripbf(&message(3, &message(1, b"roads"))));
    assert!(!is_esripbf(br#"{"features":[]}"#));
    assert!(!is_esripbf(b""));
}

/// Pages of real services read as `f=pbf`, against the same pages read as GeoJSON by RustGIS's
/// GeoJSON reader: the same features, values and vertices, coordinates within the services'
/// quantization. Set `RUSTGIS_ESRIPBF_DIR` to a folder of `<name>.pbf` and `<name>.geojson`
/// pairs, fetched with `outSR=4326`, for instance:
///
/// ```sh
/// q='query?where=1%3D1&outFields=*&returnGeometry=true&outSR=4326&resultRecordCount=500&orderByFields=OBJECTID'
/// page() { for f in pbf geojson; do curl -sf "$2/$q&f=$f" -o "$1.$f"; done; }
/// page quakes https://services9.arcgis.com/RHVPKKiFTONKtxq3/arcgis/rest/services/USGS_Seismic_Data_v1/FeatureServer/0
/// page counties https://services.arcgis.com/P3ePLMYs2RVChkJx/arcgis/rest/services/USA_Counties_Generalized_Boundaries/FeatureServer/0
/// page newtypes https://services5.arcgis.com/N82JbI5EYtAkuUKU/ArcGIS/rest/services/AllNewFieldTypes_withDateOnlyContingentValues/FeatureServer/0
/// page mlines https://sampleserver6.arcgisonline.com/arcgis/rest/services/Water_Network/FeatureServer/3
/// ```
#[test]
fn real_pages_match_the_same_pages_as_geojson() {
    let Ok(dir) = std::env::var("RUSTGIS_ESRIPBF_DIR") else { return };
    let mut pairs = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("pbf") {
            continue;
        }
        let name = path.display().to_string();
        let (pbf, _) = read(&std::fs::read(&path).unwrap(), "pbf").unwrap_or_else(|e| panic!("{name}: {e}"));
        let text = std::fs::read_to_string(path.with_extension("geojson")).unwrap();
        let (json, _) = crate::io::read_geojson(&text, "geojson").unwrap();
        assert_eq!(pbf.features.len(), json.features.len(), "{name}: features");
        for (row, (a, b)) in pbf.features.iter().zip(json.features.iter()).enumerate() {
            for (i, f) in pbf.fields.iter().enumerate() {
                let Some(j) = json.field_index(&f.name) else { continue };
                let (x, y) = (&a.attrs[i], &b.attrs[j]);
                let same = match (x, y) {
                    // GeoJSON prints single precision rounded to seven digits.
                    (Value::Float(p), Value::Float(q)) => (p - q).abs() <= 1e-6 * p.abs().max(q.abs()).max(1.0),
                    (Value::Float(p), Value::Int(q)) | (Value::Int(q), Value::Float(p)) => *p == *q as f64,
                    _ => x == y,
                };
                assert!(same, "{name}: row {row}, {}: {x:?} against {y:?}", f.name);
            }
            // A geometry without vertices is none: the protocol buffer sends it without
            // coordinates, GeoJSON as an empty `LineString`, which its reader keeps.
            let shape = |g: &Option<Geometry>| {
                g.clone().filter(|g| {
                    let mut any = false;
                    g.for_each_coord(|_| any = true);
                    any
                })
            };
            match (&shape(&a.geometry), &shape(&b.geometry)) {
                (Some(p), Some(q)) => {
                    assert_eq!(p.kind(), q.kind(), "{name}: row {row}");
                    let (mut n, mut m) = (0, 0);
                    p.for_each_coord(|_| n += 1);
                    q.for_each_coord(|_| m += 1);
                    assert_eq!(n, m, "{name}: row {row}, vertices");
                    let (e, f) = (p.extent(), q.extent());
                    for (u, v) in [(e.min_x, f.min_x), (e.min_y, f.min_y), (e.max_x, f.max_x), (e.max_y, f.max_y)] {
                        assert!((u - v).abs() < 1e-7, "{name}: row {row}, extent {e:?} against {f:?}");
                    }
                }
                (p, q) => assert_eq!(p.is_some(), q.is_some(), "{name}: row {row}, geometry"),
            }
        }
        pairs += 1;
    }
    assert!(pairs > 0, "{dir}: no .pbf and .geojson pairs");
}
