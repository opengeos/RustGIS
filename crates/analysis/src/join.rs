//! Spatial Join's measures and merge rules: the ground distance between two features, a test that
//! rules out features too far away to matter, the overlap a feature has with a polygon, and the
//! rules that roll the values of several matches up into one.
//!
//! Distances are on the sphere buffers use. The nearest points of two features are found in an
//! azimuthal equidistant plane centred between them, then measured along the great circle, so the
//! distance is true for those points and the points themselves are within a few metres of the
//! nearest pair for features up to a few hundred kilometres apart.

use rustgis_data::{Coord, Dataset, FieldType, Geometry, Value};
use rustgis_geom::{Aeqd, EARTH_RADIUS_M, Extent, geodesic_interpolate, geodesic_length_m, geodesic_ring_area_m2, haversine_m};
use serde_json::Value as Json;

/// The first coordinate of a geometry: a point on it, for an upper bound on distances.
pub fn first_coord(g: &Geometry) -> Option<Coord> {
    let mut first = None;
    g.for_each_coord(|c| {
        if first.is_none() && c.is_finite() {
            first = Some(c);
        }
    });
    first
}

/// The ground distance (metres) between two features that don't intersect: between their nearest
/// points, edges and vertices alike. `None` for a geometry without a finite coordinate.
pub fn distance_m(a: &Geometry, b: &Geometry) -> Option<f64> {
    let plane = Aeqd::new(geodesic_interpolate(first_coord(a)?, first_coord(b)?, 0.5));
    let (sa, sb) = (segments(a, &plane), segments(b, &plane));
    // Two segments that don't cross are nearest at an end of one of them.
    let mut best: Option<(f64, Coord, Coord)> = None;
    let mut consider = |p: Coord, q: Coord| {
        let d = (p.x - q.x).hypot(p.y - q.y);
        if best.is_none_or(|(b, _, _)| d < b) {
            best = Some((d, p, q));
        }
    };
    for &(a0, a1) in &sa {
        for &(b0, b1) in &sb {
            consider(a0, closest_on(a0, b0, b1));
            consider(a1, closest_on(a1, b0, b1));
            consider(closest_on(b0, a0, a1), b0);
            consider(closest_on(b1, a0, a1), b1);
        }
    }
    let (_, p, q) = best?;
    let d = haversine_m(plane.inverse(p), plane.inverse(q));
    d.is_finite().then_some(d)
}

/// A geometry's edges in the plane; a point is an edge of no length.
fn segments(g: &Geometry, plane: &Aeqd) -> Vec<(Coord, Coord)> {
    let mut out = Vec::new();
    let mut path = |pts: &[Coord], closed: bool| {
        let v: Vec<Coord> = pts.iter().filter(|c| c.is_finite()).map(|c| plane.forward(*c)).collect();
        match v.as_slice() {
            [] => {}
            [only] => out.push((*only, *only)),
            _ => {
                out.extend(v.windows(2).filter_map(|w| Some((*w.first()?, *w.get(1)?))));
                if let (true, Some(first), Some(last)) = (closed, v.first(), v.last())
                    && first != last
                {
                    out.push((*last, *first));
                }
            }
        }
    };
    match g {
        Geometry::Point(c) => path(std::slice::from_ref(c), false),
        Geometry::MultiPoint(v) => v.iter().for_each(|c| path(std::slice::from_ref(c), false)),
        Geometry::Line(parts) => parts.iter().for_each(|p| path(p, false)),
        Geometry::Polygon(polys) => polys.iter().flatten().for_each(|r| path(r, true)),
    }
    out
}

/// The point of the segment `a`–`b` nearest `p`.
fn closest_on(p: Coord, a: Coord, b: Coord) -> Coord {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { ((p.x - a.x) * dx + (p.y - a.y) * dy) / len2 } else { 0.0 };
    let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
    Coord::new(a.x + t * dx, a.y + t * dy)
}

