//! Tabulate Intersection through `rustgis_analysis::run`: class areas and sums shared out to zones.

use rustgis_analysis::{Inputs, Output, ToolResult};
use rustgis_data::{Coord, Dataset, Field, FieldType, Geometry, GeometryType, Value};
use serde_json::json;
use std::collections::HashMap;

/// The tool's inputs: layers by name, and each layer's selected rows.
#[derive(Default)]
struct Layers {
    data: HashMap<String, Dataset>,
    selected: HashMap<String, Vec<usize>>,
}
impl Inputs for Layers {
    /// The layer of this name.
    fn dataset(&self, name: &str) -> Option<&Dataset> {
        self.data.get(name)
    }
    /// The layer's selected rows; none selects nothing, so every row is used.
    fn selection(&self, name: &str) -> &[usize] {
        self.selected.get(name).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// A lon/lat rectangle from `x0` to `x1` and the equator to 1°N.
fn rect(x0: f64, x1: f64) -> Option<Geometry> {
    let ring = [(x0, 0.0), (x1, 0.0), (x1, 1.0), (x0, 1.0), (x0, 0.0)].map(|(x, y)| Coord::new(x, y)).to_vec();
    Some(Geometry::Polygon(vec![vec![ring]]))
}

/// Zones `A` (0°–1°E), `B` (1°–2°E) and `C` (10°–11°E), named by `zone`.
fn zones() -> Dataset {
    let mut ds = Dataset::new("zones", GeometryType::Polygon, vec![Field::new("zone", FieldType::Text)]);
    for (name, x0) in [("A", 0.0), ("B", 1.0), ("C", 10.0)] {
        ds.push_next(rect(x0, x0 + 1.0), vec![Value::Text(name.into())]);
    }
    ds
}

/// Classes by `kind`, with a population `pop`: a forest across A and B, a second forest over the
/// first one's half in A, and water over A's other half with no population.
fn classes() -> Dataset {
    let mut ds = Dataset::new(
        "classes",
        GeometryType::Polygon,
        vec![Field::new("kind", FieldType::Text), Field::new("pop", FieldType::Integer), Field::new("zone", FieldType::Text)],
    );
    let rows = [(0.5, 1.5, "forest", Value::Int(100)), (0.5, 1.0, "forest", Value::Int(10)), (0.0, 0.5, "water", Value::Null)];
    for (x0, x1, kind, pop) in rows {
        ds.push_next(rect(x0, x1), vec![Value::Text(kind.into()), pop, Value::Text("x".into())]);
    }
    ds
}

/// The two layers as the tool's inputs, under their own names, with nothing selected.
fn layers(zones: Dataset, classes: Dataset) -> Layers {
    let mut l = Layers::default();
    l.data.insert(zones.name.clone(), zones);
    l.data.insert(classes.name.clone(), classes);
    l
}

/// The standalone table a run made.
fn table(result: ToolResult) -> Dataset {
    match result.output {
        Output::Table(ds) => ds,
        _ => panic!("expected a table"),
    }
}

/// Each row's text fields, then its numbers rounded to 3 decimals (empty as NaN).
fn rows(ds: &Dataset) -> Vec<(Vec<String>, Vec<f64>)> {
    ds.features
        .iter()
        .map(|f| {
            let text = f.attrs.iter().filter_map(|v| if let Value::Text(t) = v { Some(t.clone()) } else { None }).collect();
            let nums = f
                .attrs
                .iter()
                .filter(|v| !matches!(v, Value::Text(_)))
                .map(|v| v.as_f64().map_or(f64::NAN, |n| (n * 1000.0).round() / 1000.0))
                .collect();
            (text, nums)
        })
        .collect()
}

/// The `area_km2` Calculate Geometry Attributes gives a polygon, rounded as [`rows`] rounds.
fn calculated_area_km2(g: Option<Geometry>) -> f64 {
    let mut ds = Dataset::new("one", GeometryType::Polygon, vec![]);
    ds.push_next(g, vec![]);
    let l = layers(ds, Dataset::new("none", GeometryType::Polygon, vec![]));
    let Output::Update { dataset, .. } = rustgis_analysis::run("calculateGeometry", &l, &json!({"input": "one"})).unwrap().output else {
        panic!("expected an update")
    };
    let area = dataset.field_index("area_km2").map(|i| dataset.value(0, i).clone());
    area.and_then(|v| v.as_f64()).map(|a| (a * 1000.0).round() / 1000.0).unwrap()
}

#[test]
fn shares_class_areas_and_sums_out_to_zones() {
    let l = layers(zones(), classes());
    let params = json!({"zones": "zones", "zoneFields": "zone", "classes": "classes", "classFields": "kind", "sumFields": ["pop"]});
    let result = rustgis_analysis::run("tabulateIntersection", &l, &params).unwrap();
    let messages = result.messages.clone();
    let out = table(result);
    let names: Vec<&str> = out.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["zone", "kind", "area_km2", "percentage", "pop"]);
    assert!(out.fields.iter().skip(2).all(|f| f.ty == FieldType::Double));
    let got = rows(&out);
    let text: Vec<Vec<String>> = got.iter().map(|(t, _)| t.clone()).collect();
    assert_eq!(text, [["A", "forest"], ["A", "water"], ["B", "forest"]].map(|r| r.map(String::from).to_vec()));
    // Half of A, measured as Calculate Geometry Attributes measures it: the two forests over that
    // half count once.
    let expected = calculated_area_km2(rect(0.5, 1.0));
    let half = got.first().and_then(|(_, n)| n.first()).copied().unwrap();
    assert!((half - expected).abs() < 0.01 && (6100.0..6200.0).contains(&half), "{half} km², not {expected}");
    for (_, nums) in &got {
        assert!((nums[0] - half).abs() < 0.01 && (nums[1] - 50.0).abs() < 0.001, "{nums:?}");
    }
    // A gets half the first forest's 100 and all the second's 10; B the other half of 100. Water
    // has no population, so its share is 0 and a notice says so.
    let pops: Vec<f64> = got.iter().map(|(_, n)| n[2]).collect();
    assert_eq!(pops, [60.0, 0.0, 50.0]);
    assert_eq!(
        messages,
        [
            "Tabulate Intersection: 3 rows",
            "1 zone overlaps no class feature, so it has no rows",
            "1 class feature has no value in 'pop', left out of its shares",
        ]
    );
}

#[test]
fn clean_input_gives_no_notices() {
    let mut l = layers(zones(), classes());
    // Zones A and B only, and the forests, which all have a population.
    l.selected.insert("zones".into(), vec![1, 0]);
    l.selected.insert("classes".into(), vec![0, 1]);
    let params = json!({"zones": "zones", "zoneFields": ["zone"], "classes": "classes", "sumFields": "pop"});
    let result = rustgis_analysis::run("tabulateIntersection", &l, &params).unwrap();
    assert_eq!(result.messages, ["Tabulate Intersection: 2 rows"]);
    let out = table(result);
    // Without class fields every class feature is one class.
    let names: Vec<&str> = out.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["zone", "area_km2", "percentage", "pop"]);
    let got = rows(&out);
    assert_eq!(got.iter().map(|(t, _)| t.join("")).collect::<Vec<_>>(), ["A", "B"]);
    assert_eq!(got.iter().map(|(_, n)| (n[1], n[2])).collect::<Vec<_>>(), [(50.0, 60.0), (50.0, 50.0)]);
}

