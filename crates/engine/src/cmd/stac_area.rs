//! The area a STAC search is limited to beyond a bounding box: GeoJSON given, the selected
//! features of a layer, or a file's features, merged into one geometry (`stac.search`'s
//! `intersects`), and widened by a distance in metres (`buffer`). The merge rules are
//! [`area`](crate::render::rustgis_tiles::stac::area)'s.

use serde_json::{Value, json};

use super::ps;
use crate::render::rustgis_tiles::stac::area::{self, Area};
use crate::{CmdResult, CommandSpec as C, Session};
use rustgis_data::Geometry;

pub fn commands() -> Vec<C> {
    vec![C::new("stac.area", "STAC Search Area", "Home › Add › Web Data › STAC Search", area_cmd).roles(&[]).effects(crate::policy::Effects::NONE.with(crate::policy::Effect::Files)).params(
        r#"{"path"?: a file of any format RustGIS reads (GeoJSON, a zipped Shapefile, GeoPackage…): its features are merged, "layer"?: a layer's name or id whose selected features are merged (default: the active layer; used when path is not given), "buffer"?: metres to widen a point or line into a polygon (default 0; polygons are not changed)} → {"intersects": GeoJSON geometry for stac.search, "positions": n, "ignored": n}. Only the highest dimension present is kept (polygons, then lines, then points); the parts of a lower one are counted and reported."#,
    )]
}

/// The most metres a search area is widened by.
pub const MAX_BUFFER_M: f64 = 1_000_000.0;

fn area_cmd(s: &mut Session, p: &Value) -> CmdResult {
    let geometries = match ps(p, "path").map(str::trim).filter(|x| !x.is_empty()) {
        Some(path) => from_file(s, path)?,
        None => selected(s, p)?,
    };
    let area = merged(s, &geometries, p)?.ok_or("there is no point, line or polygon to search with")?;
    Ok(json!({"intersects": area::to_geojson(&area.geometry), "positions": area::positions(&area.geometry), "ignored": area.ignored}))
}

#[cfg(not(target_arch = "wasm32"))]
fn from_file(s: &mut Session, path: &str) -> Result<Vec<Geometry>, String> {
    let (ds, report) = rustgis_data::io::read_path(std::path::Path::new(path)).map_err(|e| format!("{path}: {e}"))?;
    for m in report.messages {
        s.warn(format!("{path}: {m}"));
    }
    if ds.features.len() > area::MAX_FEATURES {
        return Err(format!("{path} has more than {} features: simplify it or use a bounding box", area::MAX_FEATURES));
    }
    Ok(ds.features.iter().filter_map(|f| f.geometry.clone()).collect())
}

#[cfg(target_arch = "wasm32")]
fn from_file(_s: &mut Session, _path: &str) -> Result<Vec<Geometry>, String> {
    Err("in the web app, choose the search area's file with Home › Add › Web Data › STAC Search".into())
}

/// The selected features' geometries of the layer `p` names.
fn selected(s: &Session, p: &Value) -> Result<Vec<Geometry>, String> {
    let id = s.layer_id(p)?;
    let layer = s.map().layer(id).ok_or("no such layer")?;
    if layer.selection.is_empty() {
        return Err(format!("{}: select the features to search with first", layer.name));
    }
    Ok(layer.selection.iter().filter_map(|row| layer.dataset.features.get(*row)).filter_map(|f| f.geometry.clone()).collect())
}

/// The search area of GeoJSON given as `intersects`, or of the selection when it is `"selection"`
/// (the layer `layer` names). `None` when the search has no `intersects`.
pub fn from_params(s: &mut Session, p: &Value) -> Result<Option<Area>, String> {
    let geometries = match p.get("intersects") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(v)) if v.trim().eq_ignore_ascii_case("selection") => selected(s, p)?,
        Some(v @ Value::Object(_)) => area::geometries_of_geojson(v)?,
        Some(_) => return Err("intersects is GeoJSON (a geometry, a feature or a collection of features), or \"selection\"".into()),
    };
    Ok(Some(merged(s, &geometries, p)?.ok_or("the search area has no point, line or polygon to search with")?))
}

/// Merges geometries, widens them by `buffer`, and says what was left out.
fn merged(s: &mut Session, geometries: &[Geometry], p: &Value) -> Result<Option<Area>, String> {
    let metres = match p.get("buffer") {
        None | Some(Value::Null) => 0.0,
        Some(v) => match v.as_f64() {
            Some(m) if m.is_finite() && (0.0..=MAX_BUFFER_M).contains(&m) => m,
            _ => return Err(format!("buffer is a distance in metres, 0 to {MAX_BUFFER_M}")),
        },
    };
    let Some(mut area) = area::merge(geometries)? else { return Ok(None) };
    if area.ignored > 0 {
        s.warn(format!(
            "the search area keeps only its {}: {} part(s) of a lower dimension are left out",
            match area.geometry {
                Geometry::Polygon(_) => "polygons",
                Geometry::Line(_) => "lines",
                _ => "points",
            },
            area.ignored
        ));
    }
    if area.degenerate > 0 {
        s.warn(format!(
            "{} line(s) or polygon(s) of the search area have too few positions to search with and are left out",
            area.degenerate
        ));
    }
    if metres > 0.0 {
        if matches!(area.geometry, Geometry::Polygon(_)) {
            s.warn("the search area's buffer is not applied: it widens points and lines, and a polygon already has an area");
        } else {
            let widened = rustgis_analysis::geodesic::geodesic_buffer(&area.geometry, metres)
                .ok_or("the buffer of the search area is empty: give a larger distance")?;
            // Eight decimals, as the merge keeps; the buffer is a polygon, so it is merged again.
            area = area::merge(&[widened])?
                .map(|a| Area { ignored: area.ignored, degenerate: area.degenerate, ..a })
                .ok_or("the buffer of the search area is empty")?;
        }
    }
    Ok(Some(area))
}

/// Whether the search names a shape of its own, which replaces its `bbox`.
pub fn names_bbox(p: &Value) -> bool {
    !matches!(p.get("bbox"), None | Some(Value::Null)) || super::b(p, "view").unwrap_or(false)
}
