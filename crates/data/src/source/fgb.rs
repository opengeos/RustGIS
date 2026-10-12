//! FlatGeobuf: features in a packed Hilbert R-tree, read with the pure-Rust `flatgeobuf` crate.
//!
//! Extent queries walk the file's spatial index and seek to just the matching features. Files
//! written without an index are scanned.

use super::{Batch, FeatureSource, Reproject, Request, Sampler, SourceInfo, clamp_limit, in_request};
use crate::{DataError, Feature, Field, FieldType, GeometryType, Result, Value, wkb};
use flatgeobuf::geozero::{ColumnValue, CoordDimensions, PropertyProcessor, ToWkb};
use flatgeobuf::{ColumnType, FallibleStreamingIterator, FeatureProperties, FgbReader};
use rustgis_geom::Extent;
use std::io::{BufReader, Cursor, Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A file or an upload, read and seeked.
trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

#[derive(Clone, Debug)]
enum Input {
    Path(PathBuf),
    Bytes(Arc<[u8]>),
}

/// A FlatGeobuf file.
pub struct FgbSource {
    info: SourceInfo,
    input: Input,
    reproject: Reproject,
}

impl FgbSource {
    pub fn open_path(path: &Path, name: &str) -> Result<Self> {
        let f = BufReader::new(std::fs::File::open(path)?);
        Self::new(f, Input::Path(path.to_path_buf()), name)
    }

    pub fn open_bytes(bytes: Vec<u8>, name: &str) -> Result<Self> {
        let bytes: Arc<[u8]> = bytes.into();
        Self::new(Cursor::new(bytes.clone()), Input::Bytes(bytes), name)
    }

    fn new<R: Read + Seek>(r: R, input: Input, name: &str) -> Result<Self> {
        let reader = FgbReader::open(r).map_err(err)?;
        let h = reader.header();
        let mut notes = Vec::new();
        // The code is an EPSG one only when the organization is EPSG (or not given, which means EPSG).
        let epsg_org = h.crs().is_none_or(|c| c.org().is_none_or(|o| o.eq_ignore_ascii_case("epsg")));
        let code = h.crs().map(|c| c.code()).and_then(|c| u32::try_from(c).ok()).filter(|c| *c > 0 && epsg_org);
        if !epsg_org && let Some(org) = h.crs().and_then(|c| c.org()) {
            notes.push(format!("its coordinate system is defined by {org}, not EPSG; read as longitude/latitude"));
        }
        // A coordinate system described (by WKT or name) but without an EPSG code can't be converted.
        if code.is_none() && h.crs().is_some_and(|c| c.wkt().is_some_and(|w| !w.contains("WGS 84") && !w.contains("WGS84"))) {
            notes.push("its coordinate system has no EPSG code; read as longitude/latitude".into());
        }
        let reproject = Reproject::from_epsg(code, &mut notes);
        let fields = h
            .columns()
            .map(|cols| cols.iter().map(|c| Field::new(c.name().to_string(), field_type(c.type_()))).collect())
            .unwrap_or_default();
        let geometry_type = {
            use flatgeobuf::GeometryType as G;
            match h.geometry_type() {
                G::Point | G::MultiPoint => GeometryType::Point,
                G::LineString | G::MultiLineString => GeometryType::Polyline,
                G::Polygon | G::MultiPolygon => GeometryType::Polygon,
                _ => GeometryType::None,
            }
        };
        let extent = h
            .envelope()
            .and_then(|e| match (e.iter().next(), e.iter().nth(1), e.iter().nth(2), e.iter().nth(3)) {
                (Some(a), Some(b), Some(c), Some(d)) => Some(reproject.extent_to_lonlat(&Extent::new(a, b, c, d))),
                _ => None,
            })
            .unwrap_or(Extent::EMPTY);
        let count = h.features_count();
        let info = SourceInfo {
            name: h.name().filter(|n| !n.trim().is_empty()).unwrap_or(name).to_string(),
            format: "FlatGeobuf",
            geometry_type,
            fields,
            extent,
            count: (count > 0).then_some(count),
            crs: reproject.label(),
            // The crate's index search assumes the default node size; other files are scanned.
            indexed: h.index_node_size() == flatgeobuf::packed_r_tree::PackedRTree::DEFAULT_NODE_SIZE && count > 0,
            notes,
        };
        let mut src = FgbSource { info, input, reproject };
        if src.info.geometry_type == GeometryType::None {
            src.info.geometry_type = src.peek_kind();
        }
        Ok(src)
    }

    /// Reads a request; `scan` ignores the index.
    fn read_with<R: Read + Seek>(&self, open: &impl Fn() -> Result<R>, req: &Request, scan: bool) -> Result<Batch> {
        let limit = clamp_limit(req.limit);
        let reader = FgbReader::open(open()?).map_err(err)?;
        let native = req.extent.map(|e| self.reproject.extent_from_lonlat(&e));
        let indexed = native.filter(|_| self.info.indexed && !scan);
        // Feature numbers in the file, for ids that stay put between reads: the index search's
        // results, in the order the selection visits them.
        let numbers: Option<Vec<u64>> = match indexed {
            Some(e) => Some(self.search(open()?, &e)?),
            None => None,
        };
        let mut iter = match indexed {
            Some(e) => reader.select_bbox(e.min_x, e.min_y, e.max_x, e.max_y).map_err(err)?,
            None => reader.select_all().map_err(err)?,
        };
        // The ids come from a separate walk of the index: should it ever disagree with the
        // selection, scan instead rather than give features the wrong ids.
        if let Some(nums) = &numbers
            && iter.features_count() != Some(nums.len())
        {
            return self.read_with(open, req, true);
        }
        // Without an index, an extent is tested on each feature: sample the matches as they come.
        let late = native.is_some() && indexed.is_none();
        let matched = iter.features_count().map(|n| n as u64).or(self.info.count).unwrap_or(0);
        let mut sample = if late || matched == 0 { Sampler::new(limit) } else { Sampler::with_total(limit, matched) };
        let mut out = Batch::default();
        let mut n = 0u64;
        while let Some(f) = iter.next().map_err(err)? {
            n += 1;
            let id = numbers.as_ref().and_then(|v| v.get(usize::try_from(n - 1).unwrap_or(usize::MAX)).copied()).unwrap_or(n);
            // A cheap skip when the sample won't take this feature (the extent isn't tested yet).
            if !late && !sample.wants() {
                continue;
            }
            let geometry = f.to_wkb(CoordDimensions::xy()).ok().as_deref().and_then(wkb::decode).and_then(|g| self.reproject.geometry(g));
            let Some(geometry) = geometry else {
                out.skipped += 1;
                continue;
            };
            if !in_request(&geometry, req) || (late && !sample.wants()) {
                continue;
            }
            let mut props = Props { fields: &self.info.fields, values: vec![Value::Null; self.info.fields.len()] };
            if f.process_properties(&mut props).is_err() {
                out.skipped += 1;
                continue;
            }
            sample.keep(Feature { id, geometry: Some(geometry), attrs: props.values });
        }
        out.matched = if late || matched == 0 { sample.seen() } else { matched };
        out.complete = sample.complete();
        out.features = sample.items;
        Ok(out)
    }

    /// The geometry type of a file that declares none (mixed output), from its first features: a
    /// read that stops there, not a pass over the file.
    fn peek_kind(&self) -> GeometryType {
        let first = |r: Box<dyn ReadSeek>| -> Option<GeometryType> {
            let mut iter = FgbReader::open(r).ok()?.select_all().ok()?;
            for _ in 0..16 {
                let f = iter.next().ok()??;
                if let Some(g) = f.to_wkb(CoordDimensions::xy()).ok().as_deref().and_then(wkb::decode) {
                    return Some(g.kind());
                }
            }
            None
        };
        let r: Option<Box<dyn ReadSeek>> = match &self.input {
            Input::Path(p) => std::fs::File::open(p).ok().map(|f| Box::new(BufReader::new(f)) as Box<dyn ReadSeek>),
            Input::Bytes(b) => Some(Box::new(Cursor::new(b.clone()))),
        };
        r.and_then(first).unwrap_or(GeometryType::None)
    }

    /// The 1-based numbers of the features the index finds in a native extent, in file order.
    fn search<R: Read + Seek>(&self, mut r: R, e: &Extent) -> Result<Vec<u64>> {
        // The index follows the magic bytes, the header's size and the header.
        let mut head = [0u8; 12];
        r.read_exact(&mut head)?;
        let size = u32::from_le_bytes([head[8], head[9], head[10], head[11]]);
        r.seek(std::io::SeekFrom::Start(12 + u64::from(size)))?;
        let count = usize::try_from(self.info.count.unwrap_or(0)).unwrap_or(0);
        let hits = flatgeobuf::packed_r_tree::PackedRTree::stream_search(
            &mut r,
            count,
            flatgeobuf::packed_r_tree::PackedRTree::DEFAULT_NODE_SIZE,
            e.min_x,
            e.min_y,
            e.max_x,
            e.max_y,
        )
        .map_err(err)?;
        Ok(hits.into_iter().map(|h| h.index as u64 + 1).collect())
    }
}

impl FeatureSource for FgbSource {
    fn info(&self) -> &SourceInfo {
        &self.info
    }

    fn read(&self, req: &Request) -> Result<Batch> {
        match &self.input {
            Input::Path(p) => self.read_with(&|| Ok(BufReader::new(std::fs::File::open(p)?)), req, false),
            Input::Bytes(b) => self.read_with(&|| Ok(Cursor::new(b.clone())), req, false),
        }
    }
}

fn err(e: flatgeobuf::Error) -> DataError {
    DataError::Format(format!("FlatGeobuf: {e}"))
}

fn field_type(t: ColumnType) -> FieldType {
    match t {
        ColumnType::Byte
        | ColumnType::UByte
        | ColumnType::Short
        | ColumnType::UShort
        | ColumnType::Int
        | ColumnType::UInt
        | ColumnType::Long
        | ColumnType::ULong => FieldType::Integer,
        ColumnType::Float | ColumnType::Double => FieldType::Double,
        ColumnType::Bool => FieldType::Boolean,
        _ => FieldType::Text,
    }
}

/// Collects a feature's properties into values in field order.
struct Props<'a> {
    fields: &'a [Field],
    values: Vec<Value>,
}

