//! Vector readers and writers: GeoJSON, CSV (with longitude/latitude columns) and Shapefile.

use std::path::Path;

use serde_json::{Map, Value as Json, json};

use crate::{Coord, DataError, Dataset, Field, FieldType, Geometry, GeometryType, ReadReport, Result, Value};

/// Largest file read into memory (256 MiB). Parsing JSON can take several times the file size.
const MAX_BYTES: u64 = 256 << 20;

/// File types the readers understand, for open dialogs.
pub const READ_EXTENSIONS: &[&str] = &["geojson", "json", "csv", "txt", "shp", "zip", "gpkg", "parquet", "geoparquet", "fgb"];

/// Reads vector data held in memory (a browser upload or a drop), choosing the reader by the
/// file name's extension. A Shapefile needs its sidecar files, so it comes zipped (`.zip`, whose
/// first layer is read).
pub fn read_bytes(file_name: &str, bytes: &[u8]) -> Result<(Dataset, ReadReport)> {
    let path = Path::new(file_name);
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("layer").to_string();
    if bytes.len() as u64 > MAX_BYTES {
        return Err(DataError::Format(format!("file is too large ({} bytes)", bytes.len())));
    }
    let text = || std::str::from_utf8(bytes).map_err(|_| DataError::Format(format!("{file_name} is not UTF-8 text")));
    let mut out = match ext.as_str() {
        "geojson" | "json" => read_geojson(text()?, &name)?,
        "csv" | "txt" => read_csv(text()?, &name)?,
        "shp" => return Err(DataError::Unsupported("shp on its own: zip it with its .dbf, .shx and .prj and add the zip".into())),
        // The first layer of the archive (`read_zip` reads them all), saying what else it holds.
        "zip" => {
            let layers = read_zip(file_name, bytes)?;
            let rest: Vec<String> = layers.iter().skip(1).map(|(d, _)| d.name.clone()).collect();
            let mut first = layers.into_iter().next().ok_or_else(|| DataError::Format(format!("{file_name}: empty")))?;
            if !rest.is_empty() {
                let leaf = Path::new(file_name).file_name().and_then(|n| n.to_str()).unwrap_or(file_name);
                first.1.messages.push(format!("{leaf}: read its first layer; it also holds {}", rest.join(", ")));
            }
            return Ok(first);
        }
        // A copy, since only borrowed bytes are given; `read_owned` takes them without one.
        e if crate::source::EXTENSIONS.contains(&e) => return read_owned(file_name, bytes.to_vec()),
        other => return Err(DataError::Unsupported(other.to_string())),
    };
    out.0.source = Some(file_name.to_string());
    Ok(out)
}

/// Reads a GeoPackage, GeoParquet or FlatGeobuf held in memory without copying it (other types go
/// to [`read_bytes`]).
pub fn read_owned(file_name: &str, bytes: Vec<u8>) -> Result<(Dataset, ReadReport)> {
    if !crate::source::is_source_name(file_name) {
        return read_bytes(file_name, &bytes);
    }
    if bytes.len() as u64 > MAX_BYTES {
        return Err(DataError::Format(format!("file is too large ({} bytes)", bytes.len())));
    }
    let mut out = crate::source::materialize(crate::source::open_bytes(file_name, bytes, None)?.as_ref())?;
    out.0.source = Some(file_name.to_string());
    Ok(out)
}

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
        "zip" => {
            check_size(path)?;
            read_bytes(&path.display().to_string(), &std::fs::read(path)?)?
        }
        // Read in place by a provider, so there's no size cap on the file, only on the features.
        e if crate::source::EXTENSIONS.contains(&e) => crate::source::materialize(crate::source::open_path(path, None)?.as_ref())?,
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

/// The field KML and GPX name placemarks by: one called `name`, if any.
fn name_field(ds: &Dataset) -> Option<usize> {
    ds.fields.iter().position(|f| f.name.eq_ignore_ascii_case("name"))
}

/// Features a format that holds only features with geometry leaves out.
fn without_geometry(ds: &Dataset) -> usize {
    ds.features.iter().filter(|f| f.geometry.is_none()).count()
}

