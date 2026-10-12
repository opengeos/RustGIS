//! Coordinate systems from WKT1, the text of a `.prj` file next to a Shapefile or a grid.
//!
//! An EPSG authority is used when the text names one. The `.prj` files ArcGIS and GDAL write have
//! none (`PROJCS["WGS_1984_UTM_Zone_31N",…]`), so Web Mercator and UTM are also known by name, and
//! any other projection is turned into a PROJ definition from its parameters.

use super::crs::{Crs, CustomCrs};

/// The most nested brackets read.
const MAX_DEPTH: usize = 32;

/// One value inside a WKT node's brackets.
#[derive(Debug)]
enum Item {
    Node(Node),
    Text(String),
    Num(f64),
    /// A bare word, such as `EAST` in `AXIS["X",EAST]`.
    Word,
}

/// `NAME[item, item, …]`.
#[derive(Debug)]
struct Node {
    name: String,
    items: Vec<Item>,
}

impl Node {
    /// The first child node of this name (case-insensitive).
    fn child(&self, name: &str) -> Option<&Node> {
        self.items.iter().find_map(|i| match i {
            Item::Node(n) if n.name.eq_ignore_ascii_case(name) => Some(n),
            _ => None,
        })
    }

    /// The node's quoted name: `"WGS_1984"` in `DATUM["WGS_1984",…]`.
    fn title(&self) -> &str {
        match self.items.first() {
            Some(Item::Text(t)) => t,
            _ => "",
        }
    }

    /// The `n`th number among the node's values.
    fn num(&self, n: usize) -> Option<f64> {
        self.items
            .iter()
            .filter_map(|i| match i {
                Item::Num(v) => Some(*v),
                _ => None,
            })
            .nth(n)
            .filter(|v| v.is_finite())
    }

    /// Every number among the node's values.
    fn nums(&self) -> Vec<f64> {
        self.items
            .iter()
            .filter_map(|i| match i {
                Item::Num(v) => Some(*v),
                _ => None,
            })
            .collect()
    }
}

struct Parser<'a> {
    s: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn peek(&mut self) -> Option<u8> {
        while self.s.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
        self.s.get(self.at).copied()
    }

    fn word(&mut self) -> String {
        let start = self.at;
        while self.s.get(self.at).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_') {
            self.at += 1;
        }
        String::from_utf8_lossy(self.s.get(start..self.at).unwrap_or_default()).into_owned()
    }

    fn node(&mut self, depth: usize) -> Result<Node, String> {
        if depth > MAX_DEPTH {
            return Err("the coordinate system is nested too deeply".into());
        }
        let name = self.word();
        if name.is_empty() {
            return Err("the coordinate system isn't WKT".into());
        }
        let close = match self.peek() {
            Some(b'[') => b']',
            Some(b'(') => b')',
            _ => return Err(format!("expected '[' after {name}")),
        };
        self.at += 1;
        let mut items = Vec::new();
        loop {
            let item = match self.peek() {
                Some(b'"') => Item::Text(self.text()?),
                Some(b) if b == b'-' || b == b'+' || b == b'.' || b.is_ascii_digit() => Item::Num(self.number()?),
                Some(b) if b.is_ascii_alphabetic() => {
                    let save = self.at;
                    self.word();
                    if matches!(self.peek(), Some(b'[' | b'(')) {
                        self.at = save;
                        Item::Node(self.node(depth + 1)?)
                    } else {
                        Item::Word
                    }
                }
                Some(b) if b == close && items.is_empty() => {
                    self.at += 1;
                    return Ok(Node { name, items });
                }
                _ => return Err(format!("unexpected text in {name}")),
            };
            items.push(item);
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b) if b == close => {
                    self.at += 1;
                    return Ok(Node { name, items });
                }
                _ => return Err(format!("expected ',' in {name}")),
            }
        }
    }

    /// A quoted string; `""` inside it is a quote.
    fn text(&mut self) -> Result<String, String> {
        self.at += 1;
        let mut out = Vec::new();
        loop {
            match self.s.get(self.at) {
                None => return Err("a quoted name isn't closed".into()),
                Some(b'"') if self.s.get(self.at + 1) == Some(&b'"') => {
                    out.push(b'"');
                    self.at += 2;
                }
                Some(b'"') => {
                    self.at += 1;
                    return Ok(String::from_utf8_lossy(&out).into_owned());
                }
                Some(b) => {
                    out.push(*b);
                    self.at += 1;
                }
            }
        }
    }

    fn number(&mut self) -> Result<f64, String> {
        let start = self.at;
        while self.s.get(self.at).is_some_and(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E')) {
            self.at += 1;
        }
        let t = std::str::from_utf8(self.s.get(start..self.at).unwrap_or_default()).unwrap_or("");
        t.parse().map_err(|_| format!("'{t}' isn't a number"))
    }
}

