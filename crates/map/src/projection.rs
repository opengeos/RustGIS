//! Map projections: how the flat map lays the Earth out (the globe is in [`crate::globe`]).
//!
//! A map is drawn in Web Mercator by default, or in any projected coordinate system: a preset
//! (world and polar projections, regional equal-area grids) or an EPSG code. The camera then
//! works in that system's units, so panning, zooming and hit testing are the same for all of
//! them; only conversions between longitude/latitude and map coordinates go through here.

use rustgis_geom::{Coord, Extent, lonlat_to_merc, merc_to_lonlat};

/// A named projection: (id, label, PROJ definition).
pub const PRESETS: &[(&str, &str, &str)] = &[
    ("equirectangular", "Equirectangular (Plate Carrée)", "+proj=eqc +lat_ts=0 +lon_0=0 +datum=WGS84 +units=m +no_defs"),
    ("miller", "Miller Cylindrical", "+proj=mill +lon_0=0 +R_A +datum=WGS84 +units=m +no_defs"),
    ("mollweide", "Mollweide (equal-area world)", "+proj=moll +lon_0=0 +datum=WGS84 +units=m +no_defs"),
    ("cylindricalEqualArea", "Lambert Cylindrical Equal Area", "+proj=cea +lon_0=0 +lat_ts=0 +datum=WGS84 +units=m +no_defs"),
    ("northPolar", "North Polar Stereographic", "+proj=stere +lat_0=90 +lat_ts=70 +lon_0=0 +datum=WGS84 +units=m +no_defs"),
    ("southPolar", "South Polar Stereographic", "+proj=stere +lat_0=-90 +lat_ts=-71 +lon_0=0 +datum=WGS84 +units=m +no_defs"),
    (
        "europeEqualArea",
        "Europe Equal Area (ETRS89 LAEA)",
        "+proj=laea +lat_0=52 +lon_0=10 +x_0=4321000 +y_0=3210000 +ellps=GRS80 +units=m +no_defs",
    ),
    (
        "usaAlbers",
        "USA Contiguous Albers Equal Area",
        "+proj=aea +lat_0=23 +lon_0=-96 +lat_1=29.5 +lat_2=45.5 +datum=NAD83 +units=m +no_defs",
    ),
];

/// The PROJ definition and label of a projection code: a preset id or `EPSG:<code>`.
pub fn lookup(code: &str) -> Result<(String, String), String> {
    let code = code.trim();
    if let Some((_, label, def)) = PRESETS.iter().find(|(id, label, _)| id.eq_ignore_ascii_case(code) || label.eq_ignore_ascii_case(code)) {
        return Ok((def.to_string(), label.to_string()));
    }
    let digits = code.trim_start_matches(|c: char| !c.is_ascii_digit());
    let n: u16 = digits.parse().map_err(|_| format!("unknown projection '{code}'"))?;
    if !code.to_ascii_lowercase().starts_with("epsg") && digits.len() != code.len() {
        return Err(format!("unknown projection '{code}'"));
    }
    let def = crs_definitions::from_code(n).ok_or_else(|| format!("unknown coordinate system EPSG:{n}"))?;
    if def.proj4.contains("+proj=longlat") {
        return Err(format!("EPSG:{n} is in degrees: pick a projected coordinate system"));
    }
    if def.proj4.contains("+proj=geocent") {
        return Err(format!("EPSG:{n} isn't a map projection"));
    }
    Ok((def.proj4.to_string(), format!("EPSG:{n}")))
}

/// Where a projection is meant to be used. Points outside are not drawn: far from it most
/// projections distort wildly, fold back or run off to infinity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Domain {
    /// The whole world (cylindrical and pseudo-cylindrical projections).
    World,
    /// A longitude/latitude box (regional presets).
    Box(Extent),
    /// Within `radius` degrees of a centre (azimuthal and stereographic projections).
    Cap { center: Coord, radius: f64 },
    /// Within `half` degrees of longitude of a central meridian (transverse Mercator, UTM).
    Band { lon0: f64, half: f64 },
}

