//! The globe: the Earth as a sphere seen from space (a near-side perspective projection).
//!
//! The camera sits [`GLOBE_DISTANCE`] Earth radii from the centre, looking at the map camera's
//! centre point. The sphere's radius on screen follows the camera resolution so that the ground
//! scale at the centre matches the flat map's: switching projections keeps the scale.
//!
//! Points are handled as unit vectors in the *view frame*: `x` east and `y` north at the centre
//! of view, `z` towards the viewer. A point is visible when `z` exceeds the horizon value
//! `1 / GLOBE_DISTANCE`.

use rustgis_geom::{Coord, MERCATOR_R};
use serde::{Deserialize, Serialize};

use crate::Camera;

/// Camera distance from the Earth's centre, in Earth radii.
pub const GLOBE_DISTANCE: f64 = 6.0;

/// How a map is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Projection {
    /// The flat Web Mercator map.
    #[default]
    WebMercator,
    /// A 3D globe.
    Globe,
}

impl Projection {
    pub fn code(self) -> &'static str {
        match self {
            Projection::WebMercator => "mercator",
            Projection::Globe => "globe",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Projection::WebMercator => "Web Mercator",
            Projection::Globe => "Globe",
        }
    }
    pub fn from_code(s: &str) -> Option<Projection> {
        match s.trim().to_ascii_lowercase().replace([' ', '_', '-'], "").as_str() {
            "mercator" | "webmercator" | "2d" | "map" => Some(Projection::WebMercator),
            "globe" | "3d" | "scene" | "globalscene" => Some(Projection::Globe),
            _ => None,
        }
    }
}

/// A unit vector.
pub type V3 = [f64; 3];

fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Normalises a vector (unchanged if it is zero).
pub fn normalize(v: V3) -> V3 {
    let n = dot(v, v).sqrt();
    if n > 0.0 && n.is_finite() { [v[0] / n, v[1] / n, v[2] / n] } else { v }
}

/// The globe as seen through a camera in a `w × h` view (logical pixels).
#[derive(Clone, Copy, Debug)]
pub struct Globe {
    /// Earth-frame axes of the view frame.
    east: V3,
    north: V3,
    toward: V3,
    /// Pixels per Earth radius at the centre of view.
    radius: f64,
    cx: f64,
    cy: f64,
    p: f64,
}

impl Globe {
    pub fn new(camera: &Camera, w: f64, h: f64) -> Globe {
        let c = camera.center_lonlat();
        let (lon, lat) = (c.x.to_radians(), c.y.clamp(-89.0, 89.0).to_radians());
        let (sl, cl) = lon.sin_cos();
        let (sp, cp) = lat.sin_cos();
        // Ground metres per pixel at the centre match the flat map's (res × cos lat).
        let ground = (camera.res * cp.max(1e-6)).max(1e-9);
        Globe {
            east: [-sl, cl, 0.0],
            north: [-sp * cl, -sp * sl, cp],
            toward: [cp * cl, cp * sl, sp],
            radius: MERCATOR_R / ground,
            cx: w / 2.0,
            cy: h / 2.0,
            p: GLOBE_DISTANCE,
        }
    }

    /// The horizon: points with a view-frame `z` above this are visible.
    pub fn horizon(&self) -> f64 {
        1.0 / self.p
    }

    /// The centre of the globe on screen.
    pub fn center(&self) -> (f64, f64) {
        (self.cx, self.cy)
    }

    /// The radius of the visible disc (the horizon circle) on screen.
    pub fn disc_radius(&self) -> f64 {
        self.radius * ((self.p - 1.0) / (self.p + 1.0)).sqrt()
    }

    /// Pixels per Earth radius at the centre of view.
    pub fn radius(&self) -> f64 {
        self.radius
    }

    /// A longitude/latitude point as a view-frame unit vector.
    pub fn to_view(&self, c: Coord) -> V3 {
        let (lon, lat) = (c.x.to_radians(), c.y.to_radians());
        let e = [lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()];
        [dot(e, self.east), dot(e, self.north), dot(e, self.toward)]
    }