/// Whether a feature within the lon/lat extent `b` may lie within `metres` of one within `a`:
/// `false` only when every pair of points of the two extents is farther apart than that, at the
/// poles and across the antimeridian too.
pub fn may_be_within(a: &Extent, b: &Extent, metres: f64) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    // A little slack, so a feature exactly at the distance isn't ruled out by rounding.
    let r = (metres.max(0.0) * (1.0 + 1e-9) + 1e-3) / EARTH_RADIUS_M;
    let dlat = r.to_degrees();
    let lat_gap = (b.min_y - a.max_y).max(a.min_y - b.max_y).max(0.0);
    if lat_gap > dlat {
        return false;
    }
    // Two points whose longitudes differ by Δλ, both no further from the equator than φ, are at
    // least 2R·asin(cos φ·sin(Δλ/2)) apart.
    let phi = (a.min_y.abs().max(a.max_y.abs()) + dlat).to_radians();
    let reach = (r / 2.0).min(std::f64::consts::FRAC_PI_2).sin() / phi.cos();
    if phi >= std::f64::consts::FRAC_PI_2 || reach.is_nan() || reach >= 1.0 {
        return true;
    }
    let dlon = (2.0 * reach.asin()).to_degrees();
    lon_gap(a, b) <= dlon
}

/// The gap in longitude between two extents (degrees), the short way round.
fn lon_gap(a: &Extent, b: &Extent) -> f64 {
    if a.width() >= 360.0 || b.width() >= 360.0 {
        return 0.0;
    }
    [-360.0, 0.0, 360.0].iter().map(|k| (b.min_x + k - a.max_x).max(a.min_x - (b.max_x + k)).max(0.0)).fold(f64::INFINITY, f64::min)
}

/// The ground area (square metres) of a lon/lat polygon geometry, holes taken out.
pub fn area_m2(g: &Geometry) -> f64 {
    let Geometry::Polygon(polys) = g else { return 0.0 };
    polys
        .iter()
        .flat_map(|rings| rings.iter().enumerate())
        .map(|(i, r)| if i == 0 { geodesic_ring_area_m2(r) } else { -geodesic_ring_area_m2(r) })
        .sum()
}

/// The ground length (metres) of a lon/lat line geometry.
pub fn length_m(g: &Geometry) -> f64 {
    let Geometry::Line(parts) = g else { return 0.0 };
    parts.iter().map(|p| geodesic_length_m(p)).sum()
}

/// How the values of a join field's matches become one value in a one-to-one join.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    First,
    Last,
    Sum,
    Mean,
    Min,
    Max,
    Count,
    Join,
}

/// The rules' names, as the `merge` parameter spells them.
pub const RULES: &[(&str, Rule)] = &[
    ("first", Rule::First),
    ("last", Rule::Last),
    ("sum", Rule::Sum),
    ("mean", Rule::Mean),
    ("min", Rule::Min),
    ("max", Rule::Max),
    ("count", Rule::Count),
    ("join", Rule::Join),
];

/// A merge rule for one join field.
#[derive(Clone, Debug, PartialEq)]
pub struct Merge {
    /// The field's index in the join layer.
    pub field: usize,
    pub rule: Rule,
    /// What `join` puts between values.
    pub delimiter: String,
}

impl Merge {
    /// The type of the merged field, given the join field's.
    pub fn field_type(&self, ty: FieldType) -> FieldType {
        match self.rule {
            Rule::Mean => FieldType::Double,
            Rule::Count => FieldType::Integer,
            Rule::Join => FieldType::Text,
            _ => ty,
        }
    }

