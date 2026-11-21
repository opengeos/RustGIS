//! What a field package brings back: the phone's changes, not its map (#2468).
//!
//! When a field package is opened, the rows of its feature layers as they were given are written to
//! a folder of this device's own, named by the package's identity ([`FieldOrigin::package`]): each
//! row once, in a file read a row at a time, and beside it an index of each row's id, place in the
//! file and fingerprint. Send Field Package fingerprints the rows the map holds then: a row whose
//! fingerprint differs has its given copy read from the file and is compared with it, attribute by
//! attribute; an id the map no longer has was deleted, one the copy hasn't was added. So every edit
//! is found, however it was made, with no second copy of a layer held in memory, and the copy is
//! still found after the field project is saved and opened again, or recovered.
//!
//! The returned package carries the list ([`ENTRY`]): per layer, the features added, those deleted
//! with their values and shape as given, and for each feature edited the attributes (by name) and
//! the shape it changed, with their values as given. The new values are the package's map's own.
//! Bring Back applies the list to the home map: a change whose value as given is still the home
//! map's goes in; one changed at home too is a conflict that the field's value wins, said by
//! feature. Nothing else is compared, so a field the office added, or a second phone's untouched
//! rows, are left alone.
//!
//! [`FieldOrigin::package`]: crate::project::FieldOrigin::package

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use rustgis_data::{Geometry, RowRef, Value};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::Session;
use crate::project::Project;

/// The package entry holding the list of changes.
pub(crate) const ENTRY: &str = "changes.v1.json";
/// The version of the list this RustGIS writes and reads.
const VERSION: u32 = 1;
/// The bytes of one index record: id, place in the rows file, length, fingerprint.
const RECORD: usize = 8 + 8 + 4 + 16;
/// The longest row read back from a given copy, in bytes.
const MAX_ROW: u32 = 256 << 20;
/// The file listing a given copy's layers, written last: a folder without it is incomplete.
const LAYERS: &str = "layers.json";

/// The changes of one returning package.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Changes {
    pub version: u32,
    /// The package's identity ([`crate::project::FieldOrigin::package`]).
    pub package: String,
    pub layers: Vec<LayerChanges>,
}

/// The changes to one layer, named by its id.
#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct LayerChanges {
    pub layer: u32,
    /// The ids of the features added in the field: their rows are the package map's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added: Vec<u64>,
    /// The features deleted in the field, with every value and the shape they had as given.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deleted: Vec<Given>,
    /// The features edited in the field, with the values and shape they changed, as given. Their
    /// new values are the package map's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edited: Vec<Given>,
    /// The fields the package gave that the field removed, by name: their values aren't listed, so
    /// Bring Back refuses the list while the home layer still has them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
}

/// A feature's values as the package gave them.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Given {
    pub id: u64,
    /// Attribute values by field name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<(String, Value)>,
    /// The shape, when it is part of the change (`null` for none).
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "present")]
    pub geometry: Option<Option<Geometry>>,
}

/// Reads a field that is there, even as `null`, as `Some`.
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

impl Changes {
    /// Reads the list from a package entry, refusing one from a newer RustGIS, one of another
    /// package, and one naming a feature twice in a layer.
    pub(crate) fn read(bytes: &[u8], package: &str) -> Result<Changes, String> {
        #[derive(Deserialize)]
        struct Header {
            version: u32,
        }
        let damaged = |e: serde_json::Error| format!("the field package's list of changes is damaged: {e}");
        let header: Header = serde_json::from_slice(bytes).map_err(damaged)?;
        if header.version > VERSION {
            return Err("the field package's list of changes was made by a newer RustGIS: update RustGIS to bring it back".into());
        }
        let changes: Changes = serde_json::from_slice(bytes).map_err(damaged)?;
        if changes.package != package {
            return Err("the field package's list of changes is of another package: make a new field package".into());
        }
        let mut layers = HashSet::new();
        for l in &changes.layers {
            let mut ids = HashSet::new();
            let all = l.added.iter().copied().chain(l.deleted.iter().map(|g| g.id)).chain(l.edited.iter().map(|g| g.id));
            if !layers.insert(l.layer) || !all.into_iter().all(|id| ids.insert(id)) {
                return Err("the field package's list of changes names a layer or a feature twice: it is damaged".into());
            }
        }
        Ok(changes)
    }
}

