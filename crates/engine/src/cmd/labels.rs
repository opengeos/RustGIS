//! View › Labeling (lock, pause, unplaced labels, convert to annotation) and the Style tab's label
//! visibility range and placement.

use rustgis_data::{Dataset, Field, FieldType, Geometry, GeometryType, Value as Attr};
use rustgis_map::{LabelPlacement, Renderer, Rgba, Symbol};
use serde_json::{Value, json};

use super::layer::with;
use super::{b, f, need_feature_layer, ps};
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("labels.lock", "Lock", "View › Labeling", |s, p| {
            let d = &mut s.map_mut().label_display;
            d.locked = b(p, "on").unwrap_or(!d.locked);
            Ok(json!({"locked": d.locked}))
        })
        .view()
        .params(r#"{"on"?: bool}"#),
        C::new("labels.pause", "Pause", "View › Labeling", |s, p| {
            let d = &mut s.map_mut().label_display;
            d.paused = b(p, "on").unwrap_or(!d.paused);
            Ok(json!({"paused": d.paused}))
        })
        .view()
        .params(r#"{"on"?: bool}"#),
        C::new("labels.viewUnplaced", "View Unplaced", "View › Labeling", |s, p| {
            let d = &mut s.map_mut().label_display;
            d.show_unplaced = b(p, "on").unwrap_or(!d.show_unplaced);
            Ok(json!({"showUnplaced": d.show_unplaced}))
        })
        .view()
        .params(r#"{"on"?: bool}"#),
        C::new("labels.convert", "Convert To Annotation", "View › Labeling", convert)
            .when(need_feature_layer)
            .params(r#"{"layer"?, "name"?: string}"#),
        C::new("labels.scaleRange", "Label Visibility Range", "Style › Label Range", scale_range)
            .when(need_feature_layer)
            .params(r#"{"layer"?, "min"?: scale|null (out beyond), "max"?: scale|null (in beyond), "clear"?: bool (clears the scale and distance limits, as Clear Limits does)}"#),
        C::new("labels.distanceRange", "Label Distance Range", "Style › Label Range", distance_range)
            .when(need_feature_layer)
            .params(r#"{"layer"?, "closest"?: metres|null, "farthest"?: metres|null, "clear"?: bool}"#),
        C::new("labels.placement", "Label Placement", "Style › Label Placement", |s, p| {
            let code = ps(p, "position").ok_or("position is required")?;
            let pos = LabelPlacement::from_code(code).ok_or_else(|| format!("unknown placement '{code}'"))?;
            with(s, p, |l| l.labels.placement = pos)
        })
        .when(need_feature_layer)
        .params(
            r#"{"layer"?, "position": "auto"|"center"|"above"|"below"|"left"|"right"|"aboveRight"|"aboveLeft"|"belowRight"|"belowLeft"}"#,
        ),
    ]
}

/// A limit parameter: `Some(None)` clears it, `Some(Some(v))` sets it, `None` leaves it.
fn limit(p: &Value, k: &str) -> Result<Option<Option<f64>>, String> {
    match p.get(k) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(t)) if t.trim().is_empty() => Ok(Some(None)),
        Some(_) => match f(p, k) {
            Some(v) if v > 0.0 => Ok(Some(Some(v.min(1e12)))),
            Some(v) if v.abs() < f64::EPSILON => Ok(Some(None)),
            _ => Err(format!("{k} must be a positive number")),
        },
    }
}

fn scale_range(s: &mut Session, p: &Value) -> CmdResult {
    let clear = b(p, "clear").unwrap_or(false);
    let (min, max) = (limit(p, "min")?, limit(p, "max")?);
    with(s, p, |l| {
        if clear {
            l.labels.min_scale = None;
            l.labels.max_scale = None;
            l.labels.closest = None;
            l.labels.farthest = None;
        }
        if let Some(v) = min {
            l.labels.min_scale = v;
        }
        if let Some(v) = max {
            l.labels.max_scale = v;
        }
    })
}

fn distance_range(s: &mut Session, p: &Value) -> CmdResult {
    let clear = b(p, "clear").unwrap_or(false);
    let (closest, farthest) = (limit(p, "closest")?, limit(p, "farthest")?);
    with(s, p, |l| {
        if clear {
            l.labels.closest = None;
            l.labels.farthest = None;
        }
        if let Some(v) = closest {
            l.labels.closest = v;
        }
        if let Some(v) = farthest {
            l.labels.farthest = v;
        }
    })
}

/// Converts a layer's labels into an annotation layer: one point per label, at the label point,
/// carrying the label text and drawn with the same text symbol. The layer's own labels turn off.
fn convert(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let l = s.map().layer(id).ok_or("no such layer")?;
    let fi = l.dataset.field_index(&l.labels.field).ok_or("the layer has no label field")?;
    let rows = l.visible_rows();
    let mut ds = Dataset::new(
        ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).map(String::from).unwrap_or_else(|| format!("{} Annotation", l.name)),
        GeometryType::Point,
        vec![Field::new("TEXT", FieldType::Text)],
    );
    for (row, ft) in l.dataset.features.iter().enumerate() {
        let v = l.dataset.value(row, fi);
        if rows.get(row) != Some(&true) || v.is_null() {
            continue;
        }
        if let Some(c) = ft.geometry.as_ref().and_then(Geometry::label_point) {
            ds.push(Some(Geometry::Point(c)), vec![Attr::Text(v.to_string())]);
        }
    }
    if ds.features.is_empty() {
        return Err("the layer has no labels to convert".into());
    }
    let n = ds.features.len();
    let mut labels = l.labels.clone();
    labels.enabled = true;
    labels.field = "TEXT".into();
    labels.placement = LabelPlacement::Center;
    let (min_scale, max_scale) = (l.min_scale, l.max_scale);
    if let Some(src) = s.map_mut().layer_mut(id) {
        src.labels.enabled = false;
    }
    let new_id = super::layer::add_dataset(s, ds);
    if let Some(a) = s.map_mut().layer_mut(new_id) {
        // The annotation is all text: the points themselves are invisible.
        let clear = Rgba::new(0, 0, 0, 0);
        a.renderer = Renderer::Single(Symbol { fill: clear, stroke: clear, stroke_width: 0.0, size: 0.0, marker: Default::default() });
        a.labels = labels;
        a.min_scale = min_scale;
        a.max_scale = max_scale;
    }
    Ok(json!({"layer": new_id, "annotations": n}))
}
