//! Map › Layer (Add Data), Contents pane actions, and the Feature Layer / Labeling / Data tabs.

use std::path::Path;

use rustgis_data::GeometryType;
use rustgis_map::{ClassMethod, Marker, Ramp, Renderer, Rgba};
use serde_json::{Value, json};

use super::{b, f, need_feature_layer, need_layer, ps};
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("layer.addData", "Add Data", "Map › Layer", add_data)
            .params(r#"{"path": string, "name"?: string, "layer"?: "GeoPackage or GDAL layer", "stream"?: bool, "crs"?: "EPSG:… for a GDAL file that doesn't say"}"#),
        C::new("layer.loadAll", "Load All Features", "Contents", load_all).when(need_streamed).params(r#"{"layer"?: name|id}"#),
        C::new("layer.addDataText", "Add Data (from text)", "Map › Layer", add_data_text).params(r#"{"fileName": "name.geojson|.csv", "text": string}"#),
        C::new("layer.addSample", "Add Sample Data", "Map › Layer › Add Data", add_sample).params(r#"{"name": "countries"|"cities"|"rivers"|"lakes"}"#),
        C::new("layer.remove", "Remove", "Contents", remove).key("Delete").when(need_layer).params(r#"{"layer"?: name|id}"#),
        C::new("layer.select", "Select Layer", "Contents", |s, p| {
            s.active_layer = Some(s.layer_id(p)?);
            Ok(Value::Null)
        })
        .view(),
        C::new("layer.rename", "Rename", "Contents", |s, p| {
            let n = s_req(p, "name")?.trim().chars().take(200).collect::<String>();
            if n.is_empty() {
                return Err("name is required".into());
            }
            let old = s.map().layer(s.layer_id(p)?).map(|l| l.name.clone()).unwrap_or_default();
            let r = with(s, p, |l| {
                l.name = n.clone();
                l.dataset.name = n.clone();
            })?;
            // Layers masked by this one follow the new name.
            for l in &mut s.map_mut().layers {
                if l.mask.as_deref().is_some_and(|m| m.eq_ignore_ascii_case(&old)) {
                    l.mask = Some(n.clone());
                }
            }
            Ok(r)
        })
        .when(need_layer),
        C::new("layer.visibility", "Visibility", "Contents", |s, p| {
            let on = b(p, "visible");
            with(s, p, |l| l.visible = on.unwrap_or(!l.visible))
        })
        .when(need_layer),
        C::new("layer.moveUp", "Move Up", "Contents", |s, p| move_layer(s, p, -1)).when(need_layer),
        C::new("layer.moveDown", "Move Down", "Contents", |s, p| move_layer(s, p, 1)).when(need_layer),
        C::new("layer.zoomTo", "Zoom To Layer", "Contents", |s, p| {
            let id = s.layer_id(p)?;
            let e = s.map().layer(id).map(|l| l.extent()).unwrap_or_default();
            if e.is_empty() {
                return Err("the layer has no features".into());
            }
            s.zoom_to(&e);
            Ok(Value::Null)
        })
        .view()
        .when(need_layer),
        C::new("layer.symbology", "Symbology", "Feature Layer › Drawing", symbology)
            .when(need_feature_layer)
            .params(r#"{"layer"?, "type": "single"|"unique"|"graduated"|"proportional", "field"?, "classes"?: 1..12, "method"?: "naturalBreaks"|"equalInterval"|"quantile", "ramp"?: string}"#),
        C::new("layer.ramp", "Color Scheme", "Feature Layer › Drawing", |s, p| {
            let r = Ramp::from_name(s_req(p, "ramp")?).ok_or("unknown colour scheme")?;
            with(s, p, |l| match &mut l.raster {
                Some(rl) => rl.style.set_ramp(r),
                None => {
                    let g = l.geometry_type();
                    l.renderer.apply_ramp(r, g)
                }
            })
        })
        .when(need_layer),
        C::new("layer.color", "Symbol Color", "Feature Layer › Drawing", |s, p| {
            let c = Rgba::from_hex(s_req(p, "color")?).ok_or("color must be #RRGGBB or #RRGGBBAA")?;
            with(s, p, |l| {
                let g = l.geometry_type();
                match &mut l.renderer {
                    Renderer::Single(sym) => *sym = sym.with_fill(c, g),
                    Renderer::Proportional { base, .. } => *base = base.with_fill(c, g),
                    other => *other = Renderer::Single(other.base_symbol().with_fill(c, g)),
                }
            })
        })
        .when(need_feature_layer)
        .params(r##"{"layer"?, "color": "#RRGGBB"}"##),
        C::new("layer.outline", "Outline", "Feature Layer › Drawing", |s, p| {
            let c = s.get_color(p, "color")?;
            let w = f(p, "width").map(|w| w.clamp(0.0, 20.0) as f32);
            with(s, p, |l| {
                l.renderer.map_symbols(|sym| {
                    if let Some(c) = c {
                        sym.stroke = c;
                    }
                    if let Some(w) = w {
                        sym.stroke_width = w;
                    }
                })
            })
        })
        .when(need_feature_layer)
        .params(r##"{"layer"?, "color"?: "#RRGGBB", "width"?: number}"##),
        C::new("layer.marker", "Marker", "Feature Layer › Drawing", |s, p| {
            let shape = ps(p, "shape").map(|n| Marker::ALL.into_iter().find(|m| m.label().eq_ignore_ascii_case(n)).ok_or("unknown marker shape")).transpose()?;
            let size = f(p, "size").map(|v| v.clamp(1.0, 80.0) as f32);
            with(s, p, |l| {
                l.renderer.map_symbols(|sym| {
                    if let Some(m) = shape {
                        sym.marker = m;
                    }
                    if let Some(sz) = size {
                        sym.size = sz;
                    }
                })
            })
        })
        .when(need_feature_layer)
        .params(r#"{"layer"?, "shape"?: "circle"|"square"|"triangle"|"diamond"|"star", "size"?: number}"#),
        C::new("layer.transparency", "Transparency", "Feature Layer › Effects", |s, p| {
            let t = f(p, "value").ok_or("value (0–100) is required")?.clamp(0.0, 100.0) as f32;
            with(s, p, |l| l.transparency = t)
        })
        .when(need_layer)
        .params(r#"{"layer"?, "value": 0..100}"#),
        C::new("layer.scaleRange", "Visibility Range", "Feature Layer › Visibility Range", |s, p| {
            let min = f(p, "min").filter(|v| *v > 0.0);
            let max = f(p, "max").filter(|v| *v > 0.0);
            with(s, p, |l| {
                l.min_scale = min;
                l.max_scale = max;
            })
        })
        .when(need_layer)
        .params(r#"{"layer"?, "min"?: scale (out beyond), "max"?: scale (in beyond)}"#),
        C::new("layer.selectable", "Selectable", "Contents", |s, p| {
            let on = b(p, "on");
            with(s, p, |l| l.selectable = on.unwrap_or(!l.selectable))
        })
        .when(need_feature_layer),
        C::new("layer.expand", "Expand Legend", "Contents", |s, p| {
            let on = b(p, "on");
            with(s, p, |l| l.expanded = on.unwrap_or(!l.expanded))
        })
        .view()
        .when(need_layer),
        C::new("labels.enable", "Label", "Labeling › Layer", |s, p| {
            let on = b(p, "on");
            with(s, p, |l| l.labels.enabled = on.unwrap_or(!l.labels.enabled))
        })
        .when(need_feature_layer)
        .params(r#"{"layer"?, "on"?: bool}"#),
        C::new("labels.field", "Label Field", "Labeling › Label Class", |s, p| {
            let name = s_req(p, "field")?.to_string();
            let id = s.layer_id(p)?;
            let fname = s.map().layer(id).and_then(|l| l.dataset.field_index(&name).and_then(|i| l.dataset.fields.get(i)).map(|f| f.name.clone()));
            let fname = fname.ok_or_else(|| format!("no field named '{name}'"))?;
            with(s, p, |l| {
                l.labels.field = fname.clone();
                l.labels.enabled = true;
            })
        })
        .when(need_feature_layer),
        C::new("labels.style", "Text Symbol", "Labeling › Text Symbol", |s, p| {
            let size = f(p, "size").map(|v| v.clamp(4.0, 72.0) as f32);
            let color = s.get_color(p, "color")?;
            let (halo, bold) = (b(p, "halo"), b(p, "bold"));
            with(s, p, |l| {
                if let Some(v) = size {
                    l.labels.size = v;
                }
                if let Some(c) = color {
                    l.labels.color = c;
                }
                if let Some(h) = halo {
                    l.labels.halo = h;
                }
                if let Some(bb) = bold {
                    l.labels.bold = bb;
                }
            })
        })
        .when(need_feature_layer)
        .params(r##"{"layer"?, "size"?: number, "color"?: "#RRGGBB", "halo"?: bool, "bold"?: bool}"##),
        C::new("layer.definitionQuery", "Definition Query", "Data › Definition Query", |s, p| {
            let q = ps(p, "where").map(str::trim).unwrap_or("").to_string();
            let id = s.layer_id(p)?;
            if !q.is_empty()
                && let Some(l) = s.map().layer(id)
            {
                rustgis_data::query::parse(&q, &l.dataset)?;
            }
            with(s, p, |l| l.definition_query = if q.is_empty() { None } else { Some(q.clone()) })
        })
        .when(need_feature_layer)
        .params(r#"{"layer"?, "where": "SQL clause, empty to clear"}"#),
        C::new("table.open", "Attribute Table", "Data › Table", |s, p| {
            s.table_layer = Some(s.layer_id(p)?);
            Ok(Value::Null)
        })
        .view()
        .key("Ctrl+T")
        .when(need_feature_layer),
        C::new("table.close", "Close Table", "Data › Table", |s, _| {
            s.table_layer = None;
            Ok(Value::Null)
        })
        .view(),
        C::new("layer.fields", "Fields", "Data › Data Design", |s, p| {
            let id = s.layer_id(p)?;
            let l = s.map().layer(id).ok_or("no layer")?;
            Ok(json!(l.dataset.fields.iter().map(|f| json!({"name": f.name, "type": f.ty.label(), "alias": f.alias})).collect::<Vec<_>>()))
        })
        .view()
        .when(need_feature_layer),
    ]
}

impl Session {
    fn get_color(&self, p: &Value, k: &str) -> Result<Option<Rgba>, String> {
        match ps(p, k) {
            Some(c) => Rgba::from_hex(c).map(Some).ok_or_else(|| format!("{k} must be #RRGGBB or #RRGGBBAA")),
            None => Ok(None),
        }
    }
}

fn s_req<'a>(p: &'a Value, k: &str) -> Result<&'a str, String> {
    ps(p, k).ok_or_else(|| format!("{k} is required"))
}

/// Applies `f` to the layer named by `p` (default: active layer).
pub(crate) fn with(s: &mut Session, p: &Value, f: impl FnOnce(&mut rustgis_map::Layer)) -> CmdResult {
    let id = s.layer_id(p)?;
    let l = s.map_mut().layer_mut(id).ok_or("no such layer")?;
    f(l);
    Ok(Value::Null)
}

/// Adds a dataset to the active map, selects it and returns its id.
pub fn add_dataset(s: &mut Session, mut ds: rustgis_data::Dataset) -> u32 {
    // Unique layer names keep name-based parameters unambiguous.
    let base = ds.name.clone();
    let mut i = 1;
    while s.map().layer_by_name(&ds.name).is_some() {
        i += 1;
        ds.name = format!("{base} ({i})");
    }
    let id = s.map_mut().add_dataset(ds);
    s.active_layer = Some(id);
    id
}

fn add_data(s: &mut Session, p: &Value) -> CmdResult {
    let path = s_req(p, "path")?;
    if rustgis_data::raster::is_raster_name(path) {
        return super::raster::add_path(s, path, ps(p, "name").map(str::trim).filter(|n| !n.is_empty()));
    }
    if path.to_ascii_lowercase().ends_with(".rgislyr") {
        return add_layer_file(s, path);
    }
    if rustgis_data::source::is_source_name(path) {
        return add_source(s, p, Origin::Path(path));
    }
    #[cfg(not(target_arch = "wasm32"))]
    if rustgis_data::source::gdal::is_gdal_name(path) {
        return add_gdal(s, p, path);
    }
    let read = rustgis_data::io::read_path(Path::new(path)).map_err(|e| format!("{path}: {e}"))?;
    add_read(s, p, read)
}

/// Where a source file is: on disk, or uploaded (its name and bytes).
pub(crate) enum Origin<'a> {
    Path(&'a str),
    Bytes(&'a str, std::sync::Arc<Vec<u8>>),
}

/// Most GeoPackage tables added at once.
const MAX_TABLES: usize = 32;

/// Adds a GeoPackage, GeoParquet or FlatGeobuf file: each feature table becomes a layer. Large
/// sources (or any, with `"stream": true`) are streamed, read in place a view at a time; smaller
/// ones are loaded whole, so they can be edited.
pub(crate) fn add_source(s: &mut Session, p: &Value, origin: Origin<'_>) -> CmdResult {
    use rustgis_data::source;
    let label = match &origin {
        Origin::Path(p) | Origin::Bytes(p, _) => p.to_string(),
    };
    let err = |e: rustgis_data::DataError| format!("{label}: {e}");
    let tables: Vec<Option<String>> = match (ps(p, "layer").map(str::trim).filter(|t| !t.is_empty()), &origin) {
        (Some(t), _) => vec![Some(t.to_string())],
        (None, Origin::Path(path)) if path.to_ascii_lowercase().ends_with(".gpkg") => {
            let all = source::layers(Path::new(path)).map_err(err)?;
            if all.len() > MAX_TABLES {
                s.notify(format!("{label}: adding the first {MAX_TABLES} of {} tables", all.len()), false);
            }
            all.into_iter().take(MAX_TABLES).map(Some).collect()
        }
        _ => vec![None],
    };
    if tables.is_empty() {
        return Err(format!("{label}: no feature tables"));
    }
    let empty_before = s.map().layers.is_empty();
    let mut ids = Vec::new();
    let mut streamed = false;
    let mut features = 0u64;
    for table in tables {
        let src = match &origin {
            Origin::Path(path) => source::open_path(Path::new(path), table.as_deref()),
            Origin::Bytes(name, bytes) => source::open_shared(name, bytes.clone(), table.as_deref()),
        }
        .map_err(err)?;
        let info = src.info().clone();
        let stream = b(p, "stream").unwrap_or(info.count.is_none_or(|c| c > rustgis_map::source::STREAM_ABOVE));
        // A name given applies to a single table.
        let name = match ps(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) if ids.is_empty() => n.to_string(),
            _ => info.name.clone(),
        };
        if stream {
            let layer = match &origin {
                Origin::Path(path) => {
                    let full = std::path::absolute(path).map(|p| p.display().to_string()).unwrap_or_else(|_| path.to_string());
                    rustgis_map::source::SourceLayer::from_path(full, table.clone(), src)
                }
                Origin::Bytes(file, bytes) => rustgis_map::source::SourceLayer::from_bytes(*file, bytes.clone(), table.clone(), src),
            };
            let unique = unique_name(s, &name);
            for n in &info.notes {
                s.notify(format!("{unique}: {n}"), false);
            }
            let id = s.map_mut().add_streamed(unique.clone(), layer);
            s.active_layer = Some(id);
            ids.push(id);
            streamed = true;
            features += info.count.unwrap_or(0);
            s.notify(
                format!(
                    "{unique}: {} features, read in place a view at a time (Load All Features loads them into memory)",
                    info.count.map(|c| c.to_string()).unwrap_or_else(|| "many".into())
                ),
                false,
            );
        } else {
            let (mut ds, report) = source::materialize(src.as_ref()).map_err(err)?;
            ds.name = name;
            ds.source = Some(label.clone());
            features += ds.features.len() as u64;
            let id = add_dataset(s, ds);
            ids.push(id);
            for m in report.messages {
                s.notify(m, false);
            }
        }
    }
    if empty_before {
        let e = ids.iter().filter_map(|id| s.map().layer(*id)).fold(rustgis_geom::Extent::EMPTY, |e, l| e.union(&l.extent()));
        if !e.is_empty() {
            s.zoom_to(&e);
        }
    }
    let first = ids.first().copied();
    Ok(json!({"layer": first, "layers": ids, "features": features, "streamed": streamed}))
}

/// Adds a file GDAL reads (KML, GPX, File Geodatabase, DXF…): each layer is converted once into an
/// indexed FlatGeobuf in the cache, then added like one (streamed when large). A layer mixing
/// geometry types becomes a layer per type.
#[cfg(not(target_arch = "wasm32"))]
fn add_gdal(s: &mut Session, p: &Value, path: &str) -> CmdResult {
    let crs = ps(p, "crs").map(str::trim).filter(|c| !c.is_empty());
    let converted = rustgis_data::source::gdal::convert(Path::new(path), ps(p, "layer"), crs).map_err(|e| format!("{path}: {e}"))?;
    let original = std::path::absolute(path).map(|p| p.display().to_string()).unwrap_or_else(|_| path.to_string());
    let empty_before = s.map().layers.is_empty();
    let single = converted.len() == 1;
    let (mut ids, mut features, mut streamed) = (Vec::new(), 0u64, false);
    for c in converted {
        let mut q = json!({"name": c.name});
        if let Some(n) = ps(p, "name").filter(|_| single) {
            q["name"] = json!(n);
        }
        if let Some(v) = p.get("stream") {
            q["stream"] = v.clone();
        }
        let fgb = c.path.display().to_string();
        let r = add_source(s, &q, Origin::Path(&fgb))?;
        features += r["features"].as_u64().unwrap_or(0);
        streamed |= r["streamed"].as_bool().unwrap_or(false);
        for id in r["layers"].as_array().into_iter().flatten().filter_map(Value::as_u64).filter_map(|i| u32::try_from(i).ok()) {
            if let Some(l) = s.map_mut().layer_mut(id) {
                l.dataset.source = Some(original.clone());
                if let Some(src) = &mut l.source {
                    src.gdal = Some(rustgis_map::source::GdalOrigin {
                        path: original.clone(),
                        layer: c.layer.clone(),
                        family: c.family,
                        crs: crs.map(str::to_string),
                    });
                }
            }
            // Coordinates beyond longitude/latitude: the file's coordinate system is unknown.
            let e = s.map().layer(id).map(|l| l.extent()).unwrap_or_default();
            if !e.is_empty() && (e.min_x < -181.0 || e.max_x > 181.0 || e.min_y < -91.0 || e.max_y > 91.0) {
                s.notify(
                    format!("{}: the file doesn't say its coordinate system; add it again with \"crs\" (such as EPSG:2263)", c.name),
                    true,
                );
            }
            ids.push(id);
        }
    }
    if empty_before {
        let e = ids.iter().filter_map(|id| s.map().layer(*id)).fold(rustgis_geom::Extent::EMPTY, |e, l| e.union(&l.extent()));
        if !e.is_empty() {
            s.zoom_to(&e);
        }
    }
    Ok(json!({"layer": ids.first(), "layers": ids, "features": features, "streamed": streamed}))
}

fn unique_name(s: &Session, base: &str) -> String {
    let mut name = base.to_string();
    let mut i = 1;
    while s.map().layer_by_name(&name).is_some() {
        i += 1;
        name = format!("{base} ({i})");
    }
    name
}

/// The active (or named) layer is streamed.
fn need_streamed(s: &Session) -> Option<&'static str> {
    match s.active_layer.and_then(|id| s.map().layer(id)) {
        Some(l) if l.is_streamed() => None,
        Some(_) => Some("this layer's features are already in memory"),
        None => Some("select a layer in the Contents pane first"),
    }
}

/// Loads a streamed layer's features into memory, so it can be edited like any other layer.
fn load_all(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let l = s.map().layer(id).ok_or("no such layer")?;
    let ds = l.load_all()?;
    let n = ds.features.len();
    let l = s.map_mut().layer_mut(id).ok_or("no such layer")?;
    l.dataset = ds;
    l.dataset.windowed = false;
    l.source = None;
    l.selection.clear();
    l.touch_geometry();
    Ok(json!({"layer": id, "features": n}))
}

/// Adds a layer file: a layer with its data, symbology and labels.
fn add_layer_file(s: &mut Session, path: &str) -> CmdResult {
    let len = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?.len();
    if len > 512 << 20 {
        return Err("the layer file is too large".into());
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    add_layer_text(s, &text)
}

/// Adds a layer from a layer file's JSON.
pub(crate) fn add_layer_text(s: &mut Session, text: &str) -> CmdResult {
    let mut layer: rustgis_map::Layer = serde_json::from_str(text).map_err(|e| format!("not a RustGIS layer file: {e}"))?;
    // A fresh id and a unique name in this map; the rest goes through project repair.
    let base = layer.name.clone();
    let mut i = 1;
    while s.map().layer_by_name(&layer.name).is_some() {
        i += 1;
        layer.name = format!("{base} ({i})");
    }
    let id = s.map_mut().add_layer(layer);
    s.project.normalize();
    s.active_layer = Some(id);
    Ok(json!({"layer": id}))
}

pub(crate) fn add_data_text(s: &mut Session, p: &Value) -> CmdResult {
    let name = s_req(p, "fileName")?;
    let text = s_req(p, "text")?;
    let read = rustgis_data::io::read_bytes(name, text.as_bytes()).map_err(|e| format!("{name}: {e}"))?;
    add_read(s, p, read)
}

pub(crate) fn add_read(s: &mut Session, p: &Value, read: (rustgis_data::Dataset, rustgis_data::ReadReport)) -> CmdResult {
    let (mut ds, report) = read;
    if let Some(n) = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        ds.name = n.to_string();
    }
    if ds.geometry_type == GeometryType::None {
        let name = ds.name.clone();
        s.project.tables.push(ds);
        return Ok(json!({"table": name}));
    }
    let n = ds.features.len();
    let empty_before = s.map().layers.is_empty();
    let id = add_dataset(s, ds);
    for m in report.messages {
        s.notify(m, false);
    }
    if empty_before && let Some(e) = s.map().layer(id).map(|l| l.extent()) {
        s.zoom_to(&e);
    }
    Ok(json!({"layer": id, "features": n}))
}

fn add_sample(s: &mut Session, p: &Value) -> CmdResult {
    use crate::sample::*;
    let name = s_req(p, "name")?.to_ascii_lowercase();
    let (text, title) = match name.as_str() {
        "countries" => (countries(), "Countries"),
        "cities" => (CITIES, "Cities"),
        "rivers" => (RIVERS, "Rivers"),
        "lakes" => (LAKES, "Lakes"),
        _ => return Err(format!("no sample '{name}'")),
    };
    let (ds, _) = rustgis_data::io::read_geojson(text, title).map_err(|e| e.to_string())?;
    let id = add_dataset(s, ds);
    Ok(json!({"layer": id}))
}

fn remove(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let m = s.map_mut();
    let i = m.layer_index(id).ok_or("no such layer")?;
    m.layers.remove(i);
    let next = m.layers.get(i.min(m.layers.len().saturating_sub(1))).map(|l| l.id);
    s.active_layer = next;
    Ok(Value::Null)
}

fn move_layer(s: &mut Session, p: &Value, d: i32) -> CmdResult {
    let id = s.layer_id(p)?;
    let m = s.map_mut();
    let i = m.layer_index(id).ok_or("no such layer")?;
    let j = i as i64 + d as i64;
    if j < 0 || j as usize >= m.layers.len() {
        return Err("the layer can't move further".into());
    }
    m.layers.swap(i, j as usize);
    Ok(Value::Null)
}

fn symbology(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let kind = ps(p, "type").unwrap_or("single").to_ascii_lowercase();
    let ramp = ps(p, "ramp").and_then(Ramp::from_name);
    let method = ps(p, "method").and_then(ClassMethod::from_name).unwrap_or_default();
    let classes = f(p, "classes").map(|c| c.clamp(1.0, 12.0) as usize).unwrap_or(5);
    let l = s.map().layer(id).ok_or("no such layer")?;
    let g = l.geometry_type();
    let base = l.renderer.base_symbol();
    let base = if matches!(l.renderer, Renderer::Graduated { .. } | Renderer::Unique { .. }) {
        rustgis_map::Symbol { ..rustgis_map::Symbol::default_for(g, id) }.with_fill(base.fill, g)
    } else {
        base
    };
    let field = || -> Result<String, String> {
        if let Some(x) = ps(p, "field") {
            return Ok(x.to_string());
        }
        l.renderer
            .field()
            .map(str::to_string)
            .or_else(|| l.dataset.fields.iter().find(|f| f.ty.is_numeric()).map(|f| f.name.clone()))
            .ok_or_else(|| "field is required".into())
    };
    let r = match kind.as_str() {
        "single" | "singlesymbol" => Renderer::Single(base),
        "unique" | "uniquevalues" => Renderer::unique(&l.dataset, &field()?, ramp.unwrap_or(Ramp::Harbour), base)?,
        "graduated" | "graduatedcolors" => Renderer::graduated(&l.dataset, &field()?, classes, method, ramp.unwrap_or(Ramp::Ember), base)?,
        "proportional" | "proportionalsymbols" => {
            if g != GeometryType::Point {
                return Err("Proportional Symbols needs a point layer".into());
            }
            Renderer::proportional(&l.dataset, &field()?, base)?
        }
        other => return Err(format!("unknown symbology '{other}'")),
    };
    with(s, p, |l| l.renderer = r)
}
