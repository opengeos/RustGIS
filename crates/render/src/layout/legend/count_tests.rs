//! Feature counts of a legend's streamed layers (#3238).

use super::*;
use rustgis_data::source::{Batch, FeatureSource, Request, SourceInfo};
use rustgis_data::{Dataset, Feature, Field, FieldType, Geometry, Value};
use rustgis_map::source::SourceLayer;
use rustgis_map::{Layer, Renderer, Symbol};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A file source holding `ds`, whose view reads the first feature alone.
struct Whole {
    info: SourceInfo,
    ds: Dataset,
    /// Whether the map draws it by complete traversal, as a Shapefile; else it can only be read
    /// whole, as a GeoPackage.
    streams: bool,
    visits: bool,
    /// A whole read stops after its first chunk.
    fails: bool,
    passes: Arc<AtomicUsize>,
}

impl FeatureSource for Whole {
    fn info(&self) -> &SourceInfo {
        &self.info
    }
    fn read(&self, _: &Request) -> rustgis_data::Result<Batch> {
        Ok(Batch {
            features: self.ds.features.iter().take(1).cloned().collect(),
            matched: self.ds.features.len() as u64,
            ..Default::default()
        })
    }
    fn streams_chunks(&self) -> bool {
        self.streams
    }
    fn visits_chunks(&self) -> bool {
        self.visits
    }
    fn visit_chunks(
        &self,
        _: Option<Extent>,
        _: usize,
        visit: &mut dyn FnMut(Vec<Feature>) -> rustgis_data::Result<()>,
    ) -> rustgis_data::Result<Batch> {
        self.passes.fetch_add(1, Ordering::Relaxed);
        for chunk in self.ds.features.chunks(2) {
            visit(chunk.to_vec())?;
            if self.fails {
                return Err(rustgis_data::DataError::Format("the file broke off".into()));
            }
        }
        Ok(Batch { complete: true, matched: self.ds.features.len() as u64, ..Default::default() })
    }
}

#[derive(Clone, Copy)]
struct Reads {
    streams: bool,
    visits: bool,
    fails: bool,
}

const FILE: Reads = Reads { streams: false, visits: true, fails: false };

/// `layer` read in place from a source holding all its features, holding the first alone, and the
/// source's count of whole reads.
fn in_place(layer: &mut Layer, reads: Reads) -> Arc<AtomicUsize> {
    let ds = layer.dataset.clone();
    let info = SourceInfo {
        name: ds.name.clone(),
        format: "test",
        geometry_type: ds.geometry_type,
        fields: ds.fields.clone(),
        extent: ds.extent(),
        count: Some(ds.features.len() as u64),
        crs: "EPSG:4326".into(),
        indexed: true,
        notes: Default::default(),
        zm_dropped: false,
    };
    let passes = Arc::new(AtomicUsize::new(0));
    let Reads { streams, visits, fails } = reads;
    layer.dataset.features = ds.features.iter().take(1).cloned().collect::<Vec<_>>().into();
    layer.dataset.windowed = true;
    let source = Whole { info, ds, streams, visits, fails, passes: passes.clone() };
    layer.source = Some(SourceLayer::from_path("test.gpkg", None, Box::new(source)));
    passes
}

/// Points `"pts"` with these values of `g`, in Unique Values of `a`, `b` and `c`.
fn classed(values: &[&str]) -> Map {
    let mut ds = Dataset::new("pts", GeometryType::Point, vec![Field::new("g", FieldType::Text)]);
    for v in values {
        ds.push(Some(Geometry::Point(Coord::new(0.0, 0.0))), vec![Value::Text((*v).into())]);
    }
    let mut map = Map::new("m");
    map.add_dataset(ds);
    let symbol = Symbol::default_for(GeometryType::Point, 0);
    map.layers[0].renderer = unique(["a", "b", "c"].map(String::from).to_vec(), symbol, None);
    map
}

fn unique(classes: Vec<String>, symbol: Symbol, expression: Option<String>) -> Renderer {
    Renderer::Unique {
        field: "g".into(),
        classes: classes.into_iter().map(|c| (c, symbol)).collect(),
        other: symbol,
        ramp: rustgis_map::Ramp::Ember,
        class_labels: Default::default(),
        other_label: None,
        class_strokes: Default::default(),
        other_strokes: Vec::new(),
        class_fills: Default::default(),
        other_fills: Vec::new(),
        expression,
    }
}

