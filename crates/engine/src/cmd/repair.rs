//! Undoable replacement of linked local data without replacing the layer.

use crate::{CmdResult, CommandSpec as C, Session};
use serde_json::Value;
#[cfg(not(target_arch = "wasm32"))]
use serde_json::json;

pub fn commands() -> Vec<C> {
    vec![
        C::new("layer.repairSource", "Repair Data Source", "Data › Data Source", repair_source).params(
            r#"{"layer"?: name|id (default: the active layer), "path": "the file or folder (a Zarr store, a File Geodatabase) the layer's data moved to", "table"?: "the dataset in a GeoPackage, GDAL or DuckDB file, or a raster's subdataset (default: the one the layer reads now)"}"#),
        C::new("map.repairTiles", "Repair Tile Archive", "Layers", repair_tiles).params(
            r#"{"name"?: text | "index"?: number (0 = top) | "id"?: catalog id, "path": "the .pmtiles or .mbtiles file the archive moved to"}"#),
        C::new("map.reloadTiles", "Reload Data", "Layers", reload_tiles).params(
            r#"{"name": text | "index": number (0 = top) | "id": catalog id} (give one selector) — rereads a local PMTiles or MBTiles archive at its linked path, keeping its presentation; undo cannot restore overwritten file contents"#),
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
        if let Some(rl) = &l.raster {
            let new = if rl.time_zarr.is_some() { super::zarr::relink_following(s, rl, &full)? } else { rl.relinked(&full, table)? };
            let l = s.map_mut().layer_mut(id).ok_or("the layer no longer exists")?;
            l.raster = Some(new);
            l.selection.clear();
            l.touch_geometry();
        } else {
            let src = l.source.as_ref().ok_or("the layer has no linked source")?;
            let new = src.relinked(&full, table)?;
            if let Some(info) = new.info() {
                if l.dataset.geometry_type != rustgis_data::GeometryType::None && info.geometry_type != l.dataset.geometry_type {
                    return Err(format!(
                        "{full} holds {} features; {} draws {} features: choose a file of {} features",
                        info.geometry_type.label(),
                        l.name,
                        l.dataset.geometry_type.label(),
                        l.dataset.geometry_type.label()
                    ));
                }
                let mut required: Vec<&str> = l.renderer.field().into_iter().collect();
                if l.labels.enabled {
                    required.push(&l.labels.default.field);
                    required.extend(l.labels.classes.iter().map(|c| c.field.as_str()));
                }
                if let Some(time) = &l.time {
                    required.push(&time.field);
                }
                required.retain(|f| l.dataset.fields.iter().any(|old| old.name.eq_ignore_ascii_case(f)));
                required.sort_unstable();
                required.dedup();
                let missing: Vec<&str> =
                    required.into_iter().filter(|f| !info.fields.iter().any(|new| new.name.eq_ignore_ascii_case(f))).collect();
                if !missing.is_empty() {
                    return Err(format!(
                        "{full} has no field {}, which {}'s symbology, labels or time slider use: choose a file that has them",
                        missing.join(", "),
                        l.name
                    ));
                }
                // Classes by an expression that reads on the old fields must read on the new ones.
                if let Some(e) = l.renderer.expression().filter(|e| rustgis_data::query::parse(e, &l.dataset).is_ok())
                    && let Err(why) =
                        rustgis_data::query::parse(e, &rustgis_data::Dataset::new("", info.geometry_type, info.fields.clone()))
                {
                    return Err(format!(
                        "{full} doesn't fit {}'s symbology, which classes by `{e}` ({why}): choose a file that has its fields",
                        l.name
                    ));
                }
            }
            let l = s.map_mut().layer_mut(id).ok_or("the layer no longer exists")?;
            let moved = l.replace_source(new);
            let name = l.name.clone();
            // A lock's feature ids and callout offsets belong to the old source too. The whole
            // lock goes, not just this layer's entries: while locked only the labels it names are
            // drawn, so dropping this layer's alone would hide its labels until the lock is
            // taken again. Cleared, every label is locked again where it is placed next.
            s.map_mut().label_lock = None;
            s.map_mut().label_lock_line_anchors.clear();
            if moved > 0 {
                s.notify(
                    format!("{moved} saved label move(s) reset in {name}: replacement data may reuse feature ids; undo restores them"),
                    false,
                );
            }
        }
        Ok(json!({"layer": id, "path": full, "table": s.map().layer(id).and_then(|l| l.linked_table())}))
    }
}

fn repair_tiles(s: &mut Session, p: &Value) -> CmdResult {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, p);
        Err("the web app can't link to files on this computer: add the file again with Add Data".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let i = super::tiles::find(s, p)?.ok_or("name, index or id is required")?;
        let t = s.map().tiles.get(i).ok_or("the tile layer no longer exists")?;
        if t.archive_file().is_none() {
            return Err(format!("{}: only a tile archive read from a file (PMTiles or MBTiles) can be repaired", t.name));
        }
        let full = replacement(p)?;
        let new = archive_candidate(t, &full)?;
        rustgis_render::rustgis_tiles::cache::reload_archive(&t.url);
        // The replacement path may already have been drawn by another layer.
        if t.url != full {
            rustgis_render::rustgis_tiles::cache::reload_archive(&full);
        }
        let slot = s.map_mut().tiles.get_mut(i).ok_or("the tile layer no longer exists")?;
        *slot = new;
        Ok(json!({"index": i, "path": full}))
    }
}

/// Repair and reload share one validation policy. Reading a candidate must not evict retained
/// tiles/metadata or reuse a connection to an archive replaced at the same path.
#[cfg(not(target_arch = "wasm32"))]
fn archive_candidate(t: &rustgis_map::TileLayer, path: &str) -> Result<rustgis_map::TileLayer, String> {
    use rustgis_render::rustgis_tiles::{mbtiles, pmtiles};
    let mut new = t.clone();
    new.url = path.to_string();
    new.error = None;
    new.repair()?;
    let vector = match new.kind {
        rustgis_map::TileKind::MbTiles => mbtiles::read_info_local(path)?.format.is_vector(),
        _ => pmtiles::read_info_local(path)?.tile_type.is_vector(),
    };
    if (!t.layers.is_empty() || !t.style_layers.is_empty()) && !vector {
        return Err(format!(
            "{path} holds raster tiles; {} draws vector tiles with its own layers or style: choose a vector tile archive",
            t.name
        ));
    }
    Ok(new)
}

fn reload_tiles(s: &mut Session, p: &Value) -> CmdResult {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (s, p);
        Err("the web app can't reload files on this computer: add the file again with Add Data".into())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let i = super::tiles::find(s, p)?.ok_or("name, index or id is required")?;
        let t = s.map().tiles.get(i).ok_or("the tile layer no longer exists")?;
        let path = t.archive_file().ok_or_else(|| {
            format!(
                "{}: Reload Data needs a linked local PMTiles or MBTiles file; add snapshots/uploads again, or use Reload Web Layers for web addresses",
                t.name
            )
        })?.to_string();
        let new = archive_candidate(t, &path).map_err(|e| {
            format!("{} ({path}): {e}; correct the file and retry Reload Data, or use Repair Data Source if it moved", t.name)
        })?;
        rustgis_render::rustgis_tiles::cache::reload_archive(&path);
        let slot = s.map_mut().tiles.get_mut(i).ok_or("the tile layer no longer exists")?;
        *slot = new;
        let message = format!("Reloaded {} from {path}; undo cannot restore overwritten file contents", slot.name);
        s.notify(message, false);
        Ok(json!({"index": i, "path": path}))
    }
}
