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
    vec![C::new("stac.area", "STAC Search Area", "Home › Add › Web Data › STAC Search", area_cmd).roles(&[]).effects(crate::policy::Effects::NONE.with(crate::policy::Effect::Files)).view().params(
        r#"{"path"?: a file of any format RustGIS reads (GeoJSON, a zipped Shapefile, GeoPackage…): its features are merged, "layer"?: a layer's name or id whose selected features are merged (default: the active layer; used when path is not given), "buffer"?: the distance to widen a point or line into a polygon (default 0; polygons are not changed), "unit"?: "Meters" | "Feet" | "Kilometers" | "Miles" (the buffer's unit, default Meters)} → {"intersects": GeoJSON geometry for stac.search, "positions": n, "ignored": n}. Only the highest dimension present is kept (polygons, then lines, then points); the parts of a lower one are counted and reported."#,
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

fn from_file(s: &mut Session, path: &str) -> Result<Vec<Geometry>, String> {
    let (geometries, messages) = read_geometries(path)?;
    for m in messages {
        s.warn(m);
    }
    Ok(geometries)
}

/// A file's features' geometries and what the reader says of it (each naming the file). Needs no
/// session, so the app reads a large file on a thread of its own.
#[cfg(not(target_arch = "wasm32"))]
fn read_geometries(path: &str) -> Result<(Vec<Geometry>, Vec<String>), String> {
    let (ds, report) = rustgis_data::io::read_path(std::path::Path::new(path)).map_err(|e| format!("{path}: {e}"))?;
    if ds.features.len() > area::MAX_FEATURES {
        return Err(format!("{path} has more than {} features: simplify it or use a bounding box", area::MAX_FEATURES));
    }
    let messages = report.messages.into_iter().map(|m| format!("{path}: {m}")).collect();
    Ok((ds.features.iter().filter_map(|f| f.geometry.clone()).collect(), messages))
}

#[cfg(target_arch = "wasm32")]
fn read_geometries(_path: &str) -> Result<(Vec<Geometry>, Vec<String>), String> {
    Err("in the web app, choose the search area's file with Home › Add › Web Data › STAC Search".into())
}

/// A file's features merged into one search area, with what to tell the user about it.
pub struct Loaded {
    pub geometry: Geometry,
    /// The notices: what the reader and the merge left out, one per kind.
    pub notices: Vec<String>,
}

/// Reads a file and merges its features by the search area's rules (no buffer: that is applied
/// when the search runs). Takes no session and reads no state, so it runs on any thread.
pub fn load_file(path: &str) -> Result<Loaded, String> {
    let (geometries, notices) = read_geometries(path)?;
    merge_loaded(&geometries, notices).map_err(|e| format!("{path}: {e}"))
}

/// Geometries merged into one search area by the same rules, with `notices` (already said of
/// where they came from) and what the merge left out. The merge has no Cancel check inside it
/// (one dissolve of at most [`area::MAX_FEATURES`] features), so the app runs it on a thread of
/// its own and drops the answer when the user cancels.
pub fn merge_loaded(geometries: &[Geometry], mut notices: Vec<String>) -> Result<Loaded, String> {
    let area = area::merge(geometries)?.ok_or("there is no point, line or polygon to search with")?;
    notices.extend(notes(&area));
    Ok(Loaded { geometry: area.geometry, notices })
}

/// The geometries of the selected features of the layer `p` names (`layer`, default the active
/// layer): a copy, so the merge can run off the session's thread.
pub fn selected_geometries(s: &Session, p: &Value) -> Result<Vec<Geometry>, String> {
    let geometries = selected(s, p)?;
    if geometries.len() > area::MAX_FEATURES {
        return Err(format!("more than {} features are selected: select fewer or use a bounding box", area::MAX_FEATURES));
    }
    Ok(geometries)
}

/// The distance units a buffer may be given in, with their length in metres.
fn metres_per(unit: Option<&str>) -> Result<f64, String> {
    match unit.map(str::trim).filter(|u| !u.is_empty()) {
        None => Ok(1.0),
        Some(u) => rustgis_geom::distance_unit_metres(&u.to_ascii_lowercase())
            .ok_or_else(|| "unit is Meters, Feet, Kilometers or Miles".to_string()),
    }
}

/// `distance` in `unit` (default metres) as metres, within what a search area may be widened by.
pub fn buffer_metres(distance: f64, unit: Option<&str>) -> Result<f64, String> {
    let metres = distance * metres_per(unit)?;
    if distance.is_finite() && (0.0..=MAX_BUFFER_M).contains(&metres) {
        Ok(metres)
    } else {
        Err(format!(
            "buffer is a distance of 0 to {MAX_BUFFER_M} metres ({} {})",
            MAX_BUFFER_M / metres_per(unit).unwrap_or(1.0),
            unit.unwrap_or("metres")
        ))
    }
}

/// What merging left out of an area, one notice per kind with its count.
fn notes(area: &Area) -> Vec<String> {
    let mut out = Vec::new();
    if area.ignored > 0 {
        out.push(format!(
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
        out.push(format!(
            "{} line(s) or polygon(s) of the search area have too few positions to search with and are left out",
            area.degenerate
        ));
    }
    out
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
    let unit = ps(p, "unit");
    let metres = match p.get("buffer") {
        None | Some(Value::Null) => {
            // A unit that names nothing is still refused, so a typo isn't silently metres.
            metres_per(unit)?;
            0.0
        }
        Some(v) => match v.as_f64() {
            Some(d) => buffer_metres(d, unit)?,
            None => return Err(format!("buffer is a number: a distance in {} (0 to {MAX_BUFFER_M} metres)", unit.unwrap_or("metres"))),
        },
    };
    let Some(mut area) = area::merge(geometries)? else { return Ok(None) };
    for n in notes(&area) {
        s.warn(n);
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

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_geom::Coord;

    fn point(x: f64, y: f64) -> Geometry {
        Geometry::Point(Coord::new(x, y))
    }

    fn square() -> Geometry {
        let ring = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.0)].map(|(x, y)| Coord::new(x, y));
        Geometry::Polygon(vec![vec![ring.to_vec()]])
    }

    #[test]
    fn a_buffer_is_converted_from_its_unit_and_bounded_in_metres() {
        assert_eq!(buffer_metres(100.0, None), Ok(100.0));
        assert_eq!(buffer_metres(2.0, Some("Kilometers")), Ok(2000.0));
        assert!(buffer_metres(1.0, Some("Feet")).is_ok_and(|m| (m - 0.3048).abs() < 1e-9));
        assert!(buffer_metres(1.0, Some("Miles")).is_ok_and(|m| (m - 1609.344).abs() < 1e-6));
        // Out of range in the unit given, not in the number typed.
        assert!(buffer_metres(1_000.1, Some("Kilometers")).is_err());
        assert!(buffer_metres(1_000.0, Some("Kilometers")).is_ok());
        for bad in [-1.0, f64::NAN, f64::INFINITY] {
            assert!(buffer_metres(bad, None).is_err(), "{bad}");
        }
        assert!(buffer_metres(1.0, Some("furlongs")).is_err());
    }

    #[test]
    fn merging_loaded_geometries_says_what_it_leaves_out_and_clean_input_says_nothing() {
        let clean = merge_loaded(&[point(1.0, 2.0), point(3.0, 4.0)], Vec::new()).unwrap();
        assert!(clean.notices.is_empty(), "{:?}", clean.notices);
        assert!(matches!(clean.geometry, Geometry::MultiPoint(_) | Geometry::Point(_)));
        // A polygon and a point: only the polygon is kept, and it is said once.
        let mixed = merge_loaded(&[square(), point(5.0, 5.0)], vec!["file.geojson: a note".into()]).unwrap();
        assert!(matches!(mixed.geometry, Geometry::Polygon(_)));
        assert_eq!(mixed.notices.len(), 2, "{:?}", mixed.notices);
        assert!(mixed.notices.iter().any(|n| n.contains("lower dimension")));
        assert!(merge_loaded(&[], Vec::new()).is_err());
    }

    #[test]
    fn a_missing_file_is_an_error_not_an_empty_area() {
        assert!(load_file("/no/such/file.geojson").is_err());
    }
}