/// `a` three times, `b` twice and one value no class has: in chunks of two, in file order.
const VALUES: [&str; 6] = ["a", "b", "a", "zz", "b", "a"];

static NO_CLASS_LABELS: ClassLabels = ClassLabels::NONE;

fn asked(all_classes: bool, show_no_data: bool, show_counts: bool) -> LegendAsked<'static> {
    static LABELS: BTreeMap<String, String> = BTreeMap::new();
    LegendAsked {
        all_classes,
        hide_other: false,
        show_no_data,
        layer_names: true,
        text_size: 9.0,
        exclude: &[],
        labels: &LABELS,
        class_labels: &NO_CLASS_LABELS,
        show_counts,
    }
}

/// The text of each row of the legend `asked`, or `None` when it can't be read.
fn texts(map: &mut Map, asked: &LegendAsked) -> Option<Vec<String>> {
    asked_rows(map, 1e6, asked, |rows| {
        rows.iter()
            .map(|r| match r {
                LegendRow::Head(n) => n.to_string(),
                LegendRow::Plain(_, n) => n.to_string(),
                LegendRow::Class(_, e) => e.label.clone(),
                LegendRow::Picture(_) => "picture".into(),
            })
            .collect()
    })
}

fn counted(map: &mut Map, all_classes: bool, show_no_data: bool) -> Vec<String> {
    texts(map, &asked(all_classes, show_no_data, true)).unwrap()
}

#[test]
fn a_streamed_layers_classes_count_the_whole_source() {
    for streams in [false, true] {
        let mut map = classed(&VALUES);
        in_place(&mut map.layers[0], Reads { streams, ..FILE });
        assert_eq!(counted(&mut map, false, false), ["pts", "a (3)", "b (2)", "<all other values> (1)"], "streams {streams}");
        assert_eq!(counted(&mut map, true, false), ["pts", "a (3)", "b (2)", "c (0)", "<all other values> (1)"], "streams {streams}");
    }
}

#[test]
fn a_streamed_layer_of_one_symbol_counts_the_whole_source() {
    let mut map = classed(&VALUES);
    map.layers[0].renderer = Renderer::Single(Symbol::default_for(GeometryType::Point, 0));
    in_place(&mut map.layers[0], FILE);
    assert_eq!(counted(&mut map, false, false), ["pts (6)"]);
    assert_eq!(counted(&mut map, true, false), ["pts (6)"]);
}

#[test]
fn a_streamed_layers_ranges_and_missing_values_are_counted() {
    let mut ds = Dataset::new("g", GeometryType::Point, vec![Field::new("v", FieldType::Double)]);
    for v in [Value::Float(1.0), Value::Float(2.0), Value::Null, Value::Float(9.0), Value::Null] {
        ds.push(Some(Geometry::Point(Coord::new(0.0, 0.0))), vec![v]);
    }
    let mut map = Map::new("m");
    map.add_dataset(ds);
    let symbol = Symbol::default_for(GeometryType::Point, 0);
    map.layers[0].renderer = Renderer::Graduated {
        field: "v".into(),
        breaks: vec![5.0, 10.0],
        symbols: vec![symbol, symbol],
        ramp: rustgis_map::Ramp::Ember,
        method: rustgis_map::ClassMethod::NaturalBreaks,
        min: 0.0,
        classes: 2,
        class_labels: BTreeMap::from([(0, "Low".into()), (1, "High".into())]),
        class_strokes: Default::default(),
        class_fills: Default::default(),
    };
    in_place(&mut map.layers[0], FILE);
    assert_eq!(counted(&mut map, false, true), ["g", "Low (2)", "High (1)", "No data (2)"]);
}

