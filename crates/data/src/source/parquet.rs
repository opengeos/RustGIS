//! GeoParquet: columnar files with WKB geometry, read with the pure-Rust `parquet` crate's column
//! readers (no Arrow, which would make the browser build far larger).
//!
//! Extent queries prune whole row groups by the bounding-box statistics of the bbox covering
//! column (GeoParquet 1.1, `covering.bbox`), then filter rows by those bounding boxes before any
//! geometry is decoded. Only the columns needed are read. Files without a covering still work: their
//! geometry is decoded and tested row by row.

use super::{Batch, FeatureSource, Reproject, Request, Sampler, SourceInfo, clamp_limit, in_request};
use crate::{DataError, Feature, Field, FieldType, GeometryType, Result, Value, wkb};
use bytes::Bytes;
use parquet::basic::{ConvertedType, LogicalType, Type as Physical};
use parquet::column::reader::{ColumnReader, ColumnReaderImpl};
use parquet::data_type::DataType as PqType;
use parquet::file::metadata::ParquetMetaData;
use parquet::file::reader::{FileReader, RowGroupReader, SerializedFileReader};
use parquet::file::statistics::Statistics;
use parquet::schema::types::ColumnDescPtr;
use rustgis_geom::Extent;
use std::path::Path;
use std::sync::Arc;

/// Values read per call.
const BATCH: usize = 8192;

/// A GeoParquet file.
pub struct ParquetSource {
    info: SourceInfo,
    reader: Arc<dyn FileReader>,
    /// Leaf column index of the geometry.
    geom_col: usize,
    /// Leaf column index of each field, in `info.fields` order.
    field_cols: Vec<usize>,
    /// Leaf column indices of the bbox covering's xmin, ymin, xmax, ymax.
    covering: Option<[usize; 4]>,
    reproject: Reproject,
}