// The copy of the rows a package gave, on the phone.

/// The folder the given copies are kept in: the app's own `field bases`.
fn bases() -> Option<PathBuf> {
    #[cfg(test)]
    {
        test_bases::root()
    }
    #[cfg(not(test))]
    {
        rustgis_data::dirs::app_data_dir().map(|d| d.join("field bases"))
    }
}

/// The given copy of package `package`'s rows, when this device has one.
fn base_dir(package: &str) -> Option<PathBuf> {
    crate::project::valid_field_id(package).then(|| bases().map(|b| b.join(package))).flatten()
}

/// One layer of a given copy, as [`LAYERS`] lists it.
#[derive(Serialize, Deserialize)]
struct BaseLayer {
    layer: u32,
    /// Its fields' names, in the order the rows hold their values.
    fields: Vec<String>,
    rows: usize,
}

/// A row as the rows file holds it.
#[derive(Deserialize)]
struct BaseRow {
    geometry: Option<Geometry>,
    values: Vec<Value>,
}

/// A row's shape and values as they are written and fingerprinted: `fields[i]` is where the row
/// holds the copy's field `i` (`None`: Null).
struct Print<'a> {
    row: RowRef<'a>,
    fields: &'a [Option<usize>],
}

impl Serialize for Print<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        struct Values<'a>(&'a Print<'a>);
        impl Serialize for Values<'_> {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                let Print { row, fields } = self.0;
                s.collect_seq(fields.iter().map(|j| j.map_or(&Value::Null, |j| row.value(j))))
            }
        }
        use serde::ser::SerializeStruct;
        let mut out = s.serialize_struct("BaseRow", 2)?;
        out.serialize_field("geometry", &self.row.geometry())?;
        out.serialize_field("values", &Values(self))?;
        out.end()
    }
}

/// A 128-bit FNV-1a hash of what is written to it.
struct Fnv(u128);

