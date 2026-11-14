//! An ArcGIS layer's own symbology (its description's `drawingInfo`, as ArcGIS REST's JSON has
//! it) translated onto a feature layer just added from its service (#2080).
use serde_json::Value;

use crate::symbology::{Dash, LineStroke, MAX_STROKE_WIDTH, Marker};
use crate::{Layer, Renderer, Rgba, Symbol};
use rustgis_data::GeometryType;

/// The largest marker, in points, as Style › Marker sizes one.
const MAX_MARKER_SIZE: f32 = 80.0;

/// The colour a picture marker is drawn in, which RustGIS can't draw yet.
const PICTURE_FILL: Rgba = Rgba::rgb(0x80, 0x80, 0x80);

/// What of a service's symbology was drawn differently or left out, each with how many symbols it
/// came up for, in the order first met.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Notes(Vec<(&'static str, usize)>);

impl Notes {
    fn add(&mut self, text: &'static str) {
        match self.0.iter_mut().find(|(t, _)| *t == text) {
            Some((_, n)) => *n = n.saturating_add(1),
            None => self.0.push((text, 1)),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Each note with how many times it came up.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, usize)> + '_ {
        self.0.iter().copied()
    }

    /// The notes as one sentence's list: `"hatched fills, drawn solid (3 symbols); …"`.
    pub fn text(&self) -> String {
        self.0.iter().map(|(t, n)| if *n > 1 { format!("{t} ({n} symbols)") } else { (*t).to_string() }).collect::<Vec<_>>().join("; ")
    }
}

/// Gives `layer`, a feature layer just added from an ArcGIS service, the symbology its
/// description's `drawingInfo` sets: its renderer, when RustGIS draws its kind, and its
/// transparency. What can't be drawn as the service draws it is noted; a renderer of a kind
/// RustGIS doesn't read leaves the layer's own. Anything but an object changes nothing.
pub fn apply(layer: &mut Layer, drawing_info: &Value, notes: &mut Notes) {
    if !drawing_info.is_object() || layer.is_raster() {
        return;
    }
    if let Some(t) = drawing_info.get("transparency").and_then(Value::as_f64).filter(|t| t.is_finite()) {
        layer.transparency = t.clamp(0.0, 100.0) as f32;
    }
    let Some(renderer) = drawing_info.get("renderer").filter(|r| r.is_object()) else { return };
    let g = layer.geometry_type();
    if g == GeometryType::None {
        return;
    }
    match renderer.get("type").and_then(Value::as_str) {
        Some("simple") => {
            if let Some((symbol, dash)) = symbol(renderer.get("symbol").unwrap_or(&Value::Null), g, notes) {
                layer.renderer = Renderer::Single(symbol);
                layer.line_strokes = dash.map(|d| vec![LineStroke { dash: Some(d), ..LineStroke::default() }]).unwrap_or_default();
            }
        }
        Some("uniqueValue") => notes.add("its renderer by unique values, drawn with RustGIS's single symbol"),
        Some("classBreaks") => notes.add("its renderer by class breaks, drawn with RustGIS's single symbol"),
        _ => notes.add("its renderer, of a kind RustGIS doesn't read, drawn with RustGIS's single symbol"),
    }
    if renderer.get("visualVariables").and_then(Value::as_array).is_some_and(|v| !v.is_empty()) {
        notes.add("sizes, colours or rotations by value (visual variables), left out");
    }
    if renderer.get("rotationExpression").and_then(Value::as_str).is_some_and(|e| !e.trim().is_empty()) {
        notes.add("markers rotated by a field, drawn upright");
    }
}

