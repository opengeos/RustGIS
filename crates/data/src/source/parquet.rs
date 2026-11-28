//! GeoParquet: columnar files with WKB geometry, read with the pure-Rust `parquet` crate's column
//! readers (no Arrow, which would make the browser build far larger).
//!
//! Extent queries prune whole row groups by the bounding-box statistics of the bbox covering
//! column (GeoParquet 1.1, `covering.bbox`), then filter rows by those bounding boxes before any
//! geometry is decoded. Only the columns needed are read. Files without a covering still work: their
//! geometry is decoded and tested row by row.
//!
//! A file by its web address is read by range: its footer (from the end), then for a read the
//! covering's column chunks of the row groups that meet the extent, then the geometry and field
//! chunks of the rows the read takes, each set fetched in one round. Files without a covering are
//! downloaded whole instead.

use super::remote::{self, Remote};
use super::{Batch, FeatureSource, Reproject, Request, Sampler, SourceInfo, clamp_limit};
use crate::raster::Crs;
use crate::{DataError, Feature, Field, FieldType, GeometryType, Result, Value, wkb};
use bytes::Bytes;
use parquet::basic::{ConvertedType, Encoding, LogicalType, Type as Physical};
use parquet::column::reader::{ColumnReader, ColumnReaderImpl};
use parquet::data_type::{ByteArray, DataType as PqType};
use parquet::file::metadata::ParquetMetaData;
use parquet::file::reader::{FileReader, RowGroupReader, SerializedFileReader};
use parquet::file::statistics::Statistics;
use parquet::schema::types::ColumnDescPtr;
use rustgis_geom::Extent;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Values read per call.
const BATCH: usize = 8192;

/// Most rows read from one row group (fewer in the browser, whose memory is smaller).
const MAX_GROUP_ROWS: usize = if cfg!(target_arch = "wasm32") { 1 << 21 } else { 1 << 24 };

/// Conservative decoded-column budget for one group scanned without native view bounds.
const MAX_SCAN_BYTES: u64 = if cfg!(target_arch = "wasm32") { 64 << 20 } else { 256 << 20 };

/// The least [`max_decoded_bytes`] allows, and its limit in the browser.
const DECODED_FLOOR: usize = if cfg!(target_arch = "wasm32") { 128 << 20 } else { 512 << 20 };
/// The most [`max_decoded_bytes`] allows, however much the computer has.
const DECODED_CEILING: usize = 3 << 30;

/// Most bytes of text and geometry one row group's read may decode into owned values, whatever the
/// view: on the desktop a quarter of the computer's memory, from [`DECODED_FLOOR`] up to
/// [`DECODED_CEILING`]; less once RustGIS holds more than half of it ([`crate::memory::planned`], #2431).
/// A file's sizes don't bound it: one long dictionary entry used by every row is copied for each.
fn max_decoded_bytes() -> u64 {
    u64::try_from(crate::memory::planned(DECODED_FLOOR, DECODED_CEILING, 1)).unwrap_or(u64::MAX)
}

/// What one row group's read may still decode into owned values ([`max_decoded_bytes`]), shared
/// by the columns decoded together.
struct Budget {
    left: std::cell::Cell<u64>,
    limit: u64,
    group: usize,
}

impl Budget {
    fn new(limit: u64, group: usize) -> Self {
        Budget { left: std::cell::Cell::new(limit), limit, group }
    }

    /// Takes `bytes` of the budget for a value of `column`, or refuses the group.
    fn charge(&self, bytes: usize, column: &str) -> Result<()> {
        let left = self.left.get().checked_sub(u64::try_from(bytes).unwrap_or(u64::MAX)).ok_or_else(|| self.refusal(column))?;
        self.left.set(left);
        Ok(())
    }

    fn refusal(&self, column: &str) -> DataError {
        DataError::Format(format!(
            "GeoParquet: row group {} exceeds the {} byte decoded-column budget in column '{column}'; rewrite the file with smaller row groups",
            self.group, self.limit
        ))
    }
}

/// What a GeoParquet source's reads are attributed to ([`ParquetSource::attribute`]).
#[derive(Clone, Debug)]
enum Attributed {
    /// The file read, observed as it is at each read.
    File(std::path::PathBuf),
    /// A conversion's original, as it was when the conversion was opened. Conversions run only
    /// on the desktop.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Original(Arc<Vec<crate::provenance::Fingerprint>>),
}

/// A GeoParquet file.
pub struct ParquetSource {
    /// The local input attributed to reads: the file itself, observed again at each read, or a
    /// conversion's source (never its cache) as it was when the conversion was opened, since a
    /// source changed since isn't what the conversion holds.
    path: Option<Attributed>,
    info: SourceInfo,
    reader: Arc<dyn FileReader>,
    /// Leaf column index of the geometry.
    geom_col: usize,
    /// Leaf column index of each field, in `info.fields` order.
    field_cols: Vec<usize>,
    /// Leaf column indices of the bbox covering's xmin, ymin, xmax, ymax.
    covering: Option<[usize; 4]>,
    /// The metadata declares the covering, whose null boxes then mean no geometry or an empty
    /// one, neither of which meets an extent.
    declared_covering: bool,
    reproject: Reproject,
    /// The file, when it is read by its web address.
    remote: Option<Arc<Remote>>,
    /// Every row group meeting a read's extent is read (a catalog's index), never a sample of them.
    exact: bool,
    /// The most rows read from one row group: `MAX_GROUP_ROWS`, lowered by tests.
    group_rows: usize,
    /// Decoded-column budget for an unbounded spatial scan; lowered by tests.
    scan_bytes: u64,
    /// Decoded bytes one row group's text and geometry values may take in any read, else
    /// [`max_decoded_bytes`] as each read finds it; set by tests.
    decoded_bytes: Option<u64>,
    /// Declared after `reader`: Windows must close its file before removing the workspace.
    #[cfg(not(target_arch = "wasm32"))]
    _workspace: Option<Arc<super::duckdb::ReloadWorkspace>>,
}

impl ParquetSource {
    pub fn open_path(path: &Path, name: &str) -> Result<Self> {
        let file = crate::provenance::open(path)?;
        let mut source = Self::new(Arc::new(SerializedFileReader::new(file).map_err(err)?), name, None)?;
        source.path = Some(Attributed::File(path.to_path_buf()));
        Ok(source)
    }

    /// Records the input this reads: the file as it is now, or a conversion's original as it was.
    fn attribute(&self) {
        match &self.path {
            Some(Attributed::File(path)) => crate::provenance::path(path),
            Some(Attributed::Original(seen)) => crate::provenance::record(seen.iter().cloned()),
            None => {}
        }
    }

    /// Opens a conversion while retaining its original local input for later reads and traversals.
    /// Queries without a local database have no known local path; SQL's internal reads aren't inferred.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_converted_path(
        path: &Path,
        name: &str,
        original: Option<&Path>,
        workspace: Option<Arc<super::duckdb::ReloadWorkspace>>,
    ) -> Result<Self> {
        let mut source = crate::provenance::without(|| Self::open_path(path, name))?;
        source.path =
            original.map(|original| Attributed::Original(Arc::new(crate::provenance::snapshot(|| crate::provenance::duckdb(original)).1)));
        source.attribute();
        source._workspace = workspace;
        Ok(source)
    }

    pub fn open_bytes(bytes: Vec<u8>, name: &str) -> Result<Self> {
        Self::open_shared(super::Shared(Arc::new(bytes)), name)
    }

    /// From bytes shared with the caller, without a copy.
    pub fn open_shared(bytes: super::Shared, name: &str) -> Result<Self> {
        Self::new(Arc::new(SerializedFileReader::new(Bytes::from_owner(bytes)).map_err(err)?), name, None)
    }

    /// A file by its web address, read by range. Fails with [`remote::NEED`] until its footer is
    /// fetched (run it with [`remote::drive`]); a file without a bbox covering can't be read in
    /// place ([`remote::READ_WHOLE`]).
    pub fn open_remote(file: Arc<Remote>, name: &str) -> Result<Self> {
        let reader = remote_reader(&file)?;
        let src = Self::new(reader, name, Some(file))?;
        if src.covering.is_none() {
            return Err(DataError::Unsupported(format!("a GeoParquet without a bbox covering {}", remote::READ_WHOLE)));
        }
        Ok(src)
    }

    /// Adds nested columns (a struct's fields, by dotted path such as `assets.data.href`) to the
    /// fields read, as top-level columns are read. Paths the file hasn't, repeated columns and types
    /// not read are left out.
    pub fn with_columns(mut self, paths: &[&str]) -> Self {
        let reader = self.reader.clone();
        let schema = reader.metadata().file_metadata().schema_descr();
        for p in paths.iter().take(64) {
            if self.info.fields.iter().any(|f| f.name == *p) {
                continue;
            }
            let Some(i) = (0..schema.num_columns()).find(|&i| schema.column(i).path().string() == *p) else { continue };
            let c = schema.column(i);
            if i == self.geom_col || c.max_rep_level() > 0 || self.covering.is_some_and(|cv| cv.contains(&i)) {
                continue;
            }
            if let Some(ty) = field_type(&c) {
                self.info.fields.push(Field::new(p.to_string(), ty));
                self.field_cols.push(i);
            }
        }
        self
    }

    /// Reads take every row group an extent meets (a catalog's index, whose matches are all
    /// wanted), not a sample spread over the file; an extent needing more than one read may fetch
    /// is an error.
    pub fn exact(mut self) -> Self {
        self.exact = true;
        self
    }
}

/// The metadata of a file by its web address, from its footer: fails with [`remote::NEED`] until
/// the footer is fetched.
fn remote_reader(file: &Arc<Remote>) -> Result<Arc<RemoteFile>> {
    let (mut tail, whole) = file.tail(FOOTER).ok_or_else(remote::need)?;
    let meta_len = footer_length(&tail)?;
    let need = meta_len.saturating_add(8);
    if need > tail.len() as u64 {
        if whole || need > MAX_FOOTER {
            return Err(DataError::Format(format!("GeoParquet: a footer of {need} bytes, longer than the file or too long to read")));
        }
        tail = file.tail(need).ok_or_else(remote::need)?.0;
    }
    let end = tail.len().saturating_sub(8);
    let start = end.saturating_sub(usize::try_from(meta_len).unwrap_or(usize::MAX));
    let md = parquet::file::metadata::ParquetMetaDataReader::decode_metadata(tail.get(start..end).unwrap_or_default()).map_err(err)?;
    validate_chunks(&md)?;
    let props = Arc::new(parquet::file::properties::ReaderProperties::builder().build());
    Ok(Arc::new(RemoteFile { chunks: Arc::new(Chunks(file.clone())), metadata: Arc::new(md), props }))
}

impl ParquetSource {
    /// Opens a GeoParquet from its `geo` metadata: geometry column, types, bbox and coordinate
    /// system, noting Z or M values (which reading drops).
    fn new(reader: Arc<dyn FileReader>, name: &str, remote: Option<Arc<Remote>>) -> Result<Self> {
        let md = reader.metadata();
        validate_chunks(md)?;
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
        let mut notes = crate::loss::Notes::new();
        let crs = col_meta.and_then(|c| c.get("crs"));
        let code = crs_code(crs);
        // Another body's system is said, or refused when its coordinates aren't longitude/latitude.
        let other_body = crs.filter(|_| code.is_none()).and_then(crate::raster::planetary::projjson);
        if let Some(crate::raster::planetary::Declared::Refused(e)) = &other_body {
            return Err(DataError::Format(format!("its coordinate system: {e}")));
        }
        if let Some(crate::raster::planetary::Declared::LonLat { note, .. }) = &other_body {
            notes.push(format!("its coordinate system: {note}"));
        } else if code.is_none() && crs.is_some_and(|c| !c.is_null() && !is_crs84(c)) {
            // A coordinate system given without an EPSG code (and not OGC:CRS84) can't be converted.
            notes.push(crate::loss::Note::loss(
                crate::loss::LossCode::AssumedCrs,
                "its coordinate system has no EPSG code; read as longitude/latitude",
            ));
        }
        let reproject = Reproject::from_epsg(code, &mut notes);
        let covering = find_covering(md, col_meta, &primary);
        let declared_covering = col_meta.and_then(|c| c.get("covering")).and_then(|c| c.get("bbox")).is_some();

        let mut fields = Vec::new();
        let mut field_cols = Vec::new();
        for i in 0..schema.num_columns() {
            if i == geom_col || covering.is_some_and(|c| c.contains(&i)) {
                continue;
            }
            let Some(name) = top(i) else { continue };
            let c = schema.column(i);
            if let Some(ty) = field_type(&c) {
                fields.push(Field::new(name, ty));
                field_cols.push(i);
            }
        }
        // Top-level columns not read: those holding a leaf that is neither a field, the geometry
        // nor its covering's box (whose struct may also hold zmin and zmax, bounds of the geometry).
        let read: HashSet<usize> = field_cols.iter().chain([&geom_col]).chain(covering.iter().flatten()).copied().collect();
        let box_root = covering.and_then(|[x, ..]| schema.column(x).path().parts().first().cloned());
        let unread: HashSet<String> = (0..schema.num_columns())
            .filter(|i| !read.contains(i))
            .filter_map(|i| match schema.column(i).path().parts() {
                [r, bound] if Some(r) == box_root.as_ref() && matches!(bound.as_str(), "zmin" | "zmax") => None,
                parts => parts.first().cloned(),
            })
            .collect();
        let geometries = geo.as_ref().and_then(|g| g.get("columns")).and_then(|c| c.as_object());
        let left_out: Vec<(String, String)> = schema
            .root_schema()
            .get_fields()
            .iter()
            .filter(|t| unread.contains(t.name()))
            .map(|t| {
                let kind = if geometries.is_some_and(|g| g.contains_key(t.name())) { "geometry" } else { column_kind(t) };
                (t.name().to_string(), kind.to_string())
            })
            .collect();
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
        // Types such as "Point Z" (every geometry has Z or M values when every type does), or a bbox
        // with Z bounds. Without types (any geometry), the first rows are looked at below.
        let types: Vec<&str> = col_meta
            .and_then(|c| c.get("geometry_types"))
            .and_then(|t| t.as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .collect();
        let has_zm = |v: &&str| v.ends_with(" Z") || v.ends_with(" M") || v.ends_with(" ZM");
        let all_zm = !types.is_empty() && types.iter().all(has_zm);
        let zm = types.iter().any(has_zm) || col_meta.and_then(|c| c.get("bbox")).and_then(|b| b.as_array()).is_some_and(|b| b.len() == 6);
        let count = u64::try_from(md.file_metadata().num_rows()).ok();
        let mut info = SourceInfo {
            name: name.to_string(),
            format: "GeoParquet",
            geometry_type,
            fields,
            extent,
            count,
            crs: reproject.label(),
            indexed: covering.is_some(),
            notes,
            zm_dropped: false,
        };
        info.note_columns(
            &left_out,
            ["of a type RustGIS has no field for was left out", "of types RustGIS has no field for were left out"],
        );
        // An empty file loses nothing.
        if zm && count != Some(0) {
            info.drop_zm(if all_zm { count } else { None });
        }
        let untyped = types.is_empty() && !zm;
        let mut src = ParquetSource {
            path: None,
            info,
            reader,
            geom_col,
            field_cols,
            covering,
            declared_covering,
            reproject,
            remote,
            exact: false,
            group_rows: MAX_GROUP_ROWS,
            scan_bytes: MAX_SCAN_BYTES,
            decoded_bytes: None,
            #[cfg(not(target_arch = "wasm32"))]
            _workspace: None,
        };
        if untyped {
            match src.peek_zm() {
                Some(true) => src.info.drop_zm(None),
                Some(false) => {}
                None => src.info.notes.push("couldn't tell whether it has Z or M values, which RustGIS doesn't keep"),
            }
        }
        if src.info.geometry_type == GeometryType::None {
            src.info.geometry_type = super::peek_kind(&src);
        }
        Ok(src)
    }

    /// Whether any of the first 16 geometries has Z or M values; `None` when they can't be read
    /// here (a remote file's first row group not fetched yet).
    fn peek_zm(&self) -> Option<bool> {
        let md = self.reader.metadata();
        if md.num_row_groups() == 0 {
            return Some(false);
        }
        if let Some(file) = &self.remote
            && !want_chunks(file, md, &[0], &[self.geom_col]).ok()?
        {
            return None;
        }
        let rows = usize::try_from(md.row_group(0).num_rows()).unwrap_or(0).min(16);
        let rg = self.reader.get_row_group(0).ok()?;
        let geoms = read_bytes(rg.as_ref(), self.geom_col, &vec![true; rows], &Budget::new(self.decoded_limit(), 0)).ok()?;
        Some(geoms.iter().filter_map(|b| b.as_deref()).any(wkb::has_zm))
    }
}

impl ParquetSource {
    /// The decoded bytes a row group may take in the read starting now: asked once a read, when it
    /// first reads a row group (a read that reads none never asks), as the memory the process holds
    /// changes between reads ([`crate::memory::planned`]).
    fn decoded_limit(&self) -> u64 {
        self.decoded_bytes.unwrap_or_else(max_decoded_bytes)
    }

    /// A source that reads at most `rows` of a row group, for tests of the cap.
    #[cfg(test)]
    pub(crate) fn with_group_rows(mut self, rows: usize) -> Self {
        self.group_rows = rows;
        self
    }

    /// A lower decoded-column budget, for tests without large allocations.
    #[cfg(test)]
    pub(crate) fn with_scan_bytes(mut self, bytes: u64) -> Self {
        self.scan_bytes = bytes;
        self
    }

    /// A lower decoded-byte budget for any read, for tests without large allocations.
    #[cfg(test)]
    pub(crate) fn with_decoded_bytes(mut self, bytes: u64) -> Self {
        self.decoded_bytes = Some(bytes);
        self
    }

    /// The rows of row group `g` read. Row counts come from the file: bounded by what the
    /// geometry column chunk holds, and by a cap, so a crafted count can't size a huge allocation.
    fn rows_in(&self, md: &ParquetMetaData, g: usize) -> usize {
        let rg = md.row_group(g);
        let rows = usize::try_from(rg.num_rows()).unwrap_or(0);
        let values = rg.columns().get(self.geom_col).and_then(|c| usize::try_from(c.num_values()).ok()).unwrap_or(0);
        rows.min(values).min(self.group_rows)
    }

