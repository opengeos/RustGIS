//! Opens a GeoLibre project as a new map, resolving sources through existing commands.
use super::ps;
use crate::{CmdResult, CommandSpec as C, Session};
use rustgis_map::geolibre::{self, Report, array, boolean, number, string};
use serde_json::{Value, json};

/// The importer reads files and URLs named inside the project, so callers require approval.
pub fn commands() -> Vec<C> {
    vec![C::new("project.importGeolibre", "Import GeoLibre Project", "Project › Import", import_project).params(
        r#"{"path"?: "*.geolibre|*.geolibre.json", "url"?: "https://…", "text"?: "project JSON", "data"?: "base64 JSON", "dir"?: "folder for local data"} → {map, layers, notes}"#,
    )]
}

/// Reads at most 64 MiB, before decoding or parsing hostile project data.
fn project_text(p: &Value) -> Result<String, String> {
    use std::io::Read;
    let bytes = if let Some(data) = ps(p, "data") {
        if data.len() > geolibre::MAX_TEXT / 3 * 4 + 4 {
            return Err("GeoLibre data exceeds 64 MiB".into());
        }
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.decode(data.trim()).map_err(|e| format!("data must be base64: {e}"))?
    } else if let Some(text) = ps(p, "text") {
        if text.len() > geolibre::MAX_TEXT {
            return Err("GeoLibre text exceeds 64 MiB".into());
        }
        return Ok(text.into());
    } else if let Some(url) = ps(p, "url") {
        if !rustgis_map::tiles::is_web(url) {
            return Err("url must be http:// or https://".into());
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            crate::render::rustgis_tiles::cache::download_blocking_limited(&geolibre::download_url(url), geolibre::MAX_TEXT)?
        }
        #[cfg(target_arch = "wasm32")]
        {
            return Err("pass downloaded GeoLibre JSON as text or data".into());
        }
    } else if let Some(path) = ps(p, "path") {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .and_then(|f| f.take(geolibre::MAX_TEXT as u64 + 1).read_to_end(&mut bytes))
            .map_err(|e| format!("{path}: {e}"))?;
        bytes
    } else {
        return Err("path, url, text or data is required".into());
    };
    if bytes.len() > geolibre::MAX_TEXT {
        return Err("GeoLibre project exceeds 64 MiB".into());
    }
    String::from_utf8(bytes).map_err(|_| "GeoLibre project must be UTF-8 JSON".into())
}

