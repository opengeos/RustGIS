//! A small QGIS project and style, written by hand in the shapes QGIS writes them.

use super::*;
use crate::symbology::{LabelPlacement, LineCap, Marker};

const PROJECT: &str = r#"<!DOCTYPE qgis PUBLIC 'http://mrcc.com/qgis.dtd' 'SYSTEM'>
<qgis projectname="Towns" version="3.42.0-Münster">
  <title>Towns</title>
  <projectCrs><spatialrefsys><authid>EPSG:2056</authid></spatialrefsys></projectCrs>
  <layer-tree-group>
    <layer-tree-layer id="towns_1" checked="Qt::Checked" name="Towns"/>
    <layer-tree-group name="Base" checked="Qt::Unchecked">
      <layer-tree-layer id="roads_1" checked="Qt::Checked" name="Roads"/>
    </layer-tree-group>
    <layer-tree-layer id="land_1" checked="Qt::Checked" name="Land"/>
  </layer-tree-group>
  <mapcanvas name="theMapCanvas">
    <extent><xmin>2690000</xmin><ymin>1250000</ymin><xmax>2700000</xmax><ymax>1260000</ymax></extent>
  </mapcanvas>
  <projectlayers>
    <maplayer type="vector" hasScaleBasedVisibilityFlag="1" minScale="100000" maxScale="1000" labelsEnabled="1">
      <id>land_1</id>
      <datasource>./data/base.gpkg|layername=land|subset="kind" &lt;&gt; 'sea'</datasource>
      <layername>Land</layername>
      <srs><spatialrefsys><authid>EPSG:2056</authid></spatialrefsys></srs>
      <provider encoding="UTF-8">ogr</provider>
      <renderer-v2 type="graduatedSymbol" attr="pop" referencescale="25000">
        <ranges>
          <range lower="0" upper="100" symbol="0" label="few" render="true"/>
          <range lower="100" upper="1000" symbol="1" label="many" render="true"/>
        </ranges>
        <symbols>
          <symbol type="fill" name="0" alpha="1"><layer class="SimpleFill" enabled="1" pass="0">
            <Option type="Map"><Option name="color" type="QString" value="200,220,180,255,rgb:0.78,0.86,0.7,1"/><Option name="style" type="QString" value="solid"/>
            <Option name="outline_color" type="QString" value="0,0,0,255"/><Option name="outline_width" type="QString" value="0.5"/><Option name="outline_width_unit" type="QString" value="MM"/></Option>
          </layer></symbol>
          <symbol type="fill" name="1" alpha="0.5"><layer class="LinePatternFill" enabled="1">
            <Option type="Map"><Option name="distance" value="2"/></Option>
            <symbol type="line" name="@1@0" alpha="1"><layer class="SimpleLine" enabled="1"><prop k="line_color" v="0,0,255,255"/><prop k="line_width" v="0.2"/></layer></symbol>
          </layer></symbol>
        </symbols>
      </renderer-v2>
      <layerOpacity>0.75</layerOpacity>
      <blendMode>13</blendMode>
      <labeling type="simple">
        <settings><text-style fieldName="name" fontSize="12" fontSizeUnit="Point" textColor="20,40,60,255" namedStyle="Bold Italic" fontFamily="Liberation Sans Narrow">
          <text-buffer bufferDraw="1"/></text-style><placement placement="1" quadOffset="2"/></settings>
      </labeling>
    </maplayer>
    <maplayer type="vector" labelsEnabled="1">
      <id>roads_1</id>
      <datasource>/abs/roads.shp</datasource>
      <layername>Roads</layername>
      <provider>ogr</provider>
      <renderer-v2 type="categorizedSymbol" attr="&quot;class&quot;" symbollevels="1">
        <categories>
          <category value="motorway" symbol="0" label="Motorway" render="true"/>
          <category value="track" symbol="1" label="track" render="false"/>
          <category value="" symbol="2" label="other" render="true"/>
        </categories>
        <symbols>
          <symbol type="line" name="0" alpha="1">
            <layer class="SimpleLine" enabled="1" pass="0"><prop k="line_color" v="90,40,20,255"/><prop k="line_width" v="1.2"/><prop k="capstyle" v="flat"/></layer>
            <layer class="SimpleLine" enabled="1" pass="3"><prop k="line_color" v="240,140,40,255"/><prop k="line_width" v="0.8"/><prop k="use_custom_dash" v="1"/></layer>
          </symbol>
          <symbol type="line" name="1" alpha="1"><layer class="SimpleLine" enabled="1"><prop k="line_color" v="0,0,0,255"/></layer></symbol>
          <symbol type="line" name="2" alpha="1"><layer class="MarkerLine" enabled="1"/></symbol>
        </symbols>
      </renderer-v2>
      <labeling type="rule-based">
        <rules key="root">
          <rule key="a" filter="&quot;class&quot; = 'motorway'" description="Motorways">
            <settings><text-style fieldName="ref" fontSize="9"/><placement placement="3"/></settings>
          </rule>
          <rule key="p" filter="&quot;lanes&quot; &gt; 1">
            <rule key="b" filter="ELSE" description="Rest" scalemaxdenom="50000">
              <settings><text-style fieldName="name" fontSize="7"/><placement placement="2"/></settings>
            </rule>
            <rule key="c" filter="&quot;kind&quot; = 'x'"><settings><text-style fieldName="name" fontSize="7"/></settings></rule>
          </rule>
          <rule key="d" filter="length(&quot;name&quot;) &gt; 3"><settings><text-style fieldName="name"/></settings></rule>
        </rules>
      </labeling>
    </maplayer>
    <maplayer type="vector">
      <id>towns_1</id>
      <datasource>type=xyz&amp;url=https://tile.example.org/%7Bz%7D/%7Bx%7D/%7By%7D.png&amp;zmax=19</datasource>
      <layername>Towns</layername>
      <provider>wms</provider>
      <renderer-v2 type="RuleRenderer">
        <rules key="r">
          <rule key="1" filter="&quot;rank&quot; IN (1, 2)" symbol="0"/>
          <rule key="2" filter="ELSE" symbol="1"/>
        </rules>
        <symbols>
          <symbol type="marker" name="0" alpha="1"><layer class="SimpleMarker" enabled="1"><prop k="name" v="star"/><prop k="color" v="200,0,0,255"/><prop k="size" v="4"/></layer></symbol>
          <symbol type="marker" name="1" alpha="1"><layer class="FontMarker" enabled="1"><prop k="color" v="0,0,0,255"/><prop k="chr" v="A"/></layer></symbol>
        </symbols>
      </renderer-v2>
    </maplayer>
  </projectlayers>
