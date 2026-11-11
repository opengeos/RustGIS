//! QGIS annotation layers (`<maplayer type="annotation">` and the project's main annotation
//! layer) as RustGIS graphics ([`crate::graphics::Graphic`]): marker, line and polygon items as
//! points, lines and polygons in their symbol's colours, sizes and widths; point text, rectangle
//! text and text along a line as text graphics; pictures as pictures. A rectangle text's or a
//! picture's background and frame become a rectangle under it (a picture's frame one over it).
//! Items are drawn in their z order, and those QGIS keeps switched off are left out, as QGIS
//! leaves them out.
//!
//! What QGIS draws, as measured on its renders: a point text's baseline sits on its point, the
//! text starting there (left aligned), centred on it or ending there (right aligned), further
//! lines above; a rectangle text sits in its box by its alignment; a picture whose aspect is
//! locked is fitted in its box and centred. RustGIS centres a text graphic on its point, so text
//! is placed near QGIS's, not on it (noted).

use roxmltree::Node;

use super::{Report, child, color, labels, symbol};
use crate::Rgba;
use crate::graphics::{self, Graphic};
use crate::layout::TextAlign;
use rustgis_data::Geometry;
use rustgis_geom::Coord;

/// The most items read from one annotation layer.
pub const MAX_ITEMS: usize = 100_000;

/// One QGIS annotation item as a RustGIS graphic, its geometry in the annotation layer's
/// coordinates, still to be taken to longitude/latitude where the layer is added.
#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub graphic: Graphic,
    /// A picture's file as QGIS names it (from the project's folder where relative), or its
    /// bytes after `base64:`: read where the layer is added.
    pub picture: Option<String>,
    /// A picture fitted in its box, keeping its aspect (QGIS's lock aspect ratio).
    pub keep_aspect: bool,
}

/// The items of an annotation layer's `<items>`, bottom first. Notes go to `report` under
/// `layer`.
pub(super) fn read(node: Node, dir: &str, layer: &str, report: &mut Report) -> Vec<Annotation> {
    let Some(items) = child(node, "items") else { return Vec::new() };
    let all: Vec<Node> = items.children().filter(|c| c.has_tag_name("item")).collect();
    if all.len() > MAX_ITEMS {
        report.skipped(layer, format!("{} annotations past the first {MAX_ITEMS}", all.len() - MAX_ITEMS));
    }
    let mut read = Reading::default();
    let mut drawn: Vec<(f64, Vec<Annotation>)> = Vec::new();
    for item in all.into_iter().take(MAX_ITEMS) {
        if item.attribute("enabled") == Some("0") {
            read.off += 1;
            continue;
        }
        let z = num(item, "zIndex").unwrap_or(0.0);
        let out = item_graphics(item, dir, layer, &mut read, report);
        read.clamped += usize::from(out.iter().any(|a| graphics::cut(&a.graphic)));
        if !out.is_empty() {
            drawn.push((z, out));
        }
    }
    read.notes(layer, report);
    // Bottom first: by z, then as QGIS lists them (a stable sort).
    drawn.sort_by(|a, b| a.0.total_cmp(&b.0));
    let out: Vec<Annotation> = drawn.into_iter().flat_map(|(_, a)| a).collect();
    let under = texts_under_others(&out);
    if under > 0 {
        let what = if under == 1 {
            "an annotation text QGIS draws under other annotations is drawn over them".to_string()
        } else {
            format!("{under} annotation texts QGIS draws under other annotations are drawn over them")
        };
        report.approximated(layer, format!("{what}: RustGIS draws a layer's texts after its shapes and pictures"));
    }
    out
}

/// Text graphics, of `drawn` (bottom first), with a shape or picture above them: RustGIS draws a
/// graphics layer's texts after its other graphics, so these come out on top.
fn texts_under_others(drawn: &[Annotation]) -> usize {
    let Some(last_other) = drawn.iter().rposition(|a| a.graphic.kind != "text") else { return 0 };
    drawn[..last_other].iter().filter(|a| a.graphic.kind == "text").count()
}

