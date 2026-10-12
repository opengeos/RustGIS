//! GeoParquet: columnar files with WKB geometry, read with the pure-Rust `parquet` crate.
//!
//! Extent queries prune whole row groups by the bounding-box statistics of the bbox covering
//! column (GeoParquet 1.1, `covering.bbox`), then filter rows by those bounding boxes before any
//! geometry is decoded. Only the columns needed are read. Files without a covering still work: their
//! geometry is decoded and tested row by row.

use super::{Batch, FeatureSource, Reproject, Request, Sampler, SourceInfo, clamp_limit, in_request, stride};
use crate::{DataError, Feature, Field, FieldType, GeometryType, Result, Value, wkb};
use arrow_array::{Array, ArrayRef, RecordBatch, cast::AsArray};
use arrow_schema::DataType;
use bytes::Bytes;
use parquet::arrow::ProjectionMask;
use parquet::arrow::arrow_reader::{ArrowReaderMetadata, ArrowReaderOptions, ParquetRecordBatchReaderBuilder, RowSelection, RowSelector};
use parquet::file::reader::ChunkReader;
use parquet::file::statistics::Statistics;
use rustgis_geom::Extent;
use std::path::{Path, PathBuf};

const BATCH: usize = 8192;

/// Where the bytes are: a file reopened for every read, or a browser upload held in memory.
#[derive(Clone, Debug)]
enum Input {
    Path(PathBuf),
    Bytes(Bytes),
}

/// A GeoParquet file.
pub struct ParquetSource {
    info: SourceInfo,
    input: Input,
    meta: ArrowReaderMetadata,
    /// Root column index of the geometry.
    geom_col: usize,
    /// Root column index of each field, in `info.fields` order.
    field_cols: Vec<usize>,
    /// Leaf column indices of the bbox covering's xmin, ymin, xmax, ymax.
    covering: Option<Covering>,
    reproject: Reproject,
}

#[derive(Clone, Copy, Debug)]
struct Covering {
    /// Root column index of the struct (or of the four columns, when they're top-level).
    leaves: [usize; 4],
    roots: [usize; 4],
}

impl ParquetSource {
    pub fn open_path(path: &Path, name: &str) -> Result<Self> {
        let file = std::fs::File::open(path)?;
        let meta = ArrowReaderMetadata::load(&file, ArrowReaderOptions::new()).map_err(err)?;
        Self::new(Input::Path(path.to_path_buf()), meta, name)
    }

    pub fn open_bytes(bytes: Vec<u8>, name: &str) -> Result<Self> {
        let bytes = Bytes::from(bytes);
        let meta = ArrowReaderMetadata::load(&bytes, ArrowReaderOptions::new()).map_err(err)?;
        Self::new(Input::Bytes(bytes), meta, name)
    }