    /// Screen position of a view-frame vector (whether or not it is visible).
    pub fn project(&self, v: V3) -> (f64, f64) {
        let k = (self.p - 1.0) / (self.p - v[2]).max(1e-9);
        (self.cx + self.radius * k * v[0], self.cy - self.radius * k * v[1])
    }

    /// Screen position of a longitude/latitude point, or `None` behind the horizon.
    pub fn forward(&self, c: Coord) -> Option<(f64, f64)> {
        let v = self.to_view(c);
        (v[2] > self.horizon() && c.is_finite()).then(|| self.project(v))
    }

    /// The longitude/latitude under a screen position, or `None` off the globe.
    pub fn inverse(&self, sx: f64, sy: f64) -> Option<Coord> {
        // The ray from the camera (0, 0, P) through the image-plane point (a, b, 1).
        let (a, b) = ((sx - self.cx) / self.radius, (self.cy - sy) / self.radius);
        let d = [a, b, 1.0 - self.p];
        let qa = dot(d, d);
        let qb = 2.0 * self.p * (1.0 - self.p);
        let qc = self.p * self.p - 1.0;
        let disc = qb * qb - 4.0 * qa * qc;
        if disc < 0.0 || !disc.is_finite() || qa <= 0.0 {
            return None;
        }
        let t = (-qb - disc.sqrt()) / (2.0 * qa);
        let v = [t * a, t * b, self.p + t * (1.0 - self.p)];
        let e = [
            v[0] * self.east[0] + v[1] * self.north[0] + v[2] * self.toward[0],
            v[0] * self.east[1] + v[1] * self.north[1] + v[2] * self.toward[1],
            v[0] * self.east[2] + v[1] * self.north[2] + v[2] * self.toward[2],
        ];
        let c = Coord::new(e[1].atan2(e[0]).to_degrees(), e[2].clamp(-1.0, 1.0).asin().to_degrees());
        c.is_finite().then_some(c)
    }

    /// The point where the segment from visible `a` to hidden `b` (or back) meets the horizon.
    pub fn horizon_crossing(&self, a: V3, b: V3) -> V3 {
        let h = self.horizon();
        let t = ((h - a[2]) / (b[2] - a[2])).clamp(0.0, 1.0);
        let v = normalize([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]);
        // Pull the point exactly onto the horizon circle.
        let r = (1.0 - h * h).sqrt();
        let xy = (v[0] * v[0] + v[1] * v[1]).sqrt().max(1e-12);
        [v[0] / xy * r, v[1] / xy * r, h]
    }

    /// Points along the horizon circle from `a` to `b` (both on it), the shorter way round.
    pub fn horizon_arc(&self, a: V3, b: V3) -> Vec<V3> {
        let (h, r) = (self.horizon(), (1.0 - self.horizon() * self.horizon()).sqrt());
        let t0 = a[1].atan2(a[0]);
        let mut dt = b[1].atan2(b[0]) - t0;
        while dt > std::f64::consts::PI {
            dt -= std::f64::consts::TAU;
        }
        while dt < -std::f64::consts::PI {
            dt += std::f64::consts::TAU;
        }
        let n = ((dt.abs() / 0.05).ceil() as usize).clamp(1, 128);
        (1..n)
            .map(|i| {
                let t = t0 + dt * i as f64 / n as f64;
                [r * t.cos(), r * t.sin(), h]
            })
            .collect()
    }

    /// Rotates the camera as if the globe were dragged by (`dx`, `dy`) pixels.
    pub fn drag(camera: &mut Camera, dx: f64, dy: f64, w: f64, h: f64) {
        let g = Globe::new(camera, w, h);
        let c = camera.center_lonlat();
        let lat = c.y.to_radians().cos().max(0.05);
        let lon = c.x - (dx / g.radius).to_degrees() / lat;
        let lat = (c.y + (dy / g.radius).to_degrees()).clamp(-85.0, 85.0);
        let lon = (lon + 540.0).rem_euclid(360.0) - 180.0;
        camera.center = rustgis_geom::lonlat_to_merc(Coord::new(lon, lat));
        camera.clamp();
    }
}