#[test]
fn says_what_it_leaves_out() {
    let mut z = zones();
    z.push_next(None, vec![Value::Text("D".into())]);
    // A geometry with no polygon, as GeoJSON's `"coordinates": []` reads.
    z.push_next(Some(Geometry::Polygon(vec![])), vec![Value::Text("E".into())]);
    let mut c = Dataset::new("classes", GeometryType::Polygon, vec![Field::new("pop", FieldType::Integer)]);
    // Half in B, half outside every zone; then one without geometry.
    c.push_next(rect(1.5, 2.5), vec![Value::Int(100)]);
    c.push_next(None, vec![Value::Int(7)]);
    // A line in a polygon layer: it has geometry, but no polygon.
    c.push_next(Some(Geometry::Line(vec![vec![Coord::new(1.2, 0.2), Coord::new(1.8, 0.8)]])), vec![Value::Int(9)]);
    let l = layers(z, c);
    let params = json!({"zones": "zones", "zoneFields": "zone", "classes": "classes", "sumFields": "pop"});
    let result = rustgis_analysis::run("tabulateIntersection", &l, &params).unwrap();
    assert_eq!(
        result.messages,
        [
            "Tabulate Intersection: 1 row",
            "2 zone features have no polygon and were left out",
            "2 class features have no polygon and were left out",
            "2 zones overlap no class feature, so they have no rows",
            "1 class feature lies partly or wholly outside the zones: its value outside isn't shared out",
        ]
    );
    let got = rows(&table(result));
    assert_eq!(got.iter().map(|(t, n)| (t.join(""), n[1], n[2])).collect::<Vec<_>>(), [("B".into(), 50.0, 50.0)]);
}

