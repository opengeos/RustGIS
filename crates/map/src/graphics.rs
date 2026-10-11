//! Graphics layers: shapes, text and pictures drawn on the map, each carrying its own style.
//!
//! A graphics layer is a layer of mixed geometry whose features keep their look in attributes
//! ([`FIELDS`]), so every graphic can differ. Text graphics are drawn by the label engine (centred
//! on their point), and dynamic text expands tokens such as `{scale}` when drawn.

use rustgis_data::{Dataset, Field, FieldType, GeometryType, Value};

use crate::{Marker, Rgba, Symbol};

/// The graphic kinds.
pub const SHAPES: &[&str] =
    &["point", "text", "line", "curve", "freehand", "polygon", "lasso", "rectangle", "circle", "ellipse", "picture"];

/// The attribute fields of a graphics layer: (name, type).
pub const FIELDS: &[(&str, FieldType)] = &[
    ("kind", FieldType::Text),
    ("text", FieldType::Text),
    ("fill", FieldType::Text),
    ("outline", FieldType::Text),
    ("width", FieldType::Double),
    ("size", FieldType::Double),
    ("font_size", FieldType::Double),
    ("image", FieldType::Text),
];

/// Tokens dynamic text expands: (token, description).
pub const TOKENS: &[(&str, &str)] = &[
    ("{map}", "Map name"),
    ("{scale}", "Map scale"),
    ("{date}", "Current date"),
    ("{time}", "Current time"),
    ("{crs}", "Coordinate system"),
    ("{layers}", "Number of layers"),
    ("{center}", "Map centre"),
];

/// An empty graphics dataset.
pub fn dataset(name: &str) -> Dataset {
    Dataset::new(name, GeometryType::None, FIELDS.iter().map(|(n, t)| Field::new(*n, *t)).collect())
}

fn text<'a>(ds: &'a Dataset, row: usize, field: &str) -> Option<&'a str> {
    match ds.field_index(field).map(|fi| ds.value(row, fi)) {
        Some(Value::Text(t)) => Some(t.as_str()),
        _ => None,
    }
}

fn num(ds: &Dataset, row: usize, field: &str) -> Option<f64> {
    ds.field_index(field).and_then(|fi| ds.value(row, fi).as_f64()).filter(|v| v.is_finite())
}

/// The graphic kind of a row.
pub fn kind(ds: &Dataset, row: usize) -> &str {
    text(ds, row, "kind").unwrap_or("")
}

/// The text a row shows (text graphics only).
pub fn label(ds: &Dataset, row: usize) -> Option<&str> {
    if kind(ds, row) == "text" { text(ds, row, "text").filter(|t| !t.is_empty()) } else { None }
}

/// The font size of a text graphic, in points.
pub fn font_size(ds: &Dataset, row: usize) -> f32 {
    num(ds, row, "font_size").unwrap_or(14.0).clamp(4.0, 200.0) as f32
}

/// The text colour of a text graphic.
pub fn text_color(ds: &Dataset, row: usize) -> Rgba {
    text(ds, row, "fill").and_then(Rgba::from_hex).unwrap_or(Rgba::rgb(0x20, 0x20, 0x20))
}

/// The symbol a graphic draws with.
pub fn symbol(ds: &Dataset, row: usize) -> Symbol {
    let fill = text(ds, row, "fill").and_then(Rgba::from_hex).unwrap_or(Rgba::new(0x4A, 0x90, 0xD9, 90));
    let stroke = text(ds, row, "outline").and_then(Rgba::from_hex).unwrap_or(Rgba::rgb(0x1F, 0x4E, 0x8C));
    let width = num(ds, row, "width").unwrap_or(1.5).clamp(0.0, 50.0) as f32;
    let size = num(ds, row, "size").unwrap_or(10.0).clamp(0.0, 200.0) as f32;
    match kind(ds, row) {
        // Text and pictures draw themselves; their geometry stays invisible.
        "text" | "picture" => {
            Symbol { fill: Rgba::new(0, 0, 0, 0), stroke: Rgba::new(0, 0, 0, 0), stroke_width: 0.0, size: 0.0, marker: Marker::Circle }
        }
        // A line's colour is its outline.
        "line" | "curve" | "freehand" => Symbol { fill: stroke, stroke, stroke_width: width, size, marker: Marker::Circle },
        _ => Symbol { fill, stroke, stroke_width: width, size, marker: Marker::Circle },
    }
}

/// What dynamic text tokens stand for when a map is drawn.
pub struct TokenValues {
    pub map: String,
    pub scale: f64,
    pub crs: String,
    pub layers: usize,
    pub center: rustgis_geom::Coord,
    /// Seconds since the Unix epoch.
    pub now: u64,
}

/// Expands dynamic text tokens.
pub fn expand(text: &str, v: &TokenValues) -> String {
    if !text.contains('{') {
        return text.to_string();
    }
    let (date, time) = civil(v.now);
    text.replace("{map}", &v.map)
        .replace("{scale}", &format!("1:{}", thousands(v.scale)))
        .replace("{date}", &date)
        .replace("{time}", &time)
        .replace("{crs}", &v.crs)
        .replace("{layers}", &v.layers.to_string())
        .replace("{center}", &rustgis_geom::format_lonlat(v.center))
}

fn thousands(v: f64) -> String {
    let s = format!("{:.0}", v.clamp(0.0, 1e15));
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// (YYYY-MM-DD, HH:MM) in UTC.
fn civil(secs: u64) -> (String, String) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Days to civil date (proleptic Gregorian), after Howard Hinnant's algorithm.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (format!("{y:04}-{m:02}-{d:02}"), format!("{:02}:{:02}", rem / 3600, rem % 3600 / 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_expand() {
        let v = TokenValues {
            map: "World".into(),
            scale: 25_000_000.0,
            crs: "Web Mercator".into(),
            layers: 4,
            center: rustgis_geom::Coord::new(0.0, 0.0),
            now: 1_760_140_800, // 2025-10-11 00:00 UTC
        };
        assert_eq!(expand("{map} at {scale}", &v), "World at 1:25,000,000");
        assert_eq!(expand("{date} {time} · {layers}", &v), "2025-10-11 00:00 · 4");
        assert_eq!(expand("plain", &v), "plain");
    }
}