impl ParquetSource {
    pub fn open_path(path: &Path, name: &str) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        Self::new(Arc::new(SerializedFileReader::new(file).map_err(err)?), name)
    }

    pub fn open_bytes(bytes: Vec<u8>, name: &str) -> Result<Self> {
        Self::new(Arc::new(SerializedFileReader::new(Bytes::from(bytes)).map_err(err)?), name)
    }

    fn new(reader: Arc<dyn FileReader>, name: &str) -> Result<Self> {
        let md = reader.metadata();
        let schema = md.file_metadata().schema_descr();
        let kv = md.file_metadata().key_value_metadata();
        let geo: Option<serde_json::Value> = kv
            .and_then(|kv| kv.iter().find(|k| k.key == "geo"))
            .and_then(|k| k.value.as_deref())
            .and_then(|v| serde_json::from_str(v).ok());
        // Top-level leaf columns by name (nested ones are left out of the fields).
        let top = |i: usize| -> Option<String> {
            let c = schema.column(i);
            (c.path().parts().len() == 1 && c.max_rep_level() == 0).then(|| c.name().to_string())
        };
        let leaf_named = |n: &str| (0..schema.num_columns()).find(|&i| top(i).as_deref() == Some(n));
        // The geometry column: the metadata's primary column, else a binary column with a usual name.
        let primary = geo.as_ref().and_then(|g| g.get("primary_column")).and_then(|v| v.as_str()).map(str::to_string).or_else(|| {
            ["geometry", "geom", "wkb_geometry", "the_geom", "wkb"]
                .into_iter()
                .find(|n| leaf_named(n).is_some_and(|i| schema.column(i).physical_type() == Physical::BYTE_ARRAY))
                .map(str::to_string)
        });
        let Some(primary) = primary else {
            return Err(DataError::Format("not a GeoParquet file: no geometry column".into()));
        };
        let col_meta = geo.as_ref().and_then(|g| g.get("columns")).and_then(|c| c.get(&primary));
        let encoding = col_meta.and_then(|c| c.get("encoding")).and_then(|v| v.as_str()).unwrap_or("WKB");
        if !encoding.eq_ignore_ascii_case("wkb") {
            return Err(DataError::Unsupported(format!("GeoParquet geometry encoding '{encoding}' (only WKB is read)")));
        }
        let geom_col = leaf_named(&primary).ok_or_else(|| DataError::Format(format!("no column '{primary}'")))?;
        if schema.column(geom_col).physical_type() != Physical::BYTE_ARRAY {
            return Err(DataError::Format(format!("geometry column '{primary}' is not binary")));
        }
        let mut notes = Vec::new();
        let reproject = Reproject::from_epsg(crs_code(col_meta.and_then(|c| c.get("crs"))), &mut notes);
        let covering = find_covering(md, col_meta, &primary);

        let mut fields = Vec::new();
        let mut field_cols = Vec::new();
        for i in 0..schema.num_columns() {
            if i == geom_col || covering.is_some_and(|c| c.contains(&i)) {
                continue;
            }
            let Some(name) = top(i) else { continue };
            if let Some(ty) = field_type(&schema.column(i)) {
                fields.push(Field::new(name, ty));
                field_cols.push(i);
            }
        }
        let geometry_type = col_meta
            .and_then(|c| c.get("geometry_types"))
            .and_then(|t| t.as_array())
            .and_then(|t| t.iter().filter_map(|v| v.as_str()).find_map(geometry_type_name))
            .unwrap_or(GeometryType::None);
        let bbox = col_meta.and_then(|c| c.get("bbox")).and_then(|b| b.as_array()).and_then(|b| {
            let v: Vec<f64> = b.iter().filter_map(|x| x.as_f64()).collect();
            match v.as_slice() {
                [a, b, c, d] => Some(Extent::new(*a, *b, *c, *d)),
                [a, b, _, c, d, _] => Some(Extent::new(*a, *b, *c, *d)),
                _ => None,
            }
        });
        let extent = match bbox {
            Some(e) => reproject.extent_to_lonlat(&e),
            None => covering.map(|c| reproject.extent_to_lonlat(&groups_extent(md, c))).unwrap_or(Extent::EMPTY),
        };
        let count = u64::try_from(md.file_metadata().num_rows()).ok();
        let info = SourceInfo {
            name: name.to_string(),
            format: "GeoParquet",
            geometry_type,
            fields,
            extent,
            count,
            crs: reproject.label(),
            indexed: covering.is_some(),
        };
        let mut src = ParquetSource { info, reader, geom_col, field_cols, covering, reproject };
        if src.info.geometry_type == GeometryType::None {
            src.info.geometry_type = super::peek_kind(&src);
        }
        Ok(src)
    }
}

impl FeatureSource for ParquetSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn read(&self, req: &Request) -> Result<Batch> {
        let limit = clamp_limit(req.limit);
        let md = self.reader.metadata();
        let native = req.extent.map(|e| self.reproject.extent_from_lonlat(&e));
        let rows_in = |g: usize| usize::try_from(md.row_group(g).num_rows()).unwrap_or(0);
        // Row groups whose bounding box meets the extent.
        let groups: Vec<usize> = (0..md.num_row_groups())
            .filter(|&g| match (native, self.covering) {
                (Some(e), Some(c)) => group_extent(md, g, c).is_none_or(|ge| ge.intersects(&e)),
                _ => true,
            })
            .collect();

