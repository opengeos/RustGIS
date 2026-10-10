//! Share tab: export the map as PNG and layers as GeoJSON/CSV.

use std::path::Path;

use rustgis_render::View;
use serde_json::{Value, json};

use super::{f, need_layer, ps};
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("share.exportMap", "Export Map", "Share › Output", export_map)
            .view()
            .params(r#"{"path": "*.png", "width"?: px, "height"?: px, "dpi"?: 96..600}"#),
        C::new("share.exportLayer", "Export Features", "Share › Output", export_layer)
            .view()
            .when(need_layer)
            .params(r#"{"layer"?, "path": "*.geojson|*.csv"}"#),
        C::new("share.exportTable", "Export Table", "Share › Output", |s, p| {
            let name = ps(p, "table").ok_or("table is required")?;
            let path = ps(p, "path").ok_or("path is required")?;
            let t = s.project.tables.iter().find(|t| t.name.eq_ignore_ascii_case(name)).ok_or_else(|| format!("no table '{name}'"))?;
            rustgis_data::io::write_path(t, Path::new(path)).map_err(|e| e.to_string())?;
            Ok(json!({"path": path}))
        })
        .view(),
    ]
}

/// Renders the active map at the current view (or a requested size) to PNG bytes.
pub fn map_png(s: &mut Session, width: Option<f64>, height: Option<f64>, dpi: Option<f64>) -> Result<Vec<u8>, String> {
    let (vw, vh) = s.view_size;
    // NaN would pass through `clamp`; non-finite values fall back to the defaults.
    let fin = |v: Option<f64>| v.filter(|x| x.is_finite());
    let (width, height, dpi) = (fin(width), fin(height), fin(dpi));
    let w = width.unwrap_or(vw).clamp(16.0, 8000.0);
    let h = height.unwrap_or(vh).clamp(16.0, 8000.0);
    let ppp = (dpi.unwrap_or(96.0) / 96.0).clamp(0.5, 6.0);
    let mut camera = s.map().camera;
    // Keep the same ground extent when the size differs from the view.
    if width.is_some() || height.is_some() {
        camera.res *= (vw / w).max(vh / h);
        camera.clamp();
    }
    let view = View { width: w, height: h, pixels_per_point: ppp, camera, dark: false };
    rustgis_render::render_png(s.map_mut(), &view)
}

fn export_map(s: &mut Session, p: &Value) -> CmdResult {
    let path = ps(p, "path").ok_or("path is required")?.to_string();
    let png = map_png(s, f(p, "width"), f(p, "height"), f(p, "dpi"))?;
    rustgis_data::io::write_atomic(std::path::Path::new(&path), &png).map_err(|e| format!("{path}: {e}"))?;
    s.notify(format!("Exported {path}"), false);
    Ok(json!({"path": path, "bytes": png.len()}))
}

fn export_layer(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let path = ps(p, "path").ok_or("path is required")?.to_string();
    let l = s.map().layer(id).ok_or("no layer")?;
    rustgis_data::io::write_path(&l.dataset, Path::new(&path)).map_err(|e| e.to_string())?;
    s.notify(format!("Exported {path}"), false);
    Ok(json!({"path": path}))
}
