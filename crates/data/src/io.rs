//! Vector readers and writers: GeoJSON, CSV (with longitude/latitude columns) and Shapefile.

use std::path::Path;

use serde_json::{Map, Value as Json, json};

use crate::{Coord, DataError, Dataset, Field, FieldType, Geometry, GeometryType, ReadReport, Result, Value};

/// Largest file read into memory (256 MiB). Parsing JSON can take several times the file size.
const MAX_BYTES: u64 = 256 << 20;

/// File types the readers understand, for open dialogs.
pub const READ_EXTENSIONS: &[&str] = &["geojson", "json", "csv", "txt", "shp"];

/// Reads a vector file by extension.
pub fn read_path(path: &Path) -> Result<(Dataset, ReadReport)> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("layer").to_string();
    let mut out = match ext.as_str() {
        "geojson" | "json" => {
            check_size(path)?;
            read_geojson(&std::fs::read_to_string(path)?, &name)?
        }
        "csv" | "txt" => {
            check_size(path)?;
            read_csv(&std::fs::read_to_string(path)?, &name)?
        }
        "shp" => {
            check_size(path)?;
            let dbf = path.with_extension("dbf");
            if dbf.exists() {
                check_size(&dbf)?;
            }
            read_shapefile(path, &name)?
        }
        other => return Err(DataError::Unsupported(other.to_string())),
    };
    out.0.source = Some(path.display().to_string());
    Ok(out)
}

fn check_size(path: &Path) -> Result<()> {
    let len = std::fs::metadata(path)?.len();
    if len > MAX_BYTES {
        return Err(DataError::Format(format!("file is too large ({len} bytes)")));
    }
    Ok(())
}

/// Writes a dataset by extension (`.geojson`/`.json` or `.csv`).
pub fn write_path(ds: &Dataset, path: &Path) -> Result<()> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let text = match ext.as_str() {
        "geojson" | "json" => serde_json::to_string(&to_geojson(ds)).map_err(|e| DataError::Format(e.to_string()))?,
        "csv" => to_csv(ds)?,
        other => return Err(DataError::Unsupported(other.to_string())),
    };
    write_atomic(path, text.as_bytes())?;
    Ok(())
}

/// Writes through a temporary file that is synced and renamed into place, so an interrupted
/// write never truncates an existing file. The temporary file is removed on failure.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let Some(name) = path.file_name() else {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{} is not a file path", path.display())));
    };
    // A unique temp name per write, so concurrent writes to the same target never share one,
    // and `create_new` so an existing file or symlink at that name is never followed.
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".{}.{}.tmp", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
    let tmp = path.with_file_name(tmp_name);
    let res = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

// ---------------------------------------------------------------------------------------------
// GeoJSON (RFC 7946)

fn coord(v: &Json) -> Option<Coord> {
    let a = v.as_array()?;
    let c = Coord::new(a.first()?.as_f64()?, a.get(1)?.as_f64()?);
    c.is_finite().then_some(c)
}

fn coords(v: &Json) -> Option<Vec<Coord>> {
    v.as_array()?.iter().map(coord).collect()
}

fn rings(v: &Json) -> Option<Vec<Vec<Coord>>> {
    v.as_array()?.iter().map(coords).collect()
}

/// Parses a GeoJSON geometry object (GeometryCollection keeps its first member).
pub fn geometry_from_geojson(g: &Json) -> Option<Geometry> {
    let c = g.get("coordinates");
    match g.get("type")?.as_str()? {
        "Point" => coord(c?).map(Geometry::Point),
        "MultiPoint" => coords(c?).map(Geometry::MultiPoint),
        "LineString" => coords(c?).map(|l| Geometry::Line(vec![l])),
        "MultiLineString" => rings(c?).map(Geometry::Line),
        "Polygon" => rings(c?).map(|r| Geometry::Polygon(vec![r])),
        "MultiPolygon" => c?.as_array()?.iter().map(rings).collect::<Option<Vec<_>>>().map(Geometry::Polygon),
        "GeometryCollection" => g.get("geometries")?.as_array()?.iter().find_map(geometry_from_geojson),
        _ => None,
    }
}