/// A dataset encoded in the format of extension `ext`, as one file, and the number of features the
/// format couldn't hold. A Shapefile (`shp` or `zip`) comes as a zip of its files, named after the
/// dataset. GeoPackage needs a file ([`write_path`]).
pub fn write_bytes(ds: &Dataset, ext: &str) -> Result<(Vec<u8>, usize)> {
    let fmt = |e: serde_json::Error| DataError::Format(e.to_string());
    Ok(match ext.to_ascii_lowercase().as_str() {
        "geojson" | "json" => (serde_json::to_vec(&to_geojson(ds)).map_err(fmt)?, 0),
        "csv" => (to_csv(ds)?.into_bytes(), 0),
        "fgb" => (crate::source::fgb::write(ds)?, without_geometry(ds)),
        "parquet" | "geoparquet" => (crate::source::parquet::write(ds)?, 0),
        "kml" => (crate::interop::to_kml(ds, name_field(ds)).into_bytes(), without_geometry(ds)),
        "kmz" => {
            let kml = crate::interop::to_kml(ds, name_field(ds)).into_bytes();
            (crate::zip::write(&[("doc.kml".to_string(), kml)]).map_err(DataError::Format)?, without_geometry(ds))
        }
        "gpx" => (crate::interop::to_gpx(ds, name_field(ds)).into_bytes(), without_geometry(ds)),
        "shp" | "zip" => {
            let (parts, left_out) = shapefile_parts(ds)?;
            let base: String = ds.name.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | ' ') { c } else { '_' }).collect();
            let base = if base.trim().is_empty() { "layer".to_string() } else { base };
            let entries: Vec<(String, Vec<u8>)> = parts.into_iter().map(|(e, b)| (format!("{base}.{e}"), b)).collect();
            (crate::zip::write(&entries).map_err(DataError::Format)?, left_out)
        }
        other => return Err(DataError::Unsupported(other.to_string())),
    })
}

/// Writes a dataset by extension (`.geojson`/`.json`, `.csv`, `.fgb`, `.gpkg` on the desktop,
/// `.parquet`/`.geoparquet`, `.shp`, `.zip`, `.kml`, `.kmz` or `.gpx`); returns the number of features the
/// format couldn't hold (FlatGeobuf, KML, GPX and Shapefile hold only features with geometry). A
/// Shapefile is written as its `.shp`, `.shx`, `.dbf`, `.prj` and `.cpg` files (or zipped, to a
/// `.zip`).
pub fn write_path(ds: &Dataset, path: &Path) -> Result<usize> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "gpkg" => {
            #[cfg(not(target_arch = "wasm32"))]
            {
                crate::source::gpkg::write(ds, path)?;
                Ok(0)
            }
            #[cfg(target_arch = "wasm32")]
            Err(DataError::Unsupported("GeoPackage in the browser".into()))
        }
        "shp" => {
            let (parts, left_out) = shapefile_parts(ds)?;
            // Every part is written to a temporary file first and renamed into place only once all
            // are written, the .shp last, so a failed write leaves the old Shapefile whole.
            let mut written: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
            let staged = parts.iter().try_for_each(|(e, bytes)| {
                // The .shp keeps the name given (`Roads.SHP` stays so); the others take its stem.
                let target = if *e == "shp" { path.to_path_buf() } else { path.with_extension(e) };
                let tmp = temp_beside(&target)?;
                written.push((tmp.clone(), target));
                write_new(&tmp, bytes)
            });
            if let Err(e) = staged {
                for (tmp, _) in &written {
                    let _ = std::fs::remove_file(tmp);
                }
                return Err(e.into());
            }
            written.sort_by_key(|(_, target)| target == path);
            // The old parts are moved aside first and put back if any new part fails to land, so
            // the old Shapefile stays a matching set.
            let mut backups: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
            let mut placed: Vec<std::path::PathBuf> = Vec::new();
            let mut result = Ok(());
            for (tmp, target) in &written {
                if target.exists() {
                    // Any failure here rolls back like a failed rename.
                    let bak = match temp_beside(target) {
                        Ok(b) => b,
                        Err(e) => {
                            result = Err(e);
                            break;
                        }
                    };
                    if let Err(e) = std::fs::rename(target, &bak) {
                        result = Err(e);
                        break;
                    }
                    backups.push((bak, target.clone()));
                }
                if let Err(e) = std::fs::rename(tmp, target) {
                    result = Err(e);
                    break;
                }
                placed.push(target.clone());
            }
            if let Err(e) = result {
                for p in &placed {
                    let _ = std::fs::remove_file(p);
                }
                for (bak, target) in &backups {
                    let _ = std::fs::rename(bak, target);
                }
                for (tmp, _) in &written {
                    let _ = std::fs::remove_file(tmp);
                }
                return Err(e.into());
            }
            for (bak, _) in &backups {
                let _ = std::fs::remove_file(bak);
            }
            // A spatial index left from an older file of the same name would no longer match.
            for stale in ["qix", "sbn", "sbx"] {
                let _ = std::fs::remove_file(path.with_extension(stale));
            }
            Ok(left_out)
        }
        _ => {
            let (bytes, left_out) = write_bytes(ds, &ext)?;
            write_atomic(path, &bytes)?;
            Ok(left_out)
        }
    }
}