/// What was met reading a layer's items, noted once each with a count.
#[derive(Default)]
struct Reading {
    /// Switched off.
    off: usize,
    /// By kind: items of a kind RustGIS has no graphic for.
    unknown: Vec<String>,
    unknown_count: usize,
    /// Geometry that didn't read (curves, an empty one).
    bad_geometry: usize,
    point_texts: usize,
    /// Rectangle texts QGIS may wrap to their box.
    unwrapped: usize,
    /// Rectangle texts with margins in their box.
    margins: usize,
    /// Pictures that name no file.
    no_picture: usize,
    /// Items with a text, font size, width or size past what a graphic keeps.
    clamped: usize,
    turned: usize,
    html: usize,
    expressions: usize,
    reference_scale: usize,
    along_lines: usize,
    rect_aligned: usize,
    fixed_size: usize,
    relative: usize,
    svg: usize,
    pictures_fixed: usize,
    callouts: usize,
    fonts: Vec<String>,
    /// Symbols drawn plainer: dashed, offset, patterned or of several layers.
    plain_symbols: usize,
    /// Markers of other shapes than a circle.
    round_markers: usize,
    /// Text settings RustGIS's text graphics have no place for, by name.
    unread_text: Vec<&'static str>,
}

impl Reading {
    fn notes(&self, layer: &str, report: &mut Report) {
        // One or `count` of them, each in its own words.
        let n = |count: usize, one: &str, many: &str| if count == 1 { one.to_string() } else { format!("{count} {many}") };
        let mut skipped = |text: String| report.skipped(layer, text);
        if self.off > 0 {
            skipped(n(
                self.off,
                "a switched-off annotation, which QGIS doesn't draw either",
                "switched-off annotations, which QGIS doesn't draw either",
            ));
        }
        if self.unknown_count > 0 {
            let kinds = self.unknown.join(", ");
            skipped(n(
                self.unknown_count,
                &format!("an annotation of a kind RustGIS has no graphic for ({kinds})"),
                &format!("annotations of kinds RustGIS has no graphic for ({kinds})"),
            ));
        }
        if self.bad_geometry > 0 {
            skipped(n(
                self.bad_geometry,
                "an annotation whose geometry doesn't read (a curve?)",
                "annotations whose geometry doesn't read (curves?)",
            ));
        }
        if self.relative > 0 {
            skipped(n(
                self.relative,
                "an annotation placed on the map's frame rather than on the map",
                "annotations placed on the map's frame rather than on the map",
            ));
        }
        if self.svg > 0 {
            skipped(n(
                self.svg,
                "an SVG picture annotation (RustGIS's pictures are PNG or JPEG)",
                "SVG picture annotations (RustGIS's pictures are PNG or JPEG)",
            ));
        }
        if self.no_picture > 0 {
            skipped(n(self.no_picture, "a picture annotation that names no picture", "picture annotations that name no picture"));
        }
        if self.callouts > 0 {
            skipped(n(self.callouts, "an annotation's callout", "annotations' callouts"));
        }
        if !self.unread_text.is_empty() {
            skipped(format!("text annotations' {}", self.unread_text.join(", ")));
        }
        let mut approximated = |text: String| report.approximated(layer, text);
        if self.point_texts > 0 {
            approximated(n(
                self.point_texts,
                "a text annotation, centred on its point rather than set on it by its baseline, and drawn with RustGIS's halo",
                "text annotations, centred on their points rather than set on them by their baseline, and drawn with RustGIS's halo",
            ));
        }
        if self.turned > 0 {
            approximated(n(self.turned, "a turned text annotation, drawn level", "turned text annotations, drawn level"));
        }
        if self.html > 0 {
            approximated(n(self.html, "a text annotation in HTML, drawn as plain text", "text annotations in HTML, drawn as plain text"));
        }
        if self.expressions > 0 {
            approximated(n(
                self.expressions,
                "a text annotation with expressions in `[% … %]`, drawn as written",
                "text annotations with expressions in `[% … %]`, drawn as written",
            ));
        }
        if self.reference_scale > 0 {
            approximated(n(
                self.reference_scale,
                "an annotation sized at a reference scale, drawn the same size at every scale",
                "annotations sized at a reference scale, drawn the same size at every scale",
            ));
        }
        if self.along_lines > 0 {
            approximated(n(
                self.along_lines,
                "a text along a line, drawn level at the line's middle",
                "texts along lines, drawn level at their lines' middles",
            ));
        }
        if self.rect_aligned > 0 {
            approximated(n(
                self.rect_aligned,
                "a rectangle text aligned to a side of its box, drawn centred in it",
                "rectangle texts aligned to a side of their boxes, drawn centred in them",
            ));
        }
        if self.fixed_size > 0 {
            approximated(n(
                self.fixed_size,
                "a rectangle text of a fixed size on screen, drawn without its box",
                "rectangle texts of a fixed size on screen, drawn without their boxes",
            ));
        }
        if self.unwrapped > 0 {
            approximated(n(
                self.unwrapped,
                "a rectangle text, drawn on its own lines rather than wrapped to its box",
                "rectangle texts, drawn on their own lines rather than wrapped to their boxes",
            ));
        }
        if self.margins > 0 {
            approximated(n(self.margins, "a rectangle text's margins in its box", "rectangle texts' margins in their boxes"));
        }
        if self.clamped > 0 {
            approximated(n(
                self.clamped,
                "an annotation with a text longer, or a font, mark or line larger, than a graphic takes, cut to it",
                "annotations with a text longer, or a font, mark or line larger, than a graphic takes, cut to it",
            ));
        }
        if self.pictures_fixed > 0 {
            approximated(n(
                self.pictures_fixed,
                "a picture of a fixed size on screen, drawn filling its box on the map",
                "pictures of a fixed size on screen, drawn filling their boxes on the map",
            ));
        }
        if self.plain_symbols > 0 {
            approximated(n(
                self.plain_symbols,
                "an annotation's symbol of dashes, offsets, patterns or several layers, drawn in one colour and outline",
                "annotations' symbols of dashes, offsets, patterns or several layers, drawn in one colour and outline",
            ));
        }
        if self.round_markers > 0 {
            approximated(n(
                self.round_markers,
                "a marker annotation of another shape, drawn as a circle",
                "marker annotations of other shapes, drawn as circles",
            ));
        }
        if let Some(first) = self.fonts.first() {
            let more = if self.fonts.len() > 1 { format!(" and {} more", self.fonts.len() - 1) } else { String::new() };
            approximated(format!("text annotations in {first}{more}, drawn in RustGIS's font"));
        }
    }
}