pub fn geometry_to_geojson(g: &Geometry) -> Json {
    fn c(c: &Coord) -> Json {
        json!([c.x, c.y])
    }
    fn l(v: &[Coord]) -> Json {
        Json::Array(v.iter().map(c).collect())
    }
    match g {
        Geometry::Point(p) => json!({"type": "Point", "coordinates": c(p)}),
        Geometry::MultiPoint(v) => json!({"type": "MultiPoint", "coordinates": l(v)}),
        Geometry::Line(parts) if let [only] = parts.as_slice() => json!({"type": "LineString", "coordinates": l(only)}),
        Geometry::Line(parts) => json!({"type": "MultiLineString", "coordinates": parts.iter().map(|p| l(p)).collect::<Vec<_>>()}),
        Geometry::Polygon(polys) if let [only] = polys.as_slice() => {
            json!({"type": "Polygon", "coordinates": only.iter().map(|r| l(r)).collect::<Vec<_>>()})
        }
        Geometry::Polygon(polys) => json!({"type": "MultiPolygon", "coordinates": polys
            .iter()
            .map(|rings| rings.iter().map(|r| l(r)).collect::<Vec<_>>())
            .collect::<Vec<_>>()}),
    }
}

/// Reads a GeoJSON FeatureCollection, Feature or bare geometry.
pub fn read_geojson(text: &str, name: &str) -> Result<(Dataset, ReadReport)> {
    let root: Json = serde_json::from_str(text).map_err(|e| DataError::Format(format!("GeoJSON: {e}")))?;
    // Borrow the features rather than copying a document that may be very large.
    let wrapped: Json;
    let features: Vec<&Json> = match root.get("type").and_then(Json::as_str) {
        Some("FeatureCollection") => root.get("features").and_then(Json::as_array).map(|a| a.iter().collect()).unwrap_or_default(),
        Some("Feature") => vec![&root],
        Some(_) => {
            wrapped = json!({"type": "Feature", "geometry": &root, "properties": {}});
            vec![&wrapped]
        }
        None => return Err(DataError::Format("GeoJSON: missing \"type\"".into())),
    };
    // Schema: union of property keys in first-seen order. A type stays unknown until a non-null
    // value is seen (so `[null, 1, 2]` is Integer), then widens; all-null columns become Text.
    let mut order: Vec<(String, Option<FieldType>)> = Vec::new();
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for &f in &features {
        if let Some(props) = f.get("properties").and_then(Json::as_object) {
            for (k, v) in props {
                let ty = json_type(v);
                match index.get(k).and_then(|i| order.get_mut(*i)) {
                    Some((_, cur)) => {
                        *cur = match *cur {
                            None => ty,
                            Some(c) => Some(widen(c, ty)),
                        }
                    }
                    None => {
                        index.insert(k.clone(), order.len());
                        order.push((k.clone(), ty));
                    }
                }
            }
        }
    }
    let fields: Vec<Field> = order.into_iter().map(|(k, ty)| Field::new(k, ty.unwrap_or(FieldType::Text))).collect();
    let mut report = ReadReport::default();
    let mut ds = Dataset::new(name, GeometryType::None, fields);
    for &f in &features {
        let geom = f.get("geometry").filter(|g| !g.is_null()).and_then(geometry_from_geojson);
        if geom.is_none() && f.get("geometry").is_some_and(|g| !g.is_null()) {
            report.skipped += 1;
            continue;
        }
        let props = f.get("properties").and_then(Json::as_object);
        let attrs = ds
            .fields
            .iter()
            .map(|field| {
                let v = props.and_then(|p| p.get(&field.name)).map(Value::from_json).unwrap_or(Value::Null);
                coerce(v, field.ty)
            })
            .collect();
        ds.push(geom, attrs);
    }
    ds.geometry_type = dominant_type(&ds);
    if report.skipped > 0 {
        report.messages.push(format!("{} features had unreadable geometry", report.skipped));
    }
    Ok((ds, report))
}

fn json_type(v: &Json) -> Option<FieldType> {
    match v {
        Json::Bool(_) => Some(FieldType::Boolean),
        Json::Number(n) if n.is_i64() => Some(FieldType::Integer),
        Json::Number(_) => Some(FieldType::Double),
        Json::Null => None,
        _ => Some(FieldType::Text),
    }
}

fn widen(a: FieldType, b: Option<FieldType>) -> FieldType {
    match (a, b) {
        (a, None) => a,
        (a, Some(b)) if a == b => a,
        (FieldType::Integer, Some(FieldType::Double)) | (FieldType::Double, Some(FieldType::Integer)) => FieldType::Double,
        _ => FieldType::Text,
    }
}