#[test]
fn a_streamed_layer_counts_only_what_it_draws() {
    let mut map = classed(&VALUES);
    in_place(&mut map.layers[0], FILE);
    map.layers[0].definition_query = Some("g <> 'b'".into());
    assert_eq!(counted(&mut map, false, false), ["pts", "a (3)", "<all other values> (1)"]);
    map.layers[0].display_filters =
        vec![rustgis_map::effects::DisplayFilter { name: "a".into(), min_scale: None, max_scale: None, where_: "g = 'a'".into() }];
    assert_eq!(counted(&mut map, false, false), ["pts", "a (3)"]);
    // Nothing kept: what the frame draws leaves it out, and every class says none.
    map.layers[0].definition_query = Some("g = 'nothing'".into());
    assert_eq!(counted(&mut map, false, false), [""; 0]);
    assert_eq!(counted(&mut map, true, false), ["pts", "a (0)", "b (0)", "c (0)", "<all other values> (0)"]);
    // Out of its scale range at the frame's 1:1,000,000.
    map.layers[0].definition_query = None;
    map.layers[0].display_filters = Vec::new();
    map.layers[0].max_scale = Some(5e6);
    assert_eq!(counted(&mut map, false, false), [""; 0]);
}

/// A database, a file read over the web or a source not open: no whole read, so no numbers, and
/// the classes of what the layer holds, as before.
#[test]
fn a_layer_whose_source_cannot_be_read_whole_has_no_counts() {
    let mut map = classed(&VALUES);
    let passes = in_place(&mut map.layers[0], Reads { visits: false, ..FILE });
    assert_eq!(counted(&mut map, false, false), ["pts", "a"]);
    assert_eq!(passes.load(Ordering::Relaxed), 0);
    map.layers[0].source = None;
    assert_eq!(counted(&mut map, false, false), ["pts", "a"]);
}

#[test]
fn a_failed_count_stops_the_legend() {
    for all_classes in [false, true] {
        let mut map = classed(&VALUES);
        in_place(&mut map.layers[0], Reads { fails: true, ..FILE });
        assert_eq!(texts(&mut map, &asked(all_classes, false, true)), None);
        assert!(map.layers[0].source.as_ref().unwrap().read_error().unwrap().contains("the file broke off"));
    }
}

#[test]
fn a_legend_without_counts_reads_no_source_whole() {
    let mut map = classed(&VALUES);
    let passes = in_place(&mut map.layers[0], FILE);
    for all_classes in [false, true] {
        texts(&mut map, &asked(all_classes, false, false)).unwrap();
    }
    assert_eq!(passes.load(Ordering::Relaxed), 0);
    // A layer the legend leaves out isn't read for it.
    let exclude = ["PTS".to_string()];
    texts(&mut map, &LegendAsked { exclude: &exclude, ..asked(false, false, true) }).unwrap();
    assert_eq!(passes.load(Ordering::Relaxed), 0);
}

/// Classing by expression is bounded over the whole source, not chunk by chunk.
#[test]
fn a_streamed_layer_too_large_to_class_by_expression_has_no_counts() {
    let mut map = classed(&["0"; 101]);
    let symbol = Symbol::default_for(GeometryType::Point, 0);
    let classes: Vec<String> = (0..20_000).map(|n| n.to_string()).collect();
    map.layers[0].renderer = unique(classes, symbol, Some("g".into()));
    let loaded = counted(&mut map, false, false);
    in_place(&mut map.layers[0], FILE);
    let streamed = counted(&mut map, false, false);
    assert_eq!(streamed, loaded);
    assert!(streamed.iter().all(|t| !t.ends_with(')')), "{:?}", &streamed[..3]);
}

#[test]
fn a_streamed_layers_diagram_rows_have_no_counts() {
    let mut ds = Dataset::new("pts", GeometryType::Point, vec![Field::new("n", FieldType::Integer)]);
    for n in 0..3 {
        ds.push(Some(Geometry::Point(Coord::new(0.0, 0.0))), vec![Value::Int(n)]);
    }
    let mut map = Map::new("m");
    map.add_dataset(ds);
    map.layers[0].renderer = Renderer::Single(Symbol::default_for(GeometryType::Point, 0));
    map.layers[0].diagram = Some(rustgis_map::DiagramSettings {
        kind: rustgis_map::DiagramKind::Pie,
        categories: vec![rustgis_map::DiagramCategory { field: "n".into(), colour: Rgba::rgb(255, 0, 0) }],
        size: rustgis_map::DiagramSize::Fixed(12.0),
        start_angle: 0.0,
        background: None,
        stroke: rustgis_map::DiagramSettings::default_stroke(),
        stroke_width: 0.75,
        labels: rustgis_map::DiagramLabel::None,
    });
    in_place(&mut map.layers[0], FILE);
    assert_eq!(counted(&mut map, false, false), ["pts (3)", "n"]);
}