fn num(n: Node, k: &str) -> Option<f64> {
    n.attribute(k).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite())
}

/// The graphics one item draws as, bottom first: none when it can't be drawn (noted in `read`).
fn item_graphics(item: Node, dir: &str, layer: &str, read: &mut Reading, report: &mut Report) -> Vec<Annotation> {
    let kind = item.attribute("type").unwrap_or_default();
    if item.attribute("useReferenceScale") == Some("1") && num(item, "referenceScale").is_some_and(|s| s > 0.0) {
        read.reference_scale += 1;
    }
    if child(item, "callout").is_some_and(|c| super::prop(&super::props(c), "enabled").is_some_and(|e| e == "1" || e == "true")) {
        read.callouts += 1;
    }
    let point = || Some(Geometry::Point(Coord::new(num(item, "x")?, num(item, "y")?)));
    let wkt = || item.attribute("wkt").and_then(|w| rustgis_data::interop::parse_wkt(w).ok()).and_then(|(g, _)| g);
    let one = |g: Graphic| vec![Annotation { graphic: g, picture: None, keep_aspect: false }];
    let symbol_of = |want: &str, report: &mut Report| {
        item.children()
            .find(|c| c.has_tag_name("symbol"))
            .filter(|s| s.attribute("type") == Some(want))
            .map(|s| symbol::read(s, None, false, layer, report))
    };
    match kind {
        "marker" | "linestring" | "polygon" => {
            let geometry = if kind == "marker" { point() } else { wkt() };
            let Some(geometry) = geometry else {
                read.bad_geometry += 1;
                return Vec::new();
            };
            let fits = matches!(
                (kind, &geometry),
                ("marker", Geometry::Point(_)) | ("linestring", Geometry::Line(_)) | ("polygon", Geometry::Polygon(_))
            );
            if !fits {
                read.bad_geometry += 1;
                return Vec::new();
            }
            let (shape, want) = match kind {
                "marker" => ("point", "marker"),
                "linestring" => ("line", "line"),
                _ => ("polygon", "fill"),
            };
            let mut g = Graphic::new(shape, geometry);
            if let Some(c) = symbol_of(want, report) {
                let s = c.symbol;
                read.plain_symbols += usize::from(c.stacked() || c.fills.len() > 1);
                read.round_markers += usize::from(kind == "marker" && s.marker != crate::Marker::Circle);
                match kind {
                    "linestring" => {
                        g.outline = Some(s.stroke);
                        g.width = Some(s.stroke_width);
                    }
                    _ => {
                        g.fill = Some(s.fill);
                        g.outline = Some(s.stroke);
                        g.width = Some(s.stroke_width);
                        if kind == "marker" {
                            g.size = Some(s.size);
                        }
                    }
                }
            }
            one(g)
        }
        "pointtext" => {
            let Some(geometry) = point() else {
                read.bad_geometry += 1;
                return Vec::new();
            };
            if num(item, "angle").is_some_and(|a| a.rem_euclid(360.0) != 0.0) {
                read.turned += 1;
            }
            // Qt's horizontal alignment flags: left 1, right 2, centre 4; none (a file without
            // them) is left, as QGIS reads it.
            let align = match num(item, "alignment").map_or(0, |a| a as i64) & 0xF {
                2 => TextAlign::Right,
                4 => TextAlign::Center,
                _ => TextAlign::Left,
            };
            let text = text_graphic(item, geometry, align, layer, read, report);
            read.point_texts += usize::from(text.is_some());
            text.map_or_else(Vec::new, one)
        }
        "linetext" => {
            let Some(Geometry::Line(parts)) = wkt() else {
                read.bad_geometry += 1;
                return Vec::new();
            };
            let Some(middle) = middle(&parts) else {
                read.bad_geometry += 1;
                return Vec::new();
            };
            read.along_lines += 1;
            text_graphic(item, Geometry::Point(middle), TextAlign::Center, layer, read, report).map_or_else(Vec::new, one)
        }
        "recttext" | "picture" => boxed(item, kind, dir, layer, read, report),
        other => {
            read.unknown_count += 1;
            let other = if other.is_empty() { "unnamed" } else { other };
            if read.unknown.len() < 8 && !read.unknown.iter().any(|u| u == other) {
                read.unknown.push(other.chars().take(40).collect());
            }
            Vec::new()
        }
    }
}