fn parse(text: &str) -> Result<Node, String> {
    let mut p = Parser { s: text.trim_start_matches('\u{feff}').as_bytes(), at: 0 };
    p.peek();
    let root = p.node(0)?;
    match p.peek() {
        None => Ok(root),
        Some(_) => Err("text follows the coordinate system".into()),
    }
}

/// Upper case with spaces, dashes and slashes as underscores, runs of them as one: Esri and EPSG
/// spellings of a name compare equal (`WGS 84 / UTM zone 31N` → `WGS_84_UTM_ZONE_31N`).
fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let c = if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' };
        if c != '_' || !(out.is_empty() || out.ends_with('_')) {
            out.push(c);
        }
    }
    out.trim_end_matches('_').to_string()
}

/// The coordinate system a WKT1 text (OGC or Esri) describes.
pub fn crs_from_wkt(text: &str) -> Result<Crs, String> {
    let root = parse(text)?;
    if let Some(a) = root.child("AUTHORITY")
        && a.title().eq_ignore_ascii_case("EPSG")
        && let Some(crs) = authority_code(a).and_then(Crs::from_epsg)
    {
        return Ok(crs);
    }
    match root.name.to_ascii_uppercase().as_str() {
        "GEOGCS" => geographic(&root),
        "PROJCS" => projected(&root),
        other => Err(format!("{other} coordinate systems aren't supported")),
    }
}

/// The code in `AUTHORITY["EPSG","32631"]` (quoted or not).
fn authority_code(a: &Node) -> Option<u32> {
    match a.items.get(1)? {
        Item::Text(t) => t.trim().parse().ok(),
        Item::Num(n) if n.fract() == 0.0 && *n > 0.0 && *n <= f64::from(u32::MAX) => Some(*n as u32),
        _ => None,
    }
}

/// Longitude/latitude in degrees. Datums differ by metres at most for the systems in use, so any
/// is read as WGS 84.
fn geographic(g: &Node) -> Result<Crs, String> {
    match g.child("UNIT").and_then(|u| u.num(0)) {
        Some(f) if (f - 1f64.to_radians()).abs() > 1e-9 => {
            Err(format!("angles in {} aren't supported", g.child("UNIT").map_or("", Node::title)))
        }
        _ => Ok(Crs::LonLat),
    }
}

/// A projected system: by EPSG code for the common names, else as a PROJ definition.
fn projected(p: &Node) -> Result<Crs, String> {
    let name = norm(p.title());
    let method = norm(p.child("PROJECTION").map_or("", Node::title));
    if name.contains("WEB_MERCATOR")
        || name.contains("PSEUDO_MERCATOR")
        || method == "MERCATOR_AUXILIARY_SPHERE"
        || method == "POPULAR_VISUALISATION_PSEUDO_MERCATOR"
    {
        return Ok(Crs::WebMercator);
    }
    if let Some(crs) = utm(&name).and_then(Crs::from_epsg) {
        return Ok(crs);
    }
    custom(p, &method)
}

/// The EPSG code of a UTM zone on WGS 84, NAD 83 or ETRS 89 from its name
/// (`WGS_1984_UTM_Zone_31N`, `WGS 84 / UTM zone 31N`, `NAD83 / UTM zone 17N`).
fn utm(name: &str) -> Option<u32> {
    let (datum, zone) = name.split_once("_UTM_ZONE_")?;
    let digits: String = zone.chars().take_while(char::is_ascii_digit).collect();
    let z: u32 = digits.parse().ok().filter(|z| (1..=60).contains(z))?;
    let north = match zone.get(digits.len()..)? {
        "N" => true,
        "S" => false,
        _ => return None,
    };
    match datum {
        "WGS_1984" | "WGS_84" => Some(if north { 32600 } else { 32700 } + z),
        "NAD_1983" | "NAD83" if north => Some(26900 + z),
        "ETRS_1989" | "ETRS89" if north => Some(25800 + z),
        _ => None,
    }
}