</qgis>"#;

#[test]
fn a_project_reads_into_layers_in_tree_order_with_their_styles() {
    let mut report = Report::default();
    let p = read_project(PROJECT, "/maps", &mut report).unwrap();
    assert_eq!(p.title, "Towns");
    assert_eq!(p.crs.as_deref(), Some("EPSG:2056"));
    assert_eq!(p.extent, Some([2_690_000.0, 1_250_000.0, 2_700_000.0, 1_260_000.0]));
    let names: Vec<&str> = p.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Towns", "Roads", "Land"], "top first");
    let visible: Vec<bool> = p.layers.iter().map(|l| l.visible).collect();
    assert_eq!(visible, [true, false, true], "Roads is in an unchecked group");

    // Land: a GeoPackage table, filtered; graduated; opacity, blending, scales, reference scale.
    let land = &p.layers[2];
    assert_eq!(
        land.source,
        Source::File { path: "/maps/data/base.gpkg".into(), layer: Some("land".into()), subset: Some("\"kind\" <> 'sea'".into()) }
    );
    assert_eq!(land.definition_query.as_deref(), Some("\"kind\" <> 'sea'"));
    assert_eq!(land.crs.as_deref(), Some("EPSG:2056"));
    let s = &land.style;
    assert_eq!((s.transparency, s.blend), (Some(25.0), Some(Blend::Multiply)));
    let (min, max) = (s.min_scale.flatten().unwrap(), s.max_scale.flatten().unwrap());
    assert!((min - 100_000.0 * QGIS_SCALE).abs() < 1e-6 && (max - 1000.0 * QGIS_SCALE).abs() < 1e-6);
    assert!((s.reference_scale.unwrap() - 25_000.0 * QGIS_SCALE).abs() < 1e-6);
    let Some(Renderer::Graduated { field, breaks, symbols, min, class_labels, .. }) = &s.renderer else { panic!("{:?}", s.renderer) };
    assert_eq!((field.as_str(), breaks.as_slice(), *min), ("pop", &[100.0, 1000.0][..], 0.0));
    assert_eq!(symbols[0].fill, Rgba::rgb(200, 220, 180));
    assert!((symbols[0].stroke_width - 0.5 * 96.0 / 25.4).abs() < 1e-4, "0.5 mm in points");
    // The pattern fill: a tint of its line colour, at the symbol's alpha too.
    assert_eq!((symbols[1].fill.b(), symbols[1].fill.a()), (255, 45));
    assert_eq!(class_labels.get(&1).map(String::as_str), Some("many"));
    let labels = s.labels.as_ref().unwrap();
    let d = &labels.default;
    assert!(labels.enabled && labels.classes.is_empty());
    assert_eq!((d.field.as_str(), d.size, d.bold, d.italic, d.halo), ("name", 12.0, true, true, true));
    assert_eq!((d.color, d.placement), (Rgba::rgb(20, 40, 60), LabelPlacement::AboveRight));

    // Roads: categories with stacked strokes in levels, a hidden one and the rest.
    let roads = &p.layers[1];
    assert_eq!(roads.source, Source::File { path: "/abs/roads.shp".into(), layer: None, subset: None });
    let s = &roads.style;
    assert_eq!((s.stroke_levels, s.line_strokes.as_deref()), (Some(true), Some(&[][..])));
    let Some(Renderer::Unique { field, classes, class_strokes, class_labels, other, .. }) = &s.renderer else { panic!() };
    assert_eq!(field, "class");
    assert_eq!(classes.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), ["motorway", "track"]);
    assert_eq!(classes[1].1.stroke.a(), 0, "not rendered: hidden");
    assert_eq!(other.stroke.a(), 0, "a marker line only: nothing RustGIS draws");
    let motorway = &class_strokes["motorway"];
    assert_eq!(motorway.len(), 2);
    assert_eq!((motorway[0].cap, motorway[0].level, motorway[1].level), (LineCap::Butt, Some(0), Some(3)));
    assert_eq!(class_labels.get("motorway").map(String::as_str), Some("Motorway"));
    // Labels: a class per rule; the else rule takes what its sibling doesn't, under its parent.
    let l = s.labels.as_ref().unwrap();
    assert!(!l.default.visible);
    let by = |n: &str| l.classes.iter().find(|c| c.name == n).unwrap();
    assert_eq!((by("Motorways").where_.as_deref(), by("Motorways").placement), (Some("\"class\" = 'motorway'"), LabelPlacement::Curved));
    let rest = by("Rest");
    assert_eq!(rest.where_.as_deref(), Some("(\"lanes\" > 1) AND (NOT ((\"kind\" = 'x')))"));
    assert_eq!(rest.placement, LabelPlacement::Parallel);
    assert!((rest.min_scale.unwrap() - 50_000.0 * QGIS_SCALE).abs() < 1e-6);
    assert_eq!(l.classes.len(), 3, "the function filter's rule is left out");

    // Towns: XYZ tiles as the source; rules on one field read as categories.
    let towns = &p.layers[0];
    assert_eq!(
        towns.source,
        Source::Xyz { url: "https://tile.example.org/{z}/{x}/{y}.png".into(), zmin: None, zmax: Some(19), vector: false }
    );
    let Some(Renderer::Unique { field, classes, other, .. }) = &towns.style.renderer else { panic!() };
    assert_eq!((field.as_str(), classes.len()), ("rank", 2));
    assert_eq!((classes[0].1.marker, classes[0].1.fill), (Marker::Star, Rgba::rgb(200, 0, 0)));
    assert_eq!((other.marker, other.fill), (Marker::Circle, Rgba::rgb(0, 0, 0)));

    // What didn't carry over is said, once for each thing.
    let said =
        |layer: &str, kind: NoteKind, part: &str| report.notes.iter().any(|n| n.layer == layer && n.kind == kind && n.text.contains(part));
    assert!(said("Land", NoteKind::Approximated, "line pattern"));
    assert!(said("Land", NoteKind::Approximated, "Liberation Sans Narrow"));
    assert!(said("Roads", NoteKind::Approximated, "dashed"));
    assert!(said("Roads", NoteKind::Skipped, "markers and hash marks"));
    assert!(said("Roads", NoteKind::Skipped, "length("));
    assert!(said("Towns", NoteKind::Approximated, "font markers"));
}