        // Which rows of those groups match: with a covering and an extent, exactly the rows whose
        // bounding box meets it (read from the covering alone); otherwise all of them.
        let mut keep: Vec<Option<Vec<bool>>> = Vec::with_capacity(groups.len());
        for &g in &groups {
            keep.push(match (native, self.covering) {
                (Some(e), Some(c)) => {
                    let rg = self.reader.get_row_group(g).map_err(err)?;
                    let n = rows_in(g);
                    let cols: Vec<Vec<Option<f64>>> = c.iter().map(|&i| read_f64(rg.as_ref(), i, n)).collect::<Result<_>>()?;
                    let v = |k: usize, r: usize| cols.get(k).and_then(|c| c.get(r).copied().flatten()).unwrap_or(f64::NAN);
                    Some(
                        (0..n)
                            .map(|r| {
                                let be = Extent::new(v(0, r), v(1, r), v(2, r), v(3, r));
                                be.is_empty() || be.intersects(&e)
                            })
                            .collect(),
                    )
                }
                _ => None,
            });
        }
        let matched: u64 =
            groups.iter().zip(&keep).map(|(&g, k)| k.as_ref().map(|k| k.iter().filter(|&&b| b).count()).unwrap_or(rows_in(g)) as u64).sum();
        // An extent without a covering is tested on the decoded geometry: the matches are sampled
        // as they come. Otherwise the match count is known, and every `step`-th is read.
        let late = native.is_some() && self.covering.is_none();
        let mut sample = if late { Sampler::new(limit) } else { Sampler::with_total(limit, matched) };
        let mut skipped = 0;
        let mut starts = Vec::with_capacity(md.num_row_groups());
        let mut first_row = 0u64;
        for g in 0..md.num_row_groups() {
            starts.push(first_row);
            first_row += rows_in(g) as u64;
        }
        for (&g, k) in groups.iter().zip(&keep) {
            let n = rows_in(g);
            // The rows of this group the sample takes, decided before anything else is read.
            let take: Vec<bool> =
                (0..n).map(|r| k.as_ref().is_none_or(|k| k.get(r).copied().unwrap_or(false)) && (late || sample.wants())).collect();
            if !take.iter().any(|&t| t) {
                continue;
            }
            let rg = self.reader.get_row_group(g).map_err(err)?;
            let geoms = read_bytes(rg.as_ref(), self.geom_col, &take)?;
            let attrs: Vec<Vec<Value>> = self.field_cols.iter().map(|&c| read_values(rg.as_ref(), c, &take)).collect::<Result<_>>()?;
            let base = starts.get(g).copied().unwrap_or(0);
            for (r, _) in take.iter().enumerate().filter(|(_, t)| **t) {
                let geometry = geoms.get(r).and_then(|b| b.as_deref()).and_then(wkb::decode).and_then(|g| self.reproject.geometry(g));
                let Some(geometry) = geometry else {
                    skipped += 1;
                    continue;
                };
                if !in_request(&geometry, req) || (late && !sample.wants()) {
                    continue;
                }
                let values = attrs.iter().map(|col| col.get(r).cloned().unwrap_or(Value::Null)).collect();
                sample.keep(Feature { id: base + r as u64 + 1, geometry: Some(geometry), attrs: values });
            }
        }
        Ok(Batch { matched: if late { sample.seen() } else { matched }, complete: sample.complete(), skipped, features: sample.items })
    }
}

fn err(e: parquet::errors::ParquetError) -> DataError {
    DataError::Format(format!("GeoParquet: {e}"))
}

fn geometry_type_name(t: &str) -> Option<GeometryType> {
    let t = t.trim_end_matches(" Z").trim_end_matches(" M").trim_end_matches(" ZM");
    match t {
        "Point" | "MultiPoint" => Some(GeometryType::Point),
        "LineString" | "MultiLineString" => Some(GeometryType::Polyline),
        "Polygon" | "MultiPolygon" => Some(GeometryType::Polygon),
        _ => None,
    }
}

/// The EPSG code of a GeoParquet `crs` (PROJJSON); absent means OGC:CRS84 (longitude/latitude).
fn crs_code(crs: Option<&serde_json::Value>) -> Option<u32> {
    let crs = crs?;
    if crs.is_null() {
        return None;
    }
    let id = crs.get("id")?;
    let authority = id.get("authority").and_then(|a| a.as_str()).unwrap_or("");
    let code = id.get("code").and_then(|c| c.as_u64().or_else(|| c.as_str().and_then(|s| s.parse().ok())))?;
    match authority.to_ascii_uppercase().as_str() {
        "EPSG" => u32::try_from(code).ok(),
        // OGC:CRS84 is longitude/latitude.
        _ => None,
    }
}