/// A projection's PROJ definition built from its WKT parameters.
fn custom(p: &Node, method: &str) -> Result<Crs, String> {
    let params: Vec<(String, f64)> = p
        .items
        .iter()
        .filter_map(|i| match i {
            Item::Node(n) if n.name.eq_ignore_ascii_case("PARAMETER") => Some((norm(n.title()), n.num(0)?)),
            _ => None,
        })
        .collect();
    let get = |names: &[&str]| names.iter().find_map(|n| params.iter().find(|(k, _)| k == n).map(|(_, v)| *v));
    // Metres per linear unit: false eastings and northings are in the system's units, PROJ's in
    // metres.
    let to_m = p.child("UNIT").and_then(|u| u.num(0)).filter(|m| *m > 0.0).unwrap_or(1.0);
    let lat0 = get(&["LATITUDE_OF_ORIGIN", "LATITUDE_OF_CENTER"]).unwrap_or(0.0);
    let lon0 = get(&["CENTRAL_MERIDIAN", "LONGITUDE_OF_CENTER", "LONGITUDE_OF_ORIGIN"]).unwrap_or(0.0);
    let x0 = get(&["FALSE_EASTING"]).unwrap_or(0.0) * to_m;
    let y0 = get(&["FALSE_NORTHING"]).unwrap_or(0.0) * to_m;
    let k0 = get(&["SCALE_FACTOR"]).unwrap_or(1.0);
    let lat1 = get(&["STANDARD_PARALLEL_1"]);
    let lat2 = get(&["STANDARD_PARALLEL_2"]);
    let at = format!("+lon_0={lon0} +x_0={x0} +y_0={y0}");
    let proj = match method {
        "TRANSVERSE_MERCATOR" | "GAUSS_KRUGER" => format!("+proj=tmerc +lat_0={lat0} +k_0={k0} {at}"),
        "MERCATOR" | "MERCATOR_1SP" | "MERCATOR_2SP" => match lat1 {
            Some(ts) => format!("+proj=merc +lat_ts={ts} {at}"),
            None => format!("+proj=merc +k_0={k0} {at}"),
        },
        "LAMBERT_CONFORMAL_CONIC" | "LAMBERT_CONFORMAL_CONIC_1SP" | "LAMBERT_CONFORMAL_CONIC_2SP" => {
            let l1 = lat1.unwrap_or(lat0);
            let l2 = lat2.unwrap_or(l1);
            format!("+proj=lcc +lat_1={l1} +lat_2={l2} +lat_0={lat0} +k_0={k0} {at}")
        }
        "ALBERS" | "ALBERS_CONIC_EQUAL_AREA" => {
            let l1 = lat1.unwrap_or(lat0);
            let l2 = lat2.unwrap_or(l1);
            format!("+proj=aea +lat_1={l1} +lat_2={l2} +lat_0={lat0} {at}")
        }
        "LAMBERT_AZIMUTHAL_EQUAL_AREA" => format!("+proj=laea +lat_0={lat0} {at}"),
        "AZIMUTHAL_EQUIDISTANT" => format!("+proj=aeqd +lat_0={lat0} {at}"),
        "STEREOGRAPHIC" => format!("+proj=stere +lat_0={lat0} +k_0={k0} {at}"),
        "OBLIQUE_STEREOGRAPHIC" | "DOUBLE_STEREOGRAPHIC" => format!("+proj=sterea +lat_0={lat0} +k_0={k0} {at}"),
        "POLAR_STEREOGRAPHIC" | "STEREOGRAPHIC_NORTH_POLE" | "STEREOGRAPHIC_SOUTH_POLE" => {
            // The pole on the side of the latitude of true scale.
            let ts = lat1.unwrap_or(lat0);
            let pole = if method.ends_with("SOUTH_POLE") || ts < 0.0 { -90.0 } else { 90.0 };
            let ts = if ts == 0.0 { pole } else { ts };
            format!("+proj=stere +lat_0={pole} +lat_ts={ts} +k_0={k0} {at}")
        }
        "EQUIRECTANGULAR" | "EQUIDISTANT_CYLINDRICAL" | "PLATE_CARREE" => format!("+proj=eqc +lat_ts={} {at}", lat1.unwrap_or(0.0)),
        "CYLINDRICAL_EQUAL_AREA" | "LAMBERT_CYLINDRICAL_EQUAL_AREA" | "BEHRMANN" => {
            let ts = lat1.unwrap_or(if method == "BEHRMANN" { 30.0 } else { 0.0 });
            format!("+proj=cea +lat_ts={ts} {at}")
        }
        "MOLLWEIDE" => format!("+proj=moll {at}"),
        "MILLER_CYLINDRICAL" => format!("+proj=mill {at}"),
        "" => return Err("the projection isn't named".into()),
        _ => return Err(format!("the {} projection isn't supported", p.child("PROJECTION").map_or("", Node::title))),
    };
    let units = if (to_m - 1.0).abs() > 1e-12 { format!("+to_meter={to_m}") } else { "+units=m".to_string() };
    let label = match p.title().trim() {
        "" => "Custom projection".to_string(),
        t => t.to_string(),
    };
    Crs::custom(CustomCrs {
        proj4: format!("{proj} {} {units} +no_defs", earth(p)),
        label,
        geokeys: Vec::new(),
        doubles: Vec::new(),
        ascii: String::new(),
    })
}

