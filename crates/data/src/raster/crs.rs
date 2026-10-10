//! Coordinate reference systems for rasters, by EPSG code.
//!
//! WGS 84 longitude/latitude and Web Mercator are handled directly; any other EPSG code is
//! looked up in a built-in table of PROJ definitions and transformed with a pure-Rust PROJ port,
//! so the browser build works too.

use rustgis_geom::{Coord, lonlat_to_merc, merc_to_lonlat};
use serde::{Deserialize, Serialize};

/// A raster's coordinate system.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Crs {
    /// WGS 84 longitude/latitude in degrees (EPSG:4326).
    LonLat,
    /// Web Mercator metres (EPSG:3857).
    WebMercator,
    /// Any other EPSG code.
    Epsg(u16),
}

impl Crs {
    /// The coordinate system for an EPSG code, or `None` when it isn't known.
    pub fn from_epsg(code: u32) -> Option<Crs> {
        match code {
            4326 => Some(Crs::LonLat),
            3857 | 900_913 | 102_100 | 102_113 | 3785 => Some(Crs::WebMercator),
            c => {
                let c = u16::try_from(c).ok()?;
                crs_definitions::from_code(c).map(|_| Crs::Epsg(c))
            }
        }
    }

    /// The EPSG code.
    pub fn epsg(self) -> u16 {
        match self {
            Crs::LonLat => 4326,
            Crs::WebMercator => 3857,
            Crs::Epsg(c) => c,
        }
    }

    /// `EPSG:32645`.
    pub fn label(self) -> String {
        format!("EPSG:{}", self.epsg())
    }

    /// Whether coordinates are in degrees.
    pub fn is_geographic(self) -> bool {
        match self {
            Crs::LonLat => true,
            Crs::WebMercator => false,
            Crs::Epsg(c) => crs_definitions::from_code(c).is_some_and(|d| d.proj4.contains("+proj=longlat")),
        }
    }

    /// A transformer between this system and WGS 84 longitude/latitude.
    pub fn transformer(self) -> Result<Transformer, String> {
        Ok(Transformer {
            kind: match self {
                Crs::LonLat => Kind::LonLat,
                Crs::WebMercator => Kind::Merc,
                Crs::Epsg(c) => {
                    let def = crs_definitions::from_code(c).ok_or_else(|| format!("unknown coordinate system EPSG:{c}"))?;
                    let native = proj4rs::Proj::from_proj_string(def.proj4).map_err(|e| format!("EPSG:{c}: {e}"))?;
                    let wgs84 = proj4rs::Proj::from_proj_string("+proj=longlat +datum=WGS84 +no_defs").map_err(|e| e.to_string())?;
                    Kind::Proj { native: Box::new(native), wgs84: Box::new(wgs84), geographic: self.is_geographic() }
                }
            },
        })
    }
}

#[derive(Clone)]
enum Kind {
    LonLat,
    Merc,
    Proj { native: Box<proj4rs::Proj>, wgs84: Box<proj4rs::Proj>, geographic: bool },
}

/// Converts between a raster's coordinates and WGS 84 longitude/latitude.
#[derive(Clone)]
pub struct Transformer {
    kind: Kind,
}

impl std::fmt::Debug for Transformer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let k = match self.kind {
            Kind::LonLat => "lonlat",
            Kind::Merc => "mercator",
            Kind::Proj { .. } => "proj",
        };
        write!(f, "Transformer({k})")
    }
}

impl Transformer {
    /// Native coordinates to longitude/latitude.
    pub fn to_lonlat(&self, c: Coord) -> Option<Coord> {
        let out = match &self.kind {
            Kind::LonLat => c,
            Kind::Merc => merc_to_lonlat(c),
            Kind::Proj { native, wgs84, geographic } => {
                let mut p = if *geographic { (c.x.to_radians(), c.y.to_radians(), 0.0) } else { (c.x, c.y, 0.0) };
                proj4rs::transform::transform(native, wgs84, &mut p).ok()?;
                Coord::new(p.0.to_degrees(), p.1.to_degrees())
            }
        };
        out.is_finite().then_some(out)
    }

    /// Longitude/latitude to native coordinates.
    pub fn from_lonlat(&self, c: Coord) -> Option<Coord> {
        let out = match &self.kind {
            Kind::LonLat => c,
            Kind::Merc => lonlat_to_merc(c),
            Kind::Proj { native, wgs84, geographic } => {
                let mut p = (c.x.to_radians(), c.y.to_radians(), 0.0);
                proj4rs::transform::transform(wgs84, native, &mut p).ok()?;
                if *geographic { Coord::new(p.0.to_degrees(), p.1.to_degrees()) } else { Coord::new(p.0, p.1) }
            }
        };
        out.is_finite().then_some(out)
    }

    /// Web Mercator to native coordinates (the renderer's direction).
    pub fn from_merc(&self, c: Coord) -> Option<Coord> {
        match &self.kind {
            Kind::Merc => Some(c),
            _ => self.from_lonlat(merc_to_lonlat(c)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utm_round_trip() {
        let crs = Crs::from_epsg(32617).unwrap();
        assert!(!crs.is_geographic());
        let t = crs.transformer().unwrap();
        let ll = t.to_lonlat(Coord::new(500_000.0, 4_649_776.22)).unwrap();
        assert!((ll.x + 81.0).abs() < 1e-6, "{ll:?}");
        assert!((ll.y - 42.0).abs() < 1e-4, "{ll:?}");
        let back = t.from_lonlat(ll).unwrap();
        assert!((back.x - 500_000.0).abs() < 0.01 && (back.y - 4_649_776.22).abs() < 0.01);
    }

    #[test]
    fn known_systems() {
        assert_eq!(Crs::from_epsg(4326), Some(Crs::LonLat));
        assert_eq!(Crs::from_epsg(3857), Some(Crs::WebMercator));
        assert_eq!(Crs::from_epsg(1), None);
        assert_eq!(Crs::from_epsg(u32::MAX), None);
        let m = Crs::WebMercator.transformer().unwrap();
        let p = m.from_lonlat(Coord::new(10.0, 20.0)).unwrap();
        let q = m.to_lonlat(p).unwrap();
        assert!((q.x - 10.0).abs() < 1e-9 && (q.y - 20.0).abs() < 1e-9);
    }
}
