//! Edit tab: create and delete features, edit attributes and fields.

use rustgis_data::{FieldType, Geometry, GeometryType, Value};
use serde_json::{Value as Json, json};

use super::{need_layer, need_selection, ps};
use crate::{CmdResult, CommandSpec as C, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("edit.addFeature", "Create Feature", "Edit › Features", add_feature)
            .when(need_layer)
            .params(r#"{"layer"?, "geometry": GeoJSON geometry (lon/lat), "attributes"?: {field: value}}"#),
        C::new("edit.delete", "Delete", "Edit › Features", delete).key("Delete").when(need_selection),
        C::new("edit.setValue", "Edit Attribute", "Attribute Table", set_value)
            .when(need_layer)
            .params(r#"{"layer"?, "row": number, "field": string, "value": any}"#),
        C::new("edit.addField", "Add Field", "Data › Data Design › Fields", add_field)
            .when(need_layer)
            .params(r#"{"layer"?, "name": string, "type": "long"|"double"|"text"|"boolean"}"#),
        C::new("edit.deleteField", "Delete Field", "Data › Data Design › Fields", delete_field)
            .when(need_layer)
            .params(r#"{"layer"?, "field": string}"#),
        C::new("edit.calculateField", "Calculate Field", "Attribute Table", calculate_field)
            .when(need_layer)
            .params(r#"{"layer"?, "field": string, "expression": "number, 'text', or field op field/number (+ - * /)"}"#),
    ]
}

fn add_feature(s: &mut Session, p: &Json) -> CmdResult {
    let id = s.layer_id(p)?;
    let g = p.get("geometry").and_then(rustgis_data::io::geometry_from_geojson).ok_or("geometry must be a GeoJSON geometry")?;
    let l = s.map_mut().layer_mut(id).ok_or("no layer")?;
    let want = l.geometry_type();
    let ok = want == g.kind() || want == GeometryType::None;
    if !ok {
        return Err(format!("{} can't go in a {} layer", g.kind().label(), want.label()));
    }
    if let Geometry::Polygon(polys) = &g
        && polys.iter().any(|r| r.first().is_none_or(|r| r.len() < 4))
    {
        return Err("a polygon needs at least three points".into());
    }
    let attrs: Vec<Value> = l
        .dataset
        .fields
        .iter()
        .map(|f| p.get("attributes").and_then(|a| a.get(&f.name)).map(|v| coerce(Value::from_json(v), f.ty)).unwrap_or(Value::Null))
        .collect();
    l.dataset.push(Some(g), attrs);
    l.touch_geometry();
    let row = l.dataset.features.len() - 1;
    l.selection = vec![row];
    Ok(json!({"row": row}))
}

fn coerce(v: Value, ty: FieldType) -> Value {
    match v {
        Value::Null => Value::Null,
        Value::Text(t) => Value::parse(&t, ty),
        other => Value::parse(&other.to_string(), ty),
    }
}

fn delete(s: &mut Session, _: &Json) -> CmdResult {
    let mut n = 0;
    for l in &mut s.map_mut().layers {
        if l.selection.is_empty() {
            continue;
        }
        let mut sel = std::mem::take(&mut l.selection);
        sel.sort_unstable();
        sel.dedup();
        for r in sel.into_iter().rev() {
            if r < l.dataset.features.len() {
                l.dataset.features.remove(r);
                n += 1;
            }
        }
        l.touch_geometry();
    }
    s.notify(format!("Deleted {n} features"), false);
    Ok(json!({"deleted": n}))
}

fn field_index(s: &Session, id: u32, name: &str) -> Result<usize, String> {
    s.map().layer(id).and_then(|l| l.dataset.field_index(name)).ok_or_else(|| format!("no field named '{name}'"))
}

fn set_value(s: &mut Session, p: &Json) -> CmdResult {
    let id = s.layer_id(p)?;
    let row = p.get("row").and_then(Json::as_u64).ok_or("row is required")? as usize;
    let fi = field_index(s, id, ps(p, "field").ok_or("field is required")?)?;
    let l = s.map_mut().layer_mut(id).ok_or("no layer")?;
    let ty = l.dataset.fields.get(fi).map(|f| f.ty).ok_or("no field")?;
    let raw = p.get("value").cloned().unwrap_or(Json::Null);
    let v = match raw {
        Json::String(t) => Value::parse(&t, ty),
        other => coerce(Value::from_json(&other), ty),
    };
    let cell = l.dataset.features.get_mut(row).and_then(|f| f.attrs.get_mut(fi)).ok_or("no such row")?;
    *cell = v;
    Ok(Json::Null)
}

fn add_field(s: &mut Session, p: &Json) -> CmdResult {
    let id = s.layer_id(p)?;
    let name = ps(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or("name is required")?;
    if !name.chars().all(|c| c.is_alphanumeric() || c == '_') || name.chars().count() > 64 {
        return Err("field names use letters, digits and _ (at most 64)".into());
    }
    let ty = match ps(p, "type").unwrap_or("double").to_ascii_lowercase().as_str() {
        "long" | "integer" | "int" | "short" => FieldType::Integer,
        "double" | "float" | "number" => FieldType::Double,
        "text" | "string" => FieldType::Text,
        "boolean" | "bool" => FieldType::Boolean,
        other => return Err(format!("unknown field type '{other}'")),
    };
    let l = s.map_mut().layer_mut(id).ok_or("no layer")?;
    if l.dataset.field_index(name).is_some() {
        return Err(format!("a field named '{name}' already exists"));
    }
    l.dataset.ensure_field(name, ty);
    Ok(Json::Null)
}

fn delete_field(s: &mut Session, p: &Json) -> CmdResult {
    let id = s.layer_id(p)?;
    let fi = field_index(s, id, ps(p, "field").ok_or("field is required")?)?;
    let l = s.map_mut().layer_mut(id).ok_or("no layer")?;
    l.dataset.fields.remove(fi);
    for f in &mut l.dataset.features {
        if fi < f.attrs.len() {
            f.attrs.remove(fi);
        }
    }
    Ok(Json::Null)
}

/// A tiny calculator: `a`, `a op b` where a/b are numbers, 'text' or field names.
fn calculate_field(s: &mut Session, p: &Json) -> CmdResult {
    let id = s.layer_id(p)?;
    let fi = field_index(s, id, ps(p, "field").ok_or("field is required")?)?;
    let expr = ps(p, "expression").map(str::trim).filter(|e| !e.is_empty()).ok_or("expression is required")?.to_string();
    let l = s.map_mut().layer_mut(id).ok_or("no layer")?;
    let ty = l.dataset.fields.get(fi).map(|f| f.ty).ok_or("no field")?;
    let (lhs, op, rhs) = split_op(&expr);
    enum Term {
        Num(f64),
        Text(String),
        Field(usize),
    }
    let term = |t: &str| -> Result<Term, String> {
        let t = t.trim();
        if let Some(q) = t.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')) {
            return Ok(Term::Text(q.to_string()));
        }
        if let Ok(n) = t.parse::<f64>() {
            return Ok(Term::Num(n));
        }
        let name = t.trim_matches('!').trim_matches('"');
        l.dataset.field_index(name).map(Term::Field).ok_or_else(|| format!("no field named '{name}'"))
    };
    let a = term(lhs)?;
    let b = rhs.map(term).transpose()?;
    let rows: Vec<usize> = if l.selection.is_empty() { (0..l.dataset.features.len()).collect() } else { l.selection.clone() };
    let get = |t: &Term, ds: &rustgis_data::Dataset, r: usize| -> Value {
        match t {
            Term::Num(n) => Value::Float(*n),
            Term::Text(s) => Value::Text(s.clone()),
            Term::Field(i) => ds.value(r, *i).clone(),
        }
    };
    let mut out = Vec::new();
    for r in &rows {
        let va = get(&a, &l.dataset, *r);
        let v = match (&b, op) {
            (Some(b), Some(op)) => {
                let vb = get(b, &l.dataset, *r);
                match (va.as_f64(), vb.as_f64(), op) {
                    (Some(x), Some(y), '+') => Value::Float(x + y),
                    (Some(x), Some(y), '-') => Value::Float(x - y),
                    (Some(x), Some(y), '*') => Value::Float(x * y),
                    (Some(x), Some(y), '/') if y != 0.0 => Value::Float(x / y),
                    (_, _, '+') if !va.is_null() && !vb.is_null() => Value::Text(format!("{va}{vb}")),
                    _ => Value::Null,
                }
            }
            _ => va,
        };
        out.push((*r, coerce(v, ty)));
    }
    for (r, v) in out {
        if let Some(c) = l.dataset.features.get_mut(r).and_then(|f| f.attrs.get_mut(fi)) {
            *c = v;
        }
    }
    Ok(json!({"rows": rows.len()}))
}

/// Splits on the first top-level arithmetic operator outside quotes (not a leading minus).
fn split_op(e: &str) -> (&str, Option<char>, Option<&str>) {
    let mut q = false;
    for (i, c) in e.char_indices() {
        if c == '\'' {
            q = !q;
        }
        if !q && i > 0 && matches!(c, '+' | '-' | '*' | '/') {
            let before = e.get(..i).unwrap_or("").trim_end();
            if before.ends_with(['e', 'E']) && before.chars().rev().nth(1).is_some_and(|d| d.is_ascii_digit()) {
                continue;
            }
            return (e.get(..i).unwrap_or(""), Some(c), e.get(i + c.len_utf8()..));
        }
    }
    (e, None, None)
}