/// A new temporary name beside `path`, unique per call, so concurrent writes to the same target
/// never share one.
pub(crate) fn temp_beside(path: &Path) -> std::io::Result<std::path::PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let Some(name) = path.file_name() else {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{} is not a file path", path.display())));
    };
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".{}.{}.tmp", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
    Ok(path.with_file_name(tmp_name))
}

/// Writes and syncs a new file (`create_new`, so an existing file or symlink is never followed);
/// removes it on failure.
fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let res = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(path);
    }
    res
}

/// Writes through a temporary file that is synced and renamed into place, so an interrupted
/// write never truncates an existing file. The temporary file is removed on failure.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = temp_beside(path)?;
    write_new(&tmp, bytes)?;
    let res = std::fs::rename(&tmp, path);
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
        ds.push_next(geom, attrs);
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
                let t = if has_leading_zero(v) {
                    // Identifiers such as ZIP codes keep their leading zeros.
                    FieldType::Text
                } else if v.parse::<i64>().is_ok() {
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
        let attrs =
            ds.fields.iter().enumerate().map(|(i, f)| Value::parse(&unguard(r.get(i).map(|s| s.as_str()).unwrap_or("")), f.ty)).collect();
        ds.push_next(geom, attrs);
    }
    if report.skipped > malformed {
        report.messages.push(format!("{} rows had no usable coordinates", report.skipped - malformed));
    }
    Ok((ds, report))
}

/// `0123` or `-007` (but not `0`, `0.5` or `-0.5`): a code, not a number.
fn has_leading_zero(v: &str) -> bool {
    let d = v.strip_prefix(['-', '+']).unwrap_or(v);
    d.len() > 1 && d.starts_with('0') && d.as_bytes().get(1).is_some_and(|b| b.is_ascii_digit())
}

/// Undoes the export formula guard: `'=x` reads back as `=x`.
fn unguard(cell: &str) -> std::borrow::Cow<'_, str> {
    match cell.strip_prefix('\'') {
        Some(rest) if rest.starts_with(FORMULA_START) => std::borrow::Cow::Owned(rest.to_string()),
        _ => std::borrow::Cow::Borrowed(cell),
    }
}

const FORMULA_START: [char; 6] = ['=', '+', '-', '@', '\t', '\r'];

/// A cell's text; text that a spreadsheet would run as a formula is prefixed with `'`.
fn csv_cell(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Text(t) if t.starts_with(FORMULA_START) => format!("'{t}"),
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
    let dbf = path.with_extension("dbf");
    if !dbf.is_file() {
        return Err(DataError::Format(format!("Shapefile: {} is missing", dbf.display())));
    }
    let shx = path.with_extension("shx");
    let parts = ShapefileParts {
        shp: std::fs::read(path)?,
        shx: if shx.is_file() { Some(std::fs::read(shx)?) } else { None },
        dbf: std::fs::read(dbf)?,
        prj: crate::raster::read_prj(path),
    };
    read_shapefile_parts(parts, name)
}

/// Most layers read from one zip archive.
pub const MAX_ZIP_LAYERS: usize = 32;

