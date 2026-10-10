//! Map › Selection.

use rustgis_geom::{Coord, Extent, lonlat_to_merc};
use serde_json::{Value, json};

use super::{f, need_layer, need_layers, need_selection, ps};
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("select.byAttributes", "Select By Attributes", "Map › Selection", by_attributes)
            .when(need_layers)
            .params(r#"{"layer"?, "where": "SQL clause", "mode"?: "new"|"add"|"remove"|"subset"}"#),
        C::new("select.byRect", "Select (rectangle)", "Map › Selection", by_rect)
            .params(r#"{"minLon", "minLat", "maxLon", "maxLat", "mode"?: "new"|"add"|"remove"}"#),
        C::new("select.atPoint", "Select (click)", "Map › Selection", at_point).params(r#"{"lon", "lat", "tolerancePx"?, "mode"?}"#),
        C::new("select.rows", "Select Rows", "Attribute Table", |s, p| {
            let id = s.layer_id(p)?;
            let rows: Vec<usize> = p
                .get("rows")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|v| v.as_u64()).map(|v| v as usize).collect())
                .unwrap_or_default();
            set(s, id, rows, mode(p));
            Ok(Value::Null)
        })
        .params(r#"{"layer"?, "rows": [number], "mode"?}"#),
        C::new("select.clear", "Clear", "Map › Selection", |s, _| {
            s.map_mut().clear_selection();
            Ok(Value::Null)
        })
        .when(need_selection),
        C::new("select.all", "Select All", "Data › Selection", |s, p| {
            let id = s.layer_id(p)?;
            let n = s.map().layer(id).map(|l| l.dataset.features.len()).unwrap_or(0);
            set(s, id, (0..n).collect(), Mode::New);
            Ok(Value::Null)
        })
        .when(need_layer),
        C::new("select.switch", "Switch Selection", "Data › Selection", |s, p| {
            let id = s.layer_id(p)?;
            let l = s.map().layer(id).ok_or("no layer")?;
            let rows: Vec<usize> = (0..l.dataset.features.len()).filter(|r| !l.selection.contains(r)).collect();
            set(s, id, rows, Mode::New);
            Ok(Value::Null)
        })
        .when(need_layer),
        C::new("select.zoomTo", "Zoom To Selection", "Map › Selection", |s, _| {
            let e = s.map().selection_extent();
            if e.is_empty() {
                return Err("nothing with a shape is selected".into());
            }
            s.zoom_to(&e);
            Ok(Value::Null)
        })
        .view()
        .when(need_selection),
        C::new("select.layerFromSelection", "Make Layer From Selected Features", "Data › Selection", |s, p| {
            let id = s.layer_id(p)?;
            let l = s.map().layer(id).ok_or("no layer")?;
            if l.selection.is_empty() {
                return Err("nothing is selected in this layer".into());
            }
            let mut ds = rustgis_data::Dataset::new(format!("{} selection", l.name), l.geometry_type(), l.dataset.fields.clone());
            for r in &l.selection {
                if let Some(f) = l.dataset.features.get(*r) {
                    ds.push(f.geometry.clone(), f.attrs.clone());
                }
            }
            let nid = super::layer::add_dataset(s, ds);
            Ok(json!({"layer": nid}))
        })
        .when(need_selection),
        C::new("identify", "Identify", "Map › Inquiry", identify).view().params(r#"{"lon", "lat", "tolerancePx"?}"#),
    ]
}

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    New,
    Add,
    Remove,
    Subset,
}

fn mode(p: &Value) -> Mode {
    match ps(p, "mode").unwrap_or("new").to_ascii_lowercase().as_str() {
        "add" => Mode::Add,
        "remove" => Mode::Remove,
        "subset" => Mode::Subset,
        _ => Mode::New,
    }
}

/// Applies a selection to one layer (New clears the others too, like desktop GIS).
pub fn set(s: &mut Session, id: u32, rows: Vec<usize>, m: Mode) {
    if m == Mode::New {
        s.map_mut().clear_selection();
    }
    let Some(l) = s.map_mut().layer_mut(id) else { return };
    let n = l.dataset.features.len();
    let rows: Vec<usize> = rows.into_iter().filter(|r| *r < n).collect();
    match m {
        Mode::New => l.selection = rows,
        Mode::Add => {
            for r in rows {
                if !l.selection.contains(&r) {
                    l.selection.push(r);
                }
            }
        }
        Mode::Remove => l.selection.retain(|r| !rows.contains(r)),
        Mode::Subset => l.selection.retain(|r| rows.contains(r)),
    }
    l.selection.sort_unstable();
    l.selection.dedup();
}

fn by_attributes(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let w = ps(p, "where").ok_or("where is required")?;
    let rows = rustgis_data::query::select(w, &s.map().layer(id).ok_or("no layer")?.dataset)?;
    let n = rows.len();
    set(s, id, rows, mode(p));
    s.notify(format!("{n} features selected"), false);
    Ok(json!({"selected": n}))
}

fn by_rect(s: &mut Session, p: &Value) -> CmdResult {
    let (a, b, c, d) = (f(p, "minLon"), f(p, "minLat"), f(p, "maxLon"), f(p, "maxLat"));
    let (Some(a), Some(b), Some(c), Some(d)) = (a, b, c, d) else { return Err("minLon, minLat, maxLon and maxLat are required".into()) };
    let e = Extent::from_points(lonlat_to_merc(Coord::new(a, b)), lonlat_to_merc(Coord::new(c, d)));
    let scale = s.scale();
    let m = mode(p);
    let hits = s.map_mut().hits_in(&e, scale);
    if m == Mode::New {
        s.map_mut().clear_selection();
    }
    let mut total = 0;
    for (id, rows) in hits {
        total += rows.len();
        set(s, id, rows, if m == Mode::New { Mode::Add } else { m });
    }
    Ok(json!({"selected": total}))
}

fn hit(s: &mut Session, p: &Value) -> Result<Option<(u32, usize)>, String> {
    let lon = f(p, "lon").ok_or("lon is required")?;
    let lat = f(p, "lat").ok_or("lat is required")?;
    let tol = f(p, "tolerancePx").unwrap_or(4.0).clamp(0.0, 50.0) * s.map().camera.res;
    let scale = s.scale();
    Ok(s.map_mut().hit(lonlat_to_merc(Coord::new(lon, lat)), tol, scale))
}

fn at_point(s: &mut Session, p: &Value) -> CmdResult {
    let m = mode(p);
    match hit(s, p)? {
        Some((id, row)) => {
            let selectable = s.map().layer(id).is_some_and(|l| l.selectable);
            if selectable {
                set(s, id, vec![row], m);
            }
            Ok(json!({"layer": id, "row": row}))
        }
        None => {
            if m == Mode::New {
                s.map_mut().clear_selection();
            }
            Ok(Value::Null)
        }
    }
}

fn identify(s: &mut Session, p: &Value) -> CmdResult {
    let Some((id, row)) = hit(s, p)? else {
        // No feature: report the pixel of the topmost raster instead.
        let (lon, lat) = (f(p, "lon").unwrap_or(f64::NAN), f(p, "lat").unwrap_or(f64::NAN));
        return Ok(super::raster::identify(s, lon, lat).unwrap_or(Value::Null));
    };
    let l = s.map().layer(id).ok_or("no layer")?;
    let mut attrs = serde_json::Map::new();
    for (i, fd) in l.dataset.fields.iter().enumerate() {
        attrs.insert(fd.name.clone(), json!(l.dataset.value(row, i).to_string()));
    }
    Ok(json!({"layer": l.name, "layerId": id, "row": row, "attributes": attrs}))
}