impl Domain {
    /// The area of use for a regional preset, or one worked out from the PROJ parameters.
    fn of(id: Option<&str>, def: &str) -> Domain {
        match id {
            Some("europeEqualArea") => return Domain::Box(Extent::new(-35.0, 25.0, 50.0, 75.0)),
            Some("usaAlbers") => return Domain::Box(Extent::new(-130.0, 18.0, -60.0, 55.0)),
            _ => {}
        }
        let param = |k: &str| -> Option<f64> {
            def.split_whitespace().find_map(|t| t.strip_prefix(&format!("+{k}="))).and_then(|v| v.parse().ok())
        };
        let proj = def.split_whitespace().find_map(|t| t.strip_prefix("+proj=")).unwrap_or("");
        let (lon0, lat0) = (param("lon_0").unwrap_or(0.0), param("lat_0").unwrap_or(0.0));
        match proj {
            "tmerc" | "etmerc" | "utm" => {
                let lon0 = if proj == "utm" { param("zone").map(|z| z * 6.0 - 183.0).unwrap_or(lon0) } else { lon0 };
                Domain::Band { lon0, half: 30.0 }
            }
            "stere" | "sterea" | "somerc" | "krovak" => Domain::Cap { center: Coord::new(lon0, lat0), radius: 60.0 },
            "laea" | "aeqd" => Domain::Cap { center: Coord::new(lon0, lat0), radius: 120.0 },
            "aea" | "lcc" => {
                let (a, b) = (param("lat_1").unwrap_or(lat0), param("lat_2").unwrap_or(lat0));
                let (lo, hi) = (a.min(b) - 35.0, a.max(b) + 35.0);
                Domain::Box(Extent::new(lon0 - 90.0, lo.max(-89.0), lon0 + 90.0, hi.min(89.0)))
            }
            "geos" => Domain::Cap { center: Coord::new(lon0, 0.0), radius: 75.0 },
            _ => Domain::World,
        }
    }

    pub fn contains(&self, c: Coord) -> bool {
        match *self {
            Domain::World => true,
            Domain::Box(e) => e.contains(c),
            Domain::Cap { center, radius } => rustgis_geom::haversine_m(center, c) <= radius.to_radians() * rustgis_geom::EARTH_RADIUS_M,
            Domain::Band { lon0, half } => ((c.x - lon0 + 540.0).rem_euclid(360.0) - 180.0).abs() <= half && c.y.abs() <= 84.0,
        }
    }
}

/// Converts between longitude/latitude and the coordinates of a custom projection.
#[derive(Clone)]
pub struct Projector {
    native: Box<proj4rs::Proj>,
    wgs84: Box<proj4rs::Proj>,
    /// Metres per map unit.
    to_meter: f64,
    domain: Domain,
}

impl std::fmt::Debug for Projector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Projector({})", self.native.projname())
    }
}

/// Map coordinates beyond this are treated as unprojectable (the projection's edge).
const LIMIT: f64 = 4.0e7;

impl Projector {
    pub fn new(code: &str) -> Result<Projector, String> {
        let (def, _) = lookup(code)?;
        let native = proj4rs::Proj::from_proj_string(&def).map_err(|e| format!("{code}: {e}"))?;
        let wgs84 = proj4rs::Proj::from_proj_string("+proj=longlat +datum=WGS84 +no_defs").map_err(|e| e.to_string())?;
        let to_meter = native.to_meter();
        let preset = PRESETS.iter().find(|(id, _, _)| id.eq_ignore_ascii_case(code.trim())).map(|(id, _, _)| *id);
        let domain = Domain::of(preset, &def);
        Ok(Projector { native: Box::new(native), wgs84: Box::new(wgs84), to_meter: if to_meter > 0.0 { to_meter } else { 1.0 }, domain })
    }

    /// Longitude/latitude to map coordinates, `None` outside the projection.
    pub fn forward(&self, c: Coord) -> Option<Coord> {
        if !c.is_finite() {
            return None;
        }
        let mut p = (c.x.to_radians(), c.y.clamp(-90.0, 90.0).to_radians(), 0.0);
        proj4rs::transform::transform(&self.wgs84, &self.native, &mut p).ok()?;
        (p.0.is_finite() && p.1.is_finite() && p.0.abs() < LIMIT && p.1.abs() < LIMIT).then(|| Coord::new(p.0, p.1))
    }

    /// Like [`Projector::forward`], but also rejects points that don't map back to themselves
    /// (wrapped or folded parts of a projection, such as the far hemisphere of a polar one).
    pub fn forward_checked(&self, c: Coord) -> Option<Coord> {
        if !self.domain.contains(c) {
            return None;
        }
        let m = self.forward(c)?;
        let back = self.inverse(m)?;
        let dlon = ((back.x - c.x + 540.0).rem_euclid(360.0) - 180.0).abs();
        (dlon < 1e-3 && (back.y - c.y).abs() < 1e-3 || c.y.abs() > 89.99).then_some(m)
    }

    /// Map coordinates to longitude/latitude.
    pub fn inverse(&self, m: Coord) -> Option<Coord> {
        if !m.is_finite() {
            return None;
        }
        let mut p = (m.x, m.y, 0.0);
        proj4rs::transform::transform(&self.native, &self.wgs84, &mut p).ok()?;
        let c = Coord::new(p.0.to_degrees(), p.1.to_degrees());
        (c.is_finite() && c.y.abs() <= 90.0 + 1e-9).then_some(c)
    }

    pub fn to_meter(&self) -> f64 {
        self.to_meter
    }

    pub fn domain(&self) -> Domain {
        self.domain
    }
}

/// The flat map's projection, ready to convert coordinates.
#[derive(Clone, Debug)]
pub enum Flat {
    /// Web Mercator (also the camera space of the globe).
    Mercator,
    Custom(std::sync::Arc<Projector>),
}