    fn new(input: Input, meta: ArrowReaderMetadata, name: &str) -> Result<Self> {
        let schema = meta.schema().clone();
        let kv = meta.metadata().file_metadata().key_value_metadata();
        // The file's `geo` key, or the copy in the embedded Arrow schema (some writers keep only that).
        let geo: Option<serde_json::Value> = kv
            .and_then(|kv| kv.iter().find(|k| k.key == "geo"))
            .and_then(|k| k.value.as_deref())
            .or_else(|| schema.metadata().get("geo").map(String::as_str))
            .and_then(|v| serde_json::from_str(v).ok());
        // The geometry column: the metadata's primary column, else a binary column with a usual name.
        let primary = geo.as_ref().and_then(|g| g.get("primary_column")).and_then(|v| v.as_str()).map(str::to_string).or_else(|| {
            ["geometry", "geom", "wkb_geometry", "the_geom", "wkb"]
                .into_iter()
                .find(|n| schema.fields().iter().any(|f| f.name() == n && is_binary(f.data_type())))
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
        let geom_col = schema.index_of(&primary).map_err(|_| DataError::Format(format!("no column '{primary}'")))?;
        if !is_binary(schema.field(geom_col).data_type()) {
            return Err(DataError::Format(format!("geometry column '{primary}' is not binary")));
        }
        let mut notes = Vec::new();
        let reproject = Reproject::from_epsg(crs_code(col_meta.and_then(|c| c.get("crs"))), &mut notes);
        let covering = find_covering(&meta, col_meta, &primary);
        let covering_roots: Vec<usize> = covering.map(|c| c.roots.to_vec()).unwrap_or_default();

        let mut fields = Vec::new();
        let mut field_cols = Vec::new();
        for (i, f) in schema.fields().iter().enumerate() {
            if i == geom_col || covering_roots.contains(&i) {
                continue;
            }
            if let Some(ty) = field_type(f.data_type()) {
                fields.push(Field::new(f.name().clone(), ty));
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
            None => covering.map(|c| reproject.extent_to_lonlat(&groups_extent(&meta, c))).unwrap_or(Extent::EMPTY),
        };
        let count = u64::try_from(meta.metadata().file_metadata().num_rows()).ok();
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
        let mut src = ParquetSource { info, input, meta, geom_col, field_cols, covering, reproject };
        if src.info.geometry_type == GeometryType::None {
            src.info.geometry_type = super::peek_kind(&src);
        }
        Ok(src)
    }

    fn builder<T: ChunkReader + 'static>(&self, input: T) -> ParquetRecordBatchReaderBuilder<T> {
        ParquetRecordBatchReaderBuilder::new_with_metadata(input, self.meta.clone()).with_batch_size(BATCH)
    }

    fn read_with<T: ChunkReader + 'static>(&self, input: impl Fn() -> Result<T>, req: &Request) -> Result<Batch> {
        let limit = clamp_limit(req.limit);
        let md = self.meta.metadata();
        let native = req.extent.map(|e| self.reproject.extent_from_lonlat(&e));
        // Row groups whose bounding box meets the extent.
        let groups: Vec<usize> = (0..md.num_row_groups())
            .filter(|&g| match (native, self.covering) {
                (Some(e), Some(c)) => group_extent(md, g, c).is_none_or(|ge| ge.intersects(&e)),
                _ => true,
            })
            .collect();
        let rows_in = |g: &usize| md.row_group(*g).num_rows().max(0) as u64;
        let first_row: Vec<u64> = {
            let mut acc = 0u64;
            (0..md.num_row_groups())
                .map(|g| {
                    let r = acc;
                    acc += rows_in(&g);
                    r
                })
                .collect()
        };
        if groups.is_empty() {
            return Ok(Batch { complete: true, ..Batch::default() });
        }

        // Which rows of those groups to read: with a covering and an extent, exactly the rows whose
        // bounding box meets it; otherwise all of them.
        let mut keep: Option<Vec<bool>> = None;
        if let (Some(e), Some(c)) = (native, self.covering) {
            let mask = ProjectionMask::roots(self.meta.parquet_schema(), dedup(c.roots));
            let reader = self.builder(input()?).with_row_groups(groups.clone()).with_projection(mask).build().map_err(err)?;
            let mut k = Vec::new();
            for b in reader {
                let b = b.map_err(arrow_err)?;
                let cols = bbox_columns(&b);
                for r in 0..b.num_rows() {
                    let hit = match &cols {
                        Some([x0, y0, x1, y1]) => {
                            let v = |a: &arrow_array::Float64Array| if a.is_null(r) { f64::NAN } else { a.value(r) };
                            let be = Extent::new(v(x0), v(y0), v(x1), v(y1));
                            be.is_empty() || be.intersects(&e)
                        }
                        None => true,
                    };
                    k.push(hit);
                }
            }
            keep = Some(k);
        }
        let total: u64 = groups.iter().map(rows_in).sum();
        let matched = keep.as_ref().map(|k| k.iter().filter(|&&b| b).count() as u64).unwrap_or(total);
        // An extent without a covering is tested on the decoded geometry, so every row is read and
        // the matches are sampled as they come.
        let late = native.is_some() && keep.is_none();
        let step = if late { 1 } else { stride(matched, limit) };
        let mut sample = Sampler::new(limit);
        // Every `step`-th kept row.
        let mut take = Vec::with_capacity(usize::try_from(total).unwrap_or(0).min(1 << 26));
        let mut seen = 0u64;
        for i in 0..usize::try_from(total).unwrap_or(usize::MAX) {
            let kept = keep.as_ref().is_none_or(|k| k.get(i).copied().unwrap_or(false));
            let on = kept && seen.is_multiple_of(step);
            if kept {
                seen += 1;
            }
            take.push(on);
        }
        let selection = to_selection(&take);

        let mut roots: Vec<usize> = vec![self.geom_col];
        roots.extend(&self.field_cols);
        let mask = ProjectionMask::roots(self.meta.parquet_schema(), roots);
        let reader = self
            .builder(input()?)
            .with_row_groups(groups.clone())
            .with_projection(mask)
            .with_row_selection(selection)
            .build()
            .map_err(err)?;
        // Row ids: the row's position in the file, so they stay put between reads.
        let mut ids = Vec::new();
        {
            let mut i = 0usize;
            for &g in &groups {
                let base = first_row.get(g).copied().unwrap_or(0);
                for r in 0..rows_in(&g) {
                    if take.get(i).copied().unwrap_or(false) {
                        ids.push(base + r + 1);
                    }
                    i += 1;
                }
            }
        }
        let mut out = Batch { matched, complete: step == 1, ..Batch::default() };
        let mut row = 0usize;
        for b in reader {
            let b = b.map_err(arrow_err)?;
            // A projection keeps the file's column order: find the columns by name.
            let schema = self.meta.schema();
            let col = |root: usize| b.column_by_name(schema.field(root).name());
            let Some(geoms) = col(self.geom_col) else { return Err(DataError::Format("GeoParquet: geometry column missing".into())) };
            let attrs: Vec<(FieldType, Option<ArrayRef>)> =
                self.info.fields.iter().zip(&self.field_cols).map(|(f, &root)| (f.ty, col(root).and_then(|c| convert(c, f.ty)))).collect();
            for r in 0..b.num_rows() {
                let id = ids.get(row).copied().unwrap_or(row as u64 + 1);
                row += 1;
                let geometry = wkb_at(geoms, r).and_then(wkb::decode).and_then(|g| self.reproject.geometry(g));
                let Some(geometry) = geometry else {
                    out.skipped += 1;
                    continue;
                };
                if !in_request(&geometry, req) || (late && !sample.wants()) {
                    continue;
                }
                let values = attrs.iter().map(|(ty, a)| a.as_ref().map(|a| value_at(a, *ty, r)).unwrap_or(Value::Null)).collect();
                let f = Feature { id, geometry: Some(geometry), attrs: values };
                if late {
                    sample.keep(f);
                    continue;
                }
                out.features.push(f);
                if out.features.len() >= limit {
                    out.complete = false;
                    return Ok(out);
                }
            }
        }
        if late {
            out.matched = sample.seen();
            out.complete = sample.complete();
            out.features = sample.items;
        }
        Ok(out)
    }
}

impl FeatureSource for ParquetSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn read(&self, req: &Request) -> Result<Batch> {
        match &self.input {
            Input::Path(p) => self.read_with(|| Ok(std::fs::File::open(p)?), req),
            Input::Bytes(b) => self.read_with(|| Ok(b.clone()), req),
        }
    }
}