/// Parses before mutating the session; an unreadable layer is noted and the others continue.
fn import_project(s: &mut Session, p: &Value) -> CmdResult {
    #[cfg(target_arch = "wasm32")]
    if ps(p, "url").is_some() && ps(p, "text").is_none() && ps(p, "data").is_none() {
        let mut q = p.clone();
        q["geolibreProject"] = json!(true);
        if let Some(url) = ps(p, "url") {
            if !rustgis_map::tiles::is_web(url) {
                return Err("url must be http:// or https://".into());
            }
            q["url"] = json!(geolibre::download_url(url));
        }
        return s.request_url(&q);
    }
    let text = project_text(p)?;
    let mut report = Report::default();
    let project = geolibre::read_project(&text, &mut report)?;
    let dir = ps(p, "dir").filter(|d| !d.trim().is_empty()).map(str::to_string).or_else(|| {
        ps(p, "path")
            .and_then(|p| std::path::Path::new(p).parent())
            .map(|d| if d.as_os_str().is_empty() { ".".into() } else { d.to_string_lossy().into_owned() })
    });
    s.run_part("map.new", &json!({"name":project.name}))?;
    s.map_mut().basemap = rustgis_map::Basemap::Blank;
    s.map_mut().graticule = false;
    if !project.basemap.is_empty() {
        if let Err(e) = s.run_part(
            "map.addTiles",
            &json!({"url":project.basemap,"kind":"style","name":"GeoLibre background","opacity":project.basemap_opacity}),
        ) {
            report.skipped("", format!("basemap: {e}"));
        }
        if let Some(t) = s.map_mut().tiles.last_mut() {
            t.visible = project.basemap_visible;
        }
    }
    let mut added = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<u32>> = Default::default();
    let mut order = Vec::new();
    for record in project.layers.iter().rev() {
        let name = string(record, "name");
        if !record.is_object() {
            report.skipped("", "malformed layer record");
            continue;
        }
        let mut record = record.clone();
        // Feature layers retain their own settings; their flat RustGIS folder applies the
        // inherited settings. Tiles cannot belong to folders, so bake the effective look there.
        let kind = string(&record, "type");
        let source = &record["source"];
        let address = if string(&record, "sourcePath").is_empty() { string(source, "url") } else { string(&record, "sourcePath") };
        let tile = matches!(kind, "xyz" | "wms" | "wmts" | "vector-tiles" | "pmtiles" | "mbtiles")
            || (kind == "raster" && !array(source, "tiles").is_empty())
            || (matches!(kind, "cog" | "raster") && super::layer::is_tiff_url(address) && rustgis_map::tiles::is_web(address));
        if tile {
            let (visible, opacity) = group_look(&project.groups, string(&record, "groupId"));
            record["opacity"] = json!(number(&record, "opacity", 1.0).clamp(0.0, 1.0) * opacity);
            record["visible"] = json!(boolean(&record, "visible", true) && visible);
        }
        let before = s.project.clone();
        let active = s.active_layer;
        let requests = s.url_requests.len();
        match add_layer(s, &record, dir.as_deref(), &mut report) {
            Ok(Some(id)) => {
                added.push(name.to_string());
                order.push(id);
                groups.entry(string(&record, "groupId").into()).or_default().push(id);
            }
            Ok(None) => {
                added.push(name.to_string());
            }
            Err(e) => {
                s.project = before;
                s.active_layer = active;
                s.url_requests.truncate(requests);
                report.skipped(name, format!("layer: {e}"));
            }
        }
    }
    for g in &project.groups {
        let mut group = rustgis_map::LayerGroup::new(string(g, "name"));
        group.children = groups.remove(string(g, "id")).unwrap_or_default();
        group.expanded = !boolean(g, "collapsed", false);
        let (visible, opacity) = group_look(&project.groups, string(g, "id"));
        group.visible = visible;
        group.transparency = (100.0 * (1.0 - opacity)) as f32;
        s.map_mut().add_group(group);
        if !string(g, "parentId").is_empty() {
            report.approximated(string(g, "name"), "nested folder flattened; inherited visibility and opacity retained");
        }
    }
    order.reverse();
    s.map_mut().layers.sort_by_key(|l| order.iter().position(|id| *id == l.id).unwrap_or(usize::MAX));
    s.request_only = false;
    s.map_mut().camera = project.camera;
    s.map_mut().bookmarks = project.bookmarks;
    let skipped = project.layers.len().saturating_sub(added.len());
    let changed: std::collections::HashSet<_> = report
        .notes
        .iter()
        .filter(|n| n.kind == geolibre::NoteKind::Approximated && !n.layer.is_empty())
        .map(|n| n.layer.as_str())
        .collect();
    s.notify(
        format!(
            "Imported {} of {} GeoLibre layers; {} drawn differently, {skipped} left out (see the import's notes)",
            added.len(),
            project.layers.len(),
            changed.len()
        ),
        false,
    );
    Ok(json!({"map":project.name,"layers":added,"notes":report.notes}))
}