    /// Bounds the group's decoded columns with six copies for decoding/UTF-8 conversion and
    /// 128 bytes per row/column for value slots and temporary vectors. Plain binary is linear
    /// in its encoded size. Dictionaries use their longest entry times the row count; prefix
    /// encoding retains a conservative whole-payload bound. The initial metadata-only pass
    /// runs before remote fetching; `dictionaries` adds expansion checks once chunks are present.
    fn check_scan_bytes(&self, md: &ParquetMetaData, g: usize, dictionaries: bool) -> Result<()> {
        let group = md.row_group(g);
        let rows = u64::try_from(group.num_rows()).unwrap_or(u64::MAX);
        let over = || {
            DataError::Format(format!(
                "GeoParquet: row group {g} exceeds the {} byte decoded-column budget for an unbounded view; choose a smaller view or rewrite the file with smaller row groups",
                self.scan_bytes
            ))
        };
        let mut bytes = rows.saturating_mul(2);
        for c in [self.geom_col].into_iter().chain(self.field_cols.iter().copied()) {
            let Some(column) = group.columns().get(c) else {
                return Err(DataError::Format(format!("GeoParquet: row group {g} has a missing column")));
            };
            let encoded = u64::try_from(column.uncompressed_size()).unwrap_or(u64::MAX);
            bytes = bytes.saturating_add(encoded.saturating_mul(6)).saturating_add(rows.saturating_mul(128));
            if bytes > self.scan_bytes {
                return Err(over());
            }
            if column.column_descr().physical_type() == Physical::BYTE_ARRAY {
                let expansion = if column.encodings().any(|e| e == Encoding::DELTA_BYTE_ARRAY) {
                    encoded.saturating_mul(rows)
                } else if dictionaries && column.encodings().any(|e| matches!(e, Encoding::RLE_DICTIONARY | Encoding::PLAIN_DICTIONARY)) {
                    let rg = self.reader.get_row_group(g).map_err(err)?;
                    let mut pages = rg.get_column_page_reader(c).map_err(err)?;
                    match pages.get_next_page().map_err(err)? {
                        Some(parquet::column::page::Page::DictionaryPage { buf, num_values, .. }) => {
                            binary_dictionary_max(&buf, num_values)?.saturating_mul(rows)
                        }
                        _ => return Err(DataError::Format("GeoParquet: missing binary dictionary page".into())),
                    }
                } else {
                    0
                };
                bytes = bytes.saturating_add(expansion.saturating_mul(6));
                if bytes > self.scan_bytes {
                    return Err(over());
                }
            }
        }
        Ok(())
    }

    /// Whether inferred bounds may omit or count rows that only their geometry can test.
    /// Unknown null counts require the same fallback as known nulls; declared coverings promise
    /// that a box is present if and only if a non-empty geometry is present.
    fn needs_geometry_filter(&self, md: &ParquetMetaData, g: usize) -> bool {
        let Some(c) = self.covering else { return true };
        !self.declared_covering
            && c.into_iter()
                .chain([self.geom_col])
                .any(|i| md.row_group(g).columns().get(i).and_then(|col| col.statistics()).and_then(|s| s.null_count_opt()) != Some(0))
    }

    /// Which rows of row group `g` have a bounding box meeting the native extent `e`, read from
    /// the covering columns `c` alone. Null boxes cannot match a declared covering; inferred
    /// bounds leave them for the geometry to decide.
    fn covered_rows(&self, md: &ParquetMetaData, g: usize, e: &Extent, c: [usize; 4]) -> Result<Vec<bool>> {
        let rg = self.reader.get_row_group(g).map_err(err)?;
        let n = self.rows_in(md, g);
        let cols: Vec<Vec<Option<f64>>> = c.iter().map(|&i| read_f64(rg.as_ref(), i, n)).collect::<Result<_>>()?;
        if cols.iter().any(|col| col.len() != n) {
            return Err(DataError::Format(format!("GeoParquet: row group {g} has covering columns shorter than its rows")));
        }
        let v = |k: usize, r: usize| cols.get(k).and_then(|c| c.get(r).copied().flatten());
        Ok((0..n)
            .map(|r| {
                let [Some(min_x), Some(min_y), Some(max_x), Some(max_y)] = [v(0, r), v(1, r), v(2, r), v(3, r)] else {
                    return !self.declared_covering;
                };
                let be = Extent::new(min_x, min_y, max_x, max_y);
                be.is_empty() || be.intersects(e)
            })
            .collect())
    }
}

impl FeatureSource for ParquetSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    /// From every row group's statistics of the field's column, which the footer holds: no byte of
    /// the data is read. A row group without a statistics object, or whose statistics give no
    /// bounds while it has values, or bounds that aren't finite and in order, leaves the range
    /// unknown (`None`); one whose statistics count only nulls adds nothing. The field is named exactly
    /// as the source lists it.
    fn field_range(&self, field: &str) -> Option<(f64, f64)> {
        let i = self.info.fields.iter().position(|f| f.name == field && f.ty.is_numeric())?;
        let col = *self.field_cols.get(i)?;
        let md = self.reader.metadata();
        let desc = md.file_metadata().schema_descr().column(col);
        // Unsigned integers are ordered as unsigned in the statistics, not as the values are read.
        if matches!(desc.logical_type_ref(), Some(LogicalType::Integer(t)) if !t.is_signed)
            || matches!(
                desc.converted_type(),
                ConvertedType::UINT_8 | ConvertedType::UINT_16 | ConvertedType::UINT_32 | ConvertedType::UINT_64
            )
        {
            return None;
        }
        let scale = decimal_scale(&desc).map_or(1.0, |s| 10f64.powi(-s));
        let (lo, hi) = (0..md.num_row_groups()).try_fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), g| {
            let rg = md.row_group(g);
            let s = rg.columns().get(col)?.statistics()?;
            match (number_stat(s, false), number_stat(s, true)) {
                (Some(a), Some(b)) => {
                    let (a, b) = (a * scale, b * scale);
                    (a.is_finite() && b.is_finite() && a <= b).then(|| (lo.min(a), hi.max(b)))
                }
                // A row group of nulls only has no bounds.
                _ if s.null_count_opt() == Some(u64::try_from(rg.num_rows()).ok()?) => Some((lo, hi)),
                _ => None,
            }
        })?;
        (lo.is_finite() && hi.is_finite() && lo <= hi).then_some((lo, hi))
    }

    fn reads_own_box(&self) -> bool {
        true
    }

    /// Its own system, in which it gives its features as the file holds them.
    fn coord_crs(&self) -> crate::raster::Crs {
        self.reproject.crs()
    }

    /// Keeps rows with null geometry in whole reads; they have no location to match an extent.
    /// Unbounded spatial scans bound retained row masks and refuse groups over the decoded-column
    /// budget before decoding, count rows and check Cancel between them. They decode every selected group's geometry
    /// and fields before sampling matches; a refused view therefore scans the selected file.
    fn read(&self, req: &Request) -> Result<Batch> {
        self.attribute();
        let decoded = std::cell::LazyCell::new(|| self.decoded_limit());
        let limit = clamp_limit(req.limit);
        let md = self.reader.metadata();
        let native = self.reproject.request_box(req);
        // Preserve ordinary candidate counts; a refused view needs a geometry scan to count.
        if limit == 0 && req.extent.is_some() && native.is_none() {
            return Err(super::unbounded_count());
        }
        let rows_in = |g: usize| self.rows_in(md, g);
        // Row groups whose bounding box meets the extent.
        let mut groups: Vec<usize> = (0..md.num_row_groups())
            .filter(|&g| match (native, self.covering) {
                (Some(e), Some(c)) => self.needs_geometry_filter(md, g) || group_extent(md, g, c).is_none_or(|ge| ge.intersects(&e)),
                _ => true,
            })
            .collect();
        // Read by address: no more row groups than a read may fetch (an evenly spread part of them,
        // a sample), and the covering's chunks of those asked for together.
        // A row group wholly inside the extent with no null boxes: every row matches,
        // its covering unread. Unknown null counts must be checked in the covering.
        let inside = |g: usize| match (native, self.covering) {
            (Some(e), Some(c)) => {
                let all_boxes = c.iter().all(|&i| {
                    md.row_group(g).columns().get(i).and_then(|col| col.statistics()).and_then(|s| s.null_count_opt()) == Some(0)
                });
                all_boxes && group_extent(md, g, c).is_some_and(|ge| contains(&e, &ge))
            }
            _ => true,
        };
        let mut thinned = 1usize;
        // Without native bounds every group may contain view matches. Scan within the budget,
        // or refuse explicitly: thinning groups first could discard all of a nonempty view.
        let unbounded = req.extent.is_some() && native.is_none();
        if unbounded {
            // Keep/take headers and one bool per candidate row stay live across all groups.
            // A per-group decode bound and a compressed-byte budget do not bound their sum.
            let headers = groups.len().saturating_mul(std::mem::size_of::<Option<Vec<bool>>>() + std::mem::size_of::<Vec<bool>>());
            let mask_bytes = groups.iter().try_fold(headers as u64, |n, &g| {
                crate::progress::check()?;
                Ok::<_, DataError>(n.saturating_add(rows_in(g) as u64))
            })?;
            if mask_bytes > self.scan_bytes {
                return Err(DataError::Format(format!(
                    "GeoParquet: the unbounded view exceeds the {} byte row-mask budget; choose a view that can be bounded in the file's coordinate system",
                    self.scan_bytes
                )));
            }
            // Every selected group must be scanned before sampling its view matches.
            for &g in &groups {
                crate::progress::check()?;
                self.check_scan_bytes(md, g, false)?;
                let declared = u64::try_from(md.row_group(g).num_rows()).unwrap_or(0);
                if declared > self.group_rows as u64 {
                    return Err(DataError::Format(format!(
                        "GeoParquet: row group {g} has {declared} rows, more than the {} read from one",
                        self.group_rows
                    )));
                }
                if declared > rows_in(g) as u64 {
                    return Err(DataError::Format(format!("GeoParquet: row group {g} has columns shorter than its rows")));
                }
            }
        }
        if let Some(file) = &self.remote {
            // Far more rows than the read takes: a sample of row groups evenly spread over them
            // (the read samples their rows), so what is fetched stays near what is shown. No more
            // bytes than a read may fetch.
            let rows: u64 = groups.iter().map(|&g| rows_in(g) as u64).sum();
            if rows > (limit as u64).saturating_mul(8) && !groups.is_empty() && !self.exact && !unbounded {
                thinned = usize::try_from(rows.div_ceil((limit as u64).saturating_mul(4).max(1))).unwrap_or(usize::MAX).max(2);
            }
            // A read without native bounds never fetches the covering columns.
            let cols: Vec<usize> = if native.is_some() { self.covering.map(|c| c.to_vec()).unwrap_or_default() } else { Vec::new() };
            let all: Vec<usize> = cols.iter().copied().chain([self.geom_col]).chain(self.field_cols.iter().copied()).collect();
            let cost: u64 = groups.iter().step_by(thinned).map(|&g| chunks_size(md, g, &all)).sum();
            if cost > remote::READ_BUDGET && unbounded {
                return Err(remote::view_budget());
            }
            if cost > remote::READ_BUDGET && self.exact {
                return Err(DataError::Format(format!(
                    "GeoParquet: the extent needs {} MB of the file, more than one read takes",
                    cost >> 20
                )));
            }
            if cost > remote::READ_BUDGET {
                thinned = thinned.saturating_mul(usize::try_from(cost.div_ceil(remote::READ_BUDGET)).unwrap_or(usize::MAX).max(2));
            }
            if thinned > 1 {
                groups = groups.into_iter().step_by(thinned).collect();
            }
            let edge: Vec<usize> = groups.iter().copied().filter(|&g| !inside(g)).collect();
            if native.is_some() && !want_chunks(file, md, &edge, &cols)? {
                return Err(remote::need());
            }
        }

        // Which rows of those groups match: with a covering and an extent, exactly the rows whose
        // bounding box meets it (read from the covering alone); otherwise all of them.
        let mut keep: Vec<Option<Vec<bool>>> = Vec::with_capacity(groups.len());
        for &g in &groups {
            keep.push(match (native, self.covering) {
                (Some(_), Some(_)) if inside(g) => None,
                (Some(e), Some(c)) => Some(self.covered_rows(md, g, &e, c)?),
                _ => None,
            });
        }
        let matched: u64 =
            groups.iter().zip(&keep).map(|(&g, k)| k.as_ref().map(|k| k.iter().filter(|&&b| b).count()).unwrap_or(rows_in(g)) as u64).sum();
        // Without a covering, or with nullable inferred bounds, test geometry before sampling
        // for the whole request. Otherwise the match count is known, and every `step`-th is read.
        // An extent the file's system couldn't bound (`native` is `None`) is tested on every row.
        let late = req.extent.is_some() && (native.is_none() || groups.iter().any(|&g| self.needs_geometry_filter(md, g)));
        let mut sample = if late { Sampler::new(limit) } else { Sampler::with_total(limit, matched) };
        // The rows of each group the sample takes, decided before anything else is read.
        let takes: Vec<Vec<bool>> = groups
            .iter()
            .zip(&keep)
            .map(|(&g, k)| {
                (0..rows_in(g)).map(|r| k.as_ref().is_none_or(|k| k.get(r).copied().unwrap_or(false)) && (late || sample.wants())).collect()
            })
            .collect();
        // Read by address: the geometry and field chunks of the groups any row is taken from.
        if let Some(file) = &self.remote {
            let cols: Vec<usize> = [self.geom_col].into_iter().chain(self.field_cols.iter().copied()).collect();
            let used: Vec<usize> = groups.iter().zip(&takes).filter(|(_, t)| t.iter().any(|&t| t)).map(|(&g, _)| g).collect();
            if !want_chunks(file, md, &used, &cols)? {
                return Err(remote::need());
            }
        }
        let (mut unread_dates, mut rounded_dates) = (Vec::new(), Vec::new());
        let mut skipped = 0;
        let mut starts = Vec::with_capacity(md.num_row_groups());
        let mut first_row = 0u64;
        // From the declared row counts, not the capped ones, so ids stay put whatever is read.
        for g in 0..md.num_row_groups() {
            starts.push(first_row);
            first_row = first_row.saturating_add(u64::try_from(md.row_group(g).num_rows()).unwrap_or(0));
        }
        if unbounded {
            crate::progress::begin(usize::try_from(matched).unwrap_or(0));
        }
        for (&g, take) in groups.iter().zip(&takes) {
            if unbounded {
                crate::progress::check()?;
            }
            if !take.iter().any(|&t| t) {
                continue;
            }
            if unbounded {
                self.check_scan_bytes(md, g, true)?;
            }
            let rg = self.reader.get_row_group(g).map_err(err)?;
            let budget = Budget::new(*decoded, g);
            let geoms = read_bytes(rg.as_ref(), self.geom_col, take, &budget)?;
            let base = starts.get(g).copied().unwrap_or(0);
            let wanted = take.iter().filter(|&&t| t).count();
            if late {
                // The geometry test and the sampler choose the rows first; only their attributes
                // are decoded, and so charged.
                if geoms.len() != wanted {
                    return Err(DataError::Format(format!("GeoParquet: row group {g} has columns shorter than its rows")));
                }
                for (k, (r, _)) in take.iter().enumerate().filter(|(_, t)| **t).enumerate() {
                    if unbounded {
                        crate::progress::step(1)?;
                    }
                    let geometry = match geoms.get(k).and_then(|b| b.as_deref()) {
                        Some(bytes) => {
                            let Some(geometry) = wkb::decode(bytes).and_then(|g| self.reproject.place(g, req)) else {
                                skipped += 1;
                                continue;
                            };
                            Some(geometry)
                        }
                        None => None,
                    };
                    if !geometry.as_ref().map_or(req.extent.is_none(), |g| g.inside) || !sample.wants() {
                        continue;
                    }
                    sample.keep(Feature {
                        id: base.saturating_add(r as u64 + 1),
                        geometry: geometry.map(|p| p.geometry),
                        attrs: Vec::new(),
                    });
                }
                // The rows of this group the sample still holds, in order (it may have dropped some).
                let (chosen, mine) = rows_held(&sample.items, |f| f.id, base, take.len());
                if mine.is_empty() {
                    continue;
                }
                let attrs: Vec<ColumnValues> =
                    self.field_cols.iter().map(|&c| read_values(rg.as_ref(), c, &chosen, &budget)).collect::<Result<_>>()?;
                if attrs.iter().any(|a| a.values.len() != mine.len()) {
                    return Err(DataError::Format(format!("GeoParquet: row group {g} has columns shorter than its rows")));
                }
                for (n, &i) in mine.iter().enumerate() {
                    if let Some(f) = sample.items.get_mut(i) {
                        f.attrs = attrs
                            .iter()
                            .enumerate()
                            .map(|(field, col)| col.value(n, field, &mut unread_dates, &mut rounded_dates))
                            .collect();
                    }
                }
                continue;
            }
            let attrs: Vec<ColumnValues> =
                self.field_cols.iter().map(|&c| read_values(rg.as_ref(), c, take, &budget)).collect::<Result<_>>()?;
            // A column shorter than its row group (a truncated or corrupt file) would pair values
            // with the wrong rows: refuse it.
            if geoms.len() != wanted || attrs.iter().any(|a| a.values.len() != wanted) {
                return Err(DataError::Format(format!("GeoParquet: row group {g} has columns shorter than its rows")));
            }
            // The columns hold the taken rows only, in order: the k-th is row `r`.
            for (k, (r, _)) in take.iter().enumerate().filter(|(_, t)| **t).enumerate() {
                if unbounded {
                    crate::progress::step(1)?;
                }
                // Null geometry is valid; malformed non-null WKB is skipped and counted.
                let geometry = match geoms.get(k).and_then(|b| b.as_deref()) {
                    Some(bytes) => {
                        let Some(geometry) = wkb::decode(bytes).and_then(|g| self.reproject.place(g, req)) else {
                            skipped += 1;
                            continue;
                        };
                        Some(geometry)
                    }
                    None => None,
                };
                let matches_extent = geometry.as_ref().map_or(req.extent.is_none(), |g| g.inside);
                if !matches_extent {
                    continue;
                }
                let geometry = geometry.map(|p| p.geometry);
                let values =
                    attrs.iter().enumerate().map(|(field, col)| col.value(k, field, &mut unread_dates, &mut rounded_dates)).collect();
                sample.keep(Feature { id: base.saturating_add(r as u64 + 1), geometry, attrs: values });
            }
        }
        let matched = (if late { sample.seen() } else { matched }).saturating_mul(thinned as u64);
        Ok(Batch {
            matched,
            complete: sample.complete() && thinned == 1,
            skipped,
            features: sample.items,
            unread_dates,
            rounded_dates,
            null_text: Vec::new(),
            notes: Default::default(),
            not_utf8: Vec::new(),
        })
    }

    fn remote(&self) -> Option<Arc<Remote>> {
        self.remote.clone()
    }

    /// A file or an upload; a file by its web address is read a view at a time, not in full.
    fn visits_chunks(&self) -> bool {
        self.remote.is_none()
    }

    /// A row group at a time, in file order: the features, ids and skipped rows of a complete
    /// read of the same extent. The taken rows of one row group are decoded together before
    /// they are chunked, so the peak is a row group's columns plus a chunk. Null geometry is
    /// retained without an extent filter, keeping its row and attributes in whole exports.
    /// Checks Cancel between groups and scanned rows; each group's column decoding finishes
    /// before the next check. An unbounded view also checks the decoded-column budget first.
    fn visit_chunks(&self, extent: Option<Extent>, bytes: usize, visit: &mut dyn FnMut(Vec<Feature>) -> Result<()>) -> Result<Batch> {
        self.attribute();
        let decoded = std::cell::LazyCell::new(|| self.decoded_limit());
        let mut chunks = super::chunk_memory::Chunker::new(bytes, visit)?;
        if self.remote.is_some() {
            return Err(DataError::Unsupported("a whole GeoParquet by its web address read a chunk at a time".into()));
        }
        let req = Request { extent, limit: usize::MAX, own: None };
        let md = self.reader.metadata();
        let native = extent.and_then(|e| self.reproject.extent_from_lonlat(&e));
        let mut out = Batch { complete: true, ..Batch::default() };
        let mut base = 0u64;
        crate::progress::begin(usize::try_from(md.file_metadata().num_rows()).unwrap_or(0));
        for g in 0..md.num_row_groups() {
            crate::progress::check()?;
            // From the declared row counts, as a read numbers them.
            let start = base;
            let declared = u64::try_from(md.row_group(g).num_rows()).unwrap_or(0);
            base = base.saturating_add(declared);
            let area = match (native, self.covering) {
                (Some(e), Some(c)) => Some((e, c, group_extent(md, g, c))),
                _ => None,
            };
            if let Some((e, _, Some(ge))) = area
                && !self.needs_geometry_filter(md, g)
                && !ge.intersects(&e)
            {
                continue;
            }
            // A read takes at most `MAX_GROUP_ROWS` of a row group, and no more than its geometry
            // column holds. A traversal promises every row, so a group it can't read whole is an
            // error, not a quietly shorter export; it is refused before this group is read.
            if extent.is_some() && native.is_none() {
                self.check_scan_bytes(md, g, true)?;
            }
            let readable = self.rows_in(md, g);
            if declared > self.group_rows as u64 {
                return Err(DataError::Format(format!(
                    "GeoParquet: row group {g} has {declared} rows, more than the {} read from one",
                    self.group_rows
                )));
            }
            if declared > readable as u64 {
                return Err(DataError::Format(format!("GeoParquet: row group {g} has columns shorter than its rows")));
            }
            let take = match area {
                Some((e, c, ge)) if !ge.is_some_and(|ge| contains(&e, &ge)) => self.covered_rows(md, g, &e, c)?,
                _ => vec![true; readable],
            };
            if !take.iter().any(|&t| t) {
                continue;
            }
            let rg = self.reader.get_row_group(g).map_err(err)?;
            let budget = Budget::new(*decoded, g);
            let geoms = read_bytes(rg.as_ref(), self.geom_col, &take, &budget)?;
            let wanted = take.iter().filter(|&&t| t).count();
            if extent.is_some() {
                // The geometry test chooses the rows first; only their attributes are decoded, and
                // so charged.
                if geoms.len() != wanted {
                    return Err(DataError::Format(format!("GeoParquet: row group {g} has columns shorter than its rows")));
                }
                let mut chosen = vec![false; take.len()];
                let mut found = Vec::new();
                for (k, (r, _)) in take.iter().enumerate().filter(|(_, t)| **t).enumerate() {
                    crate::progress::step(1)?;
                    let geometry = match geoms.get(k).and_then(|b| b.as_deref()) {
                        Some(bytes) => {
                            let Some(geometry) = wkb::decode(bytes).and_then(|geom| self.reproject.place(geom, &req)) else {
                                out.skipped += 1;
                                continue;
                            };
                            Some(geometry)
                        }
                        None => None,
                    };
                    if !geometry.as_ref().map_or(req.extent.is_none(), |g| g.inside) {
                        continue;
                    }
                    if let Some(c) = chosen.get_mut(r) {
                        *c = true;
                    }
                    found.push((k, r));
                }
                if found.is_empty() {
                    continue;
                }
                let attrs: Vec<ColumnValues> =
                    self.field_cols.iter().map(|&c| read_values(rg.as_ref(), c, &chosen, &budget)).collect::<Result<_>>()?;
                if attrs.iter().any(|a| a.values.len() != found.len()) {
                    return Err(DataError::Format(format!("GeoParquet: row group {g} has columns shorter than its rows")));
                }
                // Decoded again for the rows found, so no decoded geometry outlives the first pass.
                for (n, (k, r)) in found.into_iter().enumerate() {
                    let geometry = geoms
                        .get(k)
                        .and_then(|b| b.as_deref())
                        .and_then(|bytes| wkb::decode(bytes).and_then(|geom| self.reproject.place(geom, &req)))
                        .map(|p| p.geometry);
                    let values = attrs
                        .iter()
                        .enumerate()
                        .map(|(field, col)| col.value(n, field, &mut out.unread_dates, &mut out.rounded_dates))
                        .collect();
                    out.matched += 1;
                    chunks.push(Feature { id: start.saturating_add(r as u64 + 1), geometry, attrs: values })?;
                }
                continue;
            }
            let attrs: Vec<ColumnValues> =
                self.field_cols.iter().map(|&c| read_values(rg.as_ref(), c, &take, &budget)).collect::<Result<_>>()?;
            if geoms.len() != wanted || attrs.iter().any(|a| a.values.len() != wanted) {
                return Err(DataError::Format(format!("GeoParquet: row group {g} has columns shorter than its rows")));
            }
            for (k, (r, _)) in take.iter().enumerate().filter(|(_, t)| **t).enumerate() {
                crate::progress::step(1)?;
                let geometry = match geoms.get(k).and_then(|b| b.as_deref()) {
                    Some(bytes) => {
                        let Some(geometry) = wkb::decode(bytes).and_then(|geom| self.reproject.place(geom, &req)) else {
                            out.skipped += 1;
                            continue;
                        };
                        Some(geometry)
                    }
                    None => None,
                };
                if !geometry.as_ref().map_or(req.extent.is_none(), |g| g.inside) {
                    continue;
                }
                let geometry = geometry.map(|p| p.geometry);
                let values = attrs
                    .iter()
                    .enumerate()
                    .map(|(field, col)| col.value(k, field, &mut out.unread_dates, &mut out.rounded_dates))
                    .collect();
                out.matched += 1;
                chunks.push(Feature { id: start.saturating_add(r as u64 + 1), geometry, attrs: values })?;
            }
        }
        chunks.flush()?;
        Ok(out)
    }
}