fn err(e: parquet::errors::ParquetError) -> DataError {
    DataError::Format(format!("GeoParquet: {e}"))
}

fn arrow_err(e: arrow_schema::ArrowError) -> DataError {
    DataError::Format(format!("GeoParquet: {e}"))
}

fn is_binary(t: &DataType) -> bool {
    matches!(t, DataType::Binary | DataType::LargeBinary | DataType::BinaryView)
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

/// The bbox covering: the declared `covering.bbox`, else a `<geometry>_bbox` / `bbox` struct.
fn find_covering(meta: &ArrowReaderMetadata, col_meta: Option<&serde_json::Value>, primary: &str) -> Option<Covering> {
    let schema = meta.parquet_schema();
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
    let mut roots = [0usize; 4];
    for (i, p) in paths.iter().enumerate() {
        let refs: Vec<&str> = p.iter().map(String::as_str).collect();
        let l = leaf(&refs)?;
        *leaves.get_mut(i)? = l;
        *roots.get_mut(i)? = schema.get_column_root_idx(l);
    }
    Some(Covering { leaves, roots })
}

fn dedup(roots: [usize; 4]) -> Vec<usize> {
    let mut v = roots.to_vec();
    v.sort_unstable();
    v.dedup();
    v
}

fn stat(s: Option<&Statistics>, max: bool) -> Option<f64> {
    match s? {
        Statistics::Float(v) => (if max { v.max_opt() } else { v.min_opt() }).map(|x| f64::from(*x)),
        Statistics::Double(v) => (if max { v.max_opt() } else { v.min_opt() }).copied(),
        _ => None,
    }
}

/// A row group's bounding box from its covering statistics.
fn group_extent(md: &parquet::file::metadata::ParquetMetaData, g: usize, c: Covering) -> Option<Extent> {
    let rg = md.row_groups().get(g)?;
    let s = |i: usize, max: bool| stat(rg.columns().get(*c.leaves.get(i)?)?.statistics(), max);
    let e = Extent::new(s(0, false)?, s(1, false)?, s(2, true)?, s(3, true)?);
    (!e.is_empty()).then_some(e)
}

fn groups_extent(meta: &ArrowReaderMetadata, c: Covering) -> Extent {
    let md = meta.metadata();
    (0..md.num_row_groups()).filter_map(|g| group_extent(md, g, c)).fold(Extent::EMPTY, |a, e| a.union(&e))
}

/// The four covering columns of a batch read with only them projected, as doubles.
fn bbox_columns(b: &RecordBatch) -> Option<[arrow_array::Float64Array; 4]> {
    let mut leaves: Vec<ArrayRef> = Vec::new();
    for c in b.columns() {
        match c.data_type() {
            DataType::Struct(_) => leaves.extend(c.as_struct().columns().iter().cloned()),
            _ => leaves.push(c.clone()),
        }
    }
    let f = |i: usize| -> Option<arrow_array::Float64Array> {
        let a = arrow_cast::cast(leaves.get(i)?, &DataType::Float64).ok()?;
        a.as_primitive_opt::<arrow_array::types::Float64Type>().cloned()
    };
    Some([f(0)?, f(1)?, f(2)?, f(3)?])
}

/// Run-length encodes a row mask as a selection.
fn to_selection(take: &[bool]) -> RowSelection {
    let mut sel: Vec<RowSelector> = Vec::new();
    let mut i = 0;
    while i < take.len() {
        let on = take.get(i).copied().unwrap_or(false);
        let mut j = i;
        while j < take.len() && take.get(j).copied().unwrap_or(false) == on {
            j += 1;
        }
        sel.push(if on { RowSelector::select(j - i) } else { RowSelector::skip(j - i) });
        i = j;
    }
    RowSelection::from(sel)
}

fn field_type(t: &DataType) -> Option<FieldType> {
    use DataType as D;
    Some(match t {
        D::Int8 | D::Int16 | D::Int32 | D::Int64 | D::UInt8 | D::UInt16 | D::UInt32 | D::UInt64 => FieldType::Integer,
        D::Float16 | D::Float32 | D::Float64 | D::Decimal32(..) | D::Decimal64(..) | D::Decimal128(..) | D::Decimal256(..) => {
            FieldType::Double
        }
        D::Boolean => FieldType::Boolean,
        D::Utf8 | D::LargeUtf8 | D::Utf8View | D::Date32 | D::Date64 | D::Timestamp(..) | D::Time32(_) | D::Time64(_) => FieldType::Text,
        D::Dictionary(_, v) if matches!(v.as_ref(), D::Utf8 | D::LargeUtf8 | D::Utf8View) => FieldType::Text,
        _ => return None,
    })
}

/// A column cast to the field's storage type.
fn convert(a: &ArrayRef, ty: FieldType) -> Option<ArrayRef> {
    let to = match ty {
        FieldType::Integer => DataType::Int64,
        FieldType::Double => DataType::Float64,
        FieldType::Boolean => DataType::Boolean,
        FieldType::Text => DataType::Utf8,
    };
    arrow_cast::cast(a, &to).ok()
}

fn value_at(a: &ArrayRef, ty: FieldType, r: usize) -> Value {
    if r >= a.len() || a.is_null(r) {
        return Value::Null;
    }
    match ty {
        FieldType::Integer => a.as_primitive_opt::<arrow_array::types::Int64Type>().map(|v| Value::Int(v.value(r))),
        FieldType::Double => a.as_primitive_opt::<arrow_array::types::Float64Type>().map(|v| Value::Float(v.value(r))),
        FieldType::Boolean => a.as_boolean_opt().map(|v| Value::Bool(v.value(r))),
        FieldType::Text => a.as_string_opt::<i32>().map(|v| Value::Text(v.value(r).to_string())),
    }
    .unwrap_or(Value::Null)
}

fn wkb_at(a: &ArrayRef, r: usize) -> Option<&[u8]> {
    if r >= a.len() || a.is_null(r) {
        return None;
    }
    match a.data_type() {
        DataType::Binary => a.as_binary_opt::<i32>().map(|v| v.value(r)),
        DataType::LargeBinary => a.as_binary_opt::<i64>().map(|v| v.value(r)),
        DataType::BinaryView => a.as_binary_view_opt().map(|v| v.value(r)),
        _ => None,
    }
}
