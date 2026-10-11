//! Writers for exchange formats: KML, GPX and CSV with WKT geometry.

use rustgis_geom::Coord;

use crate::{Dataset, Geometry};

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn coords(pts: &[Coord]) -> String {
    pts.iter().map(|c| format!("{},{}", c.x, c.y)).collect::<Vec<_>>().join(" ")
}

fn kml_geometry(g: &Geometry) -> String {
    match g {
        Geometry::Point(c) => format!("<Point><coordinates>{},{}</coordinates></Point>", c.x, c.y),
        Geometry::MultiPoint(v) => {
            format!("<MultiGeometry>{}</MultiGeometry>", v.iter().map(|c| kml_geometry(&Geometry::Point(*c))).collect::<String>())
        }
        Geometry::Line(parts) => {
            let ls: String = parts.iter().map(|p| format!("<LineString><coordinates>{}</coordinates></LineString>", coords(p))).collect();
            if parts.len() == 1 { ls } else { format!("<MultiGeometry>{ls}</MultiGeometry>") }
        }
        Geometry::Polygon(polys) => {
            let ps: String = polys
                .iter()
                .map(|rings| {
                    let mut it = rings.iter();
                    let outer = it
                        .next()
                        .map(|r| {
                            format!("<outerBoundaryIs><LinearRing><coordinates>{}</coordinates></LinearRing></outerBoundaryIs>", coords(r))
                        })
                        .unwrap_or_default();
                    let inner: String = it
                        .map(|r| {
                            format!("<innerBoundaryIs><LinearRing><coordinates>{}</coordinates></LinearRing></innerBoundaryIs>", coords(r))
                        })
                        .collect();
                    format!("<Polygon>{outer}{inner}</Polygon>")
                })
                .collect();
            if polys.len() == 1 { ps } else { format!("<MultiGeometry>{ps}</MultiGeometry>") }
        }
    }
}

/// A KML document, one placemark per feature, named by `name_field` and with every attribute.
pub fn to_kml(ds: &Dataset, name_field: Option<usize>) -> String {
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<kml xmlns=\"http://www.opengis.net/kml/2.2\"><Document><name>{}</name>\n",
        xml(&ds.name)
    );
    for (r, f) in ds.features.iter().enumerate() {
        let Some(g) = &f.geometry else { continue };
        let name = name_field.map(|fi| ds.value(r, fi).to_string()).unwrap_or_else(|| f.id.to_string());
        let data: String = ds
            .fields
            .iter()
            .zip(&f.attrs)
            .filter(|(_, v)| !v.is_null())
            .map(|(fd, v)| format!("<Data name=\"{}\"><value>{}</value></Data>", xml(&fd.name), xml(&v.to_string())))
            .collect();
        out.push_str(&format!(
            "<Placemark><name>{}</name><ExtendedData>{data}</ExtendedData>{}</Placemark>\n",
            xml(&name),
            kml_geometry(g)
        ));
    }
    out.push_str("</Document></kml>\n");
    out
}

