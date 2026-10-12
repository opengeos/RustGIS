//! GeoPackage: an SQLite database of feature tables, read with SQLite (desktop only).
//!
//! Extent queries use the table's R-tree spatial index (`rtree_<table>_<column>`) when it has one,
//! and otherwise scan, testing the envelope in each geometry's header before decoding it.

use super::{Batch, FeatureSource, Reproject, Request, Sampler, SourceInfo, clamp_limit, in_request, stride};
use crate::{DataError, Feature, Field, FieldType, GeometryType, Result, Value, wkb};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, params_from_iter};
use rustgis_geom::Extent;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Rows fetched by id per query.
const CHUNK: usize = 500;

/// A feature table in a GeoPackage.
pub struct GpkgSource {
    info: SourceInfo,
    /// `None` only while the source is dropped.
    conn: Mutex<Option<Connection>>,
    table: String,
    geom: String,
    pk: Option<String>,
    rtree: Option<String>,
    reproject: Reproject,
    /// A copy written for a file opened from memory, removed when the source is dropped.
    temp: Option<PathBuf>,
}

impl Drop for GpkgSource {
    fn drop(&mut self) {
        // Closed first: Windows can't remove a file SQLite still holds open.
        if let Ok(c) = self.conn.get_mut() {
            drop(c.take());
        }
        if let Some(p) = &self.temp {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// The feature tables of a GeoPackage.
pub fn layers(path: &Path) -> Result<Vec<String>> {
    let conn = open_db(path)?;
    feature_tables(&conn)
}

fn open_db(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(err)?;
    // A cap on SQLite's page cache, so a huge database can't take all the memory.
    let _ = conn.execute_batch("PRAGMA cache_size = -65536;");
    Ok(conn)
}

fn feature_tables(conn: &Connection) -> Result<Vec<String>> {
    let mut st = conn.prepare("SELECT table_name FROM gpkg_geometry_columns ORDER BY table_name").map_err(not_gpkg)?;
    let rows = st.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
    Ok(rows.filter_map(std::result::Result::ok).collect())
}

impl GpkgSource {
    pub fn open(path: &Path, layer: Option<&str>) -> Result<Self> {
        Self::open_conn(open_db(path)?, layer, None)
    }

    /// Opens a GeoPackage held in memory, through a temporary copy (SQLite reads files). The copy
    /// is a new file with a name no one can guess, so another user can't plant one in its place.
    pub fn open_bytes(bytes: &[u8], layer: Option<&str>) -> Result<Self> {
        use std::io::Write;
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let mut tries = 0;
        let (p, mut f) = loop {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
            let salt = std::collections::hash_map::RandomState::new();
            let tag = std::hash::BuildHasher::hash_one(&salt, (nanos, N.fetch_add(1, Ordering::Relaxed)));
            let p = std::env::temp_dir().join(format!("rustgis-{}-{tag:016x}.gpkg", std::process::id()));
            let mut o = std::fs::OpenOptions::new();
            o.write(true).create_new(true);
            // Readable by this user only: the copy holds their data, in a shared folder.
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
            match o.open(&p) {
                Ok(f) => break (p, f),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && tries < 8 => tries += 1,
                Err(e) => return Err(e.into()),
            }
        };
        if let Err(e) = f.write_all(bytes).and_then(|_| f.sync_all()) {
            let _ = std::fs::remove_file(&p);
            return Err(e.into());
        }
        drop(f);
        match open_db(&p).and_then(|c| Self::open_conn(c, layer, Some(p.clone()))) {
            Ok(s) => Ok(s),
            Err(e) => {
                let _ = std::fs::remove_file(&p);
                Err(e)
            }
        }
    }

    fn open_conn(conn: Connection, layer: Option<&str>, temp: Option<PathBuf>) -> Result<Self> {
        let tables = feature_tables(&conn)?;
        let table = match layer {
            Some(l) => tables
                .iter()
                .find(|t| t.eq_ignore_ascii_case(l))
                .cloned()
                .ok_or_else(|| DataError::Format(format!("no feature table '{l}' (tables: {})", tables.join(", "))))?,
            None => tables.first().cloned().ok_or_else(|| DataError::Format("the GeoPackage has no feature tables".into()))?,
        };
        let mut notes = Vec::new();
        if layer.is_none() && tables.len() > 1 {
            notes.push(format!("the first of {} feature tables ({})", tables.len(), tables.join(", ")));
        }
        let (geom, geom_type, srs_id): (String, String, i64) = conn
            .query_row("SELECT column_name, geometry_type_name, srs_id FROM gpkg_geometry_columns WHERE table_name = ?1", [&table], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .map_err(err)?;
        let epsg: Option<u32> = conn
            .query_row("SELECT organization, organization_coordsys_id FROM gpkg_spatial_ref_sys WHERE srs_id = ?1", [srs_id], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })
            .ok()
            .and_then(|(org, id)| if org.eq_ignore_ascii_case("epsg") { u32::try_from(id).ok() } else { None })
            .or_else(|| u32::try_from(srs_id).ok().filter(|c| *c > 0));
        let reproject = Reproject::from_epsg(epsg, &mut notes);

        // Columns: the primary key, the geometry and the attribute fields.
        let mut pk = None;
        let mut fields = Vec::new();
        {
            let mut st = conn.prepare(&format!("PRAGMA table_info({})", quote(&table))).map_err(err)?;
            let cols = st
                .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_default(), r.get::<_, i64>(5)?)))
                .map_err(err)?;
            for c in cols.filter_map(std::result::Result::ok) {
                let (name, decl, is_pk) = c;
                if name.eq_ignore_ascii_case(&geom) {
                    continue;
                }
                if is_pk == 1 && pk.is_none() && decl.to_ascii_uppercase().contains("INT") {
                    pk = Some(name);
                    continue;
                }
                if let Some(ty) = field_type(&decl) {
                    fields.push(Field::new(name, ty));
                }
            }
        }
        let rtree_name = format!("rtree_{table}_{geom}");
        let rtree: Option<String> = conn
            .query_row("SELECT name FROM sqlite_master WHERE name = ?1 AND type = 'table'", [&rtree_name], |r| r.get(0))
            .ok()
            .filter(|_| pk.is_some());
        let declared: Option<Extent> = conn
            .query_row("SELECT min_x, min_y, max_x, max_y FROM gpkg_contents WHERE table_name = ?1", [&table], |r| {
                Ok((r.get::<_, Option<f64>>(0)?, r.get::<_, Option<f64>>(1)?, r.get::<_, Option<f64>>(2)?, r.get::<_, Option<f64>>(3)?))
            })
            .ok()
            .and_then(|e| match e {
                (Some(a), Some(b), Some(c), Some(d)) => Some(Extent::new(a, b, c, d)),
                _ => None,
            });
        let native = declared.filter(|e| !e.is_empty()).or_else(|| {
            let rt = rtree.as_ref()?;
            conn.query_row(&format!("SELECT min(minx), min(miny), max(maxx), max(maxy) FROM {}", quote(rt)), [], |r| {
                Ok(Extent::new(r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .ok()
        });
        let extent = native.map(|e| reproject.extent_to_lonlat(&e)).unwrap_or(Extent::EMPTY);
        let count = conn
            .query_row(&format!("SELECT COUNT(*) FROM {}", quote(&table)), [], |r| r.get::<_, i64>(0))
            .ok()
            .and_then(|n| u64::try_from(n).ok());
        let geometry_type = match geom_type.to_ascii_uppercase().as_str() {
            "POINT" | "MULTIPOINT" => GeometryType::Point,
            "LINESTRING" | "MULTILINESTRING" | "CURVE" | "MULTICURVE" => GeometryType::Polyline,
            "POLYGON" | "MULTIPOLYGON" | "SURFACE" | "MULTISURFACE" => GeometryType::Polygon,
            _ => GeometryType::None,
        };
        let info = SourceInfo {
            name: table.clone(),
            format: "GeoPackage",
            geometry_type,
            fields,
            extent,
            count,
            crs: reproject.label(),
            indexed: rtree.is_some(),
            notes,
        };
        let mut src = GpkgSource { info, conn: Mutex::new(Some(conn)), table, geom, pk, rtree, reproject, temp };
        if src.info.geometry_type == GeometryType::None {
            src.info.geometry_type = src.peek_kind();
        }
        Ok(src)
    }

    /// The geometry type of a table declared as plain `GEOMETRY`, from its first few rows (a
    /// query that stops there, rather than a read of the table).
    fn peek_kind(&self) -> GeometryType {
        let Ok(guard) = self.conn.lock() else { return GeometryType::None };
        let Some(conn) = guard.as_ref() else { return GeometryType::None };
        let sql = format!("SELECT {g} FROM {t} WHERE {g} IS NOT NULL LIMIT 16", g = quote(&self.geom), t = quote(&self.table));
        let Ok(mut st) = conn.prepare(&sql) else { return GeometryType::None };
        let Ok(mut rows) = st.query([]) else { return GeometryType::None };
        while let Ok(Some(r)) = rows.next() {
            if let Ok(ValueRef::Blob(b)) = r.get_ref(0)
                && let Some((_, body)) = split_blob(b)
                && let Some(g) = wkb::decode(body)
            {
                return g.kind();
            }
        }
        GeometryType::None
    }

    fn select_list(&self) -> String {
        let mut cols = vec![self.pk.as_deref().map(quote).unwrap_or_else(|| "rowid".into()), quote(&self.geom)];
        cols.extend(self.info.fields.iter().map(|f| quote(&f.name)));
        cols.join(", ")
    }

    /// Turns a row (id, geometry, fields…) into a feature; `None` for an unreadable geometry.
    fn feature(&self, r: &rusqlite::Row<'_>, native: Option<&Extent>, req: &Request) -> rusqlite::Result<Row> {
        let id = r.get::<_, Option<i64>>(0)?.and_then(|i| u64::try_from(i).ok()).unwrap_or(0);
        let blob = match r.get_ref(1)? {
            ValueRef::Blob(b) => b,
            ValueRef::Null => return Ok(Row::Skip),
            _ => return Ok(Row::Bad),
        };
        if is_empty_blob(blob) {
            return Ok(Row::Skip);
        }
        let Some((envelope, body)) = split_blob(blob) else { return Ok(Row::Bad) };
        if let (Some(q), Some(e)) = (native, envelope)
            && !e.intersects(q)
        {
            return Ok(Row::Skip);
        }
        let Some(geometry) = wkb::decode(body).and_then(|g| self.reproject.geometry(g)) else { return Ok(Row::Bad) };
        if !in_request(&geometry, req) {
            return Ok(Row::Skip);
        }
        let attrs =
            self.info.fields.iter().enumerate().map(|(i, f)| r.get_ref(i + 2).map(|v| value(v, f.ty)).unwrap_or(Value::Null)).collect();
        Ok(Row::Feature(Feature { id, geometry: Some(geometry), attrs }))
    }
}

enum Row {
    Feature(Feature),
    /// Outside the request, or without geometry.
    Skip,
    /// Unreadable.
    Bad,
}

impl FeatureSource for GpkgSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn read(&self, req: &Request) -> Result<Batch> {
        let limit = clamp_limit(req.limit);
        let guard = self.conn.lock().map_err(|_| DataError::Format("GeoPackage connection poisoned".into()))?;
        let conn = guard.as_ref().ok_or_else(|| DataError::Format("GeoPackage connection closed".into()))?;
        let native = req.extent.map(|e| self.reproject.extent_from_lonlat(&e));
        let mut out = Batch::default();
        let push = |row: Row, out: &mut Batch| match row {
            Row::Feature(f) => out.features.push(f),
            Row::Skip => {}
            Row::Bad => out.skipped += 1,
        };
        match (native, &self.rtree, &self.pk) {
            (Some(e), Some(rt), Some(pk)) => {
                // The index gives the matching ids; an evenly spread sample of them is fetched.
                let mut st = conn
                    .prepare(&format!("SELECT id FROM {} WHERE maxx >= ?1 AND minx <= ?2 AND maxy >= ?3 AND miny <= ?4", quote(rt)))
                    .map_err(err)?;
                let ids: Vec<i64> = st
                    .query_map([e.min_x, e.max_x, e.min_y, e.max_y], |r| r.get(0))
                    .map_err(err)?
                    .filter_map(std::result::Result::ok)
                    .collect();
                out.matched = ids.len() as u64;
                let step = usize::try_from(stride(out.matched, limit)).unwrap_or(usize::MAX);
                out.complete = step == 1;
                let picked: Vec<i64> = ids.into_iter().step_by(step.max(1)).collect();
                for chunk in picked.chunks(CHUNK) {
                    let marks = vec!["?"; chunk.len()].join(",");
                    let sql = format!("SELECT {} FROM {} WHERE {} IN ({marks})", self.select_list(), quote(&self.table), quote(pk));
                    let mut st = conn.prepare(&sql).map_err(err)?;
                    let mut rows = st.query(params_from_iter(chunk.iter())).map_err(err)?;
                    while let Some(r) = rows.next().map_err(err)? {
                        push(self.feature(r, native.as_ref(), req).map_err(err)?, &mut out);
                    }
                    if out.features.len() >= limit {
                        out.features.truncate(limit);
                        out.complete = false;
                        break;
                    }
                }
            }
            _ => {
                // A scan: the envelope in each geometry's header tests the extent before decoding.
                let sql = format!("SELECT {} FROM {}", self.select_list(), quote(&self.table));
                let mut st = conn.prepare(&sql).map_err(err)?;
                let mut rows = st.query([]).map_err(err)?;
                let mut sample =
                    if native.is_none() { Sampler::with_total(limit, self.info.count.unwrap_or(0)) } else { Sampler::new(limit) };
                while let Some(r) = rows.next().map_err(err)? {
                    // With an extent, the envelope in the geometry's header says whether the row
                    // matches before anything is decoded; without an envelope, only the decoded
                    // geometry can, so the match is counted after it.
                    let envelope = match (&native, r.get_ref(1)) {
                        (Some(_), Ok(ValueRef::Blob(b))) => split_blob(b).and_then(|(e, _)| e),
                        _ => None,
                    };
                    if let (Some(q), Some(e)) = (&native, envelope)
                        && !e.intersects(q)
                    {
                        continue;
                    }
                    let known = native.is_none() || envelope.is_some();
                    if known && !sample.wants() {
                        continue;
                    }
                    match self.feature(r, native.as_ref(), req).map_err(err)? {
                        Row::Feature(f) if known || sample.wants() => sample.keep(f),
                        Row::Feature(_) | Row::Skip => {}
                        Row::Bad => out.skipped += 1,
                    }
                }
                out.matched = sample.seen();
                out.complete = sample.complete();
                out.features = sample.items;
            }
        }
        Ok(out)
    }
}

/// A GeoPackage geometry blob flagged empty: valid, with nothing to draw.
fn is_empty_blob(b: &[u8]) -> bool {
    b.starts_with(b"GP") && b.get(3).is_some_and(|f| f & 0b1_0000 != 0)
}

/// A GeoPackage geometry blob: the header's envelope (if any) and the WKB after it.
fn split_blob(b: &[u8]) -> Option<(Option<Extent>, &[u8])> {
    let (magic, rest) = b.split_at_checked(2)?;
    if magic != b"GP" {
        // Some writers store plain WKB.
        return Some((None, b));
    }
    let &flags = rest.get(1)?;
    let little = flags & 1 == 1;
    let env_len = match (flags >> 1) & 0b111 {
        0 => 0,
        1 => 32,
        2 | 3 => 48,
        4 => 64,
        _ => return None,
    };
    if flags & 0b1_0000 != 0 {
        // The empty flag.
        return None;
    }
    let env = b.get(8..8 + env_len)?;
    let f = |i: usize| -> Option<f64> {
        let a: [u8; 8] = env.get(i * 8..i * 8 + 8)?.try_into().ok()?;
        Some(if little { f64::from_le_bytes(a) } else { f64::from_be_bytes(a) })
    };
    // Envelope order: min x, max x, min y, max y.
    let envelope = if env_len >= 32 { Some(Extent::new(f(0)?, f(2)?, f(1)?, f(3)?)).filter(|e| !e.is_empty()) } else { None };
    Some((envelope, b.get(8 + env_len..)?))
}

fn field_type(decl: &str) -> Option<FieldType> {
    let d = decl.to_ascii_uppercase();
    Some(if d.contains("BOOL") {
        FieldType::Boolean
    } else if d.contains("INT") {
        FieldType::Integer
    } else if d.contains("REAL") || d.contains("FLOA") || d.contains("DOUB") || d.contains("NUMERIC") || d.contains("DECIMAL") {
        FieldType::Double
    } else if d.contains("BLOB") {
        return None;
    } else {
        FieldType::Text
    })
}

fn value(v: ValueRef<'_>, ty: FieldType) -> Value {
    match (v, ty) {
        (ValueRef::Null, _) => Value::Null,
        (ValueRef::Integer(i), FieldType::Boolean) => Value::Bool(i != 0),
        (ValueRef::Integer(i), FieldType::Double) => Value::Float(i as f64),
        (ValueRef::Integer(i), _) => Value::Int(i),
        (ValueRef::Real(f), FieldType::Integer) if f.fract() == 0.0 && f.abs() < 9e15 => Value::Int(f as i64),
        (ValueRef::Real(f), _) => Value::Float(f),
        (ValueRef::Text(t), FieldType::Text) => Value::Text(String::from_utf8_lossy(t).into_owned()),
        (ValueRef::Text(t), ty) => Value::parse(&String::from_utf8_lossy(t), ty),
        (ValueRef::Blob(_), _) => Value::Null,
    }
}

/// An SQL identifier, quoted.
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn err(e: rusqlite::Error) -> DataError {
    DataError::Format(format!("GeoPackage: {e}"))
}

fn not_gpkg(e: rusqlite::Error) -> DataError {
    DataError::Format(format!("not a GeoPackage: {e}"))
}