/// A legend's size is measured on the rows as drawn, numbers included.
#[test]
fn a_streamed_legend_is_measured_with_its_numbers() {
    let mut loaded = classed(&VALUES);
    let mut map = classed(&VALUES);
    in_place(&mut map.layers[0], FILE);
    for all_classes in [false, true] {
        let asked = asked(all_classes, false, true);
        assert_eq!(legend_size_asked_mm(&mut map, 1e6, &asked, 1), legend_size_asked_mm(&mut loaded, 1e6, &asked, 1));
    }
}

/// A map frame of the map `"m"` and a legend of it that counts features.
fn counts_page(all_classes: bool) -> rustgis_map::layout::Layout {
    use rustgis_map::layout::{ElementKind, Layout};
    let mut page = Layout::new("Counts", 120.0, 90.0);
    let frame = page.add(
        "Frame",
        (5.0, 5.0, 50.0, 50.0),
        ElementKind::MapFrame {
            map: "m".into(),
            center: Coord::new(0.0, 0.0),
            scale: 1e6,
            bearing: 0.0,
            pitch: 0.0,
            shape: Default::default(),
            grid: None,
            sections: Vec::new(),
            extent_of: None,
            extent_color: rustgis_map::layout::INDICATOR_COLOR,
            extent_width: rustgis_map::layout::INDICATOR_WIDTH,
            border_color: rustgis_map::layout::FRAME_BORDER_COLOR,
            border_width: rustgis_map::layout::FRAME_BORDER_WIDTH,
        },
    );
    page.add(
        "Legend",
        (60.0, 5.0, 55.0, 80.0),
        ElementKind::Legend {
            frame,
            title: String::new(),
            all_classes,
            hide_other: false,
            show_no_data: false,
            exclude: Vec::new(),
            columns: 1,
            labels: BTreeMap::new(),
            color: None,
            text_size: rustgis_map::layout::LEGEND_TEXT_PT,
            fit: rustgis_map::layout::LegendFit::Manual,
            show_layer_names: rustgis_map::layout::LayerNames::Shown,
            class_labels: Default::default(),
            show_counts: rustgis_map::layout::FeatureCounts::from_shown(true),
        },
    );
    page
}

/// The page draws a streamed layer's numbers as it does a loaded one's, whether the legend lists
/// every class or what its frame draws; a count that fails stops the page, naming the source.
#[test]
fn a_page_counts_a_streamed_layer() {
    let page = counts_page;
    // The legend element alone: the frame's own drawing reads a GeoPackage by view.
    let draw = |page: &Layout, map: &Map| {
        let legend = page.elements[1].id;
        crate::layout::render_element(page, legend, &mut [map.clone()], 96.0, (&Default::default(), &Env::default()), &mut None)
            .map(|e| e.unwrap().0.data().to_vec())
    };
    for all_classes in [false, true] {
        let page = page(all_classes);
        let loaded = classed(&VALUES);
        let mut map = classed(&VALUES);
        in_place(&mut map.layers[0], FILE);
        assert_eq!(draw(&page, &map), draw(&page, &loaded), "all classes {all_classes}");
        in_place(&mut map.layers[0], Reads { fails: true, ..FILE });
        let error = crate::layout::render(&page, &mut [map], 96.0, &Default::default()).unwrap_err();
        assert!(error.contains("pts") && error.contains("the file broke off"), "{error}");
    }
}

/// Legends sharing a frame share its classes' read; each still counts its own numbers.
#[test]
fn every_legend_of_a_frame_counts_a_streamed_layer() {
    use rustgis_map::layout::ElementKind;
    for kinds in [[false, false], [false, true], [true, false]] {
        let mut page = counts_page(kinds[0]);
        // The legends alone are drawn: the frame's own drawing reads a GeoPackage by view.
        page.elements[0].visible = false;
        let mut second = page.elements[1].kind.clone();
        if let ElementKind::Legend { all_classes, .. } = &mut second {
            *all_classes = kinds[1];
        }
        page.add("Second", (5.0, 5.0, 50.0, 80.0), second);
        let draw = |map: Map| crate::layout::render(&page, &mut [map], 96.0, &Default::default()).unwrap().data().to_vec();
        let mut map = classed(&VALUES);
        in_place(&mut map.layers[0], FILE);
        assert!(draw(map) == draw(classed(&VALUES)), "all classes {kinds:?}");
    }
}