impl Write for Fnv {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        for b in bytes {
            self.0 = (self.0 ^ u128::from(*b)).wrapping_mul(0x0000_0000_0100_0000_0000_0000_0000_013B);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The fingerprint of a row as [`Print`] writes it. Rows are fingerprinted as they are held once
/// read, given and sent alike, so a value that reads back as it was saved (as every one does)
/// can't make an unedited row look edited.
fn fingerprint(print: &Print<'_>) -> Result<u128, String> {
    let mut hash = Fnv(0x6c62_272e_07bb_0142_62b8_2175_6295_c58d);
    serde_json::to_writer(&mut hash, print).map_err(|e| e.to_string())?;
    Ok(hash.0)
}

/// One row of a layer's index: its id, where it is in the rows file, and its fingerprint.
#[derive(Clone, Copy)]
struct Entry {
    id: u64,
    offset: u64,
    len: u32,
    print: u128,
}

/// Keeps the rows of the feature layers of field project `project` as it was given, under its
/// package's identity, unless this device keeps them already. Nothing is kept for a package without
/// one. The files are written beside the folder and moved into place once complete, so a copy cut
/// short is never read. Reads every row once and writes it once, as unpacking writes the package's
/// files: no Cancel between them.
pub(crate) fn keep_base(project: &Project) -> Result<(), String> {
    let Some(package) = project.field_origin.as_ref().map(|o| o.package.as_str()).filter(|p| !p.is_empty()) else {
        return Ok(());
    };
    let dir = base_dir(package).ok_or("this device gives RustGIS no folder of its own to keep the field package's rows in")?;
    if dir.join(LAYERS).is_file() {
        return Ok(());
    }
    let parent = dir.parent().ok_or("no folder for the field package's rows")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let part = parent.join(format!("{package}.{}.part", crate::project::field_id()?));
    let written = write_base(project, &part).and_then(|()| {
        // A copy left by an earlier try, cut short before its list of layers.
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::rename(&part, &dir).map_err(|e| format!("{}: {e}", dir.display()))
    });
    if written.is_err() {
        let _ = std::fs::remove_dir_all(&part);
    }
    written
}

/// Writes the given copy of `project`'s map into `dir`: see [`keep_base`].
fn write_base(project: &Project, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut layers = Vec::new();
    for l in project.maps.first().into_iter().flat_map(|m| &m.layers).filter(|l| !l.is_raster() && !l.is_streamed()) {
        let fields: Vec<Option<usize>> = (0..l.dataset.fields.len()).map(Some).collect();
        let rows_path = dir.join(format!("{}.rows", l.id));
        let mut rows = std::io::BufWriter::new(std::fs::File::create(&rows_path).map_err(io(&rows_path))?);
        let mut index = Vec::with_capacity(l.dataset.features.len());
        let (mut offset, mut line) = (0u64, Vec::new());
        for row in l.dataset.features.rows() {
            let print = Print { row, fields: &fields };
            line.clear();
            serde_json::to_writer(&mut line, &print).map_err(|e| e.to_string())?;
            let len = u32::try_from(line.len())
                .ok()
                .filter(|n| *n <= MAX_ROW)
                .ok_or_else(|| format!("{}: feature {} is too large to keep a copy of", l.name, row.id()))?;
            rows.write_all(&line).map_err(io(&rows_path))?;
            index.push(Entry { id: row.id(), offset, len, print: fingerprint(&print)? });
            offset += u64::from(len);
        }
        rows.flush().map_err(io(&rows_path))?;
        index.sort_unstable_by_key(|e| e.id);
        if index.windows(2).any(|w| w.first().map(|e| e.id) == w.get(1).map(|e| e.id)) {
            return Err(format!("{}: two features have the same id", l.name));
        }
        let mut bytes = Vec::with_capacity(index.len() * RECORD);
        for e in &index {
            bytes.extend_from_slice(&e.id.to_le_bytes());
            bytes.extend_from_slice(&e.offset.to_le_bytes());
            bytes.extend_from_slice(&e.len.to_le_bytes());
            bytes.extend_from_slice(&e.print.to_le_bytes());
        }
        let index_path = dir.join(format!("{}.index", l.id));
        std::fs::write(&index_path, bytes).map_err(io(&index_path))?;
        layers.push(BaseLayer { layer: l.id, fields: l.dataset.fields.iter().map(|f| f.name.clone()).collect(), rows: index.len() });
    }
    let list = dir.join(LAYERS);
    std::fs::write(&list, serde_json::to_vec(&layers).map_err(|e| e.to_string())?).map_err(io(&list))
}

/// A file error, named by the file.
fn io(path: &Path) -> impl Fn(std::io::Error) -> String + '_ {
    move |e| format!("{}: {e}", path.display())
}

/// Reads a layer's index, refusing one that doesn't hold `rows` records in id order.
fn read_index(path: &Path, rows: usize) -> Result<Vec<Entry>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() != rows.saturating_mul(RECORD) {
        return Err(format!("{}: damaged", path.display()));
    }
    let mut out = Vec::with_capacity(rows);
    for r in bytes.as_chunks::<RECORD>().0 {
        let (id, rest) = r.split_at(8);
        let (offset, rest) = rest.split_at(8);
        let (len, print) = rest.split_at(4);
        let (Ok(id), Ok(offset), Ok(len), Ok(print)) = (id.try_into(), offset.try_into(), len.try_into(), print.try_into()) else {
            return Err(format!("{}: damaged", path.display()));
        };
        let e = Entry {
            id: u64::from_le_bytes(id),
            offset: u64::from_le_bytes(offset),
            len: u32::from_le_bytes(len),
            print: u128::from_le_bytes(print),
        };
        if out.last().is_some_and(|last: &Entry| last.id >= e.id) || e.len > MAX_ROW {
            return Err(format!("{}: damaged", path.display()));
        }
        out.push(e);
    }
    Ok(out)
}

/// Reads row `e` of a rows file.
fn read_row(file: &mut std::fs::File, e: &Entry) -> Result<BaseRow, String> {
    let mut bytes = vec![0; e.len as usize];
    file.seek(SeekFrom::Start(e.offset)).and_then(|_| file.read_exact(&mut bytes)).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| format!("a kept row is damaged: {e}"))
}

// Making the list, on the phone.