/// Adds data or tiles, retaining the original layer's presentation. Tile layers have their own
/// order in RustGIS, so mixing them with features is explicitly reported.
fn add_layer(s: &mut Session, r: &Value, dir: Option<&str>, report: &mut Report) -> Result<Option<u32>, String> {
    let name = string(r, "name");
    let kind = string(r, "type");
    let src = &r["source"];
    if matches!(kind, "deckgl-viz" | "video" | "gaussian-splat" | "3d-tiles" | "arcgis" | "duckdb-query" | "sql-query" | "image")
        || string(&r["metadata"], "sourceKind") == "sql-query"
    {
        return Err(format!("unsupported {kind} layer"));
    }
    let inline = r
        .get("geojson")
        .filter(|v| v.is_object() || v.is_string())
        .or_else(|| r.get("metadata").and_then(|m| m.get("embeddedGeoJSON")).filter(|v| v.is_object() || v.is_string()))
        .or_else(|| src.get("data").filter(|d| d.is_object()));
    let before: std::collections::HashSet<_> = s.map().layers.iter().map(|l| l.id).collect();
    let result = if kind == "geojson" && inline.is_some() {
        let data = inline.ok_or("missing GeoJSON")?;
        let text = if let Some(t) = data.as_str() { t.to_string() } else { data.to_string() };
        s.run_part("layer.addDataText", &json!({"fileName":"geolibre.geojson","text":text,"name":name}))?
    } else {
        let address = string(r, "sourcePath");
        let address = if address.is_empty() && matches!(kind, "xyz" | "wms" | "wmts" | "vector-tiles" | "raster") {
            array(src, "tiles").first().and_then(Value::as_str).unwrap_or("")
        } else {
            address
        };
        let address = if address.is_empty() { string(src, "url") } else { address };
        let address = if address.is_empty() { array(src, "tiles").first().and_then(Value::as_str).unwrap_or("") } else { address };
        let address = if address.is_empty() { src.get("data").and_then(Value::as_str).unwrap_or("") } else { address };
        if address.is_empty() {
            return Err("no inline data, path or URL".into());
        }
        let address = resolve(address, dir)?;
        if kind == "mbtiles" && rustgis_map::tiles::is_web(&address) {
            #[cfg(not(target_arch = "wasm32"))]
            {
                let bytes = crate::render::rustgis_tiles::cache::download_blocking(&address)?;
                let out = super::raster::add_named_bytes(s, &json!({"name":name}), format!("{}.mbtiles", pending_token()), bytes)?;
                if let Some(t) = out.get("index").and_then(Value::as_u64).and_then(|i| s.map_mut().tiles.get_mut(i as usize)) {
                    t.visible = boolean(r, "visible", true);
                    t.opacity = number(r, "opacity", 1.0).clamp(0.0, 1.0) as f32;
                }
            }
            #[cfg(target_arch = "wasm32")]
            {
                let token = pending_token();
                let mut tile = rustgis_map::tiles::TileLayer::new(name, rustgis_map::tiles::TileKind::MbTiles, &address);
                tile.id = token.clone();
                tile.visible = false;
                s.map_mut().tiles.insert(0, tile);
                s.request_url(&json!({"url":address,"whole":true,"name":name,"geolibreLayer":{"token":token,"record":r}}))?;
                report.approximated(name, "MBTiles archive is downloading; wait before saving");
            }
            report.approximated(name, "tile overlays draw above feature layers; tile vector styling uses RustGIS defaults");
            return Ok(None);
        }
        if matches!(kind, "xyz" | "wms" | "wmts" | "vector-tiles" | "pmtiles" | "mbtiles")
            || (kind == "raster" && !array(src, "tiles").is_empty())
        {
            let tk = match kind {
                "vector-tiles" => "vector",
                "pmtiles" => "pmtiles",
                "mbtiles" => "mbtiles",
                _ => "raster",
            };
            let mut q = json!({"url":address,"kind":tk,"name":name,"overlay":true,"opacity":number(r,"opacity",1.0).clamp(0.0,1.0)});
            for (source, target) in [
                ("minzoom", "minZoom"),
                ("maxzoom", "maxZoom"),
                ("tileSize", "tileSize"),
                ("bounds", "bounds"),
                ("attribution", "attribution"),
            ] {
                if let Some(value) = src.get(source) {
                    q[target] = value.clone();
                }
            }
            if string(src, "scheme") == "tms" {
                q["tms"] = json!(true);
            }
            let out = s.run_part("map.addTiles", &q)?;
            if let Some(t) = out.get("index").and_then(Value::as_u64).and_then(|i| s.map_mut().tiles.get_mut(i as usize)) {
                t.visible = boolean(r, "visible", true);
            }
            report.approximated(name, "tile overlays draw above feature layers; feature/tile interleaving is not retained");
            if matches!(kind, "vector-tiles" | "pmtiles" | "mbtiles") {
                report.approximated(name, "tile vector styling uses RustGIS defaults");
            }
            return Ok(None);
        }
        if !matches!(kind, "geojson" | "cog" | "raster" | "flatgeobuf" | "geoparquet" | "zarr" | "lidar") {
            return Err(format!("unsupported {kind} layer"));
        }
        let remote = rustgis_map::tiles::is_web(&address);
        let mut q = json!({"name":name,"zoom":false});
        q[if remote { "url" } else { "path" }] = json!(address);
        #[cfg(target_arch = "wasm32")]
        if remote && kind != "cog" {
            // Reserve the final position and group membership before downloads can finish in
            // arbitrary order. The token follows the layer across map switches and reordering.
            let token = pending_token();
            let mut ds = rustgis_data::Dataset::new(name, rustgis_data::GeometryType::Point, Vec::new());
            ds.source = Some(token.clone());
            let id = s.map_mut().add_dataset(ds);
            if let Some(layer) = s.map_mut().layer_mut(id) {
                layer.visible = boolean(r, "visible", true);
                layer.transparency = (100.0 * (1.0 - number(r, "opacity", 1.0).clamp(0.0, 1.0))) as f32;
            }
            q["whole"] = json!(true);
            if kind == "zarr" {
                q["geolibreZarr"] = json!(true);
                if let Some(variable) = src.get("variable") {
                    q["variable"] = variable.clone();
                }
                if let Some(index) = src.get("index") {
                    q["index"] = index.clone();
                }
            }
            q["geolibreLayer"] = json!({"token":token,"record":r});
            if kind == "zarr" {
                s.request_url(&q)?;
            } else {
                s.run_part("layer.addUrl", &q)?;
            }
            report.approximated(name, "URL data is downloading; its saved style is applied when the download completes");
            return Ok(Some(id));
        }
        if kind == "zarr" {
            if let Some(v) = src.get("variable") {
                q["variable"] = v.clone();
            }
            s.run_part("layer.addZarr", &q)?
        } else {
            s.run_part(if remote { "layer.addUrl" } else { "layer.addData" }, &q)?
        }
    };
    if result.get("pending").and_then(Value::as_bool) == Some(true) {
        return Err("source is pending and has not been imported".into());
    }
    if let Some(index) = result.get("index").and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok()) {
        if let Some(t) = s.map_mut().tiles.get_mut(index) {
            t.visible = boolean(r, "visible", true);
            t.opacity = number(r, "opacity", 1.0).clamp(0.0, 1.0) as f32;
        }
        return Ok(None);
    }
    let ids: Vec<_> = s.map().layers.iter().filter(|l| !before.contains(&l.id)).map(|l| l.id).collect();
    if ids.len() != 1 {
        return Err("source must contain exactly one map layer (select or export one dataset first)".into());
    }
    let id = ids.first().copied().ok_or("source added no map layer")?;
    let layer = s.map_mut().layer_mut(id).ok_or("source added no map layer")?;
    geolibre::style::apply(layer, r, report);
    Ok(Some(id))
}