/// An ArcGIS symbol as a RustGIS symbol for a `g` layer, with a line's dash pattern; `None` (noted)
/// for a symbol that isn't for that geometry or that RustGIS can't draw.
fn symbol(s: &Value, g: GeometryType, notes: &mut Notes) -> Option<(Symbol, Option<Dash>)> {
    let kind = s.get("type").and_then(Value::as_str).unwrap_or_default();
    let base = Symbol::default_for(g, 0);
    match (kind, g) {
        ("esriSFS", GeometryType::Polygon) => {
            let style = s.get("style").and_then(Value::as_str).unwrap_or("esriSFSSolid");
            let fill = match style {
                "esriSFSNull" | "esriSFSHollow" => Rgba::TRANSPARENT,
                "esriSFSSolid" => color(s.get("color")),
                _ => {
                    notes.add("hatched fills, drawn solid");
                    color(s.get("color"))
                }
            };
            let (stroke, stroke_width, dash) = line(s.get("outline"), 0.0);
            if dash.is_some() {
                notes.add("dashed polygon outlines, drawn solid");
            }
            Some((Symbol { fill, stroke, stroke_width, ..base }, None))
        }
        ("esriSLS", GeometryType::Polyline) => {
            let (c, width, dash) = line(Some(s), base.stroke_width);
            Some((Symbol { fill: c, stroke: c, stroke_width: width, ..base }, dash))
        }
        ("esriSMS", GeometryType::Point) => {
            let marker = match s.get("style").and_then(Value::as_str) {
                Some("esriSMSSquare") => Marker::Square,
                Some("esriSMSDiamond") => Marker::Diamond,
                Some("esriSMSTriangle") => Marker::Triangle,
                Some("esriSMSCross") => Marker::Char(crate::symbology::Glyph::new('+', "")),
                Some("esriSMSX") => Marker::Char(crate::symbology::Glyph::new('×', "")),
                _ => Marker::Circle,
            };
            let (stroke, stroke_width, _) = line(s.get("outline"), 0.0);
            if offset_or_angle(s) {
                notes.add("markers' angles and offsets, left out");
            }
            Some((Symbol { fill: color(s.get("color")), stroke, stroke_width, size: size(s.get("size"), base.size), marker }, None))
        }
        ("esriPMS", GeometryType::Point) => {
            notes.add("picture markers, drawn as grey circles");
            let size = size(s.get("width"), base.size).max(size(s.get("height"), 0.0));
            Some((Symbol { fill: PICTURE_FILL, size, ..base }, None))
        }
        ("esriPFS", GeometryType::Polygon) => {
            notes.add("picture fills, drawn with their outline only");
            let (stroke, stroke_width, _) = line(s.get("outline"), 0.0);
            Some((Symbol { fill: Rgba::TRANSPARENT, stroke, stroke_width, ..base }, None))
        }
        ("esriSFS" | "esriSLS" | "esriSMS" | "esriPMS" | "esriPFS", _) => {
            notes.add("a symbol for another kind of geometry, drawn with RustGIS's own");
            None
        }
        _ => {
            notes.add("a symbol of a kind RustGIS doesn't read, drawn with RustGIS's own");
            None
        }
    }
}

/// A simple line symbol's (`esriSLS`) colour, width in points and dash pattern: none at all
/// (transparent, `0`) when absent or of style `esriSLSNull`, `width` when it names none.
fn line(s: Option<&Value>, width: f32) -> (Rgba, f32, Option<Dash>) {
    let Some(s) = s.filter(|s| s.is_object()) else { return (Rgba::TRANSPARENT, 0.0, None) };
    let style = s.get("style").and_then(Value::as_str).unwrap_or("esriSLSSolid");
    if style == "esriSLSNull" {
        return (Rgba::TRANSPARENT, 0.0, None);
    }
    let w = s
        .get("width")
        .and_then(Value::as_f64)
        .filter(|w| w.is_finite())
        .map_or(width, |w| w.clamp(0.0, f64::from(MAX_STROKE_WIDTH)) as f32);
    (color(s.get("color")), w, dash(style, w))
}

