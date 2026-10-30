//! The Diagram commands (#976): a small chart drawn on each feature from several numeric fields,
//! as ArcGIS Pro's Chart symbology and QGIS's Diagrams draw. Part of Style › Drawing, alongside
//! the renderer commands in `layer` (a diagram goes on top of the renderer's own symbol).

use rustgis_data::GeometryType;
use rustgis_map::diagram::{CATEGORY_COLORS, MAX_CATEGORIES, MAX_SIZE, MIN_SIZE};
use rustgis_map::{DiagramCategory, DiagramKind, DiagramLabel, DiagramSettings, DiagramSize, Rgba};
use serde_json::{Value, json};

use super::layer::with;
use super::{f, need_feature_layer, ps};
use crate::{CmdResult, CommandSpec as C, Session};

/// A size asked for by hand: one in points, or two it runs between as the categories' sum grows.
enum Asked {
    Fixed(f32),
    Scaled(f32, f32),
}

pub fn commands() -> Vec<C> {
    vec![
        C::new("layer.diagram", "Set Diagram", "Style › Drawing", set)
            .when(need_feature_layer)
            .params(
                r##"{"layer"?, "kind": "pie"|"bar", "fields": [numeric field names, or {"field": name, "color"?: "#RRGGBB"|"#RRGGBBAA"}] (a slice or bar each, in order; colours default to the palette new layers take), "size"?: points (fixed: a pie's radius or a bar chart's height, the same for every feature) | {"from"?: points (default 6), "to"?: points (default 40)} (grown with the categories' sum, square-root scaled as Proportional symbols are; the default), "startAngle"?: degrees clockwise from north (a pie's first slice; default 0), "labels"?: "none"|"value"|"percent"|"both" (what each slice or bar says beside it: its value in the legend's number format, its share of the total in percent, or both; default "none"), "background"?: "#RRGGBB"|"#RRGGBBAA"|null (the disc or box behind the chart; default none), "outline"?: {"color"?: "#RRGGBB"|"#RRGGBBAA" (default #4D4D4D), "width"?: points (default 0.75)}} (drawn on each feature over its symbol)"##,
            ),
        C::new("layer.clearDiagram", "Clear Diagram", "Style › Drawing", |s, p| with(s, p, |l| l.diagram = None))
            .when(need_feature_layer)
            .params(r#"{"layer"?: name|id (removes the layer's diagram; the renderer is untouched)}"#),
        C::new("layer.diagramInfo", "Diagram Settings", "Style › Drawing", info)
            .view()
            .when(need_feature_layer)
            .params(r#"{"layer"?: name|id} → {"kind", "fields": [{"field", "color"}], "size", "startAngle"?, "labels", "background"?, "outline"} or null without one"#),
    ]
}

/// Sets the layer's diagram (`layer.diagram`).
fn set(s: &mut Session, p: &Value) -> CmdResult {
    let kind = ps(p, "kind").and_then(DiagramKind::from_code).ok_or("kind is \"pie\" or \"bar\"")?;
    let list = match p.get("fields") {
        Some(Value::Array(v)) => v,
        _ => return Err("fields is required: a list of numeric field names, or of {\"field\", \"color\"?}".into()),
    };
    if list.is_empty() || list.len() > MAX_CATEGORIES {
        return Err(format!("fields lists 1 to {MAX_CATEGORIES} fields"));
    }
    // The categories asked for, their colours optional (the palette's turn otherwise).
    let mut asked: Vec<(String, Option<Rgba>)> = Vec::with_capacity(list.len());
    for item in list {
        let (field, colour) = match item {
            Value::String(f) if !f.trim().is_empty() => (f.trim().to_string(), None),
            Value::Object(o) => {
                let field = o
                    .get("field")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|f| !f.is_empty())
                    .ok_or("each field is a name, or {\"field\": name, \"color\"?: \"#RRGGBB\"}")?
                    .to_string();
                let colour = match o.get("color") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(c)) => Some(Rgba::from_hex(c).ok_or("color must be #RRGGBB or #RRGGBBAA")?),
                    Some(_) => return Err("color must be #RRGGBB or #RRGGBBAA".into()),
                };
                (field, colour)
            }
            _ => return Err("each field is a name, or {\"field\": name, \"color\"?: \"#RRGGBB\"}".into()),
        };
        asked.push((field, colour));
    }
    let size = match p.get("size") {
        None | Some(Value::Null) => Asked::Scaled(6.0, 40.0),
        Some(v @ Value::Object(_)) => Asked::Scaled(
            v.get("from").and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or(6.0) as f32,
            v.get("to").and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or(40.0) as f32,
        ),
        Some(_) => Asked::Fixed(
            f(p, "size")
                .filter(|v| *v >= f64::from(MIN_SIZE) && *v <= f64::from(MAX_SIZE))
                .ok_or("size is points from 1 to 200 (a fixed size), or {\"from\": points, \"to\": points} (grown with the sum)")?
                as f32,
        ),
    };
    let start_angle = f(p, "startAngle").filter(|a| a.is_finite()).map(|a| a.rem_euclid(360.0) as f32).unwrap_or(0.0);
    let background = match p.get("background") {
        None | Some(Value::Null) => None,
        Some(Value::String(c)) => Some(Rgba::from_hex(c).ok_or("background must be #RRGGBB or #RRGGBBAA")?),
        Some(_) => return Err("background is #RRGGBB or #RRGGBBAA, or null for none".into()),
    };
    let (stroke, stroke_width) = match p.get("outline") {
        None | Some(Value::Null) => (DiagramSettings::default_stroke(), DiagramSettings::default_stroke_width()),
        Some(v @ Value::Object(_)) => (
            match v.get("color") {
                None | Some(Value::Null) => DiagramSettings::default_stroke(),
                Some(Value::String(c)) => Rgba::from_hex(c).ok_or("outline's color must be #RRGGBB or #RRGGBBAA")?,
                Some(_) => return Err("outline's color must be #RRGGBB or #RRGGBBAA".into()),
            },
            v.get("width").and_then(Value::as_f64).filter(|w| w.is_finite() && *w >= 0.0).map(|w| w as f32).unwrap_or(0.75),
        ),
        Some(_) => return Err("outline is {\"color\"?, \"width\"?: points}".into()),
    };
    let id = s.layer_id(p)?;
    let l = s.map_mut().layer_mut(id).ok_or("no such layer")?;
    if l.graphics {
        return Err("this works on feature layers, not graphics layers".into());
    }
    if matches!(l.dataset.geometry_type, GeometryType::None) {
        return Err("this works on layers with geometry".into());
    }
    // Every field must be there, and hold numbers.
    for (field, _) in &asked {
        let fi = l.dataset.field_index(field).ok_or_else(|| format!("no field named '{field}'"))?;
        if !l.dataset.fields.get(fi).is_some_and(|fd| fd.ty.is_numeric()) {
            return Err(format!("'{field}' is not a numeric field"));
        }
    }
    let categories: Vec<DiagramCategory> = asked
        .into_iter()
        .enumerate()
        .map(|(i, (field, colour))| DiagramCategory { field, colour: colour.unwrap_or(CATEGORY_COLORS[i % CATEGORY_COLORS.len()]) })
        .collect();
    // A scaled size ranges over the sums of the features that have any (kept in the settings, as
    // Proportional keeps its range); without any, the neutral range, which draws mid-size.
    // What each slice or bar says beside it: "none" (the default), "value", "percent" or "both".
    let labels = match p.get("labels") {
        None | Some(Value::Null) => DiagramLabel::None,
        Some(Value::String(code)) => DiagramLabel::from_code(code).ok_or("labels is \"none\", \"value\", \"percent\" or \"both\"")?,
        Some(_) => return Err("labels is \"none\", \"value\", \"percent\" or \"both\"".into()),
    };
    let size = match size {
        Asked::Fixed(s) => DiagramSize::Fixed(s),
        Asked::Scaled(min_size, max_size) => {
            let probe = DiagramSettings {
                kind,
                categories: categories.clone(),
                size: DiagramSize::Fixed(1.0),
                start_angle,
                background,
                stroke,
                stroke_width,
                labels,
            };
            let mut min = f64::INFINITY;
            let mut max = f64::NEG_INFINITY;
            for row in 0..l.dataset.features.len() {
                if let Some((_, total)) = probe.values(&l.dataset, row) {
                    min = min.min(total);
                    max = max.max(total);
                }
            }
            let (min, max) = if max >= min && min.is_finite() && max.is_finite() { (min, max) } else { (0.0, 1.0) };
            DiagramSize::Scaled { min_size, max_size, min, max }
        }
    };
    l.diagram = Some(DiagramSettings { kind, categories, size, start_angle, background, stroke, stroke_width, labels }.clamped());
    Ok(json!({
        "kind": l.diagram.as_ref().map(|d| d.kind.label().to_lowercase()),
        "fields": l.diagram.iter().flat_map(|d| d.categories.iter().map(|c| json!({"field": c.field, "color": c.colour.hex()}))).collect::<Vec<_>>(),
    }))
}