/// The changes `map` holds to the rows package `package` gave it, layer by layer; `None` when
/// this device doesn't keep the package's rows (opened elsewhere, or before it kept them). Only
/// rows whose fingerprint changed are read back and compared. A layer removed from the map is
/// left out, as a sync leaves it. Cancel is looked at between rows.
pub(crate) fn list(map: &rustgis_map::Map, package: &str) -> Result<Option<Changes>, String> {
    use rustgis_data::progress;
    let Some(dir) = base_dir(package).filter(|d| d.join(LAYERS).is_file()) else { return Ok(None) };
    let list_path = dir.join(LAYERS);
    let text = std::fs::read(&list_path).map_err(|e| format!("{}: {e}", list_path.display()))?;
    let given: Vec<BaseLayer> = serde_json::from_slice(&text).map_err(|e| format!("{}: {e}", list_path.display()))?;
    let mut changes = Changes { version: VERSION, package: package.to_string(), layers: Vec::new() };
    progress::begin(given.iter().map(|g| g.rows).sum());
    for g in given {
        let Some(layer) = map.layers.iter().find(|l| l.id == g.layer && !l.is_raster()) else { continue };
        let ds = layer.dataset.in_lonlat()?;
        // Where the map holds each of the copy's fields, by name, and the fields added since.
        let fields: Vec<Option<usize>> = g.fields.iter().map(|n| ds.field_index(n)).collect();
        let added_fields: Vec<usize> = (0..ds.fields.len()).filter(|j| !fields.contains(&Some(*j))).collect();
        let index = read_index(&dir.join(format!("{}.index", g.layer)), g.rows)?;
        let mut seen = vec![false; index.len()];
        let (mut changed, mut added) = (Vec::new(), Vec::new());
        for row in ds.features.rows() {
            progress::step(1).map_err(|e| e.to_string())?;
            match index.binary_search_by_key(&row.id(), |e| e.id) {
                Ok(k) => {
                    if let Some(s) = seen.get_mut(k) {
                        *s = true;
                    }
                    let print = fingerprint(&Print { row, fields: &fields })?;
                    if index.get(k).is_none_or(|e| e.print != print) || added_fields.iter().any(|j| !row.value(*j).is_null()) {
                        changed.push((k, row));
                    }
                }
                Err(_) => added.push(row.id()),
            }
        }
        let rows_path = dir.join(format!("{}.rows", g.layer));
        let mut rows = std::fs::File::open(&rows_path).map_err(|e| format!("{}: {e}", rows_path.display()))?;
        let mut edited = Vec::new();
        for (k, row) in changed {
            progress::check().map_err(|e| e.to_string())?;
            let Some(e) = index.get(k) else { continue };
            let base = read_row(&mut rows, e)?;
            let mut values = Vec::new();
            for ((name, j), was) in g.fields.iter().zip(&fields).zip(&base.values) {
                // A field removed in the field is listed as removed, not by value.
                if j.is_some_and(|j| row.value(j) != was) {
                    values.push((name.clone(), was.clone()));
                }
            }
            for j in &added_fields {
                if let Some(f) = ds.fields.get(*j).filter(|_| !row.value(*j).is_null()) {
                    values.push((f.name.clone(), Value::Null));
                }
            }
            let geometry = (row.geometry() != base.geometry.as_ref()).then_some(base.geometry);
            if !values.is_empty() || geometry.is_some() {
                edited.push(Given { id: e.id, values, geometry });
            }
        }
        let mut deleted = Vec::new();
        for (e, _) in index.iter().zip(&seen).filter(|(_, seen)| !**seen) {
            progress::check().map_err(|e| e.to_string())?;
            let base = read_row(&mut rows, e)?;
            let values = g.fields.iter().cloned().zip(base.values).collect();
            deleted.push(Given { id: e.id, values, geometry: Some(base.geometry) });
        }
        let removed: Vec<String> = g.fields.iter().zip(&fields).filter(|(_, j)| j.is_none()).map(|(n, _)| n.clone()).collect();
        if !(added.is_empty() && deleted.is_empty() && edited.is_empty() && removed.is_empty()) {
            changes.layers.push(LayerChanges { layer: g.layer, added, deleted, edited, removed });
        }
    }
    Ok(Some(changes))
}

// Applying the list, at home.

