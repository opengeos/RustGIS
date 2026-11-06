//! Undoable replacement of linked local data without replacing the layer.

use crate::{CmdResult, CommandSpec as C, Session};
use serde_json::Value;
#[cfg(not(target_arch = "wasm32"))]
use serde_json::json;

pub fn commands() -> Vec<C> {
    vec![
        C::new("layer.repairSource", "Repair Data Source", "Data › Data Source", repair_source).params(
            r#"{"layer"?: name|id (default: the active layer), "path": "the file or folder (a Zarr store, a File Geodatabase) the layer's data moved to", "table"?: "the dataset in a GeoPackage, GDAL or DuckDB file, or a raster's subdataset (default: the one the layer reads now)"}"#),
        C::new("layer.reloadData", "Reload Data", "Data › Data Source", reload_data).params(
            r#"{"layer"?: "name|id (default: the active layer). Manually rereads linked local data, retaining presentation. Folder sources are not monitored; snapshots must be added again.", "discardEdits"?: "bool (default: false): explicitly discard pending GeoPackage edits. Undo restores in-memory edits and configuration, not overwritten files or historical streamed contents."}"#),
        C::new("map.repairTiles", "Repair Tile Archive", "Layers", repair_tiles).params(
            r#"{"name"?: text | "index"?: number (0 = top) | "id"?: catalog id, "path": "the .pmtiles or .mbtiles file the archive moved to"}"#),
    ]
}

#[cfg(not(target_arch = "wasm32"))]
fn replacement(p: &Value) -> Result<String, String> {
    let path = super::layer::s_req(p, "path")?.trim();
    if !rustgis_map::paths::is_local(path) {
        return Err(format!("{path}: give a file or folder on this computer (web addresses and databases aren't repaired here)"));
    }
    let path = rustgis_map::paths::local_path(path);
    let full = std::path::absolute(path).map_err(|e| format!("{path}: {e}"))?.display().to_string();
    if !std::path::Path::new(&full).exists() {
        return Err(format!("{full}: there's no such file or folder"));
    }
    Ok(full)
}

/// Reopens linked data after compatibility checks; undo restores the source and any reset label moves.
fn repair_source(s: &mut Session, p: &Value) -> CmdResult {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, p);
        Err("the web app can't link to files on this computer: add the file again with Add Data".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let id = s.layer_id(p)?;
        let full = replacement(p)?;
        let table = super::ps(p, "table").map(str::trim).filter(|t| !t.is_empty());
        let l = s.map().layer(id).ok_or("the layer no longer exists")?;
        if l.linked_file().is_none() {
            return Err(format!(
                "{}'s data is kept in the project or read from a database or web address: there's no file to repair",
                l.name
            ));
        }
        replace_linked(s, id, &full, table)?;
        Ok(json!({"layer": id, "path": full, "table": s.map().layer(id).and_then(|l| l.linked_table())}))
    }
}

/// Manual reload deliberately does not infer a link from a snapshot's historical dataset.source.
fn reload_data(s: &mut Session, p: &Value) -> CmdResult {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, p);
        Err("the web app can't reload local files on this computer: add the file again with Add Data".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let id = s.layer_id(p)?;
        let l = s.map().layer(id).ok_or("the layer no longer exists")?;
        let path = l.gpkg.as_ref().map(|g| g.path.as_str()).or_else(|| l.linked_file())
            .filter(|p| rustgis_map::paths::is_local(p))
            .ok_or_else(|| format!("{}: no linked local source to reload. Add snapshots again with Add Data; use database discard or remote refresh for server data", l.name))?
            .to_string();
        let table = l.gpkg.as_ref().map(|g| g.table.as_str()).or_else(|| l.linked_table()).map(String::from);
        let result = if let Some(edit) = &l.gpkg {
            let changes = edit.versions.changes(&l.dataset.features);
            if !changes.is_empty() && super::b(p, "discardEdits") != Some(true) {
                return Err(format!(
                    "{}: {} has {} inserts, {} updates and {} deletes pending; nothing was reloaded or saved. Confirm Discard Edits and Reload, or pass discardEdits:true",
                    l.name,
                    path,
                    changes.inserts.len(),
                    changes.updates.len(),
                    changes.deletes.len()
                ));
            }
            reload_editable(s, id, &path)
        } else {
            replace_linked(s, id, &path, table.as_deref())
        };
        result.map_err(|why| {
            format!(
                "{} ({path}): reload failed; existing data and configuration retained. Retry Reload Data or use Repair Data Source. {why}",
                s.map().layer(id).map_or("Layer", |l| l.name.as_str())
            )
        })?;
        s.notify(format!("{}: data reloaded from {path}", s.map().layer(id).map_or("Layer", |l| l.name.as_str())), false);
        Ok(json!({"layer": id, "path": path, "table": table}))
    }
}

/// Repair and reload use exactly the same fresh-candidate and compatibility policy.
#[cfg(not(target_arch = "wasm32"))]
fn replace_linked(s: &mut Session, id: u32, path: &str, table: Option<&str>) -> Result<(), String> {
    let l = s.map().layer(id).ok_or("the layer no longer exists")?;
    if let Some(rl) = &l.raster {
        let new = if rl.time_zarr.is_some() { super::zarr::relink_following(s, rl, path)? } else { rl.relinked(path, table)? };
        let name = l.name.clone();
        if let Some(data) = new.data.as_deref() {
            super::raster::read_notices(s, &name, data);
        }
        let l = s.map_mut().layer_mut(id).ok_or("the layer no longer exists")?;
        l.raster = Some(new);
        l.selection.clear();
        l.touch_geometry();
    } else {
        let src = l.source.as_ref().ok_or("the layer has no linked source")?;
        let new = src.relinked(path, table)?;
        let info = new.info().ok_or_else(|| format!("{path}: the source did not open"))?;
        compatible(l, info.geometry_type, &info.fields, path)?;
        let notes = info.notes.clone();
        let l = s.map_mut().layer_mut(id).ok_or("the layer no longer exists")?;
        let moved = l.replace_source(new);
        reset_labels(s, id, moved);
        for note in notes {
            s.notify(format!("{}: {note}", s.map().layer(id).map_or("Layer", |l| l.name.as_str())), false);
        }
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn reload_editable(s: &mut Session, id: u32, path: &str) -> Result<(), String> {
    let l = s.map().layer(id).ok_or("the layer no longer exists")?;
    let edit = l.gpkg.as_ref().ok_or("the layer has no editable GeoPackage source")?;
    let super::gpkg::Candidate { dataset, edit, notes, .. } = super::gpkg::read_candidate(&l.dataset.name, edit)?;
    compatible(l, dataset.geometry_type, &dataset.fields, path)?;
    let l = s.map_mut().layer_mut(id).ok_or("the layer no longer exists")?;
    let moved = l.labels.in_order().map(|class| class.moved.len()).sum();
    l.labels.default.moved.clear();
    l.labels.classes.iter_mut().for_each(|class| class.moved.clear());
    l.dataset = dataset;
    l.gpkg = Some(edit);
    l.selection.clear();
    l.touch_geometry();
    reset_labels(s, id, moved);
    for note in notes {
        s.notify(format!("{}: {note}", s.map().layer(id).map_or("Layer", |l| l.name.as_str())), false);
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn reset_labels(s: &mut Session, id: u32, moved: usize) {
    // A partial lock hides all labels not named in it, so reset the entire lock, as repair does.
    s.map_mut().label_lock = None;
    s.map_mut().label_lock_line_anchors.clear();
    if moved > 0 {
        let name = s.map().layer(id).map_or("Layer", |l| l.name.as_str());
        s.notify(format!("{moved} saved label move(s) reset in {name}: replacement data may reuse feature ids; undo restores them"), false);
    }
}

/// Check configured fields and expressions against declarations, not rows: valid empty data works.
#[cfg(not(target_arch = "wasm32"))]
fn compatible(
    l: &rustgis_map::Layer,
    geometry: rustgis_data::GeometryType,
    fields: &[rustgis_data::Field],
    path: &str,
) -> Result<(), String> {
    use rustgis_data::{Dataset, GeometryType, query};
    if l.dataset.geometry_type != GeometryType::None && geometry != l.dataset.geometry_type {
        return Err(format!(
            "{path} holds {} features; {} draws {} features: choose a file of {} features",
            geometry.label(),
            l.name,
            l.dataset.geometry_type.label(),
            l.dataset.geometry_type.label()
        ));
    }
    let mut required: Vec<&str> = l.renderer.field().into_iter().collect();
    required.extend(l.marker_rotation.as_ref().map(|m| m.field.as_str()));
    required.extend(l.extrusion.as_ref().and_then(|e| e.field.as_deref()));
    if let Some(d) = &l.diagram {
        required.extend(d.categories.iter().map(|c| c.field.as_str()));
    }
    if l.labels.enabled {
        required.extend(l.labels.in_order().filter(|c| c.by.text.is_none()).map(|c| c.field.as_str()));
    }
    required.extend(l.time.as_ref().map(|t| t.field.as_str()));
    required.retain(|f| l.dataset.fields.iter().any(|old| old.name.eq_ignore_ascii_case(f)));
    required.sort_unstable();
    required.dedup();
    let missing: Vec<_> = required.into_iter().filter(|f| !fields.iter().any(|new| new.name.eq_ignore_ascii_case(f))).collect();
    if !missing.is_empty() {
        return Err(format!(
            "{path} has no field {}, which {}'s symbology, labels or time slider use: choose a file that has them",
            missing.join(", "),
            l.name
        ));
    }
    let candidate = Dataset::new("", geometry, fields.to_vec());
    let check = |expression: &str| -> Result<(), String> {
        // Existing broken expressions are not newly invalidated by replacing the source.
        if query::parse(expression, &l.dataset).is_ok()
            && let Err(why) = query::parse(expression, &candidate)
        {
            return Err(format!(
                "{path} doesn't fit {}'s configured expression `{expression}` ({why}): choose a file that has its fields",
                l.name
            ));
        }
        Ok(())
    };
    let by = &l.symbol_by;
    for e in [
        l.renderer.expression(),
        l.definition_query.as_deref(),
        by.size.as_deref(),
        by.fill.as_deref(),
        by.stroke.as_deref(),
        by.stroke_width.as_deref(),
        by.angle.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        check(e)?;
    }
    for filter in &l.display_filters {
        check(&filter.where_)?;
    }
    if l.labels.enabled {
        for class in l.labels.in_order() {
            let by = &class.by;
            for e in [class.where_.as_deref(), by.text.as_deref(), by.size.as_deref(), by.color.as_deref(), by.angle.as_deref()]
                .into_iter()
                .flatten()
            {
                check(e)?;
            }
        }
    }
    Ok(())
}

fn repair_tiles(s: &mut Session, p: &Value) -> CmdResult {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, p);
        Err("the web app can't link to files on this computer: add the file again with Add Data".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use rustgis_render::rustgis_tiles::{cache, mbtiles, pmtiles};
        let i = super::tiles::find(s, p)?.ok_or("name, index or id is required")?;
        let t = s.map().tiles.get(i).ok_or("the tile layer no longer exists")?;
        if t.archive_file().is_none() {
            return Err(format!("{}: only a tile archive read from a file (PMTiles or MBTiles) can be repaired", t.name));
        }
        let full = replacement(p)?;
        let mut new = t.clone();
        new.url = full.clone();
        new.error = None;
        new.repair()?;
        let vector = match new.kind {
            rustgis_map::TileKind::MbTiles => mbtiles::info(&full)?.format.is_vector(),
            _ => pmtiles::info_blocking(&full, web_time::Duration::from_secs(10))?.tile_type.is_vector(),
        };
        if (!t.layers.is_empty() || !t.style_layers.is_empty()) && !vector {
            return Err(format!(
                "{full} holds raster tiles; {} draws vector tiles with its own layers or style: choose a vector tile archive",
                t.name
            ));
        }
        cache::forget(&t.url);
        let slot = s.map_mut().tiles.get_mut(i).ok_or("the tile layer no longer exists")?;
        *slot = new;
        Ok(json!({"index": i, "path": full}))
    }
}