/// Reads the vector data in a zip archive: each Shapefile in it (its `.shp` with the `.dbf`,
/// `.shx` and `.prj` beside it), then every other file the readers understand (GeoJSON, CSV,
/// GeoParquet, FlatGeobuf…; `.txt` files in archives are readmes, so they're skipped), at most
/// [`MAX_ZIP_LAYERS`]. Folders inside are fine; macOS
/// resource forks (`__MACOSX/`, `._*`) are ignored.
pub fn read_zip(file_name: &str, bytes: &[u8]) -> Result<Vec<(Dataset, ReadReport)>> {
    let lower = |n: &str| n.to_ascii_lowercase();
    let is_fork = |n: &str| n.split('/').any(|part| part == "__MACOSX" || part.starts_with("._"));
    let ext_of = |n: &str| Path::new(n).extension().and_then(|e| e.to_str()).map(lower).unwrap_or_default();
    let wanted = |n: &str| {
        let e = ext_of(n);
        !is_fork(n)
            && (matches!(e.as_str(), "shp" | "shx" | "dbf" | "prj")
                || (!matches!(e.as_str(), "zip" | "txt") && READ_EXTENSIONS.contains(&e.as_str())))
    };
    let entries = crate::zip::read_matching(bytes, wanted).map_err(|e| DataError::Format(format!("{file_name}: {e}")))?;
    // Sidecar files by their path without the extension, compared without regard to case.
    let stem = |n: &str| lower(n.get(..n.len().saturating_sub(ext_of(n).len() + 1)).unwrap_or(n));
    // Entries are taken out as they're used, so no file's bytes are copied.
    let mut entries: Vec<Option<(String, Vec<u8>)>> = entries.into_iter().map(Some).collect();
    let mut shps: Vec<String> = entries.iter().flatten().filter(|(n, _)| ext_of(n) == "shp").map(|(n, _)| n.clone()).collect();
    shps.sort();
    let take = |entries: &mut Vec<Option<(String, Vec<u8>)>>, base: &str, ext: &str| -> Option<(String, Vec<u8>)> {
        entries.iter_mut().find(|e| e.as_ref().is_some_and(|(n, _)| ext_of(n) == ext && stem(n) == base))?.take()
    };
    let mut out = Vec::new();
    // Members that couldn't be read are skipped, and said in the first layer's report.
    let mut skipped: Vec<String> = Vec::new();
    for n in shps {
        if out.len() >= MAX_ZIP_LAYERS {
            break;
        }
        let base = stem(&n);
        let name = Path::new(&n).file_stem().and_then(|s| s.to_str()).unwrap_or("layer").to_string();
        let shp = take(&mut entries, &base, "shp").map(|(_, b)| b).unwrap_or_default();
        let shx = take(&mut entries, &base, "shx").map(|(_, b)| b);
        let prj = take(&mut entries, &base, "prj").map(|(_, b)| String::from_utf8_lossy(&b).into_owned());
        let Some((_, dbf)) = take(&mut entries, &base, "dbf") else {
            skipped.push(format!("{n} (no .dbf beside it)"));
            continue;
        };
        match read_shapefile_parts(ShapefileParts { shp, shx, dbf, prj }, &name) {
            Ok(mut read) => {
                read.0.source = Some(format!("{file_name}/{n}"));
                out.push(read);
            }
            Err(e) => skipped.push(format!("{n} ({e})")),
        }
    }
    let mut others: Vec<(String, Vec<u8>)> =
        entries.into_iter().flatten().filter(|(n, _)| !matches!(ext_of(n).as_str(), "shp" | "shx" | "dbf" | "prj")).collect();
    others.sort_by(|a, b| a.0.cmp(&b.0));
    for (n, b) in others {
        if out.len() >= MAX_ZIP_LAYERS {
            skipped.push(format!("{n} (past the {MAX_ZIP_LAYERS}-layer limit)"));
            continue;
        }
        let leaf = n.rsplit('/').next().unwrap_or(&n);
        match read_bytes(leaf, &b) {
            Ok(mut read) => {
                read.0.source = Some(format!("{file_name}/{n}"));
                out.push(read);
            }
            Err(e) => skipped.push(format!("{n} ({e})")),
        }
    }
    if out.is_empty() {
        let why = if skipped.is_empty() { String::new() } else { format!(": {}", skipped.join("; ")) };
        return Err(DataError::Format(format!("{file_name}: no Shapefile or other vector data in the archive could be read{why}")));
    }
    if let Some((_, report)) = out.first_mut()
        && !skipped.is_empty()
    {
        report.messages.push(format!("{file_name}: skipped {}", skipped.join("; ")));
    }
    Ok(out)
}

/// The files of a Shapefile, read into memory.
struct ShapefileParts {
    shp: Vec<u8>,
    shx: Option<Vec<u8>>,
    dbf: Vec<u8>,
    prj: Option<String>,
}