/// The dash pattern of a simple line style, in points: ArcGIS's, in line widths (as the ArcGIS
/// Maps SDK for JavaScript draws them), at least a point wide. `None` for a solid line.
fn dash(style: &str, width: f32) -> Option<Dash> {
    let steps: &[f32] = match style {
        "esriSLSDash" => &[4.0, 3.0],
        "esriSLSDot" => &[1.0, 3.0],
        "esriSLSDashDot" => &[4.0, 3.0, 1.0, 3.0],
        "esriSLSDashDotDot" | "esriSLSLongDashDotDot" => &[8.0, 3.0, 1.0, 3.0, 1.0, 3.0],
        "esriSLSLongDash" => &[8.0, 3.0],
        "esriSLSLongDashDot" => &[8.0, 3.0, 1.0, 3.0],
        "esriSLSShortDash" => &[4.0, 1.0],
        "esriSLSShortDot" => &[1.0, 1.0],
        "esriSLSShortDashDot" => &[4.0, 1.0, 1.0, 1.0],
        "esriSLSShortDashDotDot" => &[4.0, 1.0, 1.0, 1.0, 1.0, 1.0],
        _ => return None,
    };
    let unit = width.max(1.0);
    Dash::new(&steps.iter().map(|s| s * unit).collect::<Vec<_>>())
}

/// An ArcGIS colour, `[r, g, b, a]` from 0 to 255 (`[r, g, b]` opaque); `null`, or anything else,
/// is no colour (transparent), as ArcGIS draws it.
fn color(v: Option<&Value>) -> Rgba {
    let Some(parts) = v.and_then(Value::as_array).filter(|a| a.len() == 3 || a.len() == 4) else { return Rgba::TRANSPARENT };
    let mut c = [0u8, 0, 0, 255];
    for (slot, part) in c.iter_mut().zip(parts) {
        let Some(n) = part.as_f64().filter(|n| n.is_finite()) else { return Rgba::TRANSPARENT };
        *slot = n.round().clamp(0.0, 255.0) as u8;
    }
    Rgba(c)
}

/// A marker's size in points, `default` when it names none, within Style › Marker's range.
fn size(v: Option<&Value>, default: f32) -> f32 {
    v.and_then(Value::as_f64).filter(|s| s.is_finite()).map_or(default, |s| (s as f32).clamp(0.0, MAX_MARKER_SIZE))
}