    /// The matches' values (in match order) merged into one. Every rule but `first` and `last`
    /// skips nulls; with nothing left a numeric rule gives null, `count` zero and `join` "".
    /// An integer sum too large for 64 bits is null.
    pub fn apply(&self, ty: FieldType, values: &[&Value]) -> Value {
        let present = || values.iter().copied().filter(|v| !matches!(v, Value::Null));
        let numbers: Vec<f64> = present().filter_map(Value::as_f64).collect();
        match self.rule {
            Rule::First => values.first().map(|v| (*v).clone()).unwrap_or(Value::Null),
            Rule::Last => values.last().map(|v| (*v).clone()).unwrap_or(Value::Null),
            Rule::Count => Value::Int(i64::try_from(present().count()).unwrap_or(i64::MAX)),
            Rule::Join => Value::Text(present().map(ToString::to_string).collect::<Vec<_>>().join(&self.delimiter)),
            Rule::Sum if ty == FieldType::Integer => {
                let ints: Vec<i64> = present().filter_map(|v| if let Value::Int(i) = v { Some(*i) } else { None }).collect();
                if ints.is_empty() {
                    return Value::Null;
                }
                ints.iter().try_fold(0i64, |s, i| s.checked_add(*i)).map(Value::Int).unwrap_or(Value::Null)
            }
            Rule::Sum if numbers.is_empty() => Value::Null,
            Rule::Sum => Value::Float(numbers.iter().sum()),
            Rule::Mean if numbers.is_empty() => Value::Null,
            Rule::Mean => Value::Float(numbers.iter().sum::<f64>() / numbers.len() as f64),
            Rule::Min | Rule::Max => {
                // Numbers by value, text by its characters.
                let cmp = |a: &Value, b: &Value| match (a.as_f64(), b.as_f64()) {
                    (Some(x), Some(y)) if ty.is_numeric() => x.total_cmp(&y),
                    _ => a.to_string().cmp(&b.to_string()),
                };
                let candidates = present().filter(|v| !ty.is_numeric() || v.as_f64().is_some());
                let best = if self.rule == Rule::Max {
                    // The first of equal values, as `min_by` keeps.
                    candidates.fold(None, |best: Option<&Value>, v| match best {
                        Some(b) if cmp(v, b).is_le() => Some(b),
                        _ => Some(v),
                    })
                } else {
                    candidates.min_by(|a, b| cmp(a, b))
                };
                best.cloned().unwrap_or(Value::Null)
            }
        }
    }
}

/// The `merge` parameter: a list (or the text of one) such as
/// `[{"field": "units", "rule": "sum"}, {"field": "name", "rule": "join", "delimiter": "; "}]`,
/// naming fields of the join layer. Empty or absent is no rules.
pub fn parse_merge(join: &Dataset, v: Option<&Json>) -> Result<Vec<Merge>, String> {
    let parsed;
    let list = match v {
        None | Some(Json::Null) => return Ok(Vec::new()),
        Some(Json::String(t)) if t.trim().is_empty() => return Ok(Vec::new()),
        Some(Json::String(t)) => {
            parsed = serde_json::from_str::<Json>(t).map_err(|e| format!("Merge rules: {e}"))?;
            &parsed
        }
        Some(other) => other,
    };
    let items = list.as_array().ok_or("Merge rules must be a list such as [{\"field\": \"units\", \"rule\": \"sum\"}]")?;
    let mut out: Vec<Merge> = Vec::with_capacity(items.len());
    for item in items {
        let text = |k: &str| item.get(k).and_then(Json::as_str).map(str::trim);
        let name = text("field").filter(|n| !n.is_empty()).ok_or("each merge rule needs a \"field\"")?;
        let field = crate::find_field_case_insensitive(join, name)?;
        let rule_name = text("rule").unwrap_or("");
        let rule = RULES.iter().find(|(n, _)| n.eq_ignore_ascii_case(rule_name)).map(|(_, r)| *r).ok_or_else(|| {
            format!(
                "merge rule for '{name}' must be one of {} (not '{rule_name}')",
                RULES.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
            )
        })?;
        let ty = join.fields.get(field).map(|f| f.ty).unwrap_or(FieldType::Text);
        let fits = match rule {
            Rule::Sum | Rule::Mean => ty.is_numeric(),
            Rule::Min | Rule::Max => ty.is_numeric() || ty == FieldType::Text,
            _ => true,
        };
        if !fits {
            return Err(format!("merge rule '{rule_name}' needs a numeric field ('{name}' is not)"));
        }
        if out.iter().any(|m| m.field == field) {
            return Err(format!("'{name}' has more than one merge rule"));
        }
        let delimiter = match item.get("delimiter") {
            None | Some(Json::Null) => ", ".to_string(),
            Some(Json::String(d)) => d.clone(),
            Some(_) => return Err(format!("the delimiter for '{name}' must be text")),
        };
        out.push(Merge { field, rule, delimiter });
    }
    Ok(out)
}