/// Absolute saved paths are tried first, then their basename in the supplied project folder.
/// Relative paths require a folder rather than depending on the process working directory.
fn resolve(address: &str, dir: Option<&str>) -> Result<String, String> {
    if rustgis_map::tiles::is_web(address) {
        return Ok(address.into());
    }
    if crate::map::qgis::is_drive_relative(address) {
        return Err("drive-relative path is unsupported".into());
    }
    let path = std::path::Path::new(address);
    if !crate::map::qgis::is_relative(address) && path.exists() {
        return Ok(address.into());
    }
    if let Some(dir) = dir {
        let name = address.rsplit(['/', '\\']).next().unwrap_or(address);
        let path = if crate::map::qgis::is_relative(address) {
            std::path::Path::new(dir).join(address)
        } else {
            std::path::Path::new(dir).join(name)
        };
        if path.exists() {
            return Ok(path.to_string_lossy().into_owned());
        }
    }
    Err(format!("data file {address} is unavailable (pass dir for moved files)"))
}

/// Finishes an imported URL layer in its reserved position, even after the user switches maps.
/// A deleted or undone placeholder cancels the result rather than adding to a different map.
pub(crate) fn finish_layer(s: &mut Session, p: &Value, name: String, bytes: Vec<u8>) -> CmdResult {
    let pending = p.get("geolibreLayer").ok_or("missing GeoLibre download state")?;
    let token = string(pending, "token");
    if token.is_empty() {
        return Err("missing GeoLibre download token".into());
    }
    if string(&pending["record"], "type") == "mbtiles" {
        return finish_archive(s, p, bytes);
    }
    let target = s
        .project
        .maps
        .iter()
        .enumerate()
        .find_map(|(i, m)| m.layers.iter().find(|l| l.dataset.source.as_deref() == Some(token)).map(|l| (i, l.id)))
        .ok_or("GeoLibre layer was removed while downloading")?;
    let (active, layer) = (s.project.active_map, s.active_layer);
    s.project.active_map = target.0;
    let camera = s.map().camera;
    let result = (|| {
        let before: std::collections::HashSet<_> = s.map().layers.iter().map(|l| l.id).collect();
        super::raster::add_named_bytes(s, p, name, bytes)?;
        let ids: Vec<_> = s.map().layers.iter().filter(|l| !before.contains(&l.id)).map(|l| l.id).collect();
        if ids.len() != 1 {
            return Err("GeoLibre URL must contain one map layer".into());
        }
        let id = ids.first().copied().ok_or("download added no layer")?;
        let pos = s.map().layers.iter().position(|l| l.id == id).ok_or("download added no layer")?;
        let mut loaded = s.map_mut().layers.remove(pos);
        loaded.id = target.1;
        let mut report = Report::default();
        geolibre::style::apply(&mut loaded, &pending["record"], &mut report);
        let dest = s.map_mut().layer_mut(target.1).ok_or("GeoLibre layer was removed")?;
        *dest = loaded;
        for note in report.notes {
            s.notify(format!("{}: {}", note.layer, note.text), false);
        }
        Ok(json!({"layer":target.1}))
    })();
    s.map_mut().camera = camera;
    s.project.active_map = active;
    s.active_layer = layer;
    result
}