impl Flat {
    /// Longitude/latitude to map coordinates.
    pub fn forward(&self, c: Coord) -> Option<Coord> {
        match self {
            Flat::Mercator => c.is_finite().then(|| lonlat_to_merc(c)),
            Flat::Custom(p) => p.forward(c),
        }
    }

    /// For geometry and other bulk conversion: drops points a projection folds or wraps.
    pub fn forward_checked(&self, c: Coord) -> Option<Coord> {
        match self {
            Flat::Mercator => self.forward(c),
            Flat::Custom(p) => p.forward_checked(c),
        }
    }

    /// Map coordinates to longitude/latitude.
    pub fn inverse(&self, m: Coord) -> Option<Coord> {
        match self {
            Flat::Mercator => m.is_finite().then(|| merc_to_lonlat(m)),
            Flat::Custom(p) => p.inverse(m),
        }
    }

    /// Ground metres per map unit at a map point (Mercator stretches with latitude).
    pub fn ground_per_unit(&self, at: Coord) -> f64 {
        match self {
            Flat::Mercator => merc_to_lonlat(at).y.to_radians().cos().abs().max(0.01),
            Flat::Custom(p) => p.to_meter(),
        }
    }

    /// The map-coordinate box around a longitude/latitude extent (from points along its edges and
    /// inside it, so curved projections are covered).
    pub fn extent(&self, e: &Extent) -> Extent {
        let mut out = Extent::EMPTY;
        if e.is_empty() {
            return out;
        }
        let n = 16;
        for i in 0..=n {
            for j in 0..=n {
                let c = Coord::new(e.min_x + e.width() * i as f64 / n as f64, e.min_y + e.height() * j as f64 / n as f64);
                if let Some(m) = self.forward_checked(c) {
                    out.expand(m);
                }
            }
        }
        out
    }
}

thread_local! {
    static PROJECTORS: std::cell::RefCell<std::collections::HashMap<String, Option<std::sync::Arc<Projector>>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

impl Flat {
    /// The flat projection behind a map projection (the globe's camera space is Web Mercator; an
    /// unusable custom code falls back to it). Parsed projections are kept per thread.
    pub fn of(p: &crate::Projection) -> Flat {
        let crate::Projection::Custom(code) = p else { return Flat::Mercator };
        PROJECTORS.with(|cell| {
            let mut map = cell.borrow_mut();
            let entry = map.entry(code.clone()).or_insert_with(|| Projector::new(code).ok().map(std::sync::Arc::new));
            entry.clone().map(Flat::Custom).unwrap_or(Flat::Mercator)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_and_epsg_codes() {
        for (id, _, _) in PRESETS {
            let p = Projector::new(id).unwrap_or_else(|e| panic!("{id}: {e}"));
            let c = Coord::new(10.0, if id.contains("south") { -75.0 } else { 50.0 });
            let m = p.forward(c).unwrap_or_else(|| panic!("{id}"));
            let back = p.inverse(m).unwrap();
            assert!((back.x - c.x).abs() < 1e-6 && (back.y - c.y).abs() < 1e-6, "{id}: {back:?}");
        }
        assert!(Projector::new("EPSG:3035").is_ok());
        assert!(Projector::new("epsg:32633").is_ok());
        assert!(Projector::new("3857").is_ok());
        assert!(Projector::new("EPSG:4326").is_err(), "degrees are not a map projection");
        assert!(Projector::new("EPSG:1").is_err());
        assert!(Projector::new("bogus").is_err());
        assert!(Projector::new("12x").is_err());
    }

    #[test]
    fn folded_points_are_rejected_and_extents_project() {
        let north = Projector::new("northPolar").unwrap();
        assert!(north.forward_checked(Coord::new(0.0, 60.0)).is_some());
        assert!(north.forward_checked(Coord::new(0.0, -80.0)).is_none(), "the far pole");
        assert!(north.forward_checked(Coord::new(120.0, -5.0)).is_none(), "the southern hemisphere");
        let utm = Projector::new("EPSG:32633").unwrap();
        assert_eq!(utm.domain(), Domain::Band { lon0: 15.0, half: 30.0 });
        assert!(utm.forward_checked(Coord::new(15.0, 50.0)).is_some() && utm.forward_checked(Coord::new(-100.0, 40.0)).is_none());
        let usa = Projector::new("usaAlbers").unwrap();
        assert!(usa.forward_checked(Coord::new(-95.0, 38.0)).is_some() && usa.forward_checked(Coord::new(100.0, 30.0)).is_none());
        let moll = Flat::Custom(std::sync::Arc::new(Projector::new("mollweide").unwrap()));
        let e = moll.extent(&Extent::new(-180.0, -90.0, 180.0, 90.0));
        assert!(e.width() > 3.3e7 && e.height() > 1.7e7, "{e:?}");
        assert!((moll.ground_per_unit(Coord::new(0.0, 0.0)) - 1.0).abs() < 1e-12);
        assert!(Flat::Mercator.inverse(Coord::new(f64::NAN, 0.0)).is_none());
    }
}