impl PropertyProcessor for Props<'_> {
    fn property(&mut self, idx: usize, _name: &str, v: &ColumnValue) -> flatgeobuf::geozero::error::Result<bool> {
        let value = match v {
            ColumnValue::Byte(x) => Value::Int(i64::from(*x)),
            ColumnValue::UByte(x) => Value::Int(i64::from(*x)),
            ColumnValue::Bool(x) => Value::Bool(*x),
            ColumnValue::Short(x) => Value::Int(i64::from(*x)),
            ColumnValue::UShort(x) => Value::Int(i64::from(*x)),
            ColumnValue::Int(x) => Value::Int(i64::from(*x)),
            ColumnValue::UInt(x) => Value::Int(i64::from(*x)),
            ColumnValue::Long(x) => Value::Int(*x),
            ColumnValue::ULong(x) => i64::try_from(*x).map(Value::Int).unwrap_or(Value::Float(*x as f64)),
            ColumnValue::Float(x) => Value::Float(f64::from(*x)),
            ColumnValue::Double(x) => Value::Float(*x),
            ColumnValue::String(s) | ColumnValue::Json(s) | ColumnValue::DateTime(s) => Value::Text((*s).to_string()),
            ColumnValue::Binary(_) => Value::Null,
        };
        // Integers in a Double column stay numbers of the column's type.
        let value = match (self.fields.get(idx).map(|f| f.ty), value) {
            (Some(FieldType::Double), Value::Int(i)) => Value::Float(i as f64),
            (_, v) => v,
        };
        if let Some(slot) = self.values.get_mut(idx) {
            *slot = value;
        }
        Ok(false)
    }
}