/// A class feature with a long slanted edge, cut by a meridian between two zones: the cut point lies
/// partway along the slanted edge, and its value still shares out in full.
#[test]
fn shares_add_up_where_a_cut_crosses_a_slanted_edge() {
    let square = |x0: f64, x1: f64| {
        let ring = [(x0, 40.0), (x1, 40.0), (x1, 60.0), (x0, 60.0), (x0, 40.0)].map(|(x, y)| Coord::new(x, y)).to_vec();
        Some(Geometry::Polygon(vec![vec![ring]]))
    };
    let mut z = Dataset::new("zones", GeometryType::Polygon, vec![Field::new("zone", FieldType::Text)]);
    z.push_next(square(0.0, 10.0), vec![Value::Text("A".into())]);
    z.push_next(square(10.0, 20.0), vec![Value::Text("B".into())]);
    let mut c = Dataset::new("classes", GeometryType::Polygon, vec![Field::new("pop", FieldType::Integer)]);
    let triangle = [(0.0, 40.0), (20.0, 40.0), (0.0, 60.0), (0.0, 40.0)].map(|(x, y)| Coord::new(x, y)).to_vec();
    c.push_next(Some(Geometry::Polygon(vec![vec![triangle]])), vec![Value::Int(1_000_000)]);
    let l = layers(z, c);
    let params = json!({"zones": "zones", "zoneFields": "zone", "classes": "classes", "sumFields": "pop"});
    let result = rustgis_analysis::run("tabulateIntersection", &l, &params).unwrap();
    assert_eq!(result.messages, ["Tabulate Intersection: 2 rows"]);
    let out = table(result);
    let pops: Vec<f64> = out.features.iter().filter_map(|f| f.attrs.get(3).and_then(Value::as_f64)).collect();
    assert_eq!(pops.len(), 2);
    let total: f64 = pops.iter().sum();
    assert!((total / 1_000_000.0 - 1.0).abs() < 1e-6, "{pops:?} add up to {total}");
    // Each zone's share is its part of the triangle's area, worked out here by Simpson's rule on the
    // sphere: the slanted edge is straight on the map, so its isometric latitude ψ changes linearly
    // with longitude, and the triangle's area at each longitude runs from 40° up to that edge.
    let psi = |lat: f64| lat.to_radians().tan().asinh();
    let above_40 = |lon: f64| (psi(60.0) + (psi(40.0) - psi(60.0)) * lon / 20.0).tanh() - 40f64.to_radians().sin();
    let simpson = |a: f64, b: f64| {
        let n = 2000;
        let h = (b - a) / n as f64;
        (0..=n)
            .map(|i| {
                above_40(a + h * i as f64)
                    * if i == 0 || i == n {
                        1.0
                    } else if i % 2 == 1 {
                        4.0
                    } else {
                        2.0
                    }
            })
            .sum::<f64>()
            * h
            / 3.0
    };
    let (in_a, in_b) = (simpson(0.0, 10.0), simpson(10.0, 20.0));
    let expected = [in_a, in_b].map(|part| 1_000_000.0 * part / (in_a + in_b));
    for (got, want) in pops.iter().zip(expected) {
        assert!((got / want - 1.0).abs() < 1e-6, "{pops:?}, not {expected:?}");
    }
}

/// Zones that overlap each other give a feature shares adding up to its whole value while half of
/// it lies outside them both: the notice still says so.
#[test]
fn overlapping_zones_still_say_what_lies_outside() {
    let mut z = Dataset::new("zones", GeometryType::Polygon, vec![Field::new("zone", FieldType::Text)]);
    z.push_next(rect(0.0, 1.0), vec![Value::Text("A".into())]);
    z.push_next(rect(0.5, 1.5), vec![Value::Text("B".into())]);
    let mut c = Dataset::new("classes", GeometryType::Polygon, vec![Field::new("pop", FieldType::Integer)]);
    c.push_next(rect(0.0, 2.0), vec![Value::Int(100)]);
    let l = layers(z, c);
    let params = json!({"zones": "zones", "zoneFields": "zone", "classes": "classes", "sumFields": "pop"});
    let result = rustgis_analysis::run("tabulateIntersection", &l, &params).unwrap();
    assert_eq!(
        result.messages,
        ["Tabulate Intersection: 2 rows", "1 class feature lies partly or wholly outside the zones: its value outside isn't shared out"]
    );
    let got = rows(&table(result));
    assert_eq!(got.iter().map(|(_, n)| n[2]).collect::<Vec<_>>(), [50.0, 50.0]);
}