fn coerce(v: Value, ty: FieldType) -> Value {
    match (ty, v) {
        (_, Value::Null) => Value::Null,
        (FieldType::Double, Value::Int(i)) => Value::Float(i as f64),
        (FieldType::Text, v @ Value::Text(_)) => v,
        (FieldType::Text, v) => Value::Text(v.to_string()),
        (_, v) => v,
    }
}

fn dominant_type(ds: &Dataset) -> GeometryType {
    let mut counts = [0usize; 3];
    for g in ds.features.iter().filter_map(|f| f.geometry.as_ref()) {
        match g.kind() {
            GeometryType::Point => counts[0] += 1,
            GeometryType::Polyline => counts[1] += 1,
            GeometryType::Polygon => counts[2] += 1,
            GeometryType::None => {}
        }
    }
    match counts.iter().enumerate().max_by_key(|(_, n)| **n) {
        Some((_, 0)) | None => GeometryType::None,
        Some((0, _)) => GeometryType::Point,
        Some((1, _)) => GeometryType::Polyline,
        Some(_) => GeometryType::Polygon,
    }
}

pub fn to_geojson(ds: &Dataset) -> Json {
    let features: Vec<Json> = ds
        .features
        .iter()
        .map(|f| {
            let mut props = Map::new();
            for (field, v) in ds.fields.iter().zip(&f.attrs) {
                props.insert(field.name.clone(), v.to_json());
            }
            json!({"type": "Feature", "id": f.id, "properties": props, "geometry": f.geometry.as_ref().map(geometry_to_geojson)})
        })
        .collect();
    json!({"type": "FeatureCollection", "name": ds.name, "features": features})
}

// ---------------------------------------------------------------------------------------------
// CSV

const LON_NAMES: &[&str] = &["lon", "lng", "long", "longitude", "x", "lon_dd", "longitude_dd"];
const LAT_NAMES: &[&str] = &["lat", "latitude", "y", "lat_dd", "latitude_dd"];

/// Reads CSV; columns named like longitude/latitude become point geometry.
pub fn read_csv(text: &str, name: &str) -> Result<(Dataset, ReadReport)> {
    // Spreadsheet exports often start with a byte-order mark, which would hide the first header.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut rdr = csv::ReaderBuilder::new().flexible(true).from_reader(text.as_bytes());
    let headers: Vec<String> = rdr.headers().map_err(|e| DataError::Format(format!("CSV: {e}")))?.iter().map(|s| s.to_string()).collect();
    let find = |names: &[&str]| headers.iter().position(|h| names.contains(&h.trim().to_ascii_lowercase().as_str()));
    let (lon, lat) = (find(LON_NAMES), find(LAT_NAMES));
    let mut malformed = 0usize;
    let rows: Vec<Vec<String>> = rdr
        .records()
        .filter_map(|r| match r {
            Ok(r) => Some(r.iter().map(|s| s.to_string()).collect()),
            Err(_) => {
                malformed += 1;
                None
            }
        })
        .collect();
    // Infer each column's type from its values.
    let fields: Vec<Field> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let vals = rows.iter().filter_map(|r| r.get(i)).map(|s| s.trim()).filter(|s| !s.is_empty());
            let mut ty: Option<FieldType> = None;
            for v in vals {
                let t = if v.parse::<i64>().is_ok() {
                    FieldType::Integer
                } else if v.parse::<f64>().is_ok_and(f64::is_finite) {
                    FieldType::Double
                } else {
                    FieldType::Text
                };
                ty = Some(match ty {
                    None => t,
                    Some(prev) => widen(prev, Some(t)),
                });
            }
            Field::new(h.trim(), ty.unwrap_or(FieldType::Text))
        })
        .collect();
    let mut report = ReadReport { skipped: malformed, ..ReadReport::default() };
    if malformed > 0 {
        report.messages.push(format!("{malformed} rows were malformed and skipped"));
    }
    let gt = if lon.is_some() && lat.is_some() { GeometryType::Point } else { GeometryType::None };
    let mut ds = Dataset::new(name, gt, fields);
    for r in &rows {
        let geom = match (lon, lat) {
            (Some(x), Some(y)) => {
                let c = r.get(x).and_then(|s| s.trim().parse::<f64>().ok()).zip(r.get(y).and_then(|s| s.trim().parse::<f64>().ok()));
                match c {
                    Some((x, y)) if x.is_finite() && y.is_finite() => Some(Geometry::Point(Coord::new(x, y))),
                    _ => {
                        report.skipped += 1;
                        None
                    }
                }
            }
            _ => None,
        };
        let attrs = ds.fields.iter().enumerate().map(|(i, f)| Value::parse(r.get(i).map(|s| s.as_str()).unwrap_or(""), f.ty)).collect();
        ds.push(geom, attrs);
    }
    if report.skipped > malformed {
        report.messages.push(format!("{} rows had no usable coordinates", report.skipped - malformed));
    }
    Ok((ds, report))
}

