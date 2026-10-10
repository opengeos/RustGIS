//! L0: coordinates, extents, the Web Mercator projection, geodesic measurement and map scale.
//!
//! Everything here is plain math with no I/O. Longitude/latitude are WGS 84 degrees; projected
//! coordinates are Web Mercator metres (EPSG:3857).
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use serde::{Deserialize, Serialize};

/// Mean Earth radius used by the haversine formula (metres).
pub const EARTH_RADIUS_M: f64 = 6_371_008.8;
/// Web Mercator sphere radius (metres).
pub const MERCATOR_R: f64 = 6_378_137.0;
/// Latitude limit of Web Mercator.
pub const MAX_LAT: f64 = 85.051_128_78;
/// Screen pixel size assumed when converting resolution to a map scale (0.28 mm, OGC).
pub const PIXEL_M: f64 = 0.000_28;

/// A 2-D coordinate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Coord {
    pub x: f64,
    pub y: f64,
}

impl Coord {
    pub const fn new(x: f64, y: f64) -> Self {
        Coord { x, y }
    }
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// An axis-aligned rectangle. An "empty" extent has `min > max`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Extent {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl Default for Extent {
    fn default() -> Self {
        Extent::EMPTY
    }
}

impl Extent {
    pub const EMPTY: Extent = Extent { min_x: f64::INFINITY, min_y: f64::INFINITY, max_x: f64::NEG_INFINITY, max_y: f64::NEG_INFINITY };
    /// The whole world in longitude/latitude.
    pub const WORLD_LONLAT: Extent = Extent { min_x: -180.0, min_y: -85.0, max_x: 180.0, max_y: 85.0 };

    pub const fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Self {
        Extent { min_x, min_y, max_x, max_y }
    }
    pub fn from_points(a: Coord, b: Coord) -> Self {
        Extent { min_x: a.x.min(b.x), min_y: a.y.min(b.y), max_x: a.x.max(b.x), max_y: a.y.max(b.y) }
    }
    pub fn is_empty(&self) -> bool {
        !(self.min_x <= self.max_x && self.min_y <= self.max_y)
    }
    pub fn width(&self) -> f64 {
        if self.is_empty() { 0.0 } else { self.max_x - self.min_x }
    }
    pub fn height(&self) -> f64 {
        if self.is_empty() { 0.0 } else { self.max_y - self.min_y }
    }
    pub fn center(&self) -> Coord {
        Coord::new((self.min_x + self.max_x) / 2.0, (self.min_y + self.max_y) / 2.0)
    }
    pub fn expand(&mut self, c: Coord) {
        if !c.is_finite() {
            return;
        }
        self.min_x = self.min_x.min(c.x);
        self.min_y = self.min_y.min(c.y);
        self.max_x = self.max_x.max(c.x);
        self.max_y = self.max_y.max(c.y);
    }
    pub fn union(&self, o: &Extent) -> Extent {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        Extent::new(self.min_x.min(o.min_x), self.min_y.min(o.min_y), self.max_x.max(o.max_x), self.max_y.max(o.max_y))
    }
    pub fn intersects(&self, o: &Extent) -> bool {
        !self.is_empty()
            && !o.is_empty()
            && self.min_x <= o.max_x
            && o.min_x <= self.max_x
            && self.min_y <= o.max_y
            && o.min_y <= self.max_y
    }
    pub fn contains(&self, c: Coord) -> bool {
        c.x >= self.min_x && c.x <= self.max_x && c.y >= self.min_y && c.y <= self.max_y
    }
    /// Grows every side by `f` of the size (0.1 = 10 %), with a minimum pad for points.
    pub fn padded(&self, f: f64, min_pad: f64) -> Extent {
        if self.is_empty() {
            return *self;
        }
        let px = (self.width() * f).max(min_pad);
        let py = (self.height() * f).max(min_pad);
        Extent::new(self.min_x - px, self.min_y - py, self.max_x + px, self.max_y + py)
    }
}

/// Longitude/latitude (degrees) → Web Mercator metres. Latitude is clamped to ±85.05°.
pub fn lonlat_to_merc(c: Coord) -> Coord {
    let lat = c.y.clamp(-MAX_LAT, MAX_LAT).to_radians();
    Coord::new(MERCATOR_R * c.x.to_radians(), MERCATOR_R * (std::f64::consts::FRAC_PI_4 + lat / 2.0).tan().ln())
}

/// Web Mercator metres → longitude/latitude (degrees).
pub fn merc_to_lonlat(c: Coord) -> Coord {
    let lon = (c.x / MERCATOR_R).to_degrees();
    let lat = (2.0 * (c.y / MERCATOR_R).exp().atan() - std::f64::consts::FRAC_PI_2).to_degrees();
    Coord::new(lon, lat)
}

/// Projects a lon/lat extent to Web Mercator.
pub fn extent_to_merc(e: &Extent) -> Extent {
    if e.is_empty() {
        return *e;
    }
    Extent::from_points(lonlat_to_merc(Coord::new(e.min_x, e.min_y)), lonlat_to_merc(Coord::new(e.max_x, e.max_y)))
}

/// Great-circle distance between two lon/lat points (metres).
pub fn haversine_m(a: Coord, b: Coord) -> f64 {
    let (la1, la2) = (a.y.to_radians(), b.y.to_radians());
    let dlat = la2 - la1;
    let dlon = (b.x - a.x).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * h.sqrt().min(1.0).asin()
}