/// A class feature reaching beyond the map's latitude limit, and features lacking values in two sum
/// fields: one notice for each kind.
#[test]
fn one_notice_per_kind_for_missing_values_and_the_latitude_limit() {
    let polar = |x0: f64, x1: f64, y0: f64, y1: f64| {
        let ring = [(x0, y0), (x1, y0), (x1, y1), (x0, y1), (x0, y0)].map(|(x, y)| Coord::new(x, y)).to_vec();
        Some(Geometry::Polygon(vec![vec![ring]]))
    };
    let mut z = Dataset::new("zones", GeometryType::Polygon, vec![Field::new("zone", FieldType::Text)]);
    z.push_next(polar(0.0, 10.0, 80.0, 84.0), vec![Value::Text("A".into())]);
    let mut c = Dataset::new(
        "classes",
        GeometryType::Polygon,
        vec![Field::new("pop", FieldType::Integer), Field::new("income", FieldType::Double)],
    );
    c.push_next(polar(0.0, 5.0, 80.0, 84.0), vec![Value::Null, Value::Null]);
    c.push_next(polar(5.0, 10.0, 80.0, 84.0), vec![Value::Null, Value::Float(1.0)]);
    // Reaches 88°N; its part in the zone, below 84°N, is measured as usual.
    c.push_next(polar(2.0, 3.0, 82.0, 88.0), vec![Value::Int(1), Value::Float(2.0)]);
    let l = layers(z, c);
    let params = json!({"zones": "zones", "zoneFields": "zone", "classes": "classes", "sumFields": ["pop", "income"]});
    let result = rustgis_analysis::run("tabulateIntersection", &l, &params).unwrap();
    assert_eq!(
        result.messages,
        [
            "Tabulate Intersection: 1 row",
            "class features with no value in a sum field are left out of its shares: 2 class features in 'pop', 1 class feature in 'income'",
            "1 class feature reaches beyond 85.05° latitude, where the map ends: parts there are taken as lying along it, with no area",
            "1 class feature lies partly or wholly outside the zones: its value outside isn't shared out",
        ]
    );
}

#[test]
fn zones_sharing_a_value_are_one_zone() {
    let mut z = zones();
    for f in &mut z.features {
        f.attrs[0] = Value::Text("all".into());
    }
    let l = layers(z, classes());
    let params = json!({"zones": "zones", "zoneFields": "zone", "classes": "classes", "classFields": "kind"});
    let result = rustgis_analysis::run("tabulateIntersection", &l, &params).unwrap();
    assert_eq!(result.messages, ["Tabulate Intersection: 2 rows"]);
    // The forests cover 1° of the zone's 3°, the water half a degree.
    let got = rows(&table(result));
    assert_eq!(
        got.iter().map(|(t, n)| (t.join(" "), n[1])).collect::<Vec<_>>(),
        [("all forest".into(), 33.333), ("all water".into(), 16.667)]
    );
}

#[test]
fn refuses_inputs_it_cannot_tabulate() {
    let mut lines = Dataset::new("lines", GeometryType::Polyline, vec![Field::new("zone", FieldType::Text)]);
    lines.push_next(Some(Geometry::Line(vec![vec![Coord::new(0.0, 0.0), Coord::new(1.0, 1.0)]])), vec![Value::Text("A".into())]);
    let mut l = layers(zones(), classes());
    l.data.insert("lines".into(), lines);
    let run = |params: serde_json::Value| rustgis_analysis::run("tabulateIntersection", &l, &params).unwrap_err();
    assert_eq!(run(json!({"zones": "zones", "zoneFields": "zone", "classes": "lines"})), "'lines' is not a polygon layer");
    assert_eq!(run(json!({"zones": "zones", "zoneFields": [], "classes": "classes"})), "'Zone Fields' is required");
    assert_eq!(
        run(json!({"zones": "zones", "zoneFields": "zone", "classes": "classes", "sumFields": "kind"})),
        "the sum field 'kind' is not numeric"
    );
    assert_eq!(
        run(json!({"zones": "zones", "zoneFields": "zone", "classes": "classes", "classFields": "pop", "sumFields": "pop"})),
        "the sum field 'pop' can't also be a class field"
    );
}