/// A cell's text; text that a spreadsheet would run as a formula is prefixed with `'`.
fn csv_cell(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Text(t) if t.starts_with(['=', '+', '-', '@', '\t', '\r']) => format!("'{t}"),
        other => other.to_string(),
    }
}

pub fn to_csv(ds: &Dataset) -> Result<String> {
    let mut w = csv::Writer::from_writer(Vec::new());
    let point = ds.geometry_type == GeometryType::Point;
    let mut head: Vec<String> = ds.fields.iter().map(|f| f.name.clone()).collect();
    if point {
        // Coordinate columns get names that don't collide with existing fields.
        for base in ["longitude", "latitude"] {
            let mut name = base.to_string();
            let mut i = 1;
            while head.iter().any(|h| h.eq_ignore_ascii_case(&name)) {
                name = format!("{base}_{i}");
                i += 1;
            }
            head.push(name);
        }
    }
    w.write_record(&head).map_err(|e| DataError::Format(e.to_string()))?;
    for f in &ds.features {
        let mut row: Vec<String> = f.attrs.iter().map(csv_cell).collect();
        if point {
            let c = f.geometry.as_ref().and_then(|g| g.label_point());
            row.push(c.map(|c| c.x.to_string()).unwrap_or_default());
            row.push(c.map(|c| c.y.to_string()).unwrap_or_default());
        }
        w.write_record(&row).map_err(|e| DataError::Format(e.to_string()))?;
    }
    let bytes = w.into_inner().map_err(|e| DataError::Format(e.to_string()))?;
    String::from_utf8(bytes).map_err(|e| DataError::Format(e.to_string()))
}

// ---------------------------------------------------------------------------------------------
// Shapefile