/// A runtime token cannot collide with a placeholder saved by an earlier application run.
fn pending_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let time = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    format!("geolibre-pending:{time}:{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

/// Replaces an MBTiles placeholder in the tile stack, preserving its position on late completion.
fn finish_archive(s: &mut Session, p: &Value, bytes: Vec<u8>) -> CmdResult {
    let pending = &p["geolibreLayer"];
    let token = string(pending, "token");
    let target = s
        .project
        .maps
        .iter()
        .position(|m| m.tiles.iter().any(|t| t.id == token))
        .ok_or("GeoLibre archive was removed while downloading")?;
    let (active, layer) = (s.project.active_map, s.active_layer);
    s.project.active_map = target;
    let camera = s.map().camera;
    let result = (|| {
        // A unique upload key keeps two archives with equal filenames independent.
        let filename = format!("{token}.mbtiles");
        let out = super::raster::add_named_bytes(s, p, filename, bytes)?;
        let index =
            out.get("index").and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok()).ok_or("download added no tile archive")?;
        if index >= s.map().tiles.len() {
            return Err("download added no tile archive".into());
        }
        let mut loaded = s.map_mut().tiles.remove(index);
        let dest = s.map().tiles.iter().position(|t| t.id == token).ok_or("GeoLibre archive was removed")?;
        loaded.visible = boolean(&pending["record"], "visible", true);
        loaded.opacity = number(&pending["record"], "opacity", 1.0).clamp(0.0, 1.0) as f32;
        if let Some(t) = s.map_mut().tiles.get_mut(dest) {
            *t = loaded;
        }
        Ok(json!({"index":dest}))
    })();
    s.map_mut().camera = camera;
    s.project.active_map = active;
    s.active_layer = layer;
    result
}

/// Discards a failed download's empty reservation. Tokens, rather than active-map state, ensure
/// a failure cannot remove another layer after switching maps or undoing the import.
pub fn discard_pending(s: &mut Session, p: &Value) {
    let token = p.get("geolibreLayer").map(|v| string(v, "token")).unwrap_or("");
    if token.is_empty() {
        return;
    }
    for (index, map) in s.project.maps.iter_mut().enumerate() {
        let removed: std::collections::HashSet<_> =
            map.layers.iter().filter(|l| l.dataset.source.as_deref() == Some(token)).map(|l| l.id).collect();
        map.layers.retain(|l| !removed.contains(&l.id));
        map.tiles.retain(|t| t.id != token);
        for group in &mut map.groups {
            group.children.retain(|id| !removed.contains(id));
        }
        if s.project.active_map == index && s.active_layer.is_some_and(|id| removed.contains(&id)) {
            s.active_layer = None;
        }
    }
}

/// Effective visibility and opacity of one folder, including bounded, possibly cyclic ancestry.
fn group_look(groups: &[Value], id: &str) -> (bool, f64) {
    let mut seen = std::collections::HashSet::new();
    let mut id = id;
    let (mut visible, mut opacity) = (true, 1.0);
    while !id.is_empty() && seen.insert(id) {
        let Some(group) = groups.iter().find(|g| string(g, "id") == id) else { break };
        visible &= boolean(group, "visible", true);
        opacity *= number(group, "opacity", 1.0).clamp(0.0, 1.0);
        id = string(group, "parentId");
    }
    (visible, opacity)
}