/// The footer read first from a file by its address: enough for most files' metadata.
const FOOTER: u64 = 256 << 10;
/// The longest footer read.
const MAX_FOOTER: u64 = 64 << 20;

/// The metadata's length, from a file's last 8 bytes.
fn footer_length(tail: &[u8]) -> Result<u64> {
    let n = tail.len();
    if n < 12 || tail.get(n - 4..) != Some(b"PAR1".as_slice()) {
        return Err(DataError::Format("not a Parquet file".into()));
    }
    let len = tail.get(n - 8..n - 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes).unwrap_or(0);
    Ok(u64::from(len))
}

/// Rejects invalid ranges before any column reader can reach `ColumnChunkMetaData::byte_range`,
/// which asserts on negative offsets and lengths. Also covers geometry probing and vector reads.
fn validate_chunks(md: &ParquetMetaData) -> Result<()> {
    for (g, group) in md.row_groups().iter().enumerate() {
        for (col, chunk) in group.columns().iter().enumerate() {
            let data = chunk.data_page_offset();
            let start = chunk.dictionary_page_offset().unwrap_or(data);
            let size = chunk.compressed_size();
            // A dictionary page precedes the data pages; reading from a later one would skip data.
            if data < 0 || start < 0 || start > data || size < 0 || start.checked_add(size).is_none() {
                return Err(DataError::Format(format!("GeoParquet: row group {g}, column {col} has an invalid column chunk range")));
            }
        }
    }
    Ok(())
}

/// A column chunk's bytes: (start, length), from its offsets (checked, never negative).
fn chunk_range(md: &ParquetMetaData, g: usize, col: usize) -> Option<(u64, u64)> {
    let c = md.row_groups().get(g)?.columns().get(col)?;
    let start = c.dictionary_page_offset().unwrap_or(c.data_page_offset());
    Some((u64::try_from(start).ok()?, u64::try_from(c.compressed_size()).ok()?))
}

/// The bytes of some columns' chunks of a row group.
fn chunks_size(md: &ParquetMetaData, g: usize, cols: &[usize]) -> u64 {
    cols.iter().filter_map(|&c| chunk_range(md, g, c)).map(|(_, n)| n).sum()
}

/// Asks for some columns' chunks of some row groups: `true` when all are held.
fn want_chunks(file: &Remote, md: &ParquetMetaData, groups: &[usize], cols: &[usize]) -> Result<bool> {
    let mut held = true;
    for &g in groups {
        for &c in cols {
            let (at, len) =
                chunk_range(md, g, c).ok_or_else(|| DataError::Format(format!("GeoParquet: row group {g} has no column {c}")))?;
            if len > remote::READ_BUDGET {
                return Err(DataError::Format(format!("GeoParquet: a column chunk of {len} bytes is too large to read in place")));
            }
            held &= file.want(at, len);
        }
    }
    Ok(held)
}

/// A file by its address, as the `parquet` crate reads it: from the parts held (a read of one not
/// held notes it missing and fails).
struct Chunks(Arc<Remote>);

impl parquet::file::reader::Length for Chunks {
    fn len(&self) -> u64 {
        // Not known (the footer is read from the end): reads are bounded by the parts held.
        u64::MAX
    }
}

impl parquet::file::reader::ChunkReader for Chunks {
    type T = bytes::buf::Reader<Bytes>;

    fn get_read(&self, start: u64) -> parquet::errors::Result<Self::T> {
        use bytes::Buf;
        self.0.from(start, remote::MIN_FETCH).map(Buf::reader).ok_or_else(|| parquet::errors::ParquetError::General(remote::NEED.into()))
    }

    fn get_bytes(&self, start: u64, length: usize) -> parquet::errors::Result<Bytes> {
        match self.0.bytes(start, length as u64) {
            Some(b) if b.len() == length => Ok(b),
            Some(_) => Err(parquet::errors::ParquetError::EOF("the file ends early".into())),
            None => Err(parquet::errors::ParquetError::General(remote::NEED.into())),
        }
    }
}

/// A file by its address, with its metadata read from the footer.
struct RemoteFile {
    chunks: Arc<Chunks>,
    metadata: Arc<ParquetMetaData>,
    props: parquet::file::properties::ReaderPropertiesPtr,
}

impl FileReader for RemoteFile {
    fn metadata(&self) -> &ParquetMetaData {
        &self.metadata
    }

    fn num_row_groups(&self) -> usize {
        self.metadata.num_row_groups()
    }

    fn get_row_group(&self, i: usize) -> parquet::errors::Result<Box<dyn RowGroupReader + '_>> {
        let rg = self.metadata.row_groups().get(i).ok_or_else(|| parquet::errors::ParquetError::General(format!("no row group {i}")))?;
        Ok(Box::new(parquet::file::serialized_reader::SerializedRowGroupReader::new(
            self.chunks.clone(),
            rg,
            self.metadata.page_index_for_row_group(i),
            self.props.clone(),
        )?))
    }

    fn get_row_iter(
        &self,
        projection: Option<parquet::schema::types::Type>,
    ) -> parquet::errors::Result<parquet::record::reader::RowIter<'_>> {
        parquet::record::reader::RowIter::from_file(projection, self)
    }
}

fn err(e: parquet::errors::ParquetError) -> DataError {
    DataError::Format(format!("GeoParquet: {e}"))
}

/// [`err`] for an error met in `place` (a row group and column), when it is known.
fn err_in(place: Option<&str>, e: parquet::errors::ParquetError) -> DataError {
    match place {
        Some(place) => DataError::Format(format!("GeoParquet: {place}: {e}")),
        None => err(e),
    }
}

/// The reader of a row group's column, its pages checked by [`CheckedPages`] (a page no bigger
/// than the column chunk says).
fn column_reader(rg: &dyn RowGroupReader, col: usize) -> Result<ColumnReader> {
    Ok(column_reader_within(rg, col, u64::MAX)?.0)
}

/// [`column_reader`] refusing any page of more than `limit` bytes too, with what it has seen of
/// the pages ([`PageNote`]).
fn column_reader_within(rg: &dyn RowGroupReader, col: usize, limit: u64) -> Result<(ColumnReader, Arc<PageNote>)> {
    let desc = rg.metadata().column(col).column_descr_ptr();
    let bits = match desc.physical_type() {
        Physical::BOOLEAN => 1,
        Physical::INT32 | Physical::FLOAT | Physical::BYTE_ARRAY => 32,
        Physical::INT64 | Physical::DOUBLE => 64,
        Physical::INT96 => 96,
        Physical::FIXED_LEN_BYTE_ARRAY => u64::try_from(desc.type_length()).unwrap_or(0).max(1) * 8,
    };
    let chunk = rg.metadata().column(col);
    let note = Arc::new(PageNote::default());
    let pages = CheckedPages {
        pages: rg.get_column_page_reader(col).map_err(err)?,
        bits,
        limit,
        // A footer that gives no size, or a negative one, bounds a page by the limit alone.
        chunk_bytes: u64::try_from(chunk.uncompressed_size()).ok().filter(|&n| n > 0),
        delta_listed: [Encoding::DELTA_BYTE_ARRAY, Encoding::DELTA_LENGTH_BYTE_ARRAY].map(|listed| chunk.encodings().any(|e| e == listed)),
        max_def: desc.max_def_level(),
        max_rep: desc.max_rep_level(),
        chunk_values: u64::try_from(chunk.num_values()).ok().filter(|&n| n > 0).unwrap_or(u64::MAX),
        note: note.clone(),
    };
    Ok((parquet::column::reader::get_column_reader(desc, Box::new(pages)), note))
}

/// What a column's [`CheckedPages`] have seen of its data pages, for [`Pace`].
struct PageNote {
    /// The bytes of the data page being read, decoded.
    page: std::sync::atomic::AtomicU64,
    /// The values of that page not yet read or skipped (`0`: the next read fetches a page).
    in_page: std::sync::atomic::AtomicUsize,
    /// While a `skip_records` call may pass over whole pages undecoded: the values of the run it is
    /// asked for that are still to skip ([`NOT_SKIPPING`] outside such a call).
    skip_left: std::sync::atomic::AtomicUsize,
    /// The values of the whole pages that call has passed over.
    passed: std::sync::atomic::AtomicUsize,
    /// The values of the data page last peeked at in such a call.
    peeked: std::sync::atomic::AtomicUsize,
    /// Such a call was stopped at a page it would only have skipped part of.
    stopped: std::sync::atomic::AtomicBool,
    /// What the read's budget has left, as the column's values are charged (everything until a read
    /// says): the decoder's own arrays for a page are held against it.
    left: std::sync::atomic::AtomicU64,
}

/// [`PageNote::skip_left`] when no skip is passing over whole pages.
const NOT_SKIPPING: usize = usize::MAX;

impl Default for PageNote {
    fn default() -> Self {
        Self {
            page: Default::default(),
            in_page: Default::default(),
            skip_left: std::sync::atomic::AtomicUsize::new(NOT_SKIPPING),
            passed: Default::default(),
            peeked: Default::default(),
            stopped: Default::default(),
            left: std::sync::atomic::AtomicU64::new(u64::MAX),
        }
    }
}

/// How many values of a column one `read_records` or `skip_records` call takes. The `parquet`
/// crate builds the values of a DELTA_BYTE_ARRAY page one by one in the call, each up to the page's
/// size, so a call that reads or skips into a page takes no more than the budget left holds at
/// that size, and none that reach past the page; at a page it hasn't seen it takes one value, which
/// fetches the page. A skip that starts at such a page is asked for its whole run instead, so the
/// crate passes over the pages the run covers undecoded (fetching one sets its decoder up, and the
/// decoder sizes arrays from a count the page holds), and [`CheckedPages`] stops it at a page it
/// would only skip part of, which the one-value pace then takes. A column of another encoding
/// skips its whole runs, and its reads take [`BATCH`] values, as before.
struct Pace<'a> {
    watch: Option<(&'a PageNote, &'a Budget)>,
}

impl Pace<'_> {
    /// The values of the next call, of the `left` still to read or skip (more than `0`).
    fn take(&self, left: usize) -> usize {
        let most = left.min(BATCH);
        let Some((note, budget)) = self.watch else { return most };
        let in_page = note.in_page.load(Ordering::Relaxed);
        if in_page == 0 {
            return 1;
        }
        let page = note.page.load(Ordering::Relaxed).max(1);
        let fit = usize::try_from(budget.left.get() / page).unwrap_or(BATCH);
        most.min(in_page).min(fit.max(1))
    }

    /// The values of the next `skip_records` call, of the `left` still to skip (more than `0`): the
    /// whole run unless the column is paced and the call is inside a page, or follows a stop at one.
    fn skip(&self, left: usize) -> usize {
        let Some((note, _)) = self.watch else { return left };
        if note.in_page.load(Ordering::Relaxed) > 0 || note.stopped.swap(false, Ordering::Relaxed) {
            return self.take(left);
        }
        note.passed.store(0, Ordering::Relaxed);
        note.skip_left.store(left, Ordering::Relaxed);
        left
    }

    /// Ends a skip call that [`skip`](Self::skip) asked for, which `result` is the outcome of: the
    /// values it skipped, whether it was stopped at a page it would only have skipped part of (the
    /// whole pages before that count).
    fn skipped(&self, place: Option<&str>, result: parquet::errors::Result<usize>) -> Result<(usize, bool)> {
        let Some((note, _)) = self.watch else { return result.map(|n| (n, false)).map_err(|e| err_in(place, e)) };
        note.skip_left.store(NOT_SKIPPING, Ordering::Relaxed);
        match result {
            Ok(n) => Ok((n, false)),
            Err(_) if note.stopped.load(Ordering::Relaxed) => Ok((note.passed.load(Ordering::Relaxed), true)),
            Err(e) => Err(err_in(place, e)),
        }
    }

    /// Notes that a call read or skipped `values` of the page.
    fn used(&self, values: usize) {
        if let Some((note, _)) = self.watch {
            note.in_page.store(note.in_page.load(Ordering::Relaxed).saturating_sub(values), Ordering::Relaxed);
        }
    }
}

/// A column's pages, a dictionary page refused when it says it holds more values than its bytes
/// can: the `parquet` crate makes room for the count it is told before decoding any (a few bytes
/// of header can ask for tens of gigabytes). A page bigger than the column chunk it belongs to or
/// than `limit`, and a data page in the DELTA_BYTE_ARRAY or DELTA_LENGTH_BYTE_ARRAY encoding that the chunk doesn't list (each encoding on its own), are
/// refused too: the pace of [`read_binary`] rests on both. The size is checked once the crate has
/// the page; a header asking for more than the file holds is still allocated by the crate first.
struct CheckedPages {
    pages: Box<dyn parquet::column::page::PageReader>,
    /// The fewest bits one value of the column takes in a dictionary page (plain encoded).
    bits: u64,
    /// The most bytes of one page the read allows.
    limit: u64,
    /// The bytes the column chunk's footer says its pages hold decoded, when it says.
    chunk_bytes: Option<u64>,
    /// Whether the footer lists the DELTA_BYTE_ARRAY, and the DELTA_LENGTH_BYTE_ARRAY, encoding for the chunk.
    delta_listed: [bool; 2],
    /// The most levels the column's values have, for a version 1 page's level streams.
    max_def: i16,
    max_rep: i16,
    /// The values the column chunk's footer says it holds, when it says (levels included).
    chunk_values: u64,
    note: Arc<PageNote>,
}