fn read_shapefile(path: &Path, name: &str) -> Result<(Dataset, ReadReport)> {
    use shapefile::Shape;
    use shapefile::dbase::FieldValue;
    let fmt = |e: shapefile::Error| DataError::Format(format!("Shapefile: {e}"));
    // Field order comes from the .dbf header (records are unordered maps).
    let dbf = path.with_extension("dbf");
    let order: Vec<(String, FieldType)> = match shapefile::dbase::Reader::from_path(&dbf) {
        Ok(r) => r
            .fields()
            .iter()
            .filter(|f| f.name() != "DeletionFlag")
            .map(|f| {
                use shapefile::dbase::FieldType as D;
                let ty = match f.field_type() {
                    D::Numeric | D::Float | D::Double | D::Currency => FieldType::Double,
                    D::Integer => FieldType::Integer,
                    D::Logical => FieldType::Boolean,
                    _ => FieldType::Text,
                };
                (f.name().to_string(), ty)
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    let mut reader = shapefile::Reader::from_path(path).map_err(fmt)?;
    let fields: Vec<Field> = order.iter().map(|(n, t)| Field::new(n.clone(), *t)).collect();
    let mut ds = Dataset::new(name, GeometryType::None, fields);
    let mut report = ReadReport::default();
    let pt = |x: f64, y: f64| Coord::new(x, y);
    for item in reader.iter_shapes_and_records() {
        let Ok((shape, record)) = item else {
            report.skipped += 1;
            continue;
        };
        let geom = match shape {
            Shape::NullShape => None,
            Shape::Point(p) => Some(Geometry::Point(pt(p.x, p.y))),
            Shape::PointM(p) => Some(Geometry::Point(pt(p.x, p.y))),
            Shape::PointZ(p) => Some(Geometry::Point(pt(p.x, p.y))),
            Shape::Multipoint(m) => Some(Geometry::MultiPoint(m.points().iter().map(|p| pt(p.x, p.y)).collect())),
            Shape::MultipointM(m) => Some(Geometry::MultiPoint(m.points().iter().map(|p| pt(p.x, p.y)).collect())),
            Shape::MultipointZ(m) => Some(Geometry::MultiPoint(m.points().iter().map(|p| pt(p.x, p.y)).collect())),
            Shape::Polyline(l) => Some(Geometry::Line(l.parts().iter().map(|p| p.iter().map(|q| pt(q.x, q.y)).collect()).collect())),
            Shape::PolylineM(l) => Some(Geometry::Line(l.parts().iter().map(|p| p.iter().map(|q| pt(q.x, q.y)).collect()).collect())),
            Shape::PolylineZ(l) => Some(Geometry::Line(l.parts().iter().map(|p| p.iter().map(|q| pt(q.x, q.y)).collect()).collect())),
            Shape::Polygon(p) => {
                Some(group_rings(p.rings().iter().map(|r| (r.points().iter().map(|q| pt(q.x, q.y)).collect(), is_outer(r))).collect()))
            }
            Shape::PolygonM(p) => {
                Some(group_rings(p.rings().iter().map(|r| (r.points().iter().map(|q| pt(q.x, q.y)).collect(), is_outer(r))).collect()))
            }
            Shape::PolygonZ(p) => {
                Some(group_rings(p.rings().iter().map(|r| (r.points().iter().map(|q| pt(q.x, q.y)).collect(), is_outer(r))).collect()))
            }
            Shape::Multipatch(_) => {
                report.skipped += 1;
                continue;
            }
        };
        let attrs = ds
            .fields
            .iter()
            .map(|f| match record.get(&f.name) {
                Some(FieldValue::Character(Some(s))) => Value::Text(s.trim().to_string()),
                Some(FieldValue::Numeric(Some(n))) if n.is_finite() => Value::Float(*n),
                Some(FieldValue::Float(Some(n))) => Value::Float(f64::from(*n)),
                Some(FieldValue::Integer(i)) => Value::Int(i64::from(*i)),
                Some(FieldValue::Double(d)) => Value::Float(*d),
                Some(FieldValue::Currency(d)) => Value::Float(*d),
                Some(FieldValue::Logical(Some(b))) => Value::Bool(*b),
                Some(FieldValue::Memo(s)) => Value::Text(s.clone()),
                Some(FieldValue::Date(Some(d))) => Value::Text(format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())),
                _ => Value::Null,
            })
            .collect();
        ds.push(geom, attrs);
    }
    ds.geometry_type = dominant_type(&ds);
    integral_numeric_to_integer(&mut ds);
    let e = ds.extent();
    if !e.is_empty() && (e.min_x < -180.5 || e.max_x > 180.5 || e.min_y < -90.5 || e.max_y > 90.5) {
        report.messages.push("coordinates are not longitude/latitude; RustGIS reads them as WGS 84 and they may draw out of place".into());
    }
    if report.skipped > 0 {
        report.messages.push(format!("{} shapes were unreadable or unsupported", report.skipped));
    }
    Ok((ds, report))
}

/// The .dbf header doesn't say whether a Numeric column has decimals, so a Double column whose
/// values are all whole numbers within i64 range is read as Integer.
fn integral_numeric_to_integer(ds: &mut Dataset) {
    const LIMIT: f64 = 9.0e15;
    for fi in 0..ds.fields.len() {
        if ds.fields.get(fi).map(|f| f.ty) != Some(FieldType::Double) {
            continue;
        }
        let whole = ds.features.iter().all(|f| match f.attrs.get(fi) {
            Some(Value::Float(v)) => v.fract() == 0.0 && v.abs() < LIMIT,
            _ => true,
        });
        let any = ds.features.iter().any(|f| matches!(f.attrs.get(fi), Some(Value::Float(_))));
        if !whole || !any {
            continue;
        }
        if let Some(f) = ds.fields.get_mut(fi) {
            f.ty = FieldType::Integer;
        }
        for f in &mut ds.features {
            if let Some(v) = f.attrs.get_mut(fi)
                && let Value::Float(x) = *v
            {
                *v = Value::Int(x as i64);
            }
        }
    }
}

fn is_outer<P>(r: &shapefile::PolygonRing<P>) -> bool {
    matches!(r, shapefile::PolygonRing::Outer(_))
}

/// Groups a flat list of rings (outer flag) into polygons: each hole joins the preceding outer ring.
fn group_rings(rings: Vec<(Vec<Coord>, bool)>) -> Geometry {
    let mut polys: Vec<Vec<Vec<Coord>>> = Vec::new();
    for (ring, outer) in rings {
        match polys.last_mut() {
            Some(p) if !outer => p.push(ring),
            _ => polys.push(vec![ring]),
        }
    }
    Geometry::Polygon(polys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geojson_roundtrip() {
        let text = r#"{"type":"FeatureCollection","features":[
            {"type":"Feature","properties":{"name":"A","pop":5},"geometry":{"type":"Point","coordinates":[1,2]}},
            {"type":"Feature","properties":{"name":"B","pop":2.5},"geometry":{"type":"Point","coordinates":[3,4]}},
            {"type":"Feature","properties":{"name":"C"},"geometry":{"type":"Point","coordinates":["x",4]}}]}"#;
        let (ds, rep) = read_geojson(text, "t").unwrap();
        assert_eq!(ds.features.len(), 2);
        assert_eq!(rep.skipped, 1);
        assert_eq!(ds.fields[1].ty, FieldType::Double);
        assert_eq!(ds.geometry_type, GeometryType::Point);
        let back = read_geojson(&to_geojson(&ds).to_string(), "t").unwrap().0;
        assert_eq!(back.features.len(), 2);
        assert_eq!(back.value(1, 0), &Value::Text("B".into()));
    }

    #[test]
    fn geojson_null_first_keeps_numeric_type() {
        let text = r#"{"type":"FeatureCollection","features":[
            {"type":"Feature","properties":{"v":null,"n":null},"geometry":null},
            {"type":"Feature","properties":{"v":1,"n":null},"geometry":null},
            {"type":"Feature","properties":{"v":2.5},"geometry":null}]}"#;
        let (ds, _) = read_geojson(text, "t").unwrap();
        let (v, n) = (ds.field_index("v").unwrap(), ds.field_index("n").unwrap());
        assert_eq!(ds.fields[v].ty, FieldType::Double);
        assert_eq!(ds.fields[n].ty, FieldType::Text, "all-null columns are Text");
        assert_eq!(ds.value(1, v), &Value::Float(1.0));
    }

    #[test]
    fn write_atomic_replaces_and_cleans_up() {
        let dir = std::env::temp_dir().join(format!("rustgis-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("out.txt");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temp files left behind");
        assert!(write_atomic(Path::new("/"), b"x").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn csv_points() {
        let (ds, rep) = read_csv("name,Lat,Lon,n\nA,10,20,1\nB,bad,1,2\n", "t").unwrap();
        assert_eq!(ds.geometry_type, GeometryType::Point);
        assert_eq!(rep.skipped, 1);
        assert_eq!(ds.features[0].geometry, Some(Geometry::Point(Coord::new(20.0, 10.0))));
        assert!(to_csv(&ds).unwrap().contains("longitude"));
    }

    #[test]
    fn csv_bom_injection_and_column_names() {
        let (ds, _) = read_csv("\u{feff}lon,lat,longitude,note\n1,2,9,=SUM(A1)\n", "t").unwrap();
        assert_eq!(ds.geometry_type, GeometryType::Point, "BOM must not hide the lon header");
        let out = to_csv(&ds).unwrap();
        assert!(out.contains("'=SUM(A1)"), "{out}");
        assert!(out.lines().next().unwrap().contains("longitude_1"), "{out}");
        let (ds, _) = read_csv("a\nNaN\nx\n", "t").unwrap();
        assert_eq!(ds.fields[0].ty, FieldType::Text);
    }

    #[test]
    fn hostile_geojson_is_an_error_not_a_panic() {
        for t in ["", "{", "[]", "{\"type\":5}", "{\"type\":\"FeatureCollection\",\"features\":7}"] {
            let _ = read_geojson(t, "x");
        }
    }

    #[test]
    fn read_path_dispatches_by_extension() {
        let dir = std::env::temp_dir().join(format!("rustgis-io-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("square.GeoJSON");
        std::fs::write(&p, r#"{"type":"Polygon","coordinates":[[[0,0],[1,0],[1,1],[0,0]]]}"#).unwrap();
        let (ds, _) = read_path(&p).unwrap();
        assert_eq!(ds.geometry_type, GeometryType::Polygon);
        assert!(read_path(&dir.join("x.kml")).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