fn read_shapefile_parts(parts: ShapefileParts, name: &str) -> Result<(Dataset, ReadReport)> {
    use shapefile::Shape;
    use shapefile::dbase::FieldValue;
    use std::io::Cursor;
    let fmt = |e: shapefile::Error| DataError::Format(format!("Shapefile: {e}"));
    let dbf = shapefile::dbase::Reader::new(Cursor::new(parts.dbf)).map_err(|e| DataError::Format(format!("Shapefile: the .dbf: {e}")))?;
    // Field order comes from the .dbf header (records are unordered maps).
    let order: Vec<(String, FieldType)> = dbf
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
        .collect();
    let shapes = match parts.shx {
        Some(shx) => shapefile::ShapeReader::with_shx(Cursor::new(parts.shp), Cursor::new(shx)),
        None => shapefile::ShapeReader::new(Cursor::new(parts.shp)),
    }
    .map_err(fmt)?;
    let mut reader = shapefile::Reader::new(shapes, dbf);
    let fields: Vec<Field> = order.iter().map(|(n, t)| Field::new(n.clone(), *t)).collect();
    let mut ds = Dataset::new(name, GeometryType::None, fields);
    let mut report = ReadReport::default();
    let to_lonlat = shapefile_crs(parts.prj.as_deref(), &mut report);
    let (mut converted, mut unconverted) = (0usize, 0usize);
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
        let geom = match (&to_lonlat, geom) {
            (Some(t), Some(g)) => {
                let mut ok = true;
                let out = g.map_coords(&|c| t.to_lonlat(c).unwrap_or(Coord::new(f64::NAN, f64::NAN)));
                out.for_each_coord(|c| ok &= c.is_finite());
                converted += usize::from(ok);
                unconverted += usize::from(!ok);
                ok.then_some(out)
            }
            (_, g) => g,
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
        ds.push_next(geom, attrs);
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
    if unconverted > 0 && converted == 0 {
        report.messages.push("no shape could be converted to longitude/latitude: the .prj doesn't seem to match the coordinates".into());
    } else if unconverted > 0 {
        report.messages.push(format!("{unconverted} shapes couldn't be converted to longitude/latitude and have no geometry"));
    }
    Ok((ds, report))
}

/// Converts a Shapefile's coordinates to longitude/latitude, from the coordinate system in its
/// `.prj`: `None` for longitude/latitude, or when there's no `.prj` or it isn't understood (said in
/// the report).
fn shapefile_crs(prj: Option<&str>, report: &mut ReadReport) -> Option<crate::raster::Transformer> {
    let crs = match crate::raster::wkt::read_wkt(prj?) {
        Ok((c, note)) => {
            if let Some(n) = note {
                report.messages.push(format!("the .prj file: {n}"));
            }
            c
        }
        Err(e) => {
            report.messages.push(format!("the .prj file: {e}; coordinates are read as longitude/latitude"));
            return None;
        }
    };
    if crs == crate::raster::Crs::LonLat {
        return None;
    }
    match crs.transformer() {
        Ok(t) => Some(t),
        Err(e) => {
            report.messages.push(format!("the .prj file: {e}; coordinates are read as longitude/latitude"));
            None
        }
    }
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

/// WGS 84 as ESRI WKT, for a Shapefile's `.prj`.
const WGS84_PRJ: &str = "GEOGCS[\"GCS_WGS_1984\",DATUM[\"D_WGS_1984\",SPHEROID[\"WGS_1984\",6378137.0,298.257223563]],PRIMEM[\"Greenwich\",0.0],UNIT[\"Degree\",0.0174532925199433]]";

/// The longest prefix of `s` that fits in `max` bytes, cut at a character boundary.
fn cut(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.get(..end).unwrap_or("")
}

/// dBase field names (at most 10 bytes, unique without regard to case) for the fields.
fn dbf_names(fields: &[Field]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(fields.len());
    for f in fields {
        let base = cut(f.name.trim(), 10);
        let base = if base.is_empty() { "FIELD" } else { base };
        let taken = |n: &str, out: &[String]| out.iter().any(|o| o.eq_ignore_ascii_case(n));
        let name = if !taken(base, &out) {
            base.to_string()
        } else {
            (1..100_000)
                .map(|i| {
                    let tag = format!("_{i}");
                    format!("{}{tag}", cut(base, 10 - tag.len()))
                })
                .find(|n| !taken(n, &out))
                .unwrap_or_else(|| base.to_string())
        };
        out.push(name);
    }
    out
}

/// A dataset as the files of a Shapefile: (extension, bytes) for `.shp`, `.shx`, `.dbf`, `.prj`
/// (WGS 84) and `.cpg` (UTF-8). Shapefiles hold one kind of shape and features with geometry:
/// features without it (or with nothing drawable) are left out, and the count of them returned.
/// Field names are cut to dBase's 10 bytes and text to 254.
pub fn shapefile_parts(ds: &Dataset) -> Result<(Vec<(&'static str, Vec<u8>)>, usize)> {
    use shapefile::dbase::{FieldName, FieldValue, Record, TableWriterBuilder};
    use std::io::Cursor;
    let fmt = |e: shapefile::Error| DataError::Format(format!("Shapefile: {e}"));
    let names = dbf_names(&ds.fields);
    let dbf_name = |n: &str| FieldName::try_from(n).map_err(|e| DataError::Format(format!("Shapefile: field {n}: {e}")));
    let mut table = TableWriterBuilder::new();
    // Per field: decimals of a Double field (numbers too large for 24 characters become null).
    let mut decimals = vec![0usize; ds.fields.len()];
    for (k, (f, n)) in ds.fields.iter().zip(&names).enumerate() {
        table = match f.ty {
            // 18 digits: GDAL reads a wider field as a real number.
            FieldType::Integer => table.add_numeric_field(dbf_name(n)?, 18, 0),
            FieldType::Double => {
                let max = ds
                    .features
                    .iter()
                    .filter_map(|r| r.attrs.get(k).and_then(Value::as_f64))
                    .filter(|v| v.is_finite())
                    .fold(0.0f64, |m, v| m.max(v.abs()));
                // Digits before the point, a sign and the point itself; the rest are decimals.
                let int_digits = if max >= 1.0 { (max.log10().floor() as usize).saturating_add(1) } else { 1 };
                let d = 24usize.saturating_sub(int_digits + 2).min(15);
                if let Some(slot) = decimals.get_mut(k) {
                    *slot = d;
                }
                table.add_numeric_field(dbf_name(n)?, 24, u8::try_from(d).unwrap_or(15))
            }
            FieldType::Boolean => table.add_logical_field(dbf_name(n)?),
            FieldType::Text => {
                let longest = ds.features.iter().filter_map(|r| r.attrs.get(k)).map(|v| v.to_string().len()).max().unwrap_or(0);
                table.add_character_field(dbf_name(n)?, u8::try_from(longest.clamp(1, 254)).unwrap_or(254))
            }
        };
    }
    let record = |r: &crate::Feature| -> Record {
        let mut rec = Record::default();
        for (k, (f, n)) in ds.fields.iter().zip(&names).enumerate() {
            let v = r.attrs.get(k).unwrap_or(&Value::Null);
            let value = match f.ty {
                FieldType::Integer | FieldType::Double => {
                    let x = v.as_f64().filter(|x| {
                        // Whatever doesn't fit the field is null rather than cut short; so is a
                        // whole number past 2^53, which dBase's decimal text would hold only
                        // rounded (it goes through f64).
                        let d = decimals.get(k).copied().unwrap_or(0);
                        // Likewise a number too small for the field's decimals, which would read 0.
                        let exact = !matches!(v, Value::Int(i) if i.unsigned_abs() > 1 << 53)
                            && !(f.ty == FieldType::Double && *x != 0.0 && x.abs() < 0.5 * 10f64.powi(-(d as i32)));
                        exact && format!("{x:.d$}").len() <= if f.ty == FieldType::Integer { 18 } else { 24 }
                    });
                    FieldValue::Numeric(if f.ty == FieldType::Integer { x.map(f64::round) } else { x })
                }
                FieldType::Boolean => FieldValue::Logical(match v {
                    Value::Bool(b) => Some(*b),
                    Value::Int(i) => Some(*i != 0),
                    _ => None,
                }),
                FieldType::Text => FieldValue::Character((!v.is_null()).then(|| cut(&v.to_string(), 254).to_string())),
            };
            rec.insert(n.clone(), value);
        }
        rec
    };

    let pt = |c: &Coord| shapefile::Point::new(c.x, c.y);
    let finite = |c: &&Coord| c.x.is_finite() && c.y.is_finite();
    let (mut shp, mut shx, mut dbf) = (Cursor::new(Vec::new()), Cursor::new(Vec::new()), Cursor::new(Vec::new()));
    let mut left_out = 0;
    let mut written = 0;
    {
        let shapes = shapefile::ShapeWriter::with_shx(&mut shp, &mut shx);
        let mut w = shapefile::Writer::new(shapes, table.build_with_dest(&mut dbf));
        let multi_points = ds.features.iter().any(|f| matches!(f.geometry, Some(Geometry::MultiPoint(_))));
        for f in &ds.features {
            let ok = match (&f.geometry, ds.geometry_type) {
                (Some(Geometry::Point(c)), GeometryType::Point) if !multi_points && finite(&c) => {
                    w.write_shape_and_record(&pt(c), &record(f)).map_err(fmt)?;
                    true
                }
                (Some(g @ (Geometry::Point(_) | Geometry::MultiPoint(_))), GeometryType::Point) => {
                    let mut pts = Vec::new();
                    g.for_each_coord(|c| pts.push(c));
                    let pts: Vec<shapefile::Point> = pts.iter().filter(finite).map(pt).collect();
                    if pts.is_empty() {
                        false
                    } else {
                        w.write_shape_and_record(&shapefile::Multipoint::new(pts), &record(f)).map_err(fmt)?;
                        true
                    }
                }
                (Some(Geometry::Line(parts)), GeometryType::Polyline) => {
                    let parts: Vec<Vec<shapefile::Point>> =
                        parts.iter().map(|p| p.iter().filter(finite).map(pt).collect::<Vec<_>>()).filter(|p| p.len() >= 2).collect();
                    if parts.is_empty() {
                        false
                    } else {
                        w.write_shape_and_record(&shapefile::Polyline::with_parts(parts), &record(f)).map_err(fmt)?;
                        true
                    }
                }
                (Some(Geometry::Polygon(polys)), GeometryType::Polygon) => {
                    let mut rings = Vec::new();
                    for poly in polys {
                        let mut it = poly.iter().map(|r| r.iter().filter(finite).map(pt).collect::<Vec<_>>()).filter(|r| r.len() >= 3);
                        // A polygon whose outer ring is gone loses its holes too.
                        let outer_kept = poly.first().is_some_and(|r| r.iter().filter(finite).count() >= 3);
                        if let Some(outer) = it.next().filter(|_| outer_kept) {
                            rings.push(shapefile::PolygonRing::Outer(outer));
                            rings.extend(it.map(shapefile::PolygonRing::Inner));
                        }
                    }
                    if rings.is_empty() {
                        false
                    } else {
                        w.write_shape_and_record(&shapefile::Polygon::with_rings(rings), &record(f)).map_err(fmt)?;
                        true
                    }
                }
                _ => false,
            };
            if ok {
                written += 1;
            } else {
                left_out += 1;
            }
        }
        if written == 0 {
            let what = if ds.geometry_type == GeometryType::None { "a table without geometry" } else { "a layer with no features to draw" };
            return Err(DataError::Unsupported(format!("{what} as a Shapefile")));
        }
    }
    Ok((
        vec![
            ("shp", shp.into_inner()),
            ("shx", shx.into_inner()),
            ("dbf", dbf.into_inner()),
            ("prj", WGS84_PRJ.as_bytes().to_vec()),
            ("cpg", b"UTF-8".to_vec()),
        ],
        left_out,
    ))
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
    fn read_bytes_dispatches_by_extension() {
        let (ds, _) = read_bytes("pts.CSV", b"lon,lat\n1,2\n").unwrap();
        assert_eq!(ds.geometry_type, GeometryType::Point);
        assert!(read_bytes("x.shp", b"").is_err());
        assert!(read_bytes("x.geojson", &[0xff, 0xfe]).is_err());
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
    fn csv_leading_zeros_and_guard_roundtrip() {
        let (ds, _) = read_csv("zip,n,note\n02134,0,-abc\n10001,0.5,=x\n", "t").unwrap();
        assert_eq!(ds.fields[0].ty, FieldType::Text);
        assert_eq!(ds.value(0, 0), &Value::Text("02134".into()));
        assert_eq!(ds.fields[1].ty, FieldType::Double);
        let once = to_csv(&ds).unwrap();
        let (back, _) = read_csv(&once, "t").unwrap();
        assert_eq!(back.value(0, 2), &Value::Text("-abc".into()));
        assert_eq!(back.value(1, 2), &Value::Text("=x".into()));
        assert_eq!(to_csv(&back).unwrap(), once, "export/import is stable");
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

    /// A projected Shapefile is converted to longitude/latitude by its `.prj` (issue #84).
    #[test]
    fn shapefile_prj_reprojects() {
        use shapefile::dbase::{FieldValue, Record, TableWriterBuilder};
        let dir = std::env::temp_dir().join(format!("rustgis-shp-prj-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let shp = dir.join("paris.shp");
        {
            let table = TableWriterBuilder::new().add_character_field("name".try_into().unwrap(), 10);
            let mut w = shapefile::Writer::from_path(&shp, table).unwrap();
            let mut r = Record::default();
            r.insert("name".into(), FieldValue::Character(Some("paris".into())));
            // Paris in UTM zone 31N.
            w.write_shape_and_record(&shapefile::Point::new(451_219.33, 5_411_550.20), &r).unwrap();
        }
        let at = |ds: &Dataset| ds.features[0].geometry.as_ref().and_then(Geometry::label_point).unwrap();

        // As ArcGIS and GDAL write it: Esri WKT with no EPSG authority.
        let prj = r#"PROJCS["WGS_1984_UTM_Zone_31N",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",3.0],PARAMETER["Scale_Factor",0.9996],PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#;
        std::fs::write(dir.join("paris.prj"), prj).unwrap();
        let (ds, rep) = read_path(&shp).unwrap();
        let c = at(&ds);
        assert!((c.x - 2.335).abs() < 1e-6 && (c.y - 48.855).abs() < 1e-6, "{c:?}");
        assert!(rep.messages.is_empty(), "{:?}", rep.messages);
        assert_eq!(ds.value(0, 0), &Value::Text("paris".into()));

        // A .prj that isn't understood is said, not passed over.
        std::fs::write(dir.join("paris.prj"), r#"PROJCS["x",PROJECTION["Hotine_Oblique_Mercator"]]"#).unwrap();
        let (ds, rep) = read_path(&shp).unwrap();
        assert_eq!(at(&ds).x, 451_219.33);
        assert!(rep.messages.iter().any(|m| m.contains(".prj") && m.contains("Hotine")), "{:?}", rep.messages);

        // No .prj: read as longitude/latitude, with the out-of-range warning.
        std::fs::remove_file(dir.join("paris.prj")).unwrap();
        let (_, rep) = read_path(&shp).unwrap();
        assert!(rep.messages.iter().any(|m| m.contains("not longitude/latitude")), "{:?}", rep.messages);
        std::fs::remove_dir_all(&dir).ok();
    }
    /// Zipped Shapefiles are read from memory (the web app's way to open them), with their `.prj`,
    /// from folders inside the archive, beside other vector files; truncations are errors.
    #[test]
    fn zipped_shapefiles() {
        use shapefile::dbase::{FieldValue, Record, TableWriterBuilder};
        use std::io::Cursor;
        let (mut shp, mut shx, mut dbf) = (Cursor::new(Vec::new()), Cursor::new(Vec::new()), Cursor::new(Vec::new()));
        {
            let table = TableWriterBuilder::new().add_character_field("name".try_into().unwrap(), 10);
            let mut w = shapefile::Writer::new(shapefile::ShapeWriter::with_shx(&mut shp, &mut shx), table.build_with_dest(&mut dbf));
            let mut r = Record::default();
            r.insert("name".into(), FieldValue::Character(Some("paris".into())));
            w.write_shape_and_record(&shapefile::Point::new(451_219.33, 5_411_550.20), &r).unwrap();
        }
        let prj = r#"PROJCS["WGS_1984_UTM_Zone_31N",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",3.0],PARAMETER["Scale_Factor",0.9996],PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#;
        let geojson = r#"{"type":"FeatureCollection","features":[{"type":"Feature","properties":{"n":1},"geometry":{"type":"Point","coordinates":[1,2]}}]}"#;
        let entries = vec![
            ("data/Paris.SHP".to_string(), shp.into_inner()),
            ("data/Paris.shx".to_string(), shx.into_inner()),
            ("data/Paris.dbf".to_string(), dbf.into_inner()),
            ("data/Paris.prj".to_string(), prj.as_bytes().to_vec()),
            ("__MACOSX/data/._Paris.shp".to_string(), vec![0; 10]),
            ("extra/points.geojson".to_string(), geojson.as_bytes().to_vec()),
            ("ne_VERSION.txt".to_string(), b"5.1.1".to_vec()),
        ];
        let zip = crate::zip::write(&entries).unwrap();
        let layers = read_zip("upload.zip", &zip).unwrap();
        assert_eq!(layers.len(), 2);
        let (ds, rep) = &layers[0];
        assert_eq!(ds.name, "Paris");
        let c = ds.features[0].geometry.as_ref().and_then(Geometry::label_point).unwrap();
        assert!((c.x - 2.335).abs() < 1e-6 && (c.y - 48.855).abs() < 1e-6, "{c:?}");
        assert!(rep.messages.is_empty(), "{:?}", rep.messages);
        assert_eq!(ds.value(0, 0), &Value::Text("paris".into()));
        assert_eq!(layers[1].0.name, "points");
        // read_bytes takes the first layer.
        assert_eq!(read_bytes("upload.zip", &zip).unwrap().0.name, "Paris");
        // Without its .dbf the Shapefile is skipped (and said); the rest of the archive is read.
        let no_dbf: Vec<_> = entries.iter().filter(|(n, _)| !n.ends_with(".dbf")).cloned().collect();
        let layers = read_zip("x.zip", &crate::zip::write(&no_dbf).unwrap()).unwrap();
        assert_eq!(layers.len(), 1);
        assert!(layers[0].1.messages.iter().any(|m| m.contains("no .dbf")), "{:?}", layers[0].1.messages);
        // A metadata file that isn't GeoJSON doesn't keep the Shapefile out.
        let mut with_meta = entries.clone();
        with_meta.push(("data/metadata.json".to_string(), br#"{"title": "not GeoJSON"}"#.to_vec()));
        let layers = read_zip("x.zip", &crate::zip::write(&with_meta).unwrap()).unwrap();
        assert_eq!(layers.len(), 2);
        assert!(layers[0].1.messages.iter().any(|m| m.contains("metadata.json")), "{:?}", layers[0].1.messages);
        // Nothing readable at all is an error.
        let only_shp: Vec<_> = entries.iter().filter(|(n, _)| n.ends_with(".SHP")).cloned().collect();
        assert!(read_zip("x.zip", &crate::zip::write(&only_shp).unwrap()).is_err());
        // Nothing to read.
        assert!(read_zip("x.zip", &crate::zip::write(&[("a.txt.bak".to_string(), vec![1])]).unwrap()).is_err());
        for n in (0..zip.len()).step_by(37) {
            let _ = read_zip("x.zip", &zip[..n]);
        }
        // A .shp alone is refused with a hint to zip it.
        assert!(read_bytes("x.shp", &entries[0].1).is_err());
    }
}