impl Iterator for CheckedPages {
    type Item = parquet::errors::Result<parquet::column::page::Page>;

    /// The next page, checked as [`PageReader::get_next_page`](parquet::column::page::PageReader::get_next_page) checks it.
    fn next(&mut self) -> Option<Self::Item> {
        use parquet::column::page::PageReader;
        self.get_next_page().transpose()
    }
}

impl CheckedPages {
    /// Refuses a DELTA_BYTE_ARRAY or DELTA_LENGTH_BYTE_ARRAY data page whose streams say they hold
    /// more values than the page or the column chunk does, or whose arrays the decoder would make
    /// for them (4 bytes a value for each stream, `parquet` 60.2.0 `set_data`) are more than the
    /// budget has left, or can't be read as a stream: the decoders size those arrays from the count
    /// in the stream before decoding a value, and a few bytes can ask for gigabytes. See
    /// [`delta_stream`] for what is checked of a stream.
    fn check_delta(&self, page: &parquet::column::page::Page) -> parquet::errors::Result<()> {
        use parquet::column::page::Page;
        let (buf, num_values, encoding, levels) = match page {
            Page::DataPage { buf, num_values, encoding, def_level_encoding, rep_level_encoding, .. } => {
                if !matches!(encoding, Encoding::DELTA_BYTE_ARRAY | Encoding::DELTA_LENGTH_BYTE_ARRAY) {
                    return Ok(());
                }
                // Each level stream in front of the values, as the crate's `parse_v1_level` sizes it.
                let mut at = 0usize;
                for (max, level_encoding) in [(self.max_rep, rep_level_encoding), (self.max_def, def_level_encoding)] {
                    if max <= 0 {
                        continue;
                    }
                    #[expect(deprecated, reason = "BIT_PACKED levels are deprecated but still read")]
                    let bit_packed = Encoding::BIT_PACKED;
                    let len = if *level_encoding == Encoding::RLE {
                        let prefix = buf.get(at..at.saturating_add(4)).and_then(|b| <[u8; 4]>::try_from(b).ok());
                        let len = prefix.map(u32::from_le_bytes).and_then(|n| usize::try_from(n).ok());
                        len.and_then(|n| n.checked_add(4)).ok_or_else(|| delta_err("its levels are cut off"))?
                    } else if *level_encoding == bit_packed {
                        let bits = u64::from(u64::BITS - u64::try_from(max).unwrap_or(0).leading_zeros());
                        let bytes = (u64::from(*num_values).saturating_mul(bits)).div_ceil(8);
                        usize::try_from(bytes).map_err(|_| delta_err("its levels are cut off"))?
                    } else {
                        return Err(delta_err(&format!("its levels are in the {level_encoding} encoding, which a page can't have")));
                    };
                    at = at.saturating_add(len);
                }
                (buf, *num_values, *encoding, at)
            }
            Page::DataPageV2 { buf, num_values, encoding, def_levels_byte_len, rep_levels_byte_len, .. } => {
                let at = (*def_levels_byte_len as usize).saturating_add(*rep_levels_byte_len as usize);
                (buf, *num_values, *encoding, at)
            }
            Page::DictionaryPage { .. } => return Ok(()),
        };
        if !matches!(encoding, Encoding::DELTA_BYTE_ARRAY | Encoding::DELTA_LENGTH_BYTE_ARRAY) {
            return Ok(());
        }
        let values = buf.get(levels..).ok_or_else(|| delta_err("its levels are longer than the page"))?;
        let delta = encoding == Encoding::DELTA_BYTE_ARRAY;
        let bounds = DeltaBounds {
            page: u64::from(num_values),
            chunk: self.chunk_values,
            left: self.limit.min(self.note.left.load(Ordering::Relaxed)),
            // A DELTA_BYTE_ARRAY page makes an array for its prefix lengths and one for its suffix lengths.
            per_value: if delta { 8 } else { 4 },
        };
        let (count, end) = delta_stream(values, &bounds)?;
        if delta {
            // The prefix lengths are followed by the suffixes, a DELTA_LENGTH_BYTE_ARRAY stream of as many.
            let rest = values.get(end..).ok_or_else(|| delta_err("its prefix lengths run past the page"))?;
            let (suffixes, _) = delta_stream(rest, &bounds)?;
            if suffixes != count {
                return Err(delta_err(&format!("it has {count} prefix lengths and {suffixes} suffixes")));
            }
        }
        Ok(())
    }
}

fn delta_err(what: &str) -> parquet::errors::ParquetError {
    parquet::errors::ParquetError::General(format!("a delta page is refused: {what}"))
}

/// What a delta stream's count may be.
struct DeltaBounds {
    /// The values of the page (levels included).
    page: u64,
    /// The values of the column chunk.
    chunk: u64,
    /// What the budget has left.
    left: u64,
    /// The bytes the decoders make an array of for each value of a page.
    per_value: u64,
}