/// A GPX file: points become waypoints, lines tracks (polygon outlines too).
pub fn to_gpx(ds: &Dataset, name_field: Option<usize>) -> String {
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gpx version=\"1.1\" creator=\"RustGIS\" xmlns=\"http://www.topografix.com/GPX/1/1\">\n",
    );
    let pt = |c: &Coord| format!("lat=\"{}\" lon=\"{}\"", c.y, c.x);
    let mut tracks = String::new();
    for (r, f) in ds.features.iter().enumerate() {
        let Some(g) = &f.geometry else { continue };
        let name = xml(&name_field.map(|fi| ds.value(r, fi).to_string()).unwrap_or_else(|| f.id.to_string()));
        match g {
            Geometry::Point(c) => out.push_str(&format!("<wpt {}><name>{name}</name></wpt>\n", pt(c))),
            Geometry::MultiPoint(v) => v.iter().for_each(|c| out.push_str(&format!("<wpt {}><name>{name}</name></wpt>\n", pt(c)))),
            Geometry::Line(parts) => {
                let segs: String = parts
                    .iter()
                    .map(|p| format!("<trkseg>{}</trkseg>", p.iter().map(|c| format!("<trkpt {}/>", pt(c))).collect::<String>()))
                    .collect();
                tracks.push_str(&format!("<trk><name>{name}</name>{segs}</trk>\n"));
            }
            Geometry::Polygon(polys) => {
                let segs: String = polys
                    .iter()
                    .filter_map(|r| r.first())
                    .map(|p| format!("<trkseg>{}</trkseg>", p.iter().map(|c| format!("<trkpt {}/>", pt(c))).collect::<String>()))
                    .collect();
                tracks.push_str(&format!("<trk><name>{name}</name>{segs}</trk>\n"));
            }
        }
    }
    out.push_str(&tracks);
    out.push_str("</gpx>\n");
    out
}

/// Well-known text for a geometry.
pub fn wkt(g: &Geometry) -> String {
    let c = |p: &Coord| format!("{} {}", p.x, p.y);
    let list = |v: &[Coord]| v.iter().map(c).collect::<Vec<_>>().join(", ");
    match g {
        Geometry::Point(p) => format!("POINT ({})", c(p)),
        Geometry::MultiPoint(v) => format!("MULTIPOINT ({})", v.iter().map(|p| format!("({})", c(p))).collect::<Vec<_>>().join(", ")),
        Geometry::Line(parts) if parts.len() == 1 => format!("LINESTRING ({})", parts.first().map(|p| list(p)).unwrap_or_default()),
        Geometry::Line(parts) => {
            format!("MULTILINESTRING ({})", parts.iter().map(|p| format!("({})", list(p))).collect::<Vec<_>>().join(", "))
        }
        Geometry::Polygon(polys) => {
            let poly =
                |rings: &Vec<Vec<Coord>>| format!("({})", rings.iter().map(|r| format!("({})", list(r))).collect::<Vec<_>>().join(", "));
            if polys.len() == 1 {
                format!("POLYGON {}", polys.first().map(poly).unwrap_or_default())
            } else {
                format!("MULTIPOLYGON ({})", polys.iter().map(poly).collect::<Vec<_>>().join(", "))
            }
        }
    }
}

/// CSV with a `wkt` geometry column and every attribute.
pub fn to_wkt_csv(ds: &Dataset) -> String {
    let q = |s: &str| if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() };
    let mut out = String::from("wkt");
    for f in &ds.fields {
        out.push(',');
        out.push_str(&q(&f.name));
    }
    out.push('\n');
    for f in &ds.features {
        out.push_str(&q(&f.geometry.as_ref().map(wkt).unwrap_or_default()));
        for v in &f.attrs {
            out.push(',');
            out.push_str(&q(&if v.is_null() { String::new() } else { v.to_string() }));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Field, FieldType, GeometryType, Value};

    #[test]
    fn writes_kml_gpx_wkt() {
        let mut ds = Dataset::new("t", GeometryType::Point, vec![Field::new("name", FieldType::Text)]);
        ds.push(Some(Geometry::Point(Coord::new(2.35, 48.85))), vec![Value::Text("Paris & co".into())]);
        let k = to_kml(&ds, Some(0));
        assert!(k.contains("<name>Paris &amp; co</name>") && k.contains("<coordinates>2.35,48.85</coordinates>"));
        let g = to_gpx(&ds, Some(0));
        assert!(g.contains("<wpt lat=\"48.85\" lon=\"2.35\">"));
        assert_eq!(wkt(&Geometry::Line(vec![vec![Coord::new(0.0, 0.0), Coord::new(1.0, 1.0)]])), "LINESTRING (0 0, 1 1)");
        assert!(to_wkt_csv(&ds).starts_with("wkt,name\nPOINT (2.35 48.85),Paris & co"));
    }
}