/// The bbox covering's leaf columns: the declared `covering.bbox`, else a `<geometry>_bbox` or
/// `bbox` struct.
fn find_covering(md: &ParquetMetaData, col_meta: Option<&serde_json::Value>, primary: &str) -> Option<[usize; 4]> {
    let schema = md.file_metadata().schema_descr();
    let leaf = |path: &[&str]| -> Option<usize> {
        let want = path.join(".");
        (0..schema.num_columns()).find(|&i| schema.column(i).path().string() == want)
    };
    let declared = col_meta.and_then(|c| c.get("covering")).and_then(|c| c.get("bbox"));
    let paths: Vec<Vec<String>> = match declared {
        Some(b) => ["xmin", "ymin", "xmax", "ymax"]
            .iter()
            .map(|k| b.get(*k).and_then(|p| p.as_array()).map(|p| p.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()))
            .collect::<Option<Vec<Vec<String>>>>()?,
        None => {
            let base = [format!("{primary}_bbox"), "bbox".to_string()].into_iter().find(|b| leaf(&[b.as_str(), "xmin"]).is_some())?;
            ["xmin", "ymin", "xmax", "ymax"].iter().map(|k| vec![base.clone(), (*k).to_string()]).collect()
        }
    };
    let mut leaves = [0usize; 4];
    for (i, p) in paths.iter().enumerate() {
        let refs: Vec<&str> = p.iter().map(String::as_str).collect();
        let l = leaf(&refs)?;
        if !matches!(schema.column(l).physical_type(), Physical::FLOAT | Physical::DOUBLE) || schema.column(l).max_rep_level() > 0 {
            return None;
        }
        *leaves.get_mut(i)? = l;
    }
    Some(leaves)
}

fn stat(s: Option<&Statistics>, max: bool) -> Option<f64> {
    match s? {
        Statistics::Float(v) => (if max { v.max_opt() } else { v.min_opt() }).map(|x| f64::from(*x)),
        Statistics::Double(v) => (if max { v.max_opt() } else { v.min_opt() }).copied(),
        _ => None,
    }
}

/// A row group's bounding box from its covering statistics.
fn group_extent(md: &ParquetMetaData, g: usize, c: [usize; 4]) -> Option<Extent> {
    let rg = md.row_groups().get(g)?;
    let s = |i: usize, max: bool| stat(rg.columns().get(*c.get(i)?)?.statistics(), max);
    let e = Extent::new(s(0, false)?, s(1, false)?, s(2, true)?, s(3, true)?);
    (!e.is_empty()).then_some(e)
}

fn groups_extent(md: &ParquetMetaData, c: [usize; 4]) -> Extent {
    (0..md.num_row_groups()).filter_map(|g| group_extent(md, g, c)).fold(Extent::EMPTY, |a, e| a.union(&e))
}

fn is_text(c: &ColumnDescPtr) -> bool {
    matches!(c.logical_type_ref(), Some(LogicalType::String | LogicalType::Enum | LogicalType::Json))
        || matches!(c.converted_type(), ConvertedType::UTF8 | ConvertedType::ENUM | ConvertedType::JSON)
}

/// The scale of a decimal column (its values are integers × 10^-scale).
fn decimal_scale(c: &ColumnDescPtr) -> Option<i32> {
    match c.logical_type_ref() {
        Some(LogicalType::Decimal(d)) => Some(d.scale),
        _ if c.converted_type() == ConvertedType::DECIMAL => Some(c.type_scale()),
        _ => None,
    }
}

/// A field type for a top-level column; `None` for types not read (binary, INT96, fixed-length).
fn field_type(c: &ColumnDescPtr) -> Option<FieldType> {
    Some(match c.physical_type() {
        Physical::BOOLEAN => FieldType::Boolean,
        Physical::INT32 | Physical::INT64 if decimal_scale(c).is_some() => FieldType::Double,
        Physical::INT32 | Physical::INT64 => FieldType::Integer,
        Physical::FLOAT | Physical::DOUBLE => FieldType::Double,
        Physical::BYTE_ARRAY if is_text(c) => FieldType::Text,
        _ => return None,
    })
}