/// A rectangle text or a picture: the box (`xMin` … `yMax`), its background under it and its
/// frame (under a text, over a picture), and the text or picture in it.
fn boxed(item: Node, kind: &str, dir: &str, layer: &str, read: &mut Reading, report: &mut Report) -> Vec<Annotation> {
    let corners = (num(item, "xMin"), num(item, "yMin"), num(item, "xMax"), num(item, "yMax"));
    let (Some(x0), Some(y0), Some(x1), Some(y1)) = corners else {
        read.bad_geometry += 1;
        return Vec::new();
    };
    let (x0, x1, y0, y1) = (x0.min(x1), x0.max(x1), y0.min(y1), y0.max(y1));
    let mode = item.attribute("sizeMode").unwrap_or("SpatialBounds");
    if mode == "RelativeToMapFrame" {
        read.relative += 1;
        return Vec::new();
    }
    let fixed = mode == "FixedSize";
    let rect = || {
        Geometry::Polygon(vec![vec![vec![
            Coord::new(x0, y0),
            Coord::new(x1, y0),
            Coord::new(x1, y1),
            Coord::new(x0, y1),
            Coord::new(x0, y0),
        ]]])
    };
    let wrap = |g: Graphic| Annotation { graphic: g, picture: None, keep_aspect: false };
    let symbol_named = |name: &str, report: &mut Report| {
        item.descendants()
            .find(|d| d.has_tag_name("symbol") && d.attribute("name") == Some(name))
            .map(|s| symbol::read(s, None, false, layer, report))
    };
    let none = Rgba::new(0, 0, 0, 0);
    // Under a text, background and frame are one rectangle; a picture's frame goes over it.
    let background =
        (item.attribute("backgroundEnabled") == Some("1")).then(|| symbol_named("backgroundSymbol", report)).flatten().map(|c| {
            let mut g = Graphic::new("rectangle", rect());
            g.fill = Some(c.symbol.fill);
            let outlined = c.symbol.stroke.a() > 0 && c.symbol.stroke_width > 0.0;
            g.outline = Some(if outlined { c.symbol.stroke } else { none });
            g.width = Some(if outlined { c.symbol.stroke_width } else { 0.0 });
            g
        });
    let frame = (item.attribute("frameEnabled") == Some("1")).then(|| symbol_named("frameSymbol", report)).flatten().map(|c| {
        let mut g = Graphic::new("rectangle", rect());
        g.fill = Some(none);
        g.outline = Some(c.symbol.stroke);
        g.width = Some(c.symbol.stroke_width);
        g
    });
    let centre = Geometry::Point(Coord::new((x0 + x1) / 2.0, (y0 + y1) / 2.0));
    let mut out = Vec::new();
    if kind == "recttext" {
        if fixed {
            read.fixed_size += 1;
        } else {
            match (background, frame) {
                (Some(mut b), Some(f)) => {
                    b.outline = f.outline;
                    b.width = f.width;
                    out.push(wrap(b));
                }
                (Some(b), None) => out.push(wrap(b)),
                (None, Some(f)) => out.push(wrap(f)),
                (None, None) => {}
            }
        }
        // Qt's flags: horizontally left 1, right 2, centre 4; vertically top 32, bottom 64,
        // centre 128 (none: top).
        let a = num(item, "alignment").map_or(1, |a| a as i64);
        let align = match a & 0xF {
            2 => TextAlign::Right,
            4 => TextAlign::Center,
            _ => TextAlign::Left,
        };
        if !fixed && (a & 0xF != 4 || a & 0xE0 != 128) {
            read.rect_aligned += 1;
        }
        // Margins in the box (`left,top,right,bottom`), none of them 0.
        if item.attribute("margins").is_some_and(|m| m.split(',').any(|v| v.trim().parse::<f64>().is_ok_and(|v| v != 0.0))) {
            read.margins += 1;
        }
        if let Some(t) = text_graphic(item, centre, align, layer, read, report) {
            // QGIS wraps a text to its box's width; a text graphic keeps its own lines.
            read.unwrapped += usize::from(!fixed && t.text.as_deref().is_some_and(|t| t.contains(' ')));
            out.push(wrap(t));
        }
        return out;
    }
    // A picture.
    if item.attribute("format") == Some("SVG") {
        read.svg += 1;
        return Vec::new();
    }
    let path = item.attribute("path").map(str::trim).unwrap_or_default();
    if path.is_empty() {
        read.no_picture += 1;
        return Vec::new();
    }
    if fixed {
        read.pictures_fixed += 1;
    }
    let picture = if path.starts_with("base64:") { path.to_string() } else { super::source::absolute(path, dir) };
    out.extend(background.map(wrap));
    let g = Graphic::new("picture", rect());
    out.push(Annotation { graphic: g, picture: Some(picture), keep_aspect: item.attribute("lockAspect") != Some("0") });
    out.extend(frame.map(wrap));
    out
}