/// Length of a lon/lat polyline along great circles (metres).
pub fn geodesic_length_m(pts: &[Coord]) -> f64 {
    pts.windows(2)
        .map(|w| match w {
            [a, b] => haversine_m(*a, *b),
            _ => 0.0,
        })
        .sum()
}

/// Area of a lon/lat ring on the sphere (square metres, always positive).
///
/// Uses the spherical-excess approximation from Chamberlain & Duquette (JPL, 2007).
pub fn geodesic_ring_area_m2(ring: &[Coord]) -> f64 {
    let n = ring.len();
    if n < 3 {
        return 0.0;
    }
    let mut total = 0.0;
    for i in 0..n {
        let (Some(p1), Some(p2)) = (ring.get(i), ring.get((i + 1) % n)) else { continue };
        total += (p2.x - p1.x).to_radians() * (2.0 + p1.y.to_radians().sin() + p2.y.to_radians().sin());
    }
    (total * EARTH_RADIUS_M * EARTH_RADIUS_M / 2.0).abs()
}

/// Map scale denominator (1:N) for a Web Mercator resolution (metres per screen pixel) at a latitude.
pub fn scale_denominator(merc_m_per_px: f64, lat_deg: f64) -> f64 {
    merc_m_per_px * lat_deg.to_radians().cos().abs() / PIXEL_M
}

/// Rounds a distance down to a "nice" 1, 2 or 5 × 10ⁿ for scale bars.
pub fn nice_distance(d: f64) -> f64 {
    if d <= 0.0 || !d.is_finite() {
        return 0.0;
    }
    let p = 10f64.powf(d.log10().floor());
    let f = d / p;
    let n = if f >= 5.0 {
        5.0
    } else if f >= 2.0 {
        2.0
    } else {
        1.0
    };
    n * p
}

/// Formats a distance in metres as `850 m` / `12.5 km`.
pub fn format_distance(m: f64) -> String {
    if m >= 1000.0 {
        let km = m / 1000.0;
        if km >= 100.0 { format!("{km:.0} km") } else { format!("{km:.1} km") }
    } else {
        format!("{m:.0} m")
    }
}

/// Formats an area in square metres as `km²` or `m²`.
pub fn format_area(m2: f64) -> String {
    if m2 >= 1.0e6 { format!("{:.1} km²", m2 / 1.0e6) } else { format!("{m2:.0} m²") }
}

/// Formats a lon/lat pair in decimal degrees with hemispheres: `12.345° E 45.678° N`.
pub fn format_lonlat(c: Coord) -> String {
    let ew = if c.x < 0.0 { 'W' } else { 'E' };
    let ns = if c.y < 0.0 { 'S' } else { 'N' };
    format!("{:.4}° {ew}  {:.4}° {ns}", c.x.abs(), c.y.abs())
}

/// Formats a lon/lat pair as degrees-minutes-seconds.
pub fn format_dms(c: Coord) -> String {
    fn dms(v: f64) -> String {
        let a = v.abs();
        let d = a.floor();
        let m = ((a - d) * 60.0).floor();
        let s = (a - d - m / 60.0) * 3600.0;
        format!("{d:.0}°{m:02.0}′{s:04.1}″")
    }
    let ew = if c.x < 0.0 { 'W' } else { 'E' };
    let ns = if c.y < 0.0 { 'S' } else { 'N' };
    format!("{} {ew}  {} {ns}", dms(c.x), dms(c.y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mercator_roundtrip() {
        for (x, y) in [(0.0, 0.0), (12.5, 41.9), (-122.4, 37.8), (179.0, -60.0)] {
            let m = lonlat_to_merc(Coord::new(x, y));
            let back = merc_to_lonlat(m);
            assert!((back.x - x).abs() < 1e-9 && (back.y - y).abs() < 1e-9);
        }
    }

    #[test]
    fn haversine_paris_london() {
        let d = haversine_m(Coord::new(2.3522, 48.8566), Coord::new(-0.1276, 51.5072));
        assert!((d - 343_500.0).abs() < 2_000.0, "{d}");
    }

    #[test]
    fn ring_area_one_degree_square_at_equator() {
        let ring = [Coord::new(0.0, 0.0), Coord::new(1.0, 0.0), Coord::new(1.0, 1.0), Coord::new(0.0, 1.0)];
        let a = geodesic_ring_area_m2(&ring);
        assert!((a - 1.2364e10).abs() / 1.2364e10 < 0.01, "{a}");
    }

    #[test]
    fn nice_distances() {
        assert_eq!(nice_distance(730.0), 500.0);
        assert_eq!(nice_distance(2_400.0), 2_000.0);
        assert_eq!(nice_distance(0.0), 0.0);
        assert_eq!(nice_distance(f64::NAN), 0.0);
    }

    #[test]
    fn extent_ops() {
        let mut e = Extent::EMPTY;
        assert!(e.is_empty());
        e.expand(Coord::new(1.0, 2.0));
        e.expand(Coord::new(f64::NAN, 0.0));
        e.expand(Coord::new(-1.0, 5.0));
        assert_eq!(e, Extent::new(-1.0, 2.0, 1.0, 5.0));
        assert!(e.intersects(&Extent::new(0.0, 0.0, 3.0, 3.0)));
        assert!(!e.intersects(&Extent::EMPTY));
    }
}