/// Reads `n` rows of a flat column, `None` where a row is null.
fn read_column<T: PqType>(r: &mut ColumnReaderImpl<T>, n: usize, max_def: i16) -> Result<Vec<Option<T::T>>> {
    let mut out: Vec<Option<T::T>> = Vec::with_capacity(n.min(1 << 24));
    let mut defs = Vec::new();
    let mut values = Vec::new();
    while out.len() < n {
        defs.clear();
        values.clear();
        let want = (n - out.len()).min(BATCH);
        let (records, _, levels) = r.read_records(want, Some(&mut defs), None, &mut values).map_err(err)?;
        if records == 0 {
            break;
        }
        if max_def == 0 {
            out.extend(values.drain(..).map(Some));
        } else {
            let mut it = values.drain(..);
            for d in defs.iter().take(levels) {
                out.push(if *d == max_def { it.next() } else { None });
            }
        }
    }
    out.truncate(n);
    out.resize_with(n, || None);
    Ok(out)
}

/// Reads a field column as attribute values: those of the rows `take` marks (the rest are null,
/// so a sample doesn't copy every row).
fn read_values(rg: &dyn RowGroupReader, col: usize, take: &[bool]) -> Result<Vec<Value>> {
    let n = take.len();
    let on = |r: usize| take.get(r).copied().unwrap_or(false);
    let desc = rg.metadata().column(col).column_descr_ptr();
    let max_def = desc.max_def_level();
    let scale = decimal_scale(&desc).map(|s| 10f64.powi(-s));
    let int = |v: Option<i64>| match (v, scale) {
        (None, _) => Value::Null,
        (Some(i), Some(f)) => Value::Float(i as f64 * f),
        (Some(i), None) => Value::Int(i),
    };
    Ok(match rg.get_column_reader(col).map_err(err)? {
        ColumnReader::BoolColumnReader(mut r) => {
            read_column(&mut r, n, max_def)?.into_iter().map(|v| v.map(Value::Bool).unwrap_or_default()).collect()
        }
        ColumnReader::Int32ColumnReader(mut r) => read_column(&mut r, n, max_def)?.into_iter().map(|v| int(v.map(i64::from))).collect(),
        ColumnReader::Int64ColumnReader(mut r) => read_column(&mut r, n, max_def)?.into_iter().map(int).collect(),
        ColumnReader::FloatColumnReader(mut r) => {
            read_column(&mut r, n, max_def)?.into_iter().map(|v| v.map(|f| Value::Float(f64::from(f))).unwrap_or_default()).collect()
        }
        ColumnReader::DoubleColumnReader(mut r) => {
            read_column(&mut r, n, max_def)?.into_iter().map(|v| v.map(Value::Float).unwrap_or_default()).collect()
        }
        ColumnReader::ByteArrayColumnReader(mut r) => read_column(&mut r, n, max_def)?
            .into_iter()
            .enumerate()
            .map(|(i, v)| v.filter(|_| on(i)).map(|b| Value::Text(String::from_utf8_lossy(b.data()).into_owned())).unwrap_or_default())
            .collect(),
        _ => vec![Value::Null; n],
    })
}

/// Reads a binary column (the WKB geometry), copying only the rows `take` marks.
fn read_bytes(rg: &dyn RowGroupReader, col: usize, take: &[bool]) -> Result<Vec<Option<Vec<u8>>>> {
    let max_def = rg.metadata().column(col).column_descr().max_def_level();
    let on = |r: usize| take.get(r).copied().unwrap_or(false);
    match rg.get_column_reader(col).map_err(err)? {
        ColumnReader::ByteArrayColumnReader(mut r) => Ok(read_column(&mut r, take.len(), max_def)?
            .into_iter()
            .enumerate()
            .map(|(i, v)| v.filter(|_| on(i)).map(|b| b.data().to_vec()))
            .collect()),
        _ => Err(DataError::Format("GeoParquet: the geometry column is not binary".into())),
    }
}

/// Reads `n` rows of a float or double column as doubles.
fn read_f64(rg: &dyn RowGroupReader, col: usize, n: usize) -> Result<Vec<Option<f64>>> {
    let max_def = rg.metadata().column(col).column_descr().max_def_level();
    match rg.get_column_reader(col).map_err(err)? {
        ColumnReader::FloatColumnReader(mut r) => Ok(read_column(&mut r, n, max_def)?.into_iter().map(|v| v.map(f64::from)).collect()),
        ColumnReader::DoubleColumnReader(mut r) => read_column(&mut r, n, max_def),
        _ => Ok(vec![None; n]),
    }
}