/// A text graphic of the item's `text` and `<text-style>` at `geometry`, or `None` for an empty
/// text.
fn text_graphic(item: Node, geometry: Geometry, align: TextAlign, layer: &str, read: &mut Reading, report: &mut Report) -> Option<Graphic> {
    let raw = item.attribute("text").unwrap_or_default();
    let style = item.descendants().find(|d| d.has_tag_name("text-style"));
    let html = style.is_some_and(|s| s.attribute("allowHtml") == Some("1"));
    let text = if html { plain(raw) } else { raw.to_string() };
    if text.trim().is_empty() {
        return None;
    }
    read.html += usize::from(html && text != raw);
    read.expressions += usize::from(text.contains("[%"));
    let mut g = Graphic::new("text", geometry);
    g.text = Some(text);
    g.align = Some(align);
    if let Some(s) = style {
        let unit = s.attribute("fontSizeUnit").unwrap_or("Point");
        g.font_size = Some(labels::points(num(s, "fontSize").unwrap_or(10.0), unit, "text annotation sizes", layer, report) as f32);
        let opacity = num(s, "textOpacity").unwrap_or(1.0).clamp(0.0, 1.0) as f32;
        let c = s.attribute("textColor").and_then(color).unwrap_or(Rgba::rgb(0, 0, 0));
        g.fill = Some(c.with_alpha((f32::from(c.a()) * opacity).round() as u8));
        (g.bold, g.italic) = labels::bold_italic(s);
        g.bold |= s.attribute("forcedBold") == Some("1");
        g.italic |= s.attribute("forcedItalic") == Some("1");
        g.letter_spacing = num(s, "fontLetterSpacing")
            .filter(|v| *v != 0.0)
            .map_or(0.0, |v| labels::points(v, unit, "text annotation letter spacing", layer, report) as f32);
        if let Some(f) = s.attribute("fontFamily").map(str::trim).filter(|f| !f.is_empty() && !generic(f))
            && read.fonts.len() < 8
            && !read.fonts.iter().any(|had| had == f)
        {
            read.fonts.push(f.chars().take(60).collect());
        }
        let on = |tag: &str, k: &str| child(s, tag).is_some_and(|n| n.attribute(k) == Some("1"));
        let unread = [
            (s.attribute("capitalization").is_some_and(|c| c != "0"), "capitalisation"),
            (s.attribute("fontUnderline") == Some("1"), "underlines"),
            (s.attribute("fontStrikeout") == Some("1"), "strikeouts"),
            (on("background", "shapeDraw"), "backgrounds"),
            (on("shadow", "shadowDraw"), "shadows"),
            (on("text-mask", "maskEnabled"), "masks"),
        ];
        for (set, what) in unread {
            if set && !read.unread_text.contains(&what) {
                read.unread_text.push(what);
            }
        }
    }
    Some(g)
}