/// The layer's diagram settings, or null without one.
fn info(s: &mut Session, p: &Value) -> CmdResult {
    let id = s.layer_id(p)?;
    let l = s.map().layer(id).ok_or("no such layer")?;
    let Some(d) = &l.diagram else { return Ok(Value::Null) };
    Ok(json!({
        "kind": d.kind.label().to_lowercase(),
        "fields": d.categories.iter().map(|c| json!({"field": c.field, "color": c.colour.hex()})).collect::<Vec<_>>(),
        "size": match d.size {
            DiagramSize::Fixed(s) => json!({"fixed": s}),
            DiagramSize::Scaled { min_size, max_size, min, max } => {
                json!({"scaled": {"from": min_size, "to": max_size, "min": min, "max": max}})
            }
        },
        "startAngle": d.start_angle,
        "labels": d.labels.code(),
        "background": d.background.map_or(Value::Null, |c| Value::String(c.hex())),
        "outline": {"color": d.stroke.hex(), "width": d.stroke_width},
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Project, Session};
    use rustgis_data::{Dataset, Field, FieldType, Geometry, GeometryType, Value};
    use rustgis_geom::Coord;

    /// A session with a two-feature Regions layer: (52k, 48k) and (30k, 45k).
    fn regions() -> (Session, u32) {
        let mut s = Session::new(Project::default());
        let square = |x: f64| {
            Geometry::Polygon(vec![vec![vec![
                Coord::new(x, 0.0),
                Coord::new(x + 1.0, 0.0),
                Coord::new(x + 1.0, 1.0),
                Coord::new(x, 1.0),
                Coord::new(x, 0.0),
            ]]])
        };
        let mut ds = Dataset::new(
            "Regions",
            GeometryType::Polygon,
            vec![Field::new("pop_male", FieldType::Integer), Field::new("pop_female", FieldType::Integer)],
        );
        ds.push(Some(square(0.0)), vec![Value::Int(52_000), Value::Int(48_000)]);
        ds.push(Some(square(2.0)), vec![Value::Int(30_000), Value::Int(45_000)]);
        let id = s.map_mut().add_dataset(ds);
        s.active_layer = Some(id);
        (s, id)
    }

    #[test]
    fn a_diagram_is_set_read_and_cleared() {
        let (mut s, id) = regions();
        s.run("layer.diagram", &json!({"kind": "pie", "fields": ["pop_male", "pop_female"]})).unwrap();
        let d = s.map().layer(id).unwrap().diagram.clone().expect("set");
        assert_eq!(d.kind, DiagramKind::Pie);
        assert_eq!(d.categories.iter().map(|c| c.field.as_str()).collect::<Vec<_>>(), ["pop_male", "pop_female"]);
        // Colours default to the palette's turn.
        assert_eq!(d.categories[0].colour, CATEGORY_COLORS[0]);
        assert_eq!(d.categories[1].colour, CATEGORY_COLORS[1]);
        // The scaled size ranges over the features' sums: 75,000 to 100,000.
        assert!(matches!(d.size, DiagramSize::Scaled { min: 75_000.0, max: 100_000.0, .. }));
        // Read back.
        let info = s.run("layer.diagramInfo", &json!({})).unwrap();
        assert_eq!(info["kind"], "pie");
        assert_eq!(info["fields"].as_array().unwrap().len(), 2);
        assert_eq!(info["fields"][0]["field"], "pop_male");
        // A fixed size and hand colours.
        s.run("layer.diagram", &json!({"kind": "bar", "fields": [{"field": "pop_male", "color": "#4A90D9"}], "size": 18})).unwrap();
        let d = s.map().layer(id).unwrap().diagram.clone().expect("replaced");
        assert_eq!(d.kind, DiagramKind::Bar);
        assert!(matches!(d.size, DiagramSize::Fixed(18.0)));
        // Value and percent labels ride along, and read back.
        s.run("layer.diagram", &json!({"kind": "pie", "fields": ["pop_male", "pop_female"], "labels": "both"})).unwrap();
        let d = s.map().layer(id).unwrap().diagram.clone().expect("labelled");
        assert_eq!(d.labels, DiagramLabel::Both);
        assert_eq!(s.run("layer.diagramInfo", &json!({})).unwrap()["labels"], "both");
        assert!(s.run("layer.diagram", &json!({"kind": "pie", "fields": ["pop_male"], "labels": "shout"})).is_err());
        // Errors: an unknown field, a non-numeric one, a bad kind.
        assert!(s.run("layer.diagram", &json!({"kind": "pie", "fields": ["nope"]})).is_err());
        let (mut s, id) = regions();
        assert!(s.run("layer.diagram", &json!({"kind": "hexagon", "fields": ["pop_male"]})).is_err());
        assert!(s.map().layer(id).unwrap().diagram.is_none(), "refused, so unset");
        // Cleared, and undo brings it back.
        let (mut s, id) = regions();
        s.run("layer.diagram", &json!({"kind": "pie", "fields": ["pop_male", "pop_female"]})).unwrap();
        s.run("layer.clearDiagram", &json!({})).unwrap();
        assert!(s.map().layer(id).unwrap().diagram.is_none());
        assert!(s.undo());
        assert!(s.map().layer(id).unwrap().diagram.is_some(), "the clear is undone");
    }
}