/// Applies `changes`, of a package of offline area `ai` whose map is `edited`, to the area's source
/// map, as the module describes, and renews the area from it. Refused, changing nothing, when a
/// change is to a field the source map hasn't, or has with another type. Says by feature which
/// changes replaced one made at home, which deletions were kept for one made at home, and which
/// edits were dropped for a deletion at home; and the layers no longer there. A feature added in the
/// field keeps its id unless the source map uses it, else takes the next free one (`renumbered`).
///
/// It runs on the window's thread, as Bring Back does: one pass over each changed layer of the
/// source and of the package's map, the rest as large as the changes.
pub(crate) fn apply(s: &mut Session, ai: usize, edited: &rustgis_map::Map, changes: &Changes) -> Result<serde_json::Value, String> {
    let area = s.project.offline_areas.get(ai).ok_or("no such area")?;
    let (area_name, source) = (area.name.clone(), area.map.clone());
    let offline_map = super::workspace::area_map_index(&s.project, ai);
    let si = s
        .project
        .maps
        .iter()
        .position(|m| m.name.eq_ignore_ascii_case(&source))
        .ok_or_else(|| format!("the source map '{source}' is gone"))?;
    crate::project::ensure_map_imports_complete(s.project.maps.get(si).ok_or("the source map is gone")?)?;
    let next_generation =
        s.project.offline_areas.get(ai).is_some_and(|a| !a.generation.is_empty()).then(crate::project::field_id).transpose()?;
    let src = s.project.maps.get(si).ok_or("the source map is gone")?;
    // Every change to a field the source can hold, checked before anything changes.
    let (mut misfits, mut removed) = (Vec::new(), Vec::new());
    for lc in &changes.layers {
        let (Some(ol), Some(sl)) = (
            edited.layers.iter().find(|l| l.id == lc.layer && !l.is_raster()),
            src.layers.iter().find(|l| l.id == lc.layer && !l.is_raster()),
        ) else {
            continue;
        };
        let mut names: HashSet<String> = lc.edited.iter().flat_map(|g| g.values.iter().map(|(n, _)| n.to_ascii_lowercase())).collect();
        if !lc.added.is_empty() {
            let added: HashSet<u64> = lc.added.iter().copied().collect();
            for row in ol.dataset.features.rows().filter(|r| added.contains(&r.id())) {
                for (f, v) in ol.dataset.fields.iter().zip(row.values()) {
                    if !v.is_null() {
                        names.insert(f.name.to_ascii_lowercase());
                    }
                }
            }
        }
        let mut cut: Vec<String> = names
            .into_iter()
            .filter(|n| {
                let theirs = ol.dataset.field_index(n).and_then(|i| ol.dataset.fields.get(i)).map(|f| f.ty);
                let ours = sl.dataset.field_index(n).and_then(|j| sl.dataset.fields.get(j)).map(|f| f.ty);
                ours.is_none() || theirs.is_some_and(|t| Some(t) != ours)
            })
            .collect();
        cut.sort();
        if !cut.is_empty() {
            misfits.push(format!("'{}' ({})", ol.name, cut.join(", ")));
        }
        let mut gone: Vec<&str> = lc.removed.iter().filter(|n| sl.dataset.field_index(n).is_some()).map(String::as_str).collect();
        gone.sort_unstable();
        if !gone.is_empty() {
            removed.push(format!("'{}' ({})", ol.name, gone.join(", ")));
        }
    }
    if !misfits.is_empty() {
        return Err(format!(
            "cannot bring back the field's changes: the map '{source}' hasn't the field(s) they change, or has them with another type, in layer(s) {}. Nothing was changed; add the fields, then Bring Back again",
            misfits.join("; ")
        ));
    }
    // The field's values in a field it removed aren't known, so none can be brought back.
    if !removed.is_empty() {
        return Err(format!(
            "cannot bring back the field's changes: the field package removed field(s) the map '{source}' still has, in layer(s) {}, so their values in the field aren't known. Nothing was changed; add the fields back in the field project and send it again",
            removed.join("; ")
        ));
    }
    let mut done = Applied::default();
    let tag = |layer: &str, id: u64| format!("{layer} #{id}");
    let src = s.project.maps.get_mut(si).ok_or("the source map is gone")?;
    for lc in &changes.layers {
        let Some(ol) = edited.layers.iter().find(|l| l.id == lc.layer && !l.is_raster()) else {
            done.unknown += 1;
            continue;
        };
        let Some(sl) = src.layers.iter_mut().find(|l| l.id == lc.layer && !l.is_raster()) else {
            done.gone.push(ol.name.clone());
            continue;
        };
        let name = sl.name.clone();
        let error = |e: String| super::workspace::sync_error(&ol.name, e);
        let theirs = ol.dataset.in_lonlat().map_err(error)?;
        // The source is changed in longitude/latitude, as the package holds its features, and its
        // own coordinates put back after, those it left as they were as stored.
        let held = (sl.dataset.coord_crs() != rustgis_data::raster::Crs::LonLat).then(|| sl.dataset.clone());
        if held.is_some() {
            sl.dataset = sl.dataset.in_lonlat().map_err(error)?.into_owned();
        }
        let wanted: HashSet<u64> =
            lc.added.iter().copied().chain(lc.deleted.iter().map(|g| g.id)).chain(lc.edited.iter().map(|g| g.id)).collect();
        let at_field: HashMap<u64, usize> =
            theirs.features.rows().enumerate().filter(|(_, r)| wanted.contains(&r.id())).map(|(i, r)| (r.id(), i)).collect();
        let mut at: HashMap<u64, usize> = HashMap::new();
        let mut top = 0;
        for (i, r) in sl.dataset.features.rows().enumerate() {
            top = top.max(r.id());
            if wanted.contains(&r.id()) {
                at.insert(r.id(), i);
            }
        }
        let field_value =
            |row: usize, field: &str| theirs.field_index(field).map_or(&Value::Null, |i| theirs.features.value(row, i)).clone();
        let mut written = HashSet::new();
        for g in &lc.edited {
            let Some(&r) = at.get(&g.id) else {
                done.skipped += 1;
                done.dropped.push(tag(&name, g.id));
                continue;
            };
            let Some(&p) = at_field.get(&g.id) else {
                done.unknown += 1;
                continue;
            };
            let (mut changed, mut conflict) = (false, false);
            for (field, was) in &g.values {
                let Some(j) = sl.dataset.field_index(field) else { continue };
                let new = field_value(p, field);
                let now = sl.dataset.features.value(r, j);
                if now != &new {
                    conflict |= now != was;
                    sl.dataset.features.set_value(r, j, new);
                    changed = true;
                }
            }
            if let Some(was) = &g.geometry {
                let new = theirs.features.geometry(p);
                let now = sl.dataset.features.geometry(r);
                if now != new {
                    conflict |= now != was.as_ref();
                    sl.dataset.features.set_geometry(r, new.cloned());
                    changed = true;
                }
            }
            if changed {
                done.updated += 1;
                written.insert(g.id);
            }
            if conflict {
                done.conflicts.push(tag(&name, g.id));
            }
        }
        // A deletion goes in while the feature is as it was given; one changed at home stays.
        let mut gone = HashSet::new();
        for g in &lc.deleted {
            let Some(row) = at.get(&g.id).and_then(|r| sl.dataset.features.row(*r)) else { continue };
            let same_place = g.geometry.as_ref().is_none_or(|was| row.geometry() == was.as_ref());
            let same_values = g.values.iter().all(|(field, was)| sl.dataset.field_index(field).is_none_or(|j| row.value(j) == was));
            if same_place && same_values {
                gone.insert(g.id);
            } else {
                done.kept.push(tag(&name, g.id));
            }
        }
        if !gone.is_empty() {
            let before = sl.dataset.features.len();
            sl.dataset.features.retain_rows(|r| !gone.contains(&r.id()));
            done.deleted += before - sl.dataset.features.len();
        }
        for id in &lc.added {
            let Some(row) = at_field.get(id).and_then(|p| theirs.features.row(*p)) else {
                done.unknown += 1;
                continue;
            };
            let own = if at.contains_key(id) {
                done.renumbered += 1;
                top.saturating_add(1)
            } else {
                *id
            };
            top = top.max(own);
            at.insert(own, sl.dataset.features.len());
            let mut attrs = vec![Value::Null; sl.dataset.fields.len()];
            for (f, v) in theirs.fields.iter().zip(row.values()) {
                if let Some(cell) = sl.dataset.field_index(&f.name).and_then(|j| attrs.get_mut(j)) {
                    *cell = v.clone();
                }
            }
            sl.dataset.features.push_parts(own, row.geometry().cloned(), attrs);
            written.insert(own);
            done.added += 1;
        }
        if let Some(own) = held {
            let merged = std::mem::replace(&mut sl.dataset, own);
            sl.dataset = sl.dataset.merged_from_lonlat(merged, &written).map_err(error)?;
        }
        sl.selection.clear();
        sl.touch_geometry();
    }
    super::workspace::renew_area(s, ai, si, offline_map, next_generation)?;
    done.say(s, &area_name, &source);
    Ok(json!({
        "updated": done.updated,
        "added": done.added,
        "deleted": done.deleted,
        "skipped": done.skipped,
        "kept": done.kept.len(),
        "conflicts": done.conflicts.len() + done.kept.len(),
        "renumbered": done.renumbered,
    }))
}