/// A font family that only names a kind of font (QGIS's default "Sans Serif"): RustGIS's is one
/// too.
fn generic(family: &str) -> bool {
    matches!(family.to_ascii_lowercase().as_str(), "sans serif" | "sans-serif" | "sans" | "ms shell dlg 2")
}

/// HTML's text without its tags, `<br>` and the ends of paragraphs as line breaks, and the common
/// entities read.
fn plain(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        out.push_str(rest.get(..at).unwrap_or_default());
        let tail = rest.get(at..).unwrap_or_default();
        let end = tail.find('>').map_or(tail.len(), |e| e + 1);
        let tag = tail.get(..end).unwrap_or_default().to_ascii_lowercase();
        if tag.starts_with("<br") || tag.starts_with("</p") || tag.starts_with("</div") {
            out.push('\n');
        }
        rest = tail.get(end..).unwrap_or_default();
    }
    out.push_str(rest);
    let out = out.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&amp;", "&");
    out.trim_end_matches('\n').to_string()
}

/// The point halfway along the longest part of a line.
fn middle(parts: &[Vec<Coord>]) -> Option<Coord> {
    let dist = |a: &Coord, b: &Coord| (b.x - a.x).hypot(b.y - a.y);
    let len = |p: &[Coord]| p.iter().zip(p.iter().skip(1)).map(|(a, b)| dist(a, b)).sum::<f64>();
    let part = parts.iter().max_by(|a, b| len(a).total_cmp(&len(b)))?;
    let half = len(part) / 2.0;
    let mut gone = 0.0;
    for (a, b) in part.iter().zip(part.iter().skip(1)) {
        let d = dist(a, b);
        if gone + d >= half && d > 0.0 {
            let t = (half - gone) / d;
            return Some(Coord::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t));
        }
        gone += d;
    }
    part.first().copied().filter(|c| c.x.is_finite() && c.y.is_finite())
}