/// One ULEB128 number at `at` of `data`, moving `at` past it (`None` when it is cut off or longer
/// than the 10 bytes the crate reads: it panics at an 11th).
fn uleb(data: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = *data.get(*at)?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// Reads the layout of the DELTA_BINARY_PACKED stream at the start of `data` as the crate's
/// `DeltaBitPackDecoder` does, without decoding a value: the header (block size, miniblocks per
/// block, count of values, first value), then each block's minimum delta and miniblock bit widths.
/// It returns the count and the offset where the stream ends. It refuses a stream whose count is
/// more than `bounds` allow, whose blocks don't fit in `data`, or that is cut off or not the sizes
/// the format needs: the crate's decoders trust all of that when they size their arrays, and panic
/// on some of it.
fn delta_stream(data: &[u8], bounds: &DeltaBounds) -> parquet::errors::Result<(u64, usize)> {
    let mut at = 0usize;
    let cut = || delta_err("a stream header is cut off or malformed");
    let block = uleb(data, &mut at).ok_or_else(cut)?;
    let mini_blocks = uleb(data, &mut at).ok_or_else(cut)?;
    let count = uleb(data, &mut at).ok_or_else(cut)?;
    uleb(data, &mut at).ok_or_else(cut)?;
    if block == 0
        || !block.is_multiple_of(128)
        || mini_blocks == 0
        || !block.is_multiple_of(mini_blocks)
        || !(block / mini_blocks).is_multiple_of(32)
    {
        return Err(delta_err(&format!("its block of {block} values in {mini_blocks} miniblocks is not one the format allows")));
    }
    if count > bounds.page {
        return Err(delta_err(&format!("a stream says it holds {count} values, more than the page's {}", bounds.page)));
    }
    if count > bounds.chunk {
        return Err(delta_err(&format!("a stream says it holds {count} values, more than the column chunk's {}", bounds.chunk)));
    }
    if count.saturating_mul(bounds.per_value) > bounds.left {
        return Err(delta_err(&format!(
            "a stream says it holds {count} values, whose arrays of {} bytes each are more than the {} bytes the decoded-column budget has left",
            bounds.per_value, bounds.left
        )));
    }
    let per_mini = block / mini_blocks;
    let widths = usize::try_from(mini_blocks).map_err(|_| cut())?;
    let mut left = count.saturating_sub(1);
    while left > 0 {
        let short = || delta_err(&format!("its {count} values need more blocks than its {} bytes can hold", data.len()));
        uleb(data, &mut at).ok_or_else(short)?;
        let end = at.checked_add(widths).ok_or_else(cut)?;
        let bits = data.get(at..end).ok_or_else(short)?;
        at = end;
        let mut packed = 0u64;
        for &width in bits {
            // Miniblocks past the last value have their widths taken as 0, as the crate does.
            let width = if left == 0 { 0 } else { u64::from(width) };
            if width > 32 {
                return Err(delta_err(&format!("a miniblock has a bit width of {width}, more than 32")));
            }
            left = left.saturating_sub(per_mini);
            packed = packed.saturating_add(width.saturating_mul(per_mini) / 8);
        }
        at = usize::try_from(packed).ok().and_then(|n| at.checked_add(n)).ok_or_else(cut)?;
        if at > data.len() {
            return Err(delta_err(&format!("its {count} values need more bytes than the page's {} bytes can hold", data.len())));
        }
    }
    Ok((count, at))
}

impl parquet::column::page::PageReader for CheckedPages {
    /// The next page, or an error for a dictionary page whose count its bytes can't hold.
    fn get_next_page(&mut self) -> parquet::errors::Result<Option<parquet::column::page::Page>> {
        let page = self.pages.get_next_page()?;
        if let Some(parquet::column::page::Page::DictionaryPage { buf, num_values, .. }) = &page
            && u64::from(*num_values).saturating_mul(self.bits) > u64::try_from(buf.len()).unwrap_or(u64::MAX).saturating_mul(8)
        {
            return Err(parquet::errors::ParquetError::General(format!(
                "a dictionary page says it holds {num_values} values, more than its {} bytes can",
                buf.len()
            )));
        }
        if let Some(page) = &page {
            let len = u64::try_from(page.buffer().len()).unwrap_or(u64::MAX);
            if let Some(chunk) = self.chunk_bytes
                && len > chunk
            {
                return Err(parquet::errors::ParquetError::General(format!(
                    "a page holds {len} bytes, more than the {chunk} its column chunk says it holds"
                )));
            }
            if len > self.limit {
                return Err(parquet::errors::ParquetError::General(format!(
                    "a page holds {len} bytes, more than the {} byte decoded-column budget",
                    self.limit
                )));
            }
            if let parquet::column::page::Page::DataPage { encoding, .. } | parquet::column::page::Page::DataPageV2 { encoding, .. } = page
                && let Some(at) = [Encoding::DELTA_BYTE_ARRAY, Encoding::DELTA_LENGTH_BYTE_ARRAY].iter().position(|e| e == encoding)
                && !self.delta_listed.get(at).copied().unwrap_or(false)
            {
                return Err(parquet::errors::ParquetError::General(format!(
                    "a page uses the {encoding} encoding, which its column chunk doesn't list"
                )));
            }
            self.check_delta(page)?;
            if let parquet::column::page::Page::DataPage { num_values, .. } | parquet::column::page::Page::DataPageV2 { num_values, .. } =
                page
            {
                #[cfg(test)]
                if self.delta_listed.contains(&true) {
                    DELTA_PAGES.with(|pages| pages.set(pages.get() + 1));
                }
                self.note.page.store(len, Ordering::Relaxed);
                self.note.in_page.store(usize::try_from(*num_values).unwrap_or(usize::MAX), Ordering::Relaxed);
            }
        }
        Ok(page)
    }

    /// The next page's header, unchecked: a peek decodes no values. While a skip is passing over
    /// whole pages, a data page the skip would only pass over part of is refused (the skip would
    /// set its decoder up first): [`Pace`] takes that page at its own pace.
    fn peek_next_page(&mut self) -> parquet::errors::Result<Option<parquet::column::page::PageMetadata>> {
        let meta = self.pages.peek_next_page()?;
        let left = self.note.skip_left.load(Ordering::Relaxed);
        if left != NOT_SKIPPING
            && let Some(meta) = &meta
        {
            let rows = if meta.is_dict { Some(0) } else { meta.num_rows.or(meta.num_levels) };
            if rows.is_none_or(|rows| rows > left) {
                self.note.stopped.store(true, Ordering::Relaxed);
                return Err(parquet::errors::ParquetError::General("a skip stops at a page it would only partly pass over".into()));
            }
            self.note.peeked.store(rows.unwrap_or(0), Ordering::Relaxed);
        }
        Ok(meta)
    }

    /// Skips the next page unchecked: a skipped page is never decoded.
    fn skip_next_page(&mut self) -> parquet::errors::Result<()> {
        self.pages.skip_next_page()?;
        if self.note.skip_left.load(Ordering::Relaxed) != NOT_SKIPPING {
            let rows = self.note.peeked.swap(0, Ordering::Relaxed);
            self.note.skip_left.store(self.note.skip_left.load(Ordering::Relaxed).saturating_sub(rows), Ordering::Relaxed);
            self.note.passed.store(self.note.passed.load(Ordering::Relaxed).saturating_add(rows), Ordering::Relaxed);
        }
        Ok(())
    }

    /// Whether the next page starts a record, as the column's own pages say.
    fn at_record_boundary(&mut self) -> parquet::errors::Result<bool> {
        self.pages.at_record_boundary()
    }
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

/// Whether a PROJJSON `crs` is OGC:CRS84 (or EPSG:4326's longitude/latitude twin).
fn is_crs84(crs: &serde_json::Value) -> bool {
    let id = crs.get("id");
    let authority = id.and_then(|i| i.get("authority")).and_then(|a| a.as_str()).unwrap_or("");
    let code = id.and_then(|i| i.get("code")).map(|c| c.to_string().trim_matches('"').to_string()).unwrap_or_default();
    (authority.eq_ignore_ascii_case("OGC") && code.eq_ignore_ascii_case("CRS84"))
        || crs.get("name").and_then(|n| n.as_str()).is_some_and(|n| n.contains("CRS84"))
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

/// A numeric column's smallest or largest value in a row group's statistics.
fn number_stat(s: &Statistics, max: bool) -> Option<f64> {
    match s {
        Statistics::Int32(v) => (if max { v.max_opt() } else { v.min_opt() }).map(|x| f64::from(*x)),
        // Past 2^53, the next double out, so the bound still holds every value.
        Statistics::Int64(v) => (if max { v.max_opt() } else { v.min_opt() }).map(|&x| {
            let y = x as f64;
            match (y as i128).cmp(&i128::from(x)) {
                std::cmp::Ordering::Less if max => y.next_up(),
                std::cmp::Ordering::Greater if !max => y.next_down(),
                _ => y,
            }
        }),
        _ => stat(Some(s), max),
    }
}

/// A row group's bounding box from its covering statistics.
fn group_extent(md: &ParquetMetaData, g: usize, c: [usize; 4]) -> Option<Extent> {
    let rg = md.row_groups().get(g)?;
    let s = |i: usize, max: bool| stat(rg.columns().get(*c.get(i)?)?.statistics(), max);
    let e = Extent::new(s(0, false)?, s(1, false)?, s(2, true)?, s(3, true)?);
    (!e.is_empty()).then_some(e)
}

/// Whether `outer` holds all of `inner`.
fn contains(outer: &Extent, inner: &Extent) -> bool {
    outer.min_x <= inner.min_x && outer.min_y <= inner.min_y && outer.max_x >= inner.max_x && outer.max_y >= inner.max_y
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

/// What a top-level column not read as a field holds, for the note saying it was left out: a
/// list, map or struct, or a decimal, geometry, INT96 timestamp or binary value.
fn column_kind(t: &parquet::schema::types::Type) -> &'static str {
    let info = t.get_basic_info();
    let logical = info.logical_type_ref();
    if info.has_repetition() && info.repetition() == parquet::basic::Repetition::REPEATED {
        return "list";
    }
    if t.is_group() {
        return match (logical, info.converted_type()) {
            (Some(LogicalType::List), _) | (_, ConvertedType::LIST) => "list",
            (Some(LogicalType::Map), _) | (_, ConvertedType::MAP | ConvertedType::MAP_KEY_VALUE) => "map",
            _ => "struct",
        };
    }
    match (t.get_physical_type(), logical, info.converted_type()) {
        (_, Some(LogicalType::Decimal(_)), _) | (_, _, ConvertedType::DECIMAL) => "decimal",
        (_, Some(LogicalType::Geometry(_) | LogicalType::Geography(_)), _) => "geometry",
        (Physical::INT96, ..) => "INT96 timestamp",
        _ => "binary",
    }
}

/// The unit and UTC adjustment of a time-only column, retained in its ISO text.
fn clock_kind(c: &ColumnDescPtr) -> Option<(parquet::basic::TimeUnit, bool)> {
    use parquet::basic::TimeUnit;
    match (c.logical_type_ref(), c.converted_type()) {
        (Some(LogicalType::Time(t)), _) => Some((t.unit, t.is_adjusted_to_u_t_c)),
        (None, ConvertedType::TIME_MILLIS) => Some((TimeUnit::MILLIS, true)),
        (None, ConvertedType::TIME_MICROS) => Some((TimeUnit::MICROS, true)),
        _ => None,
    }
}

/// A time without a day is text rather than a date. Its fractional precision is retained.
fn clock_value(n: i64, (unit, utc): (parquet::basic::TimeUnit, bool)) -> Value {
    use parquet::basic::TimeUnit;
    let (scale, width): (i128, usize) = match unit {
        TimeUnit::MILLIS => (1_000_000, 3),
        TimeUnit::MICROS => (1000, 6),
        TimeUnit::NANOS => (1, 9),
    };
    let ns = i128::from(n) * scale;
    if !(0..86_400_000_000_000).contains(&ns) {
        return Value::Null;
    }
    let seconds = ns / 1_000_000_000;
    let mut text = format!("{:02}:{:02}:{:02}", seconds / 3600, seconds / 60 % 60, seconds % 60);
    let fraction = ns % 1_000_000_000;
    if utc {
        // Fixed precision keeps a whole second before its fractions when both carry Z.
        text.push_str(&format!(".{:0width$}Z", fraction / scale));
    } else if fraction != 0 {
        text.push('.');
        text.push_str(format!("{fraction:09}").trim_end_matches('0'));
    }
    Value::Text(text)
}

/// A field type for a top-level column; `None` for types not read (binary, fixed-length).
fn field_type(c: &ColumnDescPtr) -> Option<FieldType> {
    if clock_kind(c).is_some() {
        return Some(FieldType::Text);
    }
    if c.physical_type() == Physical::INT96 {
        return Some(FieldType::DateTime);
    }
    match (c.logical_type_ref(), c.converted_type(), c.physical_type()) {
        (Some(LogicalType::Date), _, Physical::INT32) | (None, ConvertedType::DATE, Physical::INT32) => return Some(FieldType::Date),
        (Some(LogicalType::Timestamp(_)), _, Physical::INT64)
        | (None, ConvertedType::TIMESTAMP_MILLIS | ConvertedType::TIMESTAMP_MICROS, Physical::INT64) => return Some(FieldType::DateTime),
        _ => {}
    }
    Some(match c.physical_type() {
        Physical::BOOLEAN => FieldType::Boolean,
        Physical::INT32 | Physical::INT64 if decimal_scale(c).is_some() => FieldType::Double,
        Physical::INT32 | Physical::INT64 => FieldType::Integer,
        Physical::FLOAT | Physical::DOUBLE => FieldType::Double,
        Physical::BYTE_ARRAY if is_text(c) => FieldType::Text,
        _ => return None,
    })
}

/// Reads the rows `take` marks of a flat column, in order (`None` where a row is null). Runs of
/// rows not taken are skipped without being decoded into values, so a sample of a large row
/// group doesn't hold all of it.
fn read_column<T: PqType>(r: &mut ColumnReaderImpl<T>, take: &[bool], max_def: i16) -> Result<Vec<Option<T::T>>> {
    read_column_with(r, take, max_def, &Pace { watch: None }, None, &mut |_| Ok(()))
}

/// Forgets the longest skip this thread has made, for tests.
#[cfg(test)]
pub(crate) fn reset_largest_skip() {
    LARGEST_SKIP.with(|largest| largest.set(0));
}

/// The most values one `skip_records` call was asked to skip on this thread, for tests.
#[cfg(test)]
pub(crate) fn largest_skip() -> usize {
    LARGEST_SKIP.with(std::cell::Cell::get)
}

#[cfg(test)]
thread_local! {
    /// The most values one `skip_records` call was asked to skip on this thread, for tests.
    static LARGEST_SKIP: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// The DELTA_BYTE_ARRAY data pages the crate fetched (and so set a decoder up for) on this
    /// thread, for tests.
    static DELTA_PAGES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Forgets the delta pages this thread has fetched, for tests.
#[cfg(test)]
pub(crate) fn reset_delta_pages() {
    DELTA_PAGES.with(|pages| pages.set(0));
}

/// The DELTA_BYTE_ARRAY data pages the crate fetched on this thread, for tests.
#[cfg(test)]
pub(crate) fn delta_pages() -> usize {
    DELTA_PAGES.with(std::cell::Cell::get)
}

/// [`read_column`] reading as `pace` says (an error naming `place`) and asking `charge` about each value
/// it keeps, before the caller copies any: a read that passes its budget stops there.
fn read_column_with<T: PqType>(
    r: &mut ColumnReaderImpl<T>,
    take: &[bool],
    max_def: i16,
    pace: &Pace,
    place: Option<&str>,
    charge: &mut dyn FnMut(&T::T) -> Result<()>,
) -> Result<Vec<Option<T::T>>> {
    let wanted = take.iter().filter(|&&t| t).count();
    let mut out: Vec<Option<T::T>> = Vec::with_capacity(wanted.min(1 << 24));
    let mut defs = Vec::new();
    let mut values = Vec::new();
    let mut i = 0;
    while i < take.len() {
        let on = take.get(i).copied().unwrap_or(false);
        let mut j = i;
        while j < take.len() && take.get(j).copied().unwrap_or(false) == on {
            j += 1;
        }
        let mut left = j - i;
        i = j;
        if !on {
            while left > 0 {
                let asked = pace.skip(left);
                #[cfg(test)]
                LARGEST_SKIP.with(|largest| largest.set(largest.get().max(asked)));
                let (skipped, stopped) = pace.skipped(place, r.skip_records(asked))?;
                pace.used(skipped);
                left = left.saturating_sub(skipped);
                if skipped < asked && !stopped {
                    return Ok(out);
                }
            }
            continue;
        }
        while left > 0 {
            defs.clear();
            values.clear();
            let (records, _, levels) = r.read_records(pace.take(left), Some(&mut defs), None, &mut values).map_err(|e| err_in(place, e))?;
            if records == 0 {
                return Ok(out);
            }
            pace.used(records);
            left = left.saturating_sub(records);
            if max_def == 0 {
                for v in values.drain(..) {
                    charge(&v)?;
                    out.push(Some(v));
                }
            } else {
                let mut it = values.drain(..);
                for d in defs.iter().take(levels) {
                    let v = if *d == max_def { it.next() } else { None };
                    if let Some(v) = &v {
                        charge(v)?;
                    }
                    out.push(v);
                }
            }
        }
    }
    Ok(out)
}

/// Native Parquet dates and timestamps, respecting UTC adjustment. Nanoseconds are rounded to the
/// nearest microsecond (cut where that would leave years 1 to 9999); the flag says they were.
/// Legacy converted timestamps are UTC, INT96 is unzoned.
fn temporal_value(c: &ColumnDescPtr, n: i64) -> (Value, bool) {
    use parquet::basic::TimeUnit;
    if field_type(c) == Some(FieldType::Date) {
        return (crate::Date::from_days(n).map(Value::Date).unwrap_or_default(), false);
    }
    let (unit, utc) = match (c.logical_type_ref(), c.converted_type()) {
        (Some(LogicalType::Timestamp(t)), _) => (t.unit, t.is_adjusted_to_u_t_c),
        (None, ConvertedType::TIMESTAMP_MILLIS) => (TimeUnit::MILLIS, true),
        (None, ConvertedType::TIMESTAMP_MICROS) => (TimeUnit::MICROS, true),
        _ => return (Value::Null, false),
    };
    let (rounded, cut, lossy) = match unit {
        TimeUnit::MILLIS => (n.checked_mul(1000), None, false),
        TimeUnit::MICROS => (Some(n), None, false),
        TimeUnit::NANOS => nanos_to_micros(n),
    };
    let at = |v: i64| if utc { crate::DateTime::with_offset(v, 0) } else { crate::DateTime::local(v) };
    match rounded.and_then(at).or_else(|| cut.and_then(at)) {
        Some(t) => (Value::DateTime(t), lossy),
        None => (Value::Null, false),
    }
}

/// Nanoseconds since 1970 as microseconds: rounded to the nearest (halves up), the same cut down
/// for where rounding up would leave the supported years, and whether any were lost.
fn nanos_to_micros(n: i64) -> (Option<i64>, Option<i64>, bool) {
    let (cut, rest) = (n.div_euclid(1000), n.rem_euclid(1000));
    (Some(cut + i64::from(rest >= 500)), Some(cut), rest != 0)
}

/// One decoded column, with invalid non-null dates distinguished from stored nulls. Flags
/// follow sampled row order so only features accepted by the geometry filter are counted.
struct ColumnValues {
    values: Vec<Value>,
    /// Per row: 0, 1 for an invalid date, [`ROUNDED`] for a date-time rounded to the microsecond.
    flags: Vec<u8>,
}
const INVALID: u8 = 1;
const ROUNDED: u8 = 2;
impl ColumnValues {
    /// Returns an accepted feature's value and counts its invalid or rounded date, excluding filtered
    /// rows.
    fn value(&self, row: usize, field: usize, unread: &mut Vec<usize>, rounded: &mut Vec<usize>) -> Value {
        match self.flags.get(row) {
            Some(&INVALID) => super::unread_date(unread, field),
            Some(&ROUNDED) => super::unread_date(rounded, field),
            _ => {}
        }
        self.values.get(row).cloned().unwrap_or_default()
    }
}

/// Reads a field column for the selected rows, keeping date flags for the final filter.
fn read_values(rg: &dyn RowGroupReader, col: usize, take: &[bool], budget: &Budget) -> Result<ColumnValues> {
    let desc = rg.metadata().column(col).column_descr_ptr();
    let max_def = desc.max_def_level();
    let scale = decimal_scale(&desc).map(|s| 10f64.powi(-s));
    // The column's kind, worked out once rather than for each value.
    let clock = clock_kind(&desc);
    let temporal = matches!(field_type(&desc), Some(FieldType::Date | FieldType::DateTime));
    let mut flags = Vec::new();
    let mut int = |v: Option<i64>| {
        flags.push(0u8);
        if let Some(unit) = clock {
            return v
                .map(|n| {
                    let value = clock_value(n, unit);
                    if value.is_null()
                        && let Some(flag) = flags.last_mut()
                    {
                        *flag = INVALID;
                    }
                    value
                })
                .unwrap_or_default();
        }
        if temporal {
            return v
                .map(|i| {
                    let (value, rounded) = temporal_value(&desc, i);
                    if let Some(flag) = flags.last_mut() {
                        *flag = if value.is_null() {
                            INVALID
                        } else if rounded {
                            ROUNDED
                        } else {
                            0
                        };
                    }
                    value
                })
                .unwrap_or_default();
        }
        match (v, scale) {
            (None, _) => Value::Null,
            (Some(i), Some(f)) => Value::Float(i as f64 * f),
            (Some(i), None) => Value::Int(i),
        }
    };
    let values = if let Some(binary) = read_binary(rg, col, take, budget, text_len)? {
        binary.into_iter().map(|v| v.map(|b| Value::Text(String::from_utf8_lossy(b.data()).into_owned())).unwrap_or_default()).collect()
    } else {
        match column_reader(rg, col)? {
            ColumnReader::BoolColumnReader(mut r) => {
                read_column(&mut r, take, max_def)?.into_iter().map(|v| v.map(Value::Bool).unwrap_or_default()).collect()
            }
            ColumnReader::Int32ColumnReader(mut r) => {
                read_column(&mut r, take, max_def)?.into_iter().map(|v| int(v.map(i64::from))).collect()
            }
            ColumnReader::Int64ColumnReader(mut r) => read_column(&mut r, take, max_def)?.into_iter().map(int).collect(),
            ColumnReader::Int96ColumnReader(mut r) => read_column(&mut r, take, max_def)?
                .into_iter()
                .map(|v| {
                    flags.push(0u8);
                    v.map(|v| {
                        let words = v.data();
                        let ns = u64::from(words.first().copied().unwrap_or(0)) | (u64::from(words.get(1).copied().unwrap_or(0)) << 32);
                        let day = i64::from(words.get(2).copied().unwrap_or(0)) - 2_440_588;
                        let (rounded, cut, lossy) = nanos_to_micros(ns as i64);
                        let at = |us: Option<i64>| day.checked_mul(86_400_000_000)?.checked_add(us?).and_then(crate::DateTime::local);
                        let t = if ns < 86_400_000_000_000 { at(rounded).or_else(|| at(cut)) } else { None };
                        if let Some(flag) = flags.last_mut() {
                            *flag = if t.is_none() {
                                INVALID
                            } else if lossy {
                                ROUNDED
                            } else {
                                0
                            };
                        }
                        t.map(Value::DateTime).unwrap_or_default()
                    })
                    .unwrap_or_default()
                })
                .collect(),
            ColumnReader::FloatColumnReader(mut r) => {
                read_column(&mut r, take, max_def)?.into_iter().map(|v| v.map(|f| Value::Float(f64::from(f))).unwrap_or_default()).collect()
            }
            ColumnReader::DoubleColumnReader(mut r) => {
                read_column(&mut r, take, max_def)?.into_iter().map(|v| v.map(Value::Float).unwrap_or_default()).collect()
            }
            _ => Vec::new(),
        }
    };
    Ok(ColumnValues { values, flags })
}

/// Longest plain-encoded binary dictionary entry without copying values. Validate lengths
/// and count against the page bytes before using them to bound expansion into owned rows.
/// Checks Cancel between batches of dictionary entries.
fn binary_dictionary_max(bytes: &[u8], count: u32) -> Result<u64> {
    let invalid = || DataError::Format("GeoParquet: invalid binary dictionary".into());
    if u64::from(count) > bytes.len() as u64 / 4 {
        return Err(invalid());
    }
    let mut at = 0usize;
    let mut longest = 0u64;
    for i in 0..count {
        if (i as usize).is_multiple_of(BATCH) {
            crate::progress::check()?;
        }
        let length =
            bytes.get(at..at.saturating_add(4)).and_then(|b| <[u8; 4]>::try_from(b).ok()).map(u32::from_le_bytes).ok_or_else(invalid)?;
        at = at.checked_add(4).and_then(|n| n.checked_add(length as usize)).filter(|&n| n <= bytes.len()).ok_or_else(invalid)?;
        longest = longest.max(u64::from(length));
    }
    Ok(longest)
}

/// Reads a binary column (the WKB geometry), for the rows `take` marks, in order, within `budget`.
fn read_bytes(rg: &dyn RowGroupReader, col: usize, take: &[bool], budget: &Budget) -> Result<Vec<Option<Vec<u8>>>> {
    match read_binary(rg, col, take, budget, <[u8]>::len)? {
        Some(values) => Ok(values.into_iter().map(|v| v.map(|b| b.data().to_vec())).collect()),
        None => Err(DataError::Format("GeoParquet: the geometry column is not binary".into())),
    }
}

/// The bytes `bytes` takes as a `String` made by [`String::from_utf8_lossy`], counted without
/// building it: valid text keeps its length, and each invalid sequence becomes the 3 bytes of
/// U+FFFD, which can be more than it held.
fn text_len(bytes: &[u8]) -> usize {
    bytes.utf8_chunks().map(|c| c.valid().len() + if c.invalid().is_empty() { 0 } else { 3 }).sum()
}

/// The rows `take` marks of a BYTE_ARRAY column, charging `budget` for each value as it is read,
/// as `size` counts what the caller makes of its bytes, before any is copied (`None` for a column
/// of another type). Only the rows taken count, not the
/// column chunk's size; a page of more than the budget is refused, as the `parquet` crate makes
/// room for a whole page. A column with DELTA_BYTE_ARRAY pages, whose values the decoder builds
/// one by one, or DELTA_LENGTH_BYTE_ARRAY pages, whose length arrays are made as a page is
/// fetched, is read at the [`Pace`] its page size and the budget left allow.
fn read_binary(
    rg: &dyn RowGroupReader,
    col: usize,
    take: &[bool],
    budget: &Budget,
    size: fn(&[u8]) -> usize,
) -> Result<Option<Vec<Option<ByteArray>>>> {
    let chunk = rg.metadata().column(col);
    if chunk.column_descr().physical_type() != Physical::BYTE_ARRAY {
        return Ok(None);
    }
    let name = chunk.column_path().string();
    let (reader, note) = column_reader_within(rg, col, budget.limit)?;
    let ColumnReader::ByteArrayColumnReader(mut r) = reader else { return Ok(None) };
    // DELTA_LENGTH_BYTE_ARRAY pages are paced too: the decoder makes an array of a page's count
    // when it is fetched, which must see what the values before it were charged.
    let delta = chunk.encodings().any(|e| matches!(e, Encoding::DELTA_BYTE_ARRAY | Encoding::DELTA_LENGTH_BYTE_ARRAY));
    let pace = Pace { watch: delta.then_some((&*note, budget)) };
    let place = format!("row group {}, column '{name}'", budget.group);
    note.left.store(budget.left.get(), Ordering::Relaxed);
    read_column_with(&mut r, take, chunk.column_descr().max_def_level(), &pace, Some(&place), &mut |b: &ByteArray| {
        budget.charge(size(b.data()), &name)?;
        note.left.store(budget.left.get(), Ordering::Relaxed);
        Ok(())
    })
    .map(Some)
}

/// Reads every one of `n` rows of a float or double column as doubles (the covering).
fn read_f64(rg: &dyn RowGroupReader, col: usize, n: usize) -> Result<Vec<Option<f64>>> {
    let max_def = rg.metadata().column(col).column_descr().max_def_level();
    let all = vec![true; n];
    match column_reader(rg, col)? {
        ColumnReader::FloatColumnReader(mut r) => Ok(read_column(&mut r, &all, max_def)?.into_iter().map(|v| v.map(f64::from)).collect()),
        ColumnReader::DoubleColumnReader(mut r) => read_column(&mut r, &all, max_def),
        _ => Ok(Vec::new()),
    }
}

/// Points with a vector each (patch embeddings, such as Earth Index's), read from a GeoParquet by
/// its web address: the points in an extent, and their vectors.
#[derive(Clone, Debug, Default)]
pub struct Vectors {
    /// Each point's longitude and latitude.
    pub points: Vec<(f64, f64)>,
    /// Values a vector.
    pub dims: usize,
    /// The vectors, one after another (`points.len() × dims` values).
    pub values: Vec<f32>,
}

/// The most values one vector may have.
const MAX_DIMS: usize = 8192;
/// The most vector values one read keeps (in all; 4 bytes each).
const MAX_VECTOR_VALUES: usize = if cfg!(target_arch = "wasm32") { 8 << 20 } else { 32 << 20 };

/// Reads the points of a GeoParquet in `extent` (longitude/latitude) with the vectors of a list
/// column (`column`, such as `embedding`), from a file by its web address. The geometry of every
/// row group is read (it has no bbox covering), then the vectors of those holding points in the
/// extent: at most `max_groups` of them and `max_points` points, or an error saying how many the
/// extent needs. Fails with [`remote::NEED`] until the bytes it needs are fetched (run it with
/// [`remote::drive`]).
pub fn read_vectors(file: &Arc<Remote>, extent: &Extent, column: &str, max_groups: usize, max_points: usize) -> Result<Vectors> {
    let reader = remote_reader(file)?;
    let md = reader.metadata();
    let schema = md.file_metadata().schema_descr();
    let leaf = |pred: &dyn Fn(&ColumnDescPtr) -> bool| (0..schema.num_columns()).find(|&i| pred(&schema.column(i)));
    let geom = leaf(&|c| c.path().string() == "geometry" && c.physical_type() == Physical::BYTE_ARRAY)
        .ok_or_else(|| DataError::Format("GeoParquet: no geometry column".into()))?;
    let prefix = format!("{column}.");
    let vec_col = leaf(&|c| {
        c.path().string().starts_with(&prefix) && c.max_rep_level() == 1 && matches!(c.physical_type(), Physical::FLOAT | Physical::DOUBLE)
    })
    .ok_or_else(|| DataError::Format(format!("GeoParquet: no list column '{column}' of numbers")))?;
    let groups: Vec<usize> = (0..md.num_row_groups()).collect();
    if groups.iter().map(|&g| chunks_size(md, g, &[geom])).sum::<u64>() > remote::READ_BUDGET {
        return Err(DataError::Format("GeoParquet: its geometry is too large to read in place".into()));
    }
    if !want_chunks(file, md, &groups, &[geom])? {
        return Err(remote::need());
    }
    // The points in the extent, by row group.
    let decoded = std::cell::LazyCell::new(max_decoded_bytes);
    let mut found: Vec<(usize, Vec<(usize, (f64, f64))>)> = Vec::new();
    let mut total = 0usize;
    let mut hit = 0usize;
    for &g in &groups {
        let rg = reader.get_row_group(g).map_err(err)?;
        let rows = usize::try_from(rg.metadata().num_rows()).unwrap_or(0).min(MAX_GROUP_ROWS);
        let geoms = read_bytes(rg.as_ref(), geom, &vec![true; rows], &Budget::new(*decoded, g))?;
        let inside: Vec<(usize, (f64, f64))> = geoms
            .iter()
            .enumerate()
            .filter_map(|(r, b)| match b.as_deref().and_then(wkb::decode) {
                Some(crate::Geometry::Point(c)) if extent.contains(c) => Some((r, (c.x, c.y))),
                _ => None,
            })
            .collect();
        if !inside.is_empty() {
            total = total.saturating_add(inside.len());
            hit += 1;
            // Past the limits the points are only counted, for the error.
            if total <= max_points && hit <= max_groups {
                found.push((g, inside));
            }
        }
    }
    if hit > max_groups || total > max_points {
        return Err(DataError::Format(format!("the area holds {total} points in {hit} row groups, more than are read at once: zoom in")));
    }
    let used: Vec<usize> = found.iter().map(|(g, _)| *g).collect();
    if !want_chunks(file, md, &used, &[vec_col])? {
        return Err(remote::need());
    }
    let mut out = Vectors::default();
    for (g, inside) in &found {
        let rg = reader.get_row_group(*g).map_err(err)?;
        let rows = usize::try_from(rg.metadata().num_rows()).unwrap_or(0).min(MAX_GROUP_ROWS);
        let mut take = vec![false; rows];
        for (r, _) in inside {
            if let Some(t) = take.get_mut(*r) {
                *t = true;
            }
        }
        let lists = read_lists(rg.as_ref(), vec_col, &take)?;
        for (r, p) in inside {
            let Some(v) = lists.get(*r) else { continue };
            if out.dims == 0 && !v.is_empty() && v.len() <= MAX_DIMS {
                out.dims = v.len();
            }
            if v.len() == out.dims && out.dims > 0 {
                if out.values.len().saturating_add(v.len()) > MAX_VECTOR_VALUES {
                    return Err(DataError::Format(format!(
                        "the area's vectors hold more than {MAX_VECTOR_VALUES} values, more than are read at once: zoom in"
                    )));
                }
                out.points.push(*p);
                out.values.extend_from_slice(v);
            }
        }
    }
    Ok(out)
}

/// Reads a list column of numbers over the rows of a row group (`take.len()` of them): each row's
/// values for the rows `take` marks, empty for the others (whose values are passed over, not
/// kept) and for a null or empty list.
fn read_lists(rg: &dyn RowGroupReader, col: usize, take: &[bool]) -> Result<Vec<Vec<f32>>> {
    let n = take.len();
    let desc = rg.metadata().column(col).column_descr_ptr();
    let max_def = desc.max_def_level();
    let mut out: Vec<Vec<f32>> = Vec::with_capacity(n.min(1 << 20));
    let (mut defs, mut reps) = (Vec::new(), Vec::new());
    // Values kept over the row group: bounded, so a crafted file can't fill memory.
    let mut budget = 1usize << 24;
    macro_rules! lists {
        ($r:expr, $conv:expr) => {{
            let mut values = Vec::new();
            let mut left = n;
            while left > 0 {
                defs.clear();
                reps.clear();
                values.clear();
                let (records, _, levels) = $r.read_records(left.min(BATCH), Some(&mut defs), Some(&mut reps), &mut values).map_err(err)?;
                if records == 0 {
                    break;
                }
                left = left.saturating_sub(records);
                let mut it = values.drain(..);
                let mut kept = false;
                for (d, r) in defs.iter().zip(&reps).take(levels) {
                    if *r == 0 {
                        kept = take.get(out.len()).copied().unwrap_or(false);
                        out.push(Vec::new());
                    }
                    if *d == max_def
                        && let Some(v) = it.next()
                        && kept
                        && let Some(last) = out.last_mut()
                    {
                        budget = budget.checked_sub(1).ok_or_else(|| DataError::Format("GeoParquet: lists too long to read".into()))?;
                        last.push($conv(v));
                    }
                }
            }
        }};
    }
    match column_reader(rg, col)? {
        ColumnReader::FloatColumnReader(mut r) => lists!(r, |v: f32| v),
        ColumnReader::DoubleColumnReader(mut r) => lists!(r, |v: f64| v as f32),
        _ => return Err(DataError::Format("GeoParquet: the list column holds no numbers".into())),
    }
    Ok(out)
}

/// Rows per row group written.
const WRITE_GROUP_ROWS: usize = 65_536;

/// A column name not taken by a field (`base`, else `base_1`, `base_2`, …).
fn free_name(fields: &[Field], base: &str) -> String {
    let taken = |n: &str| fields.iter().any(|f| f.name.eq_ignore_ascii_case(n));
    if !taken(base) {
        return base.to_string();
    }
    (1..).map(|i| format!("{base}_{i}")).find(|n| !taken(n)).unwrap_or_else(|| base.to_string())
}

/// The GeoParquet name of a geometry's type, as WKB encodes it.
fn wkb_type_name(g: &crate::Geometry) -> &'static str {
    match g {
        crate::Geometry::Point(_) => "Point",
        crate::Geometry::MultiPoint(_) => "MultiPoint",
        crate::Geometry::Line(_) => "MultiLineString",
        crate::Geometry::Polygon(_) => "MultiPolygon",
    }
}

/// Writes a dataset as GeoParquet 1.1 (WGS 84 longitude/latitude, WKB geometry, Zstd level 9), with a
/// `bbox` covering column so readers can skip rows and row groups by extent. A table without
/// geometry is written as plain Parquet.
pub fn write(ds: &crate::Dataset) -> Result<Vec<u8>> {
    write_in(ds, Crs::LonLat)
}

/// As [`write`], for a dataset whose coordinates are in `crs`, which the geometry column's `crs`
/// says ([`projjson`]); refused for a system it can't say.
pub fn write_in(ds: &crate::Dataset, crs: Crs) -> Result<Vec<u8>> {
    write_features(ds.geometry_type, &ds.fields, crs, Vec::new(), &mut |add| add(&ds.features)).map(|(bytes, _, _)| bytes)
}

/// The GeoParquet `crs` (PROJJSON) of a system other than longitude/latitude, which GeoParquet
/// takes when a column has none. Only a system with an EPSG code: its PROJJSON carries the code,
/// which every release of RustGIS reads ([`crs_code`]), where one without would be read as
/// longitude/latitude. `None` for one without a code, or one terrio-crs can't write.
pub(crate) fn projjson(crs: Crs) -> Option<serde_json::Value> {
    let code = u32::from(crs.epsg());
    if code == 0 || crs == Crs::LonLat {
        return None;
    }
    let text = terrio_crs::SpatialRef::from_epsg(code).ok()?.to_projjson().ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    (crs_code(Some(&value)) == Some(code)).then_some(value)
}

/// Why a GeoParquet can't be written in `crs` ([`projjson`] has none for it).
pub(crate) fn projjson_refusal(crs: Crs) -> String {
    match crs.epsg() {
        0 => "it has no EPSG code; give the system as an EPSG code".to_string(),
        code => format!("RustGIS can't write a PROJJSON definition of EPSG:{code}"),
    }
}

/// An Integer column's value, `None` for one it can't hold: a whole float fits; a fraction, a
/// float out of range or text that isn't a whole number would lose its value.
fn int_cell(value: &Value) -> Option<i64> {
    match value {
        Value::Int(x) => Some(*x),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::Float(x) if x.fract() == 0.0 && x.abs() < 9.0e18 => Some(*x as i64),
        Value::Text(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// A Boolean column's value, `None` for one it can't hold (text, a float).
fn bool_cell(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(b) => Some(*b),
        Value::Int(x) => Some(*x != 0),
        _ => None,
    }
}

/// A timestamp column has one zone policy. A mixed zoned/unzoned column is written as ISO
/// text so no assumption is invented; a wholly zoned column is normalized to UTC.
#[derive(Default)]
struct DateLayout {
    modes: Vec<u8>,
    offsets: Vec<usize>,
    counts: Vec<usize>,
}
impl DateLayout {
    /// Accumulates the column's zone policy without keeping any rows; the export feed is replayable.
    fn add(&mut self, fields: &[Field], rows: &[Feature]) {
        self.modes.resize(fields.len(), 0);
        self.offsets.resize(fields.len(), 0);
        self.counts.resize(fields.len(), 0);
        for row in rows {
            for (k, _f) in fields.iter().enumerate().filter(|(_, f)| f.ty == FieldType::DateTime) {
                if let Some(t) = row.attrs.get(k).and_then(datetime_cell) {
                    self.modes[k] |= if t.offset().is_some() { 2 } else { 1 };
                    self.offsets[k] += usize::from(t.offset().is_some_and(|o| o != 0));
                    self.counts[k] += 1;
                }
            }
        }
    }
    /// Counts each representation change and names its fields for the user-facing export notice,
    /// each after `target: ` (nothing before it when `target` is empty: the caller adds the path).
    /// An offset left out is a value lost; ISO text keeps the value.
    fn notes(&self, fields: &[Field], target: &str) -> Vec<crate::loss::Note> {
        let target: String = target.chars().map(crate::io::dbf_notice_char).collect();
        let target = if target.is_empty() { String::new() } else { format!("{target}: ") };
        let mut notes = Vec::new();
        let utc: Vec<usize> = self.offsets.iter().enumerate().map(|(k, &n)| if self.modes.get(k) == Some(&2) { n } else { 0 }).collect();
        let text: Vec<usize> = self.counts.iter().enumerate().map(|(k, &n)| if self.modes.get(k) == Some(&3) { n } else { 0 }).collect();
        for (counts, what, lost) in [
            (&utc, "date-times were written in UTC without their original offsets", true),
            (&text, "date-times were written as ISO text because their column mixes zoned and unzoned values", false),
        ] {
            let n: usize = counts.iter().sum();
            if n > 0 {
                let text = format!("{target}{n} {what}, in {}", crate::io::named_fields(fields, counts));
                notes.push(if lost {
                    crate::loss::Note::loss(crate::loss::LossCode::DroppedValue, text)
                } else {
                    crate::loss::Note::plain(text)
                });
            }
        }
        notes
    }
}

/// Values accepted by a native timestamp column; a Date is its unzoned midnight.
fn datetime_cell(v: &Value) -> Option<crate::DateTime> {
    match v {
        Value::DateTime(t) => Some(*t),
        Value::Date(d) => Some(crate::DateTime::from_date(*d)),
        Value::Text(s) => crate::DateTime::parse(s),
        _ => None,
    }
}

/// Values accepted by a native date column, without dropping a time or a zone.
fn date_cell(v: &Value) -> Option<crate::Date> {
    match v {
        Value::Date(d) => Some(*d),
        Value::DateTime(t) if t.is_date() => Some(t.date()),
        Value::Text(s) => crate::DateTime::parse(s).filter(|t| t.is_date()).map(|t| t.date()),
        _ => None,
    }
}

/// Whether the GeoParquet writer writes `value` as null in a column of type `ty` although it
/// isn't null (in a Double column: text that isn't a number, NaN or an infinity).
fn unfit(ty: FieldType, value: &Value) -> bool {
    !value.is_null()
        && match ty {
            FieldType::Integer => int_cell(value).is_none(),
            FieldType::Double => value.as_f64().is_none(),
            FieldType::Boolean => bool_cell(value).is_none(),
            FieldType::Date => date_cell(value).is_none(),
            FieldType::DateTime => datetime_cell(value).is_none(),
            FieldType::Text => false,
        }
}

/// The notice for values a GeoParquet export wrote as null because their column's type couldn't
/// hold them, given the count per field (as `write_features` returns); `None` when there are
/// none. Names the first five fields.
pub fn unfit_note(fields: &[Field], counts: &[usize], target: &str) -> Option<crate::loss::Note> {
    let total = counts.iter().fold(0usize, |a, n| a.saturating_add(*n));
    if total == 0 {
        return None;
    }
    let named: Vec<&str> = fields.iter().zip(counts).filter(|(_, n)| **n > 0).map(|(f, _)| f.name.as_str()).collect();
    // Names and the target flattened and bounded as the Shapefile notices' are, on one line.
    let list: Vec<String> = named.iter().take(5).map(|n| format!("'{}'", crate::io::dbf_notice_name(n))).collect();
    let more = if named.len() > 5 { format!(" and {} more", named.len() - 5) } else { String::new() };
    let what = if total == 1 {
        "1 value couldn't be written as its column's type and was".to_string()
    } else {
        format!("{total} values couldn't be written as their columns' types and were")
    };
    let target: String = target.chars().map(crate::io::dbf_notice_char).collect();
    Some(crate::loss::Note::loss(
        crate::loss::LossCode::DroppedValue,
        format!("{target}: {what} written as null, in {}{more}", list.join(", ")),
    ))
}

/// Export notices for the whole of `ds`: unfit values and timestamp representation changes.
/// Cancellation omits notices; the following writer's check returns the cancellation error.
pub fn values_note(ds: &crate::Dataset, target: &str) -> Option<crate::loss::Note> {
    let mut counts = vec![0usize; ds.fields.len()];
    let mut layout = DateLayout::default();
    for chunk in ds.features.chunks(BATCH) {
        crate::progress::check().ok()?;
        layout.add(&ds.fields, chunk);
        for row in chunk {
            crate::progress::check().ok()?;
            for ((field, value), n) in ds.fields.iter().zip(&row.attrs).zip(counts.iter_mut()) {
                if unfit(field.ty, value) {
                    *n = n.saturating_add(1);
                }
            }
        }
    }
    // One notice: the values written as null, then how date-times are written; coded when either
    // loses a value.
    let said: Vec<crate::loss::Note> =
        unfit_note(&ds.fields, &counts, target).into_iter().chain(layout.notes(&ds.fields, target)).collect();
    let loss = said.iter().find_map(|n| n.loss);
    (!said.is_empty()).then(|| {
        let text = said.iter().map(|n| n.text.as_str()).collect::<Vec<_>>().join("; ");
        match loss {
            Some(code) => crate::loss::Note::loss(code, text),
            None => crate::loss::Note::plain(text),
        }
    })
}

/// Writes GeoParquet to `out`: the features `feed` hands to the function it is given, a run at
/// a time, their attributes named by `fields`. Each run is written as row groups of at most
/// `WRITE_GROUP_ROWS` rows, so only one run is held (a run's last group can be smaller, and a
/// streamed export's groups end with its chunks); the geometry types and bounding box the `geo`
/// metadata gives are gathered as the rows go and written with the footer.
/// Timestamp columns require replaying the feed before writing, to select one Parquet timezone
/// flag per column without buffering the source. That pass checks Cancel but counts no writes.
/// Returns the writer, the values each field couldn't hold, and the notes on date-times, which
/// name no file: the caller says the path.
pub(crate) fn write_features<W: std::io::Write + Send>(
    geometry_type: GeometryType,
    fields: &[Field],
    crs: Crs,
    out: W,
    feed: &mut dyn FnMut(&mut dyn FnMut(&[Feature]) -> Result<()>) -> Result<()>,
) -> Result<(W, Vec<usize>, Vec<crate::loss::Note>)> {
    use parquet::basic::{Compression, Encoding, Repetition, ZstdLevel};
    use parquet::data_type::{BoolType, ByteArrayType, DoubleType, Int32Type, Int64Type};
    use parquet::file::metadata::KeyValue;
    use parquet::file::properties::WriterProperties;
    use parquet::file::writer::SerializedFileWriter;
    use parquet::schema::types::{ColumnPath, Type};

    let mut layout = DateLayout::default();
    if fields.iter().any(|f| f.ty == FieldType::DateTime) {
        feed(&mut |rows| {
            for chunk in rows.chunks(BATCH) {
                crate::progress::check()?;
                layout.add(fields, chunk);
            }
            Ok(())
        })?;
    }
    let has_geom = geometry_type != GeometryType::None;
    // Before anything is written: a system the file couldn't say would be read as degrees.
    let crs_json = match crs {
        Crs::LonLat => None,
        _ if !has_geom => None,
        other => Some(
            projjson(other).ok_or_else(|| DataError::Unsupported(format!(".parquet in {}: {}", other.label(), projjson_refusal(other))))?,
        ),
    };
    let geom_name = free_name(fields, "geometry");
    let bbox_name = free_name(fields, "bbox");
    let prim = |name: &str, t: Physical, rep: Repetition, logical: Option<LogicalType>| {
        Type::primitive_type_builder(name, t).with_repetition(rep).with_logical_type(logical).build().map(Arc::new).map_err(err)
    };
    let mut cols = Vec::new();
    if has_geom {
        cols.push(prim(&geom_name, Physical::BYTE_ARRAY, Repetition::OPTIONAL, None)?);
        let corners = ["xmin", "ymin", "xmax", "ymax"]
            .iter()
            .map(|k| prim(k, Physical::DOUBLE, Repetition::REQUIRED, None))
            .collect::<Result<Vec<_>>>()?;
        cols.push(Arc::new(
            Type::group_type_builder(&bbox_name).with_repetition(Repetition::OPTIONAL).with_fields(corners).build().map_err(err)?,
        ));
    }
    for (k, f) in fields.iter().enumerate() {
        cols.push(match f.ty {
            FieldType::Integer => prim(&f.name, Physical::INT64, Repetition::OPTIONAL, None)?,
            FieldType::Double => prim(&f.name, Physical::DOUBLE, Repetition::OPTIONAL, None)?,
            FieldType::Boolean => prim(&f.name, Physical::BOOLEAN, Repetition::OPTIONAL, None)?,
            FieldType::Date => prim(&f.name, Physical::INT32, Repetition::OPTIONAL, Some(LogicalType::Date))?,
            FieldType::DateTime if layout.modes.get(k) != Some(&3) => prim(
                &f.name,
                Physical::INT64,
                Repetition::OPTIONAL,
                Some(LogicalType::timestamp(layout.modes.get(k) == Some(&2), parquet::basic::TimeUnit::MICROS)),
            )?,
            FieldType::Text | FieldType::DateTime => prim(&f.name, Physical::BYTE_ARRAY, Repetition::OPTIONAL, Some(LogicalType::String))?,
        });
    }
    let schema = Arc::new(Type::group_type_builder("schema").with_fields(cols).build().map_err(err)?);

    let mut props = WriterProperties::builder()
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(9).map_err(err)?))
        .set_created_by("RustGIS".to_string());
    if has_geom {
        // Keep the covering for range reads; split its doubles into byte streams so Zstd can
        // compress their shared exponent bytes without rounding any coordinate values.
        for corner in ["xmin", "ymin", "xmax", "ymax"] {
            let path = ColumnPath::new(vec![bbox_name.clone(), corner.to_string()]);
            props = props.set_column_dictionary_enabled(path.clone(), false).set_column_encoding(path, Encoding::BYTE_STREAM_SPLIT);
        }
    }
    let mut w = SerializedFileWriter::new(out, schema, Arc::new(props.build())).map_err(err)?;

    /// The row group's next column writer; an error should the schema end first.
    fn next_column<'a, W: std::io::Write + Send>(
        rg: &'a mut parquet::file::writer::SerializedRowGroupWriter<'_, W>,
    ) -> Result<parquet::file::writer::SerializedColumnWriter<'a>> {
        rg.next_column().map_err(err)?.ok_or_else(|| DataError::Format("GeoParquet: the schema ended early".into()))
    }
    /// The present values of an optional column and its definition levels.
    fn levels<T>(values: impl Iterator<Item = Option<T>>) -> (Vec<T>, Vec<i16>) {
        let (mut vals, mut defs) = (Vec::new(), Vec::new());
        for v in values {
            defs.push(i16::from(v.is_some()));
            vals.extend(v);
        }
        (vals, defs)
    }
    let mut types: Vec<&'static str> = Vec::new();
    let mut extent = Extent::EMPTY;
    // Per field, the values written as null because its type couldn't hold them.
    let mut unfit_counts = vec![0usize; fields.len()];
    feed(&mut |features| {
        for rows in features.chunks(WRITE_GROUP_ROWS) {
            crate::progress::step(rows.len())?;
            for g in rows.iter().filter_map(|f| f.geometry.as_ref()) {
                let t = wkb_type_name(g);
                if !types.contains(&t) {
                    types.push(t);
                }
                extent = extent.union(&g.extent());
            }
            let mut rg = w.next_row_group().map_err(err)?;
            if has_geom {
                let (vals, defs) = levels(rows.iter().map(|f| f.geometry.as_ref().map(|g| ByteArray::from(wkb::encode(g)))));
                let mut c = next_column(&mut rg)?;
                c.typed::<ByteArrayType>().write_batch(&vals, Some(&defs), None).map_err(err)?;
                c.close().map_err(err)?;
                // A feature's bounding box, `None` without geometry (or with an empty one).
                let boxes: Vec<Option<Extent>> =
                    rows.iter().map(|f| f.geometry.as_ref().map(|g| g.extent()).filter(|e| !e.is_empty())).collect();
                for corner in 0..4 {
                    let (vals, defs) = levels(boxes.iter().map(|b| {
                        b.map(|e| match corner {
                            0 => e.min_x,
                            1 => e.min_y,
                            2 => e.max_x,
                            _ => e.max_y,
                        })
                    }));
                    let mut c = next_column(&mut rg)?;
                    c.typed::<DoubleType>().write_batch(&vals, Some(&defs), None).map_err(err)?;
                    c.close().map_err(err)?;
                }
            }
            for (i, f) in fields.iter().enumerate() {
                let attr = |r: &Feature| r.attrs.get(i).cloned().unwrap_or(Value::Null);
                // A value the column's type can't hold is written as null, and counted.
                if let Some(n) = unfit_counts.get_mut(i) {
                    *n = n.saturating_add(rows.iter().filter(|r| r.attrs.get(i).is_some_and(|v| unfit(f.ty, v))).count());
                }
                let mut c = next_column(&mut rg)?;
                match f.ty {
                    FieldType::Integer => {
                        let (v, d) = levels(rows.iter().map(|r| int_cell(&attr(r))));
                        c.typed::<Int64Type>().write_batch(&v, Some(&d), None).map_err(err)?;
                    }
                    FieldType::Double => {
                        let (v, d) = levels(rows.iter().map(|r| attr(r).as_f64()));
                        c.typed::<DoubleType>().write_batch(&v, Some(&d), None).map_err(err)?;
                    }
                    FieldType::Boolean => {
                        let (v, d) = levels(rows.iter().map(|r| bool_cell(&attr(r))));
                        c.typed::<BoolType>().write_batch(&v, Some(&d), None).map_err(err)?;
                    }
                    FieldType::Date => {
                        let (v, d) = levels(rows.iter().map(|r| date_cell(&attr(r)).map(crate::Date::days)));
                        c.typed::<Int32Type>().write_batch(&v, Some(&d), None).map_err(err)?;
                    }
                    FieldType::DateTime if layout.modes.get(i) != Some(&3) => {
                        let (v, d) = levels(rows.iter().map(|r| datetime_cell(&attr(r)).map(crate::DateTime::micros)));
                        c.typed::<Int64Type>().write_batch(&v, Some(&d), None).map_err(err)?;
                    }
                    FieldType::Text | FieldType::DateTime => {
                        let (v, d) = levels(rows.iter().map(|r| match attr(r) {
                            Value::Null => None,
                            v if unfit(f.ty, &v) => None,
                            v => Some(ByteArray::from(v.to_string().into_bytes())),
                        }));
                        c.typed::<ByteArrayType>().write_batch(&v, Some(&d), None).map_err(err)?;
                    }
                }
                c.close().map_err(err)?;
            }
            rg.close().map_err(err)?;
        }
        Ok(())
    })?;
    // A run cancelled during the last row group gets no footer either.
    crate::progress::check()?;
    if has_geom {
        types.sort_unstable();
        let e = extent;
        let mut col = serde_json::json!({
            "encoding": "WKB",
            "geometry_types": types,
            "covering": {"bbox": {
                "xmin": [bbox_name, "xmin"], "ymin": [bbox_name, "ymin"],
                "xmax": [bbox_name, "xmax"], "ymax": [bbox_name, "ymax"],
            }},
        });
        if !e.is_empty() {
            col["bbox"] = serde_json::json!([e.min_x, e.min_y, e.max_x, e.max_y]);
        }
        if let Some(c) = crs_json {
            col["crs"] = c;
        }
        let geo = serde_json::json!({"version": "1.1.0", "primary_column": geom_name, "columns": {geom_name.clone(): col}});
        w.append_key_value_metadata(KeyValue::new("geo".to_string(), geo.to_string()));
    }
    // Without a target: the caller says the path the export is written to.
    Ok((w.into_inner().map_err(err)?, unfit_counts, layout.notes(fields, "")))
}

/// The rows of a group (ids from `base + 1`, `rows` of them) that the sample still holds, and
/// where in the sample each one sits. The sample is in id order (groups are read in order and
/// the sampler keeps order), so the search starts at the group's first possible id and stops at its last.
fn rows_held<T>(items: &[T], id: impl Fn(&T) -> u64, base: u64, rows: usize) -> (Vec<bool>, Vec<usize>) {
    let mut chosen = vec![false; rows];
    let mut mine = Vec::new();
    let first = base.saturating_add(1);
    let from = items.partition_point(|f| id(f) < first);
    for (i, f) in items.iter().enumerate().skip(from) {
        let Some(r) = id(f).checked_sub(first).and_then(|r| usize::try_from(r).ok()) else { continue };
        let Some(c) = chosen.get_mut(r) else { break };
        *c = true;
        mine.push(i);
    }
    (chosen, mine)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parquet::column::page::{Page, PageMetadata, PageReader};

    /// Pages handed out one by one, as a file's would be.
    struct Pages(std::vec::IntoIter<Page>);

    impl Iterator for Pages {
        type Item = parquet::errors::Result<Page>;
        fn next(&mut self) -> Option<Self::Item> {
            self.0.next().map(Ok)
        }
    }

    impl PageReader for Pages {
        fn get_next_page(&mut self) -> parquet::errors::Result<Option<Page>> {
            Ok(self.0.next())
        }
        fn peek_next_page(&mut self) -> parquet::errors::Result<Option<PageMetadata>> {
            Ok(None)
        }
        fn skip_next_page(&mut self) -> parquet::errors::Result<()> {
            Ok(())
        }
        fn at_record_boundary(&mut self) -> parquet::errors::Result<bool> {
            Ok(true)
        }
    }

    fn data_page(bytes: usize, encoding: Encoding) -> Page {
        Page::DataPage {
            buf: Bytes::from(vec![0u8; bytes]),
            num_values: 1,
            encoding,
            def_level_encoding: Encoding::RLE,
            rep_level_encoding: Encoding::RLE,
            statistics: None,
        }
    }

    fn data_page_v2(bytes: usize, encoding: Encoding) -> Page {
        Page::DataPageV2 {
            buf: Bytes::from(vec![0u8; bytes]),
            num_values: 1,
            encoding,
            num_nulls: 0,
            num_rows: 1,
            def_levels_byte_len: 0,
            rep_levels_byte_len: 0,
            is_compressed: false,
            statistics: None,
        }
    }

    /// `page` through [`CheckedPages`] of a column chunk the footer gives `chunk_bytes` for, read with `limit`.
    fn checked(page: Page, chunk_bytes: Option<u64>, limit: u64, delta_declared: bool) -> parquet::errors::Result<Option<Page>> {
        CheckedPages {
            pages: Box::new(Pages(vec![page].into_iter())),
            bits: 32,
            limit,
            chunk_bytes,
            delta_listed: [delta_declared; 2],
            max_def: 0,
            max_rep: 0,
            chunk_values: u64::MAX,
            note: Arc::new(PageNote::default()),
        }
        .get_next_page()
    }

    /// A page can't hold more than the column chunk it belongs to says it does, nor more than the
    /// read allows; a footer without a size leaves the read's limit.
    #[test]
    fn checked_pages_refuse_a_page_larger_than_its_chunk_or_the_limit() {
        let error = checked(data_page(11, Encoding::PLAIN), Some(10), u64::MAX, false).unwrap_err().to_string();
        assert!(error.contains("a page holds 11 bytes, more than the 10 its column chunk says it holds"), "{error}");
        assert!(checked(data_page(10, Encoding::PLAIN), Some(10), u64::MAX, false).unwrap().is_some());
        for chunk in [None, Some(100)] {
            let error = checked(data_page(11, Encoding::PLAIN), chunk, 10, false).unwrap_err().to_string();
            assert!(error.contains("a page holds 11 bytes, more than the 10 byte decoded-column budget"), "{error}");
            assert!(checked(data_page(10, Encoding::PLAIN), chunk, 10, false).unwrap().is_some());
        }
    }

    /// A footer that gives no usable size (zero or negative) is read as no size, not as a large one.
    #[test]
    fn checked_pages_take_the_limit_for_a_footer_without_a_size() {
        let size = |n: i64| u64::try_from(n).ok().filter(|&n| n > 0);
        for n in [0, -1, i64::MIN] {
            assert_eq!(size(n), None, "{n}");
            assert!(checked(data_page(11, Encoding::PLAIN), size(n), 10, false).is_err(), "{n}");
        }
        assert_eq!(size(7), Some(7));
    }

    /// A page in the one encoding that builds its values one by one must be in the chunk's list,
    /// whichever version of data page it is.
    #[test]
    fn checked_pages_refuse_an_undeclared_delta_page() {
        let mut sound = delta_flat(3, 0);
        sound.extend(delta_flat(3, 4));
        sound.extend_from_slice(b"ababab");
        for (v2, page) in [(false, data_page as fn(usize, Encoding) -> Page), (true, data_page_v2)] {
            let error = checked(page(4, Encoding::DELTA_BYTE_ARRAY), Some(10), u64::MAX, false).unwrap_err().to_string();
            assert!(error.contains("the DELTA_BYTE_ARRAY encoding, which its column chunk doesn't list"), "{error}");
            let declared = delta_page(0, sound.clone(), 3, Encoding::DELTA_BYTE_ARRAY, v2);
            assert!(checked(declared, Some(100), u64::MAX, true).unwrap().is_some());
            assert!(checked(page(4, Encoding::PLAIN), Some(10), u64::MAX, false).unwrap().is_some());
        }
    }

    /// A DELTA_LENGTH_BYTE_ARRAY page, like a DELTA_BYTE_ARRAY one, must be in the chunk's list: the
    /// pace is only on for a column that lists one, so an unlisted page would be checked against a
    /// budget the values before it have not been charged to.
    #[test]
    fn checked_pages_refuse_an_undeclared_delta_length_page() {
        let mut sound = delta_flat(3, 4);
        sound.extend_from_slice(b"ababab");
        for v2 in [false, true] {
            let page = || delta_page(0, sound.clone(), 3, Encoding::DELTA_LENGTH_BYTE_ARRAY, v2);
            let error = checked(page(), Some(100), u64::MAX, false).unwrap_err().to_string();
            assert!(error.contains("the DELTA_LENGTH_BYTE_ARRAY encoding, which its column chunk doesn't list"), "v2 {v2}: {error}");
            assert!(checked(page(), Some(100), u64::MAX, true).unwrap().is_some(), "v2 {v2}");
        }
    }

    /// `page` through [`CheckedPages`] of a chunk whose footer lists exactly `listed` among the encodings.
    fn checked_listing(page: Page, listed: &[Encoding]) -> parquet::errors::Result<Option<Page>> {
        CheckedPages {
            pages: Box::new(Pages(vec![page].into_iter())),
            bits: 32,
            limit: u64::MAX,
            chunk_bytes: Some(100),
            delta_listed: [Encoding::DELTA_BYTE_ARRAY, Encoding::DELTA_LENGTH_BYTE_ARRAY].map(|e| listed.contains(&e)),
            max_def: 0,
            max_rep: 0,
            chunk_values: u64::MAX,
            note: Arc::new(PageNote::default()),
        }
        .get_next_page()
    }

    /// A late-filtered read looks only at the sample's rows from the current group on, not at every
    /// row kept from earlier groups: the work stays linear in the groups.
    #[test]
    fn rows_held_inspects_only_the_current_groups_suffix() {
        use std::cell::Cell;
        let groups = 2_000u64;
        let per = 4u64;
        let mut items: Vec<u64> = Vec::new();
        let looked = Cell::new(0u64);
        for g in 0..groups {
            let base = g * per;
            items.extend((1..=per).map(|r| base + r));
            let (chosen, mine) = rows_held(
                &items,
                |&id| {
                    looked.set(looked.get() + 1);
                    id
                },
                base,
                per as usize,
            );
            assert_eq!(chosen, vec![true; per as usize]);
            assert_eq!(mine, (items.len() - per as usize..items.len()).collect::<Vec<_>>());
        }
        // Binary search plus the suffix: far below the quadratic groups * rows / 2 of a full scan.
        assert!(looked.get() < groups * (per + 16), "inspected {} entries", looked.get());
    }

    /// The page's own encoding must be in the chunk's list: a chunk that lists one delta encoding
    /// doesn't make a page in the other one declared.
    #[test]
    fn checked_pages_refuse_a_delta_page_of_the_other_delta_encoding() {
        let mut flat = delta_flat(3, 4);
        flat.extend_from_slice(b"ababab");
        let mut both = delta_flat(3, 0);
        both.extend(delta_flat(3, 4));
        both.extend_from_slice(b"ababab");
        for v2 in [false, true] {
            let length = || delta_page(0, flat.clone(), 3, Encoding::DELTA_LENGTH_BYTE_ARRAY, v2);
            let prefixed = || delta_page(0, both.clone(), 3, Encoding::DELTA_BYTE_ARRAY, v2);
            let error = checked_listing(length(), &[Encoding::DELTA_BYTE_ARRAY]).unwrap_err().to_string();
            assert!(error.contains("the DELTA_LENGTH_BYTE_ARRAY encoding, which its column chunk doesn't list"), "v2 {v2}: {error}");
            let error = checked_listing(prefixed(), &[Encoding::DELTA_LENGTH_BYTE_ARRAY]).unwrap_err().to_string();
            assert!(error.contains("the DELTA_BYTE_ARRAY encoding, which its column chunk doesn't list"), "v2 {v2}: {error}");
            assert!(checked_listing(length(), &[Encoding::DELTA_LENGTH_BYTE_ARRAY]).unwrap().is_some(), "v2 {v2}");
            assert!(checked_listing(prefixed(), &[Encoding::DELTA_BYTE_ARRAY]).unwrap().is_some(), "v2 {v2}");
            assert!(
                checked_listing(length(), &[Encoding::DELTA_BYTE_ARRAY, Encoding::DELTA_LENGTH_BYTE_ARRAY]).unwrap().is_some(),
                "v2 {v2}"
            );
        }
    }

    /// A data page tells the pace its size and its values.
    #[test]
    fn checked_pages_note_the_data_page_they_hand_over() {
        for page in [data_page(40, Encoding::PLAIN), data_page_v2(40, Encoding::PLAIN)] {
            let note = Arc::new(PageNote::default());
            let mut pages = CheckedPages {
                pages: Box::new(Pages(vec![page].into_iter())),
                bits: 32,
                limit: u64::MAX,
                chunk_bytes: None,
                delta_listed: [false; 2],
                max_def: 0,
                max_rep: 0,
                chunk_values: u64::MAX,
                note: note.clone(),
            };
            assert!(pages.get_next_page().unwrap().is_some());
            assert_eq!((note.page.load(Ordering::Relaxed), note.in_page.load(Ordering::Relaxed)), (40, 1));
        }
    }

    /// The length of text as `from_utf8_lossy` makes it, for any bytes.
    #[test]
    fn text_len_is_the_length_of_the_lossy_string() {
        for bytes in
            [&b""[..], b"abc", "h\u{e9}llo \u{4e2d}".as_bytes(), b"\xff", b"a\xffb\xfe\xfec", b"\xe2\x82", b"ok\xe2\x82", b"\xf0\x9f\x98"]
        {
            assert_eq!(text_len(bytes), String::from_utf8_lossy(bytes).len(), "{bytes:?}");
        }
    }

    /// A DELTA_BINARY_PACKED stream of `count` values, written as the format lays it out: blocks of
    /// 128 values in 4 miniblocks of 32, whose bit widths cycle through `widths` (a miniblock past
    /// the last value has width 0 and no bytes), the packed bytes all zero.
    fn delta_bits(count: u32, first: u8, widths: [u8; 4]) -> Vec<u8> {
        let mut out = delta_head(count, first);
        let mut left = count.saturating_sub(1);
        let mut at = 0;
        while left > 0 {
            out.push(0);
            let mut packed = 0usize;
            let mut bits = Vec::new();
            for _ in 0..4 {
                let width = if left == 0 { 0 } else { widths[at % 4] };
                at += 1;
                left = left.saturating_sub(32);
                bits.push(width);
                packed += usize::from(width) * 32 / 8;
            }
            out.extend(bits);
            out.extend(std::iter::repeat_n(0u8, packed));
        }
        out
    }

    /// The header of such a stream: block size 128, 4 miniblocks, the count, the first value.
    fn delta_head(count: u32, first: u8) -> Vec<u8> {
        let mut out = vec![0x80, 0x01, 0x04];
        out.extend(uleb_bytes(u64::from(count)));
        out.push(first);
        out
    }

    /// `delta_bits` of width 0 (a run of equal values).
    fn delta_flat(count: u32, first: u8) -> Vec<u8> {
        delta_bits(count, first, [0; 4])
    }

    fn uleb_bytes(mut n: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let low = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                out.push(low);
                return out;
            }
            out.push(low | 0x80);
        }
    }

    fn delta_page(levels: usize, values: Vec<u8>, num_values: u32, encoding: Encoding, v2: bool) -> Page {
        let mut buf = vec![0xAAu8; levels];
        buf.extend(values);
        let buf = Bytes::from(buf);
        if v2 {
            Page::DataPageV2 {
                buf,
                num_values,
                encoding,
                num_nulls: 0,
                num_rows: num_values,
                def_levels_byte_len: u32::try_from(levels).unwrap_or(0),
                rep_levels_byte_len: 0,
                is_compressed: false,
                statistics: None,
            }
        } else {
            Page::DataPage {
                buf,
                num_values,
                encoding,
                def_level_encoding: Encoding::RLE,
                rep_level_encoding: Encoding::RLE,
                statistics: None,
            }
        }
    }

    /// `page` through [`CheckedPages`] of a column with `max_def` levels, in a chunk of `chunk_values`
    /// values, with `left` bytes of budget.
    fn checked_in(page: Page, max_def: i16, chunk_values: u64, left: u64) -> parquet::errors::Result<Option<Page>> {
        let note = Arc::new(PageNote::default());
        note.left.store(left, Ordering::Relaxed);
        CheckedPages {
            pages: Box::new(Pages(vec![page].into_iter())),
            bits: 32,
            limit: u64::MAX,
            chunk_bytes: None,
            delta_listed: [true; 2],
            max_def,
            max_rep: 0,
            chunk_values,
            note,
        }
        .get_next_page()
    }

    /// A version 1 delta page of a column with a definition level, its levels `levels` in front.
    fn delta_page_with_levels(levels: Vec<u8>, values: Vec<u8>, num_values: u32, def_level_encoding: Encoding) -> Page {
        Page::DataPage {
            buf: Bytes::from([levels, values].concat()),
            num_values,
            encoding: Encoding::DELTA_BYTE_ARRAY,
            def_level_encoding,
            rep_level_encoding: Encoding::RLE,
            statistics: None,
        }
    }

    /// The two streams of a DELTA_BYTE_ARRAY page of `count` values, then `tail` bytes.
    fn delta_bytes(count: u32, widths: [u8; 4], tail: &[u8]) -> Vec<u8> {
        [delta_bits(count, 0, widths), delta_bits(count, 4, widths), tail.to_vec()].concat()
    }

    /// A delta page whose count is more than its values, or more than its bytes can hold, or whose
    /// header is cut off or malformed, is refused before the crate's decoder sizes an array from the
    /// count (a few bytes can say two billion values), and a sound one is handed over. Each refusal
    /// says which of these it is.
    #[test]
    fn checked_pages_refuse_a_delta_page_with_a_count_its_bytes_cannot_hold() {
        let huge = delta_head(i32::MAX as u32, 0);
        let delta = Encoding::DELTA_BYTE_ARRAY;
        let lengths = Encoding::DELTA_LENGTH_BYTE_ARRAY;
        let sound = delta_bytes(3, [0; 4], b"ababab");
        // Widths that need bits, over several blocks.
        let wide = delta_bytes(300, [3, 0, 5, 7], b"");
        for v2 in [false, true] {
            // Level bytes in front, in version 2 (a version 1 page of a column without levels has none).
            for levels in [0, 2] {
                if !v2 && levels > 0 {
                    continue;
                }
                let at = format!("v2 {v2} levels {levels}");
                let page = |values: Vec<u8>, n: u32, e| delta_page(levels, values, n, e, v2);
                let refused = |values: Vec<u8>, n: u32, e| checked(page(values, n, e), None, 1 << 30, true).unwrap_err().to_string();
                let says = |error: String, what: &str| {
                    assert!(error.contains(&format!("a delta page is refused: {what}")), "{at}: {error} -- wanted {what}")
                };
                says(refused(huge.clone(), 3, delta), "a stream says it holds 2147483647 values, more than the page's 3");
                says(refused(huge.clone(), 3, lengths), "a stream says it holds 2147483647 values, more than the page's 3");
                // A count the page's own value count allows, but its few bytes can't hold.
                says(
                    refused([delta_head(1000, 0), vec![0; 5]].concat(), 1000, delta),
                    "its 1000 values need more blocks than its 11 bytes can hold",
                );
                // The decoder's arrays for that count, more than the budget has.
                let error = checked_in(page(delta_flat(1 << 20, 0), 1 << 20, lengths), 0, u64::MAX, 1 << 10).unwrap_err().to_string();
                says(
                    error,
                    "a stream says it holds 1048576 values, whose arrays of 4 bytes each are more than the 1024 bytes the decoded-column budget has left",
                );
                // Cut off in its header, a block's minimum delta or widths, or not a header at all.
                let flat = delta_flat(3, 0);
                for (cut, what) in [
                    (0, "a stream header is cut off or malformed"),
                    (2, "a stream header is cut off or malformed"),
                    (4, "a stream header is cut off or malformed"),
                    (5, "its 3 values need more blocks than its 5 bytes can hold"),
                    (7, "its 3 values need more blocks than its 7 bytes can hold"),
                    (9, "its 3 values need more blocks than its 9 bytes can hold"),
                ] {
                    says(refused(flat[..cut].to_vec(), 3, delta), what);
                }
                says(refused(vec![0x80; 12], 3, delta), "a stream header is cut off or malformed");
                says(
                    refused(vec![0x81, 0x01, 0x04, 0x03, 0x00], 3, delta),
                    "its block of 129 values in 4 miniblocks is not one the format allows",
                );
                says(
                    refused(vec![0x80, 0x01, 0x00, 0x03, 0x00], 3, delta),
                    "its block of 128 values in 0 miniblocks is not one the format allows",
                );
                says(
                    refused(vec![0x80, 0x01, 0x03, 0x03, 0x00], 3, delta),
                    "its block of 128 values in 3 miniblocks is not one the format allows",
                );
                // A width above 32, and packed bytes the page doesn't have.
                let mut wide_bits = delta_flat(3, 0);
                wide_bits[6] = 33;
                says(refused(wide_bits, 3, delta), "a miniblock has a bit width of 33, more than 32");
                let mut packed = delta_bits(3, 0, [3, 0, 0, 0]);
                packed.truncate(packed.len() - 1);
                says(refused(packed, 3, delta), "its 3 values need more bytes than the page's 21 bytes can hold");
                // The two streams' counts must agree.
                let odd = [delta_flat(3, 0), delta_flat(2, 4)].concat();
                says(refused(odd, 3, delta), "it has 3 prefix lengths and 2 suffixes");
                // Sound pages go through, and so does a column of nothing but nulls.
                for (values, n) in
                    [(sound.clone(), 3), (wide.clone(), 300), (delta_bytes(0, [0; 4], b""), 3), (delta_bytes(1, [0; 4], b"a"), 1)]
                {
                    assert!(checked(page(values, n, delta), None, 1 << 30, true).unwrap().is_some(), "{at}");
                }
                assert!(checked(page(delta_flat(3, 4), 3, lengths), None, 1 << 30, true).unwrap().is_some(), "{at}");
            }
        }
    }

    /// Levels in front of a version 1 page's values: an RLE stream with its length, in the order
    /// the crate reads them, and a page with them cut off or in no encoding a page has is refused.
    #[test]
    fn checked_pages_find_the_values_after_a_version_one_pages_levels() {
        let sound = delta_bytes(300, [3, 0, 5, 7], b"");
        let rle = |bytes: &[u8]| [u32::try_from(bytes.len()).unwrap().to_le_bytes().to_vec(), bytes.to_vec()].concat();
        let page = |levels: Vec<u8>, values: Vec<u8>, encoding| delta_page_with_levels(levels, values, 300, encoding);
        // A definition level stream of 3 bytes, then the values.
        assert!(checked_in(page(rle(&[1, 2, 3]), sound.clone(), Encoding::RLE), 1, u64::MAX, u64::MAX).unwrap().is_some());
        assert!(checked_in(page(rle(&[]), sound.clone(), Encoding::RLE), 1, u64::MAX, u64::MAX).unwrap().is_some());
        let error = checked_in(page(vec![9, 0], vec![], Encoding::RLE), 1, u64::MAX, u64::MAX).unwrap_err().to_string();
        assert!(error.contains("a delta page is refused: its levels are cut off"), "{error}");
        let error = checked_in(page(rle(&[1]), sound.clone(), Encoding::PLAIN), 1, u64::MAX, u64::MAX).unwrap_err().to_string();
        assert!(error.contains("its levels are in the PLAIN encoding, which a page can't have"), "{error}");
        // A level length that runs past the page.
        let error = checked_in(page(vec![0xff, 0xff, 0xff, 0x7f], sound, Encoding::RLE), 1, u64::MAX, u64::MAX).unwrap_err().to_string();
        assert!(error.contains("its levels are longer than the page"), "{error}");
    }

    /// A stream says it holds more values than the column chunk does: refused, though the page's own
    /// count allows it; and one the budget left can't hold the arrays of, though the whole limit could.
    #[test]
    fn checked_pages_hold_a_delta_count_to_the_chunk_and_the_budget_left() {
        let values = delta_bytes(300, [3, 0, 5, 7], b"");
        let page = || delta_page(0, values.clone(), 300, Encoding::DELTA_BYTE_ARRAY, true);
        assert!(checked_in(page(), 0, 300, 2400).unwrap().is_some());
        let error = checked_in(page(), 0, 299, u64::MAX).unwrap_err().to_string();
        assert!(error.contains("more than the column chunk's 299"), "{error}");
        let error = checked_in(page(), 0, 300, 2399).unwrap_err().to_string();
        assert!(error.contains("the 2399 bytes the decoded-column budget has left"), "{error}");
    }

    /// The layout `delta_stream` reads agrees with a real writer's: the count and the end of the
    /// stream (the page holds nothing else) for many shapes of values, in pages of one stream.
    #[test]
    fn delta_stream_ends_where_the_writer_ended_it() {
        use arrow_array::{ArrayRef, Int32Array, RecordBatch};
        use arrow_schema::{DataType, Field as AField, Schema};
        use parquet::file::reader::{FileReader as _, SerializedFileReader};
        let wide: Vec<i32> = (0..5000).map(|i| (i * 7919) % 100_003 - 50_000).collect();
        let shapes: Vec<Vec<i32>> = vec![
            vec![5],
            vec![5, 5],
            vec![0; 127],
            vec![3; 128],
            vec![3; 129],
            (0..300).collect(),
            (0..1000).map(|i| i * i % 1009).collect(),
            wide,
            (0..4096).map(|i| if i % 2 == 0 { i32::MAX - i } else { i32::MIN + i }).collect(),
        ];
        for values in shapes {
            let schema = Arc::new(Schema::new(vec![AField::new("v", DataType::Int32, false)]));
            let column: ArrayRef = Arc::new(Int32Array::from(values.clone()));
            let batch = RecordBatch::try_new(schema.clone(), vec![column]).unwrap();
            let props = ::parquet::file::properties::WriterProperties::builder()
                .set_dictionary_enabled(false)
                .set_encoding(Encoding::DELTA_BINARY_PACKED)
                .set_data_page_row_count_limit(usize::MAX)
                .set_data_page_size_limit(usize::MAX)
                .build();
            let mut bytes = Vec::new();
            let mut writer = ::parquet::arrow::ArrowWriter::try_new(&mut bytes, schema, Some(props)).unwrap();
            writer.write(&batch).unwrap();
            writer.close().unwrap();
            let reader = SerializedFileReader::new(Bytes::from(bytes)).unwrap();
            let mut pages = reader.get_row_group(0).unwrap().get_column_page_reader(0).unwrap();
            let mut seen = 0;
            while let Some(page) = pages.get_next_page().unwrap() {
                if let Page::DataPage { buf, num_values, .. } | Page::DataPageV2 { buf, num_values, .. } = &page {
                    let bounds = DeltaBounds { page: u64::MAX, chunk: u64::MAX, left: u64::MAX, per_value: 4 };
                    let (count, end) = delta_stream(buf, &bounds).unwrap();
                    assert_eq!((count, end), (u64::from(*num_values), buf.len()), "{} values", values.len());
                    // With bytes after it the stream still ends in the same place.
                    let longer = [buf.to_vec(), vec![0xAB; 9]].concat();
                    assert_eq!(delta_stream(&longer, &bounds).unwrap(), (count, end), "{} values", values.len());
                    seen += 1;
                }
            }
            assert!(seen > 0);
        }
    }

    /// The crate's decoders make an array of 4 bytes a value for each of a DELTA_BYTE_ARRAY page's two
    /// streams: a count the page and the chunk allow, but whose two arrays are more than the budget,
    /// is refused.
    #[test]
    fn checked_pages_refuse_a_delta_count_whose_arrays_are_more_than_the_budget() {
        let limit = 1u64 << 20;
        // Under a quarter of the limit: one array fits, the two don't.
        let count = u32::try_from(limit / 4 - 1).unwrap();
        let mut both = delta_flat(count, 0);
        both.extend(delta_flat(count, 4));
        let error = checked(delta_page(0, both, count, Encoding::DELTA_BYTE_ARRAY, true), None, limit, true).unwrap_err().to_string();
        assert!(error.contains("decoded-column budget"), "{error}");
        // The one array of DELTA_LENGTH_BYTE_ARRAY fits.
        let page = delta_page(0, delta_flat(count, 4), count, Encoding::DELTA_LENGTH_BYTE_ARRAY, true);
        assert!(checked(page, None, limit, true).unwrap().is_some());
    }

    /// Levels written in the deprecated BIT_PACKED encoding have no length in front: the page's
    /// values start after as many bytes as its values have bits, as the crate reads them.
    #[test]
    fn checked_pages_read_past_bit_packed_levels() {
        let mut sound = delta_flat(3, 0);
        sound.extend(delta_flat(3, 4));
        sound.extend_from_slice(b"ababab");
        let mut buf = vec![0b101u8];
        buf.extend(sound);
        #[expect(deprecated, reason = "BIT_PACKED levels are deprecated but still read")]
        let def_level_encoding = Encoding::BIT_PACKED;
        let page = Page::DataPage {
            buf: Bytes::from(buf),
            num_values: 3,
            encoding: Encoding::DELTA_BYTE_ARRAY,
            def_level_encoding,
            rep_level_encoding: Encoding::RLE,
            statistics: None,
        };
        let mut pages = CheckedPages {
            pages: Box::new(Pages(vec![page].into_iter())),
            bits: 32,
            limit: u64::MAX,
            chunk_bytes: None,
            delta_listed: [true; 2],
            max_def: 1,
            max_rep: 0,
            chunk_values: u64::MAX,
            note: Arc::new(PageNote::default()),
        };
        assert!(pages.get_next_page().unwrap().is_some());
    }

    /// Headers handed out one by one, as a file's pages would be peeked at and skipped.
    struct Headers(Vec<PageMetadata>);

    impl Iterator for Headers {
        type Item = parquet::errors::Result<Page>;
        fn next(&mut self) -> Option<Self::Item> {
            None
        }
    }

    impl PageReader for Headers {
        fn get_next_page(&mut self) -> parquet::errors::Result<Option<Page>> {
            Ok(None)
        }
        fn peek_next_page(&mut self) -> parquet::errors::Result<Option<PageMetadata>> {
            Ok(self.0.first().cloned())
        }
        fn skip_next_page(&mut self) -> parquet::errors::Result<()> {
            self.0.remove(0);
            Ok(())
        }
        fn at_record_boundary(&mut self) -> parquet::errors::Result<bool> {
            Ok(true)
        }
    }

    /// A skip passes over the whole pages its run covers, and is stopped (an error, with the values
    /// it passed over noted) at a data page it would only skip part of, whose decoder it would set up.
    #[test]
    fn checked_pages_stop_a_skip_at_a_page_it_would_only_partly_pass_over() {
        let page = |rows| PageMetadata { num_rows: Some(rows), num_levels: Some(rows), is_dict: false };
        let dictionary = PageMetadata { num_rows: None, num_levels: None, is_dict: true };
        let note = Arc::new(PageNote::default());
        let mut pages = CheckedPages {
            pages: Box::new(Headers(vec![dictionary, page(4), page(4), page(4)])),
            bits: 32,
            limit: u64::MAX,
            chunk_bytes: None,
            delta_listed: [true; 2],
            max_def: 0,
            max_rep: 0,
            chunk_values: u64::MAX,
            note: note.clone(),
        };
        let budget = Budget::new(1 << 20, 0);
        let pace = Pace { watch: Some((&note, &budget)) };
        // Not skipping: a peek is a peek.
        assert!(pages.peek_next_page().unwrap().is_some_and(|m| m.is_dict));
        assert_eq!(pace.skip(10), 10, "a run from a page's start is asked for whole");
        assert!(pages.peek_next_page().unwrap().is_some_and(|m| m.is_dict), "a dictionary page is left to the crate");
        pages.skip_next_page().unwrap();
        for _ in 0..2 {
            assert_eq!(pages.peek_next_page().unwrap().and_then(|m| m.num_rows), Some(4));
            pages.skip_next_page().unwrap();
        }
        // 2 values of the run are left, the next page holds 4: the skip stops there.
        assert!(pages.peek_next_page().is_err());
        let (skipped, stopped) = pace.skipped(None, Err(parquet::errors::ParquetError::General("stop".into()))).unwrap();
        assert_eq!((skipped, stopped), (8, true));
        assert_eq!(note.skip_left.load(Ordering::Relaxed), NOT_SKIPPING);
        assert_eq!(pages.peek_next_page().unwrap().and_then(|m| m.num_rows), Some(4), "nothing was fetched");
        assert_eq!(pace.skip(2), 1, "the page is then taken a value at a time");
        // Another error is the caller's.
        assert_eq!(pace.skip(3), 3);
        assert!(pace.skipped(None, Err(parquet::errors::ParquetError::General("other".into()))).is_err());
        assert_eq!(pace.skipped(None, Ok(3)).unwrap(), (3, false));
    }

    /// A DELTA_BYTE_ARRAY column takes one value at a page's start, then no more than its page
    /// and what the budget left holds at that page's size; other columns take a batch.
    #[test]
    fn delta_pace_follows_the_page_and_the_budget_left() {
        let note = PageNote::default();
        let budget = Budget::new(8 << 10, 0);
        let pace = Pace { watch: Some((&note, &budget)) };
        assert_eq!(pace.take(5000), 1, "a page not yet seen");
        note.in_page.store(5000, Ordering::Relaxed);
        note.page.store(1024, Ordering::Relaxed);
        assert_eq!(pace.take(5000), 8, "8 KiB left holds eight values of a page of 1 KiB");
        pace.used(3);
        assert_eq!(note.in_page.load(Ordering::Relaxed), 4997);
        note.page.store(1 << 20, Ordering::Relaxed);
        assert_eq!(pace.take(5000), 1, "a page bigger than what is left still moves on");
        note.page.store(1, Ordering::Relaxed);
        note.in_page.store(100, Ordering::Relaxed);
        assert_eq!((pace.take(5000), pace.take(50)), (100, 50), "never past the page or the run");
        note.in_page.store(1 << 20, Ordering::Relaxed);
        assert_eq!(pace.take(1 << 20), BATCH, "small values take a full batch");
        pace.used(usize::MAX);
        assert_eq!(note.in_page.load(Ordering::Relaxed), 0);
        assert_eq!(Pace { watch: None }.take(1 << 20), BATCH);
        assert_eq!(Pace { watch: None }.take(7), 7);
        assert_eq!(Pace { watch: None }.skip(1 << 20), 1 << 20, "a skip of another encoding is one call");
        note.in_page.store(100, Ordering::Relaxed);
        assert_eq!(pace.skip(1 << 20), 100, "a skip inside a page takes no more than the page's rest");
        note.in_page.store(0, Ordering::Relaxed);
        assert_eq!(pace.skip(1 << 20), 1 << 20, "a delta column's skip starts a page with its whole run");
        assert_eq!(note.skip_left.load(Ordering::Relaxed), 1 << 20, "so the pages it covers pass undecoded");
    }
}