/// Whether a marker is turned or shifted from its point.
fn offset_or_angle(s: &Value) -> bool {
    ["angle", "xoffset", "yoffset"].iter().any(|k| s.get(*k).and_then(Value::as_f64).is_some_and(|v| v.is_finite() && v != 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_data::{Dataset, Field, FieldType};
    use serde_json::json;

    fn layer(g: GeometryType) -> Layer {
        Layer::new(1, Dataset::new("l", g, vec![Field::new("NAME", FieldType::Text)]))
    }

    fn styled(g: GeometryType, info: Value) -> (Layer, Notes) {
        let mut l = layer(g);
        let mut notes = Notes::default();
        apply(&mut l, &info, &mut notes);
        (l, notes)
    }

    #[test]
    fn a_simple_fill_takes_the_services_colours_outline_and_transparency() {
        let info = json!({"renderer": {"type": "simple", "symbol": {"type": "esriSFS", "style": "esriSFSSolid", "color": [255, 211, 127, 128],
            "outline": {"type": "esriSLS", "style": "esriSLSSolid", "color": [110, 110, 110, 255], "width": 0.4}}}, "transparency": 30});
        let (l, notes) = styled(GeometryType::Polygon, info);
        let Renderer::Single(s) = l.renderer else { panic!("a single symbol") };
        assert_eq!((s.fill, s.stroke, s.stroke_width), (Rgba::new(255, 211, 127, 128), Rgba::rgb(110, 110, 110), 0.4));
        assert_eq!(l.transparency, 30.0);
        assert!(notes.is_empty());
    }

    #[test]
    fn a_hollow_fill_or_a_null_outline_draws_nothing_there() {
        let info = json!({"renderer": {"type": "simple", "symbol": {"type": "esriSFS", "style": "esriSFSNull", "color": [1, 2, 3, 255],
            "outline": {"type": "esriSLS", "style": "esriSLSNull", "color": [0, 0, 0, 255], "width": 1}}}});
        let (l, _) = styled(GeometryType::Polygon, info);
        let Renderer::Single(s) = l.renderer else { panic!("a single symbol") };
        assert_eq!((s.fill.a(), s.stroke.a(), s.stroke_width), (0, 0, 0.0));
        // A null colour is no colour, as ArcGIS draws it.
        let info = json!({"renderer": {"type": "simple", "symbol": {"type": "esriSFS", "color": null}}});
        let (l, _) = styled(GeometryType::Polygon, info);
        let Renderer::Single(s) = l.renderer else { panic!("a single symbol") };
        assert_eq!((s.fill.a(), s.stroke.a()), (0, 0));
    }

    #[test]
    fn a_hatched_fill_is_drawn_solid_and_said() {
        let info = json!({"renderer": {"type": "simple", "symbol": {"type": "esriSFS", "style": "esriSFSForwardDiagonal", "color": [0, 0, 255, 255],
            "outline": {"type": "esriSLS", "style": "esriSLSDash", "color": [0, 0, 0, 255], "width": 1}}}});
        let (l, notes) = styled(GeometryType::Polygon, info);
        let Renderer::Single(s) = l.renderer else { panic!("a single symbol") };
        assert_eq!(s.fill, Rgba::rgb(0, 0, 255));
        assert_eq!(notes.text(), "hatched fills, drawn solid; dashed polygon outlines, drawn solid");
    }

    #[test]
    fn a_dashed_line_keeps_its_width_and_pattern_in_line_widths() {
        let info = json!({"renderer": {"type": "simple", "symbol": {"type": "esriSLS", "style": "esriSLSDashDot", "color": [230, 0, 0, 255], "width": 2}}});
        let (l, notes) = styled(GeometryType::Polyline, info);
        let Renderer::Single(s) = l.renderer else { panic!("a single symbol") };
        assert_eq!((s.stroke, s.fill, s.stroke_width), (Rgba::rgb(230, 0, 0), Rgba::rgb(230, 0, 0), 2.0));
        assert_eq!(l.line_strokes.len(), 1);
        assert_eq!(l.line_strokes[0].dash.map(|d| d.steps().to_vec()), Some(vec![8.0, 6.0, 2.0, 6.0]));
        assert_eq!((l.line_strokes[0].color, l.line_strokes[0].width), (None, None), "the stroke follows the symbol");
        assert!(notes.is_empty());
        // A solid line has no stacked strokes.
        let info = json!({"renderer": {"type": "simple", "symbol": {"type": "esriSLS", "style": "esriSLSSolid", "color": [0, 0, 0, 255], "width": 1}}});
        assert!(styled(GeometryType::Polyline, info).0.line_strokes.is_empty());
    }

    #[test]
    fn a_simple_marker_takes_its_shape_size_and_outline() {
        let info = json!({"renderer": {"type": "simple", "symbol": {"type": "esriSMS", "style": "esriSMSDiamond", "color": [56, 168, 0, 255], "size": 9,
            "outline": {"color": [255, 255, 255, 255], "width": 1.5}}}});
        let (l, notes) = styled(GeometryType::Point, info);
        let Renderer::Single(s) = l.renderer else { panic!("a single symbol") };
        assert_eq!(
            (s.marker, s.fill, s.stroke, s.stroke_width, s.size),
            (Marker::Diamond, Rgba::rgb(56, 168, 0), Rgba::rgb(255, 255, 255), 1.5, 9.0)
        );
        assert!(notes.is_empty());
        for (style, ch) in [("esriSMSCross", '+'), ("esriSMSX", '×')] {
            let info = json!({"renderer": {"type": "simple", "symbol": {"type": "esriSMS", "style": style, "color": [0, 0, 0, 255], "size": 8, "angle": 45}}});
            let (l, notes) = styled(GeometryType::Point, info);
            let Renderer::Single(s) = l.renderer else { panic!("a single symbol") };
            assert_eq!(s.marker, Marker::Char(crate::symbology::Glyph::new(ch, "")));
            assert_eq!(notes.text(), "markers' angles and offsets, left out");
        }
    }

    #[test]
    fn a_picture_marker_is_a_grey_circle_of_its_size_and_said() {
        let info =
            json!({"renderer": {"type": "simple", "symbol": {"type": "esriPMS", "imageData": "iVBORw0KGgo=", "width": 15, "height": 18}}});
        let (l, notes) = styled(GeometryType::Point, info);
        let Renderer::Single(s) = l.renderer else { panic!("a single symbol") };
        assert_eq!((s.marker, s.fill, s.size), (Marker::Circle, PICTURE_FILL, 18.0));
        assert_eq!(notes.text(), "picture markers, drawn as grey circles");
    }

    #[test]
    fn what_rustgis_doesnt_draw_leaves_the_layers_own_symbol_and_is_said() {
        let own = layer(GeometryType::Polygon).renderer;
        for (info, said) in [
            (
                json!({"renderer": {"type": "uniqueValue", "field1": "NAME", "uniqueValueInfos": []}}),
                "its renderer by unique values, drawn with RustGIS's single symbol",
            ),
            (
                json!({"renderer": {"type": "classBreaks", "field": "NAME", "classBreakInfos": []}}),
                "its renderer by class breaks, drawn with RustGIS's single symbol",
            ),
            (json!({"renderer": {"type": "heatmap"}}), "its renderer, of a kind RustGIS doesn't read, drawn with RustGIS's single symbol"),
            (
                json!({"renderer": {"type": "simple", "symbol": {"type": "esriSMS"}}}),
                "a symbol for another kind of geometry, drawn with RustGIS's own",
            ),
            (
                json!({"renderer": {"type": "simple", "symbol": {"type": "CIMSymbolReference"}}}),
                "a symbol of a kind RustGIS doesn't read, drawn with RustGIS's own",
            ),
        ] {
            let (l, notes) = styled(GeometryType::Polygon, info);
            assert_eq!(l.renderer, own);
            assert_eq!(notes.text(), said);
        }
    }

    #[test]
    fn visual_variables_and_rotation_are_said_left_out() {
        let info = json!({"renderer": {"type": "simple", "symbol": {"type": "esriSMS", "color": [0, 0, 0, 255], "size": 6},
            "visualVariables": [{"type": "sizeInfo", "field": "POP"}], "rotationExpression": "[ANGLE]"}});
        let (_, notes) = styled(GeometryType::Point, info);
        assert_eq!(
            notes.text(),
            "sizes, colours or rotations by value (visual variables), left out; markers rotated by a field, drawn upright"
        );
    }

    #[test]
    fn notes_count_what_comes_up_again() {
        let mut notes = Notes::default();
        notes.add("hatched fills, drawn solid");
        notes.add("hatched fills, drawn solid");
        assert_eq!(notes.text(), "hatched fills, drawn solid (2 symbols)");
    }

    #[test]
    fn hostile_values_are_brought_into_range_or_ignored() {
        let info = json!({"renderer": {"type": "simple", "symbol": {"type": "esriSMS", "color": [999, -5, "x", 1], "size": 1e9,
            "outline": {"color": [0, 0, 0], "width": -3}}}, "transparency": 1e9});
        let (l, _) = styled(GeometryType::Point, info);
        let Renderer::Single(s) = l.renderer else { panic!("a single symbol") };
        assert_eq!((s.fill, s.stroke, s.stroke_width, s.size), (Rgba::TRANSPARENT, Rgba::rgb(0, 0, 0), 0.0, MAX_MARKER_SIZE));
        assert_eq!(l.transparency, 100.0);
        // Not an object: nothing changes.
        for info in [json!(null), json!([]), json!("simple"), json!({"renderer": "simple"})] {
            let (l, notes) = styled(GeometryType::Point, info);
            assert_eq!(l.renderer, layer(GeometryType::Point).renderer);
            assert!(notes.is_empty());
        }
    }
}