/// The datum, by name for the common ones, else by its spheroid's axes.
fn earth(p: &Node) -> String {
    let geog = p.child("GEOGCS");
    let datum = geog.and_then(|g| g.child("DATUM"));
    let name = norm(datum.map_or("", Node::title));
    let mut out = match name.trim_start_matches("D_") {
        "WGS_1984" | "WGS84" | "WORLD_GEODETIC_SYSTEM_1984" => "+datum=WGS84".to_string(),
        "NORTH_AMERICAN_1983"
        | "NORTH_AMERICAN_DATUM_1983"
        | "NAD_1983"
        | "NAD83"
        | "ETRS_1989"
        | "ETRS89"
        | "EUROPEAN_TERRESTRIAL_REFERENCE_SYSTEM_1989"
        | "GDA_1994"
        | "GDA94" => "+ellps=GRS80 +towgs84=0,0,0".to_string(),
        _ => {
            let sph = datum.and_then(|d| d.child("SPHEROID").or_else(|| d.child("ELLIPSOID")));
            let mut s = match (sph.and_then(|s| s.num(0)), sph.and_then(|s| s.num(1))) {
                (Some(a), Some(rf)) if a > 0.0 && rf > 0.0 => format!("+a={a} +rf={rf}"),
                (Some(a), _) if a > 0.0 => format!("+a={a} +b={a}"),
                _ => "+datum=WGS84".to_string(),
            };
            if let Some(t) = datum.and_then(|d| d.child("TOWGS84")).map(Node::nums)
                && (t.len() == 3 || t.len() == 7)
                && t.iter().all(|v| v.is_finite())
            {
                s.push_str(&format!(" +towgs84={}", t.iter().map(f64::to_string).collect::<Vec<_>>().join(",")));
            }
            s
        }
    };
    if let Some(pm) = geog.and_then(|g| g.child("PRIMEM")).and_then(|m| m.num(0))
        && pm != 0.0
    {
        out.push_str(&format!(" +pm={pm}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use rustgis_geom::Coord;

    use super::*;

    const GEOG_4326: &str = r#"GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]]"#;
    const ESRI_3857: &str = r#"PROJCS["WGS_1984_Web_Mercator_Auxiliary_Sphere",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Mercator_Auxiliary_Sphere"],PARAMETER["False_Easting",0.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",0.0],PARAMETER["Standard_Parallel_1",0.0],PARAMETER["Auxiliary_Sphere_Type",0.0],UNIT["Meter",1.0]]"#;
    const ESRI_32631: &str = r#"PROJCS["WGS_1984_UTM_Zone_31N",GEOGCS["GCS_WGS_1984",DATUM["D_WGS_1984",SPHEROID["WGS_1984",6378137.0,298.257223563]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Transverse_Mercator"],PARAMETER["False_Easting",500000.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",3.0],PARAMETER["Scale_Factor",0.9996],PARAMETER["Latitude_Of_Origin",0.0],UNIT["Meter",1.0]]"#;

    #[test]
    fn esri_names() {
        assert_eq!(crs_from_wkt(GEOG_4326), Ok(Crs::LonLat));
        assert_eq!(crs_from_wkt(ESRI_3857), Ok(Crs::WebMercator));
        assert_eq!(crs_from_wkt(ESRI_32631), Ok(Crs::Epsg(32631)));
        assert_eq!(utm("WGS_84_UTM_ZONE_17S"), Some(32717));
        assert_eq!(utm(&norm("NAD83 / UTM zone 17N")), Some(26917));
        assert_eq!(utm("ETRS_1989_UTM_ZONE_32N"), Some(25832));
        assert_eq!(utm("WGS_1984_UTM_ZONE_61N"), None);
    }

    #[test]
    fn epsg_authority() {
        let wkt = r#"PROJCS["anything",GEOGCS["WGS 84",DATUM["WGS_1984",SPHEROID["WGS 84",6378137,298.257223563,AUTHORITY["EPSG","7030"]],AUTHORITY["EPSG","6326"]],PRIMEM["Greenwich",0],UNIT["degree",0.0174532925199433],AUTHORITY["EPSG","4326"]],PROJECTION["Transverse_Mercator"],PARAMETER["central_meridian",-81],UNIT["metre",1],AXIS["Easting",EAST],AXIS["Northing",NORTH],AUTHORITY["EPSG","32617"]]"#;
        assert_eq!(crs_from_wkt(wkt), Ok(Crs::Epsg(32617)));
    }

    #[test]
    fn projection_parameters() {
        // NAD 1983 StatePlane New York Long Island FIPS 3104 (US feet), as ArcGIS writes it.
        let wkt = r#"PROJCS["NAD_1983_StatePlane_New_York_Long_Island_FIPS_3104_Feet",GEOGCS["GCS_North_American_1983",DATUM["D_North_American_1983",SPHEROID["GRS_1980",6378137.0,298.257222101]],PRIMEM["Greenwich",0.0],UNIT["Degree",0.0174532925199433]],PROJECTION["Lambert_Conformal_Conic"],PARAMETER["False_Easting",984250.0],PARAMETER["False_Northing",0.0],PARAMETER["Central_Meridian",-74.0],PARAMETER["Standard_Parallel_1",40.66666666666666],PARAMETER["Standard_Parallel_2",41.03333333333333],PARAMETER["Latitude_Of_Origin",40.16666666666666],UNIT["Foot_US",0.3048006096012192]]"#;
        let crs = crs_from_wkt(wkt).unwrap();
        assert!(matches!(crs, Crs::Custom(_)));
        assert_eq!(crs.label(), "NAD_1983_StatePlane_New_York_Long_Island_FIPS_3104_Feet");
        // Times Square, from PROJ: 988_267 ft E, 215_437 ft N.
        let ll = crs.transformer().unwrap().to_lonlat(Coord::new(988_267.0, 215_437.1)).unwrap();
        assert!((ll.x + 73.9855).abs() < 1e-5 && (ll.y - 40.758).abs() < 1e-5, "{ll:?}");

        // The UTM zone by its parameters, when the name isn't one known.
        let renamed = ESRI_32631.replace("WGS_1984_UTM_Zone_31N", "My_Grid");
        let crs = crs_from_wkt(&renamed).unwrap();
        let ll = crs.transformer().unwrap().to_lonlat(Coord::new(500_000.0, 5_000_000.0)).unwrap();
        assert!((ll.x - 3.0).abs() < 1e-6 && (ll.y - 45.153_477).abs() < 1e-5, "{ll:?}");
    }

    #[test]
    fn hostile_text() {
        for bad in
            ["", "garbage", "PROJCS[", "PROJCS[\"x\"", "PROJCS[\"x\",PROJECTION[\"Hotine\"]]", "GEOGCS[\"x\"] trailing", "VERT_CS[\"x\"]"]
        {
            assert!(crs_from_wkt(bad).is_err(), "{bad}");
        }
        let deep = format!("{}{}", "A[".repeat(10_000), "]".repeat(10_000));
        assert!(crs_from_wkt(&deep).is_err());
        assert_eq!(crs_from_wkt(r#"GEOGCS["a ""quoted"" name",UNIT["Degree",0.0174532925199433]]"#), Ok(Crs::LonLat));
    }
}