#[test]
fn a_style_file_reads_in_either_settings_format() {
    // QGIS before 3.26 writes a symbol layer's settings as <prop k v>, after as <Option>.
    let qml = |settings: &str| {
        format!(
            r#"<!DOCTYPE qgis><qgis version="3.10"><renderer-v2 type="singleSymbol"><symbols><symbol type="line" name="0" alpha="1"><layer class="SimpleLine" enabled="1">{settings}</layer></symbol></symbols></renderer-v2></qgis>"#
        )
    };
    for settings in [
        r#"<prop k="line_color" v="10,20,30,255"/><prop k="line_width" v="2"/><prop k="line_width_unit" v="Pixel"/>"#,
        r#"<Option type="Map"><Option name="line_color" value="10,20,30,255"/><Option name="line_width" value="2"/><Option name="line_width_unit" value="Pixel"/></Option>"#,
    ] {
        let mut report = Report::default();
        let s = read_style(&qml(settings), "Rivers", &mut report).unwrap();
        let Some(Renderer::Single(sym)) = s.renderer else { panic!() };
        assert_eq!((sym.stroke, sym.stroke_width), (Rgba::rgb(10, 20, 30), 2.0));
        assert_eq!(s.line_strokes.as_deref(), Some(&[][..]), "one plain line: no stack");
        assert!(report.notes.is_empty(), "{:?}", report.notes);
    }
    // Applied to a layer: its parts only.
    let mut layer = crate::Layer::new(1, rustgis_data::Dataset::new("r", rustgis_data::GeometryType::Polyline, vec![]));
    layer.transparency = 40.0;
    let s = read_style(&qml(r#"<prop k="line_color" v="1,2,3,255"/>"#), "r", &mut Report::default()).unwrap();
    s.apply(&mut layer);
    assert_eq!((layer.transparency, matches!(layer.renderer, Renderer::Single(_))), (40.0, true));
}

#[test]
fn hostile_files_are_refused_or_bounded() {
    let mut r = Report::default();
    assert!(read_project("not xml", "", &mut r).is_err());
    assert!(read_project("<other/>", "", &mut r).is_err());
    assert!(read_style("<qgis><renderer-v2 type=\"x\"", "", &mut r).is_err());
    // An entity expansion bomb: no DTD is read.
    let bomb = r#"<!DOCTYPE qgis [<!ENTITY a "aaaaaaaaaa"><!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;">]><qgis><title>&b;</title></qgis>"#;
    assert!(read_project(bomb, "", &mut r).is_err());
    // Rules nested past any sensible depth, and nonsense values: read without a crash.
    let deep = format!(
        r#"<qgis><renderer-v2 type="RuleRenderer" referencescale="-5"><rules>{}{}</rules><symbols/></renderer-v2><layerOpacity>NaN</layerOpacity></qgis>"#,
        "<rule filter=\"a = 1\">".repeat(200),
        "</rule>".repeat(200)
    );
    let s = read_style(&deep, "x", &mut r).unwrap();
    assert_eq!((s.reference_scale, s.transparency), (None, None));
    // The same note many times is kept once, counted.
    let mut r = Report::default();
    for _ in 0..10_000 {
        r.skipped("a", "the same");
    }
    assert_eq!((r.notes.len(), r.notes[0].count), (1, 10_000));
    // Many different notes stop at the cap; the ones kept still count their repeats, each in its
    // own place (layer, kind and text all tell notes apart).
    for i in 0..(MAX_NOTES + 500) {
        r.approximated("a", format!("note {i}"));
    }
    assert_eq!(r.notes.len(), MAX_NOTES);
    r.approximated("a", "note 7");
    r.approximated("b", "note 7");
    r.skipped("a", "note 7");
    let at = r.notes.iter().position(|n| n.text == "note 7").unwrap();
    assert_eq!((r.notes[at].count, r.notes[at].kind), (2, NoteKind::Approximated));
    assert!(!r.notes.iter().any(|n| n.layer == "b"), "past the cap, a new note isn't kept");
}

#[test]
fn paths_on_a_drive_are_told_apart_from_absolute_ones() {
    for p in ["C:roads.gpkg", "d:data\\x.shp", "C:"] {
        assert!(is_drive_relative(p) && !is_relative(p), "{p}");
    }
    for p in ["C:/gis/roads.gpkg", "C:\\gis\\roads.gpkg", "/srv/x.shp", "https://x.org/a.geojson"] {
        assert!(!is_drive_relative(p) && !is_relative(p), "{p}");
    }
    assert!(is_relative("./data/x.shp") && is_relative("data/x.shp") && !is_drive_relative("data/x.shp"));
}