/// What [`apply`] did, and what it says.
#[derive(Default)]
struct Applied {
    updated: usize,
    added: usize,
    deleted: usize,
    /// Edits to features deleted at home meanwhile, left deleted.
    skipped: usize,
    renumbered: usize,
    /// Changes naming a layer or feature the package's map hasn't.
    unknown: usize,
    /// Features edited at home and in the field, as `layer #id`: the field's values went in.
    conflicts: Vec<String>,
    /// Features deleted in the field but changed at home, kept.
    kept: Vec<String>,
    /// Features edited in the field but deleted at home, whose edits were dropped.
    dropped: Vec<String>,
    /// Layers changed in the field that the source map no longer has.
    gone: Vec<String>,
}

impl Applied {
    /// Says, one notice of each kind, what didn't go in as the field left it.
    fn say(&self, s: &mut Session, area: &str, source: &str) {
        let listed = |ids: &[String]| {
            let mut said: Vec<String> = ids.iter().take(20).cloned().collect();
            if ids.len() > 20 {
                said.push(format!("{} more", ids.len() - 20));
            }
            said.join(", ")
        };
        if !self.conflicts.is_empty() {
            s.notify(
                format!(
                    "Offline area '{area}': {} feature(s) were changed in the map '{source}' since the field package was made and also in the field, so the field's values replaced the map's changes: {}. Undo restores the map's versions",
                    self.conflicts.len(),
                    listed(&self.conflicts)
                ),
                false,
            );
        }
        if !self.kept.is_empty() {
            s.notify(
                format!(
                    "Offline area '{area}': {} feature(s) deleted in the field were changed in the map '{source}' since the field package was made, so they were kept: {}",
                    self.kept.len(),
                    listed(&self.kept)
                ),
                false,
            );
        }
        if !self.dropped.is_empty() {
            s.notify(
                format!(
                    "Offline area '{area}': {} feature(s) edited in the field were deleted from the map '{source}' meanwhile, so those edits were dropped: {}",
                    self.dropped.len(),
                    listed(&self.dropped)
                ),
                false,
            );
        }
        if !self.gone.is_empty() {
            s.notify(
                format!(
                    "Offline area '{area}': {} layer(s) changed in the field are no longer in the map '{source}', so their changes weren't brought back: {}",
                    self.gone.len(),
                    self.gone.join(", ")
                ),
                true,
            );
        }
        if self.unknown > 0 {
            s.notify(
                format!(
                    "Offline area '{area}': {} change(s) in the field package name a layer or feature its map hasn't, so they were left out: the package may be damaged",
                    self.unknown
                ),
                true,
            );
        }
    }
}

/// In tests, given copies go in a folder of each test's own thread, removed when it ends.
#[cfg(test)]
pub(crate) mod test_bases {
    use std::path::PathBuf;

    struct Root(PathBuf);

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    thread_local! {
        static ROOT: Root = {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Root(std::env::temp_dir().join(format!("rustgis-field-bases-{}-{n}", std::process::id())))
        };
    }

    /// This thread's folder.
    pub(crate) fn root() -> Option<PathBuf> {
        ROOT.try_with(|r| r.0.clone()).ok()
    }
}