impl Globe {
    /// Points the camera at a longitude/latitude extent: centred on it, with the whole globe in
    /// view when the extent spans most of a hemisphere, otherwise zoomed as the flat map would.
    pub fn fit(camera: &mut Camera, e: &rustgis_geom::Extent, w: f64, h: f64) {
        if e.is_empty() || w <= 1.0 || h <= 1.0 {
            return;
        }
        if e.width() > 100.0 || e.height() > 70.0 {
            let c = e.center();
            let lat = c.y.clamp(-60.0, 60.0);
            camera.center = rustgis_geom::lonlat_to_merc(Coord::new(c.x, lat));
            // Disc radius ≈ 44% of the shorter side.
            let disc = 0.44 * w.min(h);
            let radius = disc / ((GLOBE_DISTANCE - 1.0) / (GLOBE_DISTANCE + 1.0)).sqrt();
            camera.res = MERCATOR_R / radius / lat.to_radians().cos().max(0.05);
            camera.clamp();
        } else {
            camera.fit_lonlat(e, w, h);
        }
    }
}

/// Great-circle points between `a` and `b` (view-frame unit vectors), at most `step` radians
/// apart, excluding `a` and including `b`.
pub fn densify(a: V3, b: V3, step: f64) -> Vec<V3> {
    let angle = dot(a, b).clamp(-1.0, 1.0).acos();
    if !angle.is_finite() || angle <= step {
        return vec![b];
    }
    let n = ((angle / step).ceil() as usize).clamp(1, 360);
    (1..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            normalize([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use rustgis_geom::lonlat_to_merc;

    use super::*;

    fn at(lon: f64, lat: f64, res: f64) -> Globe {
        Globe::new(&Camera { center: lonlat_to_merc(Coord::new(lon, lat)), res }, 800.0, 600.0)
    }

    #[test]
    fn forward_inverse_round_trip() {
        let g = at(10.0, 45.0, 20_000.0);
        assert_eq!(g.forward(Coord::new(10.0, 45.0)), Some((400.0, 300.0)));
        for (lon, lat) in [(10.0, 45.0), (30.0, 60.0), (-20.0, 10.0), (60.0, 30.0)] {
            let (x, y) = g.forward(Coord::new(lon, lat)).unwrap();
            let back = g.inverse(x, y).unwrap();
            assert!((back.x - lon).abs() < 1e-6 && (back.y - lat).abs() < 1e-6, "{lon},{lat} -> {back:?}");
        }
        // The far side is hidden; points off the disc have no position.
        assert_eq!(g.forward(Coord::new(-170.0, -45.0)), None);
        assert_eq!(g.inverse(400.0 + g.disc_radius() + 2.0, 300.0), None);
        assert!(g.inverse(400.0 + g.disc_radius() - 2.0, 300.0).is_some());
    }

    #[test]
    fn north_is_up_and_east_is_right() {
        let g = at(0.0, 0.0, 20_000.0);
        let (x, y) = g.forward(Coord::new(5.0, 5.0)).unwrap();
        assert!(x > 400.0 && y < 300.0);
        // Ground scale at the centre matches the flat map's.
        let (x1, _) = g.forward(Coord::new(0.01, 0.0)).unwrap();
        let metres = 0.01_f64.to_radians() * MERCATOR_R;
        assert!(((x1 - 400.0) - metres / 20_000.0).abs() < 1e-3);
    }

    #[test]
    fn drag_rotates_and_degenerate_input_is_safe() {
        let mut cam = Camera { center: lonlat_to_merc(Coord::new(0.0, 0.0)), res: 20_000.0 };
        Globe::drag(&mut cam, -100.0, 0.0, 800.0, 600.0);
        assert!(cam.center_lonlat().x > 0.0, "dragging left turns the globe east");
        Globe::drag(&mut cam, f64::NAN, 1e12, 800.0, 600.0);
        let g = Globe::new(&Camera { center: Coord::new(f64::NAN, 0.0), res: 0.0 }, 0.0, 0.0);
        let _ = g.inverse(1.0, 1.0);
        let _ = g.forward(Coord::new(f64::NAN, f64::INFINITY));
        assert_eq!(densify([1.0, 0.0, 0.0], [1.0, 0.0, 0.0], 0.1).len(), 1);
        assert_eq!(densify([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], 0.1).len(), 16);
    }
}
