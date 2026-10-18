//! The map camera: a Web Mercator centre, a resolution (metres per logical pixel), a bearing
//! and a pitch.

use rustgis_geom::{Coord, Extent, MERCATOR_R, extent_to_merc, merc_to_lonlat, scale_denominator};
use serde::{Deserialize, Serialize};

/// Resolution limits (metres per pixel): about 1:500 to the whole world.
pub const MIN_RES: f64 = 0.15;
pub const MAX_RES: f64 = 160_000.0;

/// The steepest pitch (degrees from looking straight down), as MapLibre allows.
pub const MAX_PITCH: f64 = 85.0;
/// The tangent of half the vertical field of view: MapLibre's 36.87°. The horizon comes into
/// view above about 71.6° of pitch.
const TAN_HALF_FOV: f64 = 1.0 / 3.0;
/// How far a tilted view reaches, as a multiple of the camera's distance from the centre: past
/// it (a sixteenth of the centre's scale) is sky.
pub const FAR: f64 = 16.0;
/// The nearest a point can be drawn, as a fraction of the camera's distance from the centre.
const NEAR: f64 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    pub center: Coord,
    pub res: f64,
    /// The compass direction the top of the view faces, in degrees clockwise from north (as in
    /// MapLibre): at 90 east is up. The globe ignores it.
    #[serde(default)]
    pub bearing: f64,
    /// How far the view tilts from looking straight down, in degrees (0 to [`MAX_PITCH`], as in
    /// MapLibre): the centre keeps its scale, nearer ground grows and farther ground shrinks
    /// toward the horizon. The globe ignores it.
    #[serde(default)]
    pub pitch: f64,
}

/// A tilted view's perspective: the pitch's cosine and sine, and the camera's distance from the
/// view's centre in logical px.
#[derive(Clone, Copy)]
struct Tilt {
    c: f64,
    s: f64,
    d: f64,
}

impl Camera {
    pub fn new(center: Coord, res: f64) -> Self {
        Camera { center, res, bearing: 0.0, pitch: 0.0 }
    }

    /// The perspective of a view `h` px tall, `None` when it looks straight down.
    fn tilt(&self, h: f64) -> Option<Tilt> {
        if self.pitch == 0.0 || !self.pitch.is_finite() || !(h.is_finite() && h > 0.0) {
            return None;
        }
        let p = self.pitch.clamp(0.0, MAX_PITCH).to_radians();
        Some(Tilt { c: p.cos(), s: p.sin(), d: h / 2.0 / TAN_HALF_FOV })
    }

    /// Ground offsets from the centre (logical px at the centre's scale, x right and y down in
    /// the view turned to the bearing) to the screen, `None` behind the camera.
    fn project(&self, u: f64, v: f64, w: f64, h: f64) -> Option<(f64, f64)> {
        match self.tilt(h) {
            None => Some((u + w / 2.0, v + h / 2.0)),
            Some(t) => {
                let z = t.d - v * t.s;
                (z > t.d * NEAR).then(|| (w / 2.0 + u * t.d / z, h / 2.0 + v * t.c * t.d / z))
            }
        }
    }

    /// A screen point's ground offsets from the centre, `None` above the horizon or past the
    /// view's reach ([`FAR`]).
    fn unproject(&self, sx: f64, sy: f64, w: f64, h: f64) -> Option<(f64, f64)> {
        let (a, b) = (sx - w / 2.0, sy - h / 2.0);
        let Some(t) = self.tilt(h) else { return Some((a, b)) };
        let den = t.d * t.c + b * t.s;
        if den <= 0.0 {
            return None;
        }
        let v = b * t.d / den;
        let z = t.d - v * t.s;
        (z <= t.d * FAR).then(|| (a * z / t.d, v))
    }

    /// The ground offsets at the view's reach straight up from a screen point (where a point
    /// above the horizon or past the reach is taken to be).
    fn far_point(&self, sx: f64, w: f64, h: f64) -> (f64, f64) {
        let a = sx - w / 2.0;
        match self.tilt(h) {
            Some(t) if t.s > 0.0 => (a * FAR, t.d * (1.0 - FAR) / t.s),
            _ => (a, -h / 2.0),
        }
    }

    /// The ground offsets (see [`Camera::project`]) of a Mercator point.
    fn ground(&self, p: Coord) -> (f64, f64) {
        let (ux, uy) = ((p.x - self.center.x) / self.res, (self.center.y - p.y) / self.res);
        let (c, s) = self.rotation();
        (ux * c + uy * s, uy * c - ux * s)
    }

    /// The Mercator point at ground offsets from the centre.
    fn ground_to_map(&self, u: f64, v: f64) -> Coord {
        let (ux, uy) = self.unrotate(u, v);
        Coord::new(self.center.x + ux * self.res, self.center.y - uy * self.res)
    }

    /// The screen y (logical px) above which a tilted `h` px tall view shows sky: where its reach
    /// ([`FAR`]) ends, or the horizon. `None` when no sky shows.
    pub fn sky_y(&self, h: f64) -> Option<f64> {
        let t = self.tilt(h)?;
        let (_, v) = self.far_point(0.0, 0.0, h);
        let z = t.d - v * t.s;
        let y = h / 2.0 + v * t.c * t.d / z;
        (y > 0.0).then_some(y)
    }

    /// How much nearer (above 1) or farther (below 1) than the centre a Mercator point is in a
    /// tilted view: its scale relative to the centre's. 1 in a flat view; `None` behind the
    /// camera.
    pub fn depth_scale(&self, p: Coord, h: f64) -> Option<f64> {
        let Some(t) = self.tilt(h) else { return Some(1.0) };
        let (_, v) = self.ground(p);
        let z = t.d - v * t.s;
        (z > t.d * NEAR).then(|| t.d / z)
    }

    pub fn world() -> Self {
        Camera::new(Coord::new(0.0, 2_000_000.0), 30_000.0)
    }

    /// The bearing's cosine and sine (exactly 1 and 0 when the map faces north).
    fn rotation(&self) -> (f64, f64) {
        if self.bearing == 0.0 || !self.bearing.is_finite() {
            return (1.0, 0.0);
        }
        let b = self.bearing.to_radians();
        (b.cos(), b.sin())
    }

    /// Screen position (logical px, origin top-left) of a Mercator point in a `w`×`h` view. In a
    /// tilted view a point behind the camera is put just in front of it (far off screen); see
    /// [`Camera::try_to_screen`].
    pub fn to_screen(&self, p: Coord, w: f64, h: f64) -> (f64, f64) {
        let (u, v) = self.ground(p);
        if let Some(s) = self.project(u, v, w, h) {
            return s;
        }
        match self.tilt(h) {
            Some(t) => {
                let z = t.d * NEAR;
                (w / 2.0 + u * t.d / z, h / 2.0 + v * t.c * t.d / z)
            }
            None => (u + w / 2.0, v + h / 2.0),
        }
    }

    /// Screen position of a Mercator point, `None` behind the camera of a tilted view.
    pub fn try_to_screen(&self, p: Coord, w: f64, h: f64) -> Option<(f64, f64)> {
        let (u, v) = self.ground(p);
        self.project(u, v, w, h)
    }

    /// The Mercator point under a screen position. In a tilted view a point above the horizon
    /// (or past the view's reach) is taken at the reach straight below it; see
    /// [`Camera::try_to_map`].
    pub fn to_map(&self, sx: f64, sy: f64, w: f64, h: f64) -> Coord {
        let (u, v) = self.unproject(sx, sy, w, h).unwrap_or_else(|| self.far_point(sx, w, h));
        self.ground_to_map(u, v)
    }

    /// The Mercator point under a screen position, `None` where a tilted view shows sky.
    pub fn try_to_map(&self, sx: f64, sy: f64, w: f64, h: f64) -> Option<Coord> {
        self.unproject(sx, sy, w, h).map(|(u, v)| self.ground_to_map(u, v))
    }

    /// A screen offset (logical px) turned back to the map's north-up axes.
    fn unrotate(&self, dx: f64, dy: f64) -> (f64, f64) {
        let (c, s) = self.rotation();
        (dx * c - dy * s, dx * s + dy * c)
    }

    /// Mercator extent visible in a `w`×`h` view (around a rotated view, the extent of its
    /// corners; in a tilted one, of the ground it shows up to its reach).
    pub fn extent(&self, w: f64, h: f64) -> Extent {
        if self.tilt(h).is_some() {
            let mut e = Extent::EMPTY;
            for (sx, sy) in [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)] {
                e.expand(self.to_map(sx, sy, w, h));
            }
            return e;
        }
        let (hw, hh) = self.rotated_size(w, h);
        Extent::new(
            self.center.x - hw / 2.0 * self.res,
            self.center.y - hh / 2.0 * self.res,
            self.center.x + hw / 2.0 * self.res,
            self.center.y + hh / 2.0 * self.res,
        )
    }

    /// The size (logical px) of the north-up box around a `w`×`h` view turned to the bearing.
    pub fn rotated_size(&self, w: f64, h: f64) -> (f64, f64) {
        let (c, s) = self.rotation();
        let (c, s) = (c.abs(), s.abs());
        (w * c + h * s, w * s + h * c)
    }

    /// Sets the bearing, normalised to (-180, 180]; non-finite bearings face north.
    pub fn set_bearing(&mut self, degrees: f64) {
        self.bearing = degrees;
        self.clamp();
    }

    /// Sets the pitch, within 0 to [`MAX_PITCH`]; a non-finite pitch looks straight down.
    pub fn set_pitch(&mut self, degrees: f64) {
        self.pitch = degrees;
        self.clamp();
    }

    pub fn clamp(&mut self) {
        if !self.res.is_finite() {
            self.res = 30_000.0;
        }
        self.bearing = normalize_bearing(self.bearing);
        self.pitch = if self.pitch.is_finite() { self.pitch.clamp(0.0, MAX_PITCH) } else { 0.0 };
        self.res = self.res.clamp(MIN_RES, MAX_RES);
        let lim = std::f64::consts::PI * MERCATOR_R;
        if !self.center.is_finite() {
            self.center = Coord::new(0.0, 0.0);
        }
        self.center.x = self.center.x.clamp(-lim * 1.5, lim * 1.5);
        self.center.y = self.center.y.clamp(-lim, lim);
    }

    /// Zooms by `factor` (>1 zooms in) keeping the screen point (sx, sy) fixed.
    pub fn zoom_at(&mut self, factor: f64, sx: f64, sy: f64, w: f64, h: f64) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        // Over the sky of a tilted view, about the centre.
        let (sx, sy) = if self.try_to_map(sx, sy, w, h).is_some() { (sx, sy) } else { (w / 2.0, h / 2.0) };
        let anchor = self.to_map(sx, sy, w, h);
        self.res /= factor;
        self.clamp();
        let after = self.to_map(sx, sy, w, h);
        self.center.x += anchor.x - after.x;
        self.center.y += anchor.y - after.y;
        self.clamp();
    }

    /// Moves the map by a screen offset, as dragging it at the view's centre would (in a tilted
    /// view the ground there is foreshortened up and down).
    pub fn pan_px(&mut self, dx: f64, dy: f64) {
        let dy = match self.tilt(1.0) {
            Some(t) => dy / t.c.max(0.05),
            None => dy,
        };
        let (ux, uy) = self.unrotate(dx, dy);
        self.center.x -= ux * self.res;
        self.center.y += uy * self.res;
        self.clamp();
    }

    /// Moves the map so the ground under the screen point `from` comes under `to`: dragging it
    /// with the pointer. Off the ground (the sky of a tilted view) it pans as at the centre.
    pub fn drag_px(&mut self, from: (f64, f64), to: (f64, f64), w: f64, h: f64) {
        match (self.try_to_map(from.0, from.1, w, h), self.try_to_map(to.0, to.1, w, h)) {
            (Some(a), Some(b)) if self.tilt(h).is_some() => {
                self.center.x += a.x - b.x;
                self.center.y += a.y - b.y;
                self.clamp();
            }
            _ => self.pan_px(to.0 - from.0, to.1 - from.1),
        }
    }

    /// The ground a tilted `w`×`h` view shows, cut into flat, north-up bands to draw: nearest
    /// first, each reaching twice as far as the last and drawn at the scale of its near edge, so
    /// the ground is as sharp as the screen shows it and far bands are drawn (and their tiles
    /// fetched) coarser. A flat view is one band: itself.
    pub fn tilt_bands(&self, w: f64, h: f64) -> Vec<Band> {
        let Some(t) = self.tilt(h) else {
            return vec![Band { camera: *self, width: w, height: h }];
        };
        let z_of = |v: f64| t.d - v * t.s;
        let v_of = |z: f64| (t.d - z) / t.s;
        let z_near = self.unproject(w / 2.0, h, w, h).map_or(t.d, |(_, v)| z_of(v)).max(t.d * NEAR);
        let z_far = self.unproject(w / 2.0, 0.0, w, h).map_or(t.d * FAR, |(_, v)| z_of(v)).min(t.d * FAR);
        let mut bands = Vec::new();
        let mut z0 = z_near;
        while z0 < z_far * 0.999 && bands.len() < 8 {
            let mut z1 = (z0 * 2.0).min(z_far);
            if z_far / z1 < 1.25 {
                z1 = z_far;
            }
            // Ground px per band px: the scale at the band's near edge.
            let k = z0 / t.d;
            let pad = 2.0 * k;
            let (hw0, hw1) = (w / 2.0 * z0 / t.d + pad, w / 2.0 * z1 / t.d + pad);
            let (v0, v1) = (v_of(z0) + pad, v_of(z1) - pad);
            let mut e = Extent::EMPTY;
            for (u, v) in [(-hw1, v1), (hw1, v1), (-hw0, v0), (hw0, v0)] {
                e.expand(self.ground_to_map(u, v));
            }
            let mut res = self.res * k;
            // No band past a texture's size: coarser instead.
            let side = (e.width().max(e.height()) / res).max(1.0);
            if side > MAX_BAND_SIDE {
                res *= side / MAX_BAND_SIDE;
            }
            let camera = Camera { center: e.center(), res, bearing: 0.0, pitch: 0.0 };
            bands.push(Band { camera, width: (e.width() / res).ceil().max(1.0), height: (e.height() / res).ceil().max(1.0) });
            z0 = z1;
        }
        bands
    }

    /// Fits a lon/lat extent into a `w`×`h` view with a margin.
    pub fn fit_lonlat(&mut self, e: &Extent, w: f64, h: f64) {
        if e.is_empty() || w <= 1.0 || h <= 1.0 {
            return;
        }
        // A lone point gets a neighbourhood; real extents (a small raster) just a margin.
        let min_pad = if e.width() > 0.0 || e.height() > 0.0 { 1e-6 } else { 0.02 };
        self.fit_extent(&extent_to_merc(&e.padded(0.06, min_pad)), w, h);
    }

    /// Fits an extent in the camera's coordinates into a `w`×`h` view (turned to the bearing).
    pub fn fit_extent(&mut self, m: &Extent, w: f64, h: f64) {
        let (c, s) = self.rotation();
        let (c, s) = (c.abs(), s.abs());
        self.center = m.center();
        self.res = ((m.width() * c + m.height() * s) / w).max((m.width() * s + m.height() * c) / h).max(MIN_RES * 2.0);
        self.clamp();
    }

    /// Map scale denominator at the view centre.
    pub fn scale(&self) -> f64 {
        scale_denominator(self.res, merc_to_lonlat(self.center).y)
    }

    /// Sets the resolution from a scale denominator.
    pub fn set_scale(&mut self, scale: f64) {
        if !scale.is_finite() || scale <= 0.0 {
            return;
        }
        let lat = merc_to_lonlat(self.center).y.to_radians().cos().abs().max(0.01);
        self.res = scale * rustgis_geom::PIXEL_M / lat;
        self.clamp();
    }

    pub fn center_lonlat(&self) -> Coord {
        merc_to_lonlat(self.center)
    }
}

/// The longest side (logical px) of a band of a tilted view.
const MAX_BAND_SIDE: f64 = 4096.0;

/// A flat, north-up part of a tilted view's ground ([`Camera::tilt_bands`]): drawn with its
/// camera at `width`×`height` logical px.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub camera: Camera,
    pub width: f64,
    pub height: f64,
}

/// A bearing in (-180, 180]; a non-finite one is north (0).
pub fn normalize_bearing(degrees: f64) -> f64 {
    if !degrees.is_finite() {
        return 0.0;
    }
    let b = degrees.rem_euclid(360.0);
    if b > 180.0 { b - 360.0 } else { b }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_roundtrip_and_zoom_anchor() {
        let mut c = Camera::world();
        let p = c.to_map(100.0, 50.0, 800.0, 600.0);
        let (x, y) = c.to_screen(p, 800.0, 600.0);
        assert!((x - 100.0).abs() < 1e-6 && (y - 50.0).abs() < 1e-6);
        c.zoom_at(2.0, 100.0, 50.0, 800.0, 600.0);
        let p2 = c.to_map(100.0, 50.0, 800.0, 600.0);
        assert!((p.x - p2.x).abs() < 1.0 && (p.y - p2.y).abs() < 1.0);
        c.zoom_at(f64::NAN, 0.0, 0.0, 1.0, 1.0);
        c.set_scale(-5.0);
        assert!(c.res.is_finite());
    }

    #[test]
    fn bearing_turns_the_view() {
        let (w, h) = (800.0, 600.0);
        let mut c = Camera::new(Coord::new(0.0, 0.0), 10.0);
        c.set_bearing(90.0);
        // Facing east, a point east of the centre is straight up.
        let (x, y) = c.to_screen(Coord::new(1000.0, 0.0), w, h);
        assert!((x - 400.0).abs() < 1e-9 && (y - 200.0).abs() < 1e-9, "{x},{y}");
        // And north is to the left.
        let (x, y) = c.to_screen(Coord::new(0.0, 1000.0), w, h);
        assert!((x - 300.0).abs() < 1e-9 && (y - 300.0).abs() < 1e-9, "{x},{y}");
        c.set_bearing(-37.0);
        let p = c.to_map(120.0, 45.0, w, h);
        let (x, y) = c.to_screen(p, w, h);
        assert!((x - 120.0).abs() < 1e-6 && (y - 45.0).abs() < 1e-6);
        // Dragging moves the map with the pointer, whichever way it faces.
        let before = c.to_map(100.0, 100.0, w, h);
        c.pan_px(30.0, -20.0);
        let after = c.to_map(130.0, 80.0, w, h);
        assert!((before.x - after.x).abs() < 1e-6 && (before.y - after.y).abs() < 1e-6);
        // The extent holds every corner of the turned view.
        let e = c.extent(w, h);
        for (sx, sy) in [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)] {
            let p = c.to_map(sx, sy, w, h);
            assert!(e.padded(0.0, 1e-6).contains(p));
        }
        c.set_bearing(540.0);
        assert_eq!(c.bearing, 180.0);
        c.set_bearing(-190.0);
        assert!((c.bearing - 170.0).abs() < 1e-9);
        c.set_bearing(f64::NAN);
        assert_eq!(c.bearing, 0.0);
    }

    #[test]
    fn pitch_tilts_the_view_in_perspective() {
        let (w, h) = (800.0, 600.0);
        let mut c = Camera::new(Coord::new(0.0, 0.0), 10.0);
        c.set_pitch(60.0);
        // The centre keeps its place and scale across the view.
        let (x, y) = c.to_screen(Coord::new(0.0, 0.0), w, h);
        assert!((x - 400.0).abs() < 1e-9 && (y - 300.0).abs() < 1e-9);
        let (x, _) = c.to_screen(Coord::new(100.0, 0.0), w, h);
        assert!((x - 410.0).abs() < 1e-6, "{x}");
        // Ahead (north, up the screen) is foreshortened and shrinks; behind (down) grows.
        let (_, up) = c.to_screen(Coord::new(0.0, 1000.0), w, h);
        let (_, down) = c.to_screen(Coord::new(0.0, -1000.0), w, h);
        assert!(300.0 - up < 100.0 * 0.5 && down - 300.0 > 300.0 - up, "{up} {down}");
        assert!(c.depth_scale(Coord::new(0.0, 1000.0), h).unwrap() < 1.0);
        assert!(c.depth_scale(Coord::new(0.0, -1000.0), h).unwrap() > 1.0);
        // Screen and ground round trip, turned too.
        c.set_bearing(-35.0);
        for (sx, sy) in [(10.0, 20.0), (400.0, 300.0), (790.0, 590.0), (200.0, 450.0)] {
            let m = c.try_to_map(sx, sy, w, h).expect("on the ground");
            let (x, y) = c.try_to_screen(m, w, h).expect("in front");
            assert!((x - sx).abs() < 1e-6 && (y - sy).abs() < 1e-6, "{sx},{sy} -> {x},{y}");
        }
        // Dragging keeps the grabbed ground under the pointer.
        let grabbed = c.to_map(300.0, 200.0, w, h);
        c.drag_px((300.0, 200.0), (340.0, 260.0), w, h);
        let under = c.to_map(340.0, 260.0, w, h);
        assert!((grabbed.x - under.x).abs() < 1e-6 && (grabbed.y - under.y).abs() < 1e-6);
        // The extent holds the ground at the corners.
        let e = c.extent(w, h).padded(0.0, 1e-6);
        for (sx, sy) in [(0.0, 0.0), (w, 0.0), (0.0, h), (w, h)] {
            assert!(e.contains(c.to_map(sx, sy, w, h)));
        }
        // No sky below about 71.6°; above it the top shows sky, with no ground under it.
        assert!(c.sky_y(h).is_none() && c.try_to_map(400.0, 0.0, w, h).is_some());
        c.set_pitch(80.0);
        let sky = c.sky_y(h).expect("sky shows");
        assert!(sky > 0.0 && sky < 300.0);
        assert!(c.try_to_map(400.0, sky - 1.0, w, h).is_none() && c.try_to_map(400.0, sky + 1.0, w, h).is_some());
        assert!(c.to_map(400.0, 0.0, w, h).is_finite(), "the sky's point is at the view's reach");
        // Pitch is kept within 0 to 85, and a flat view is unchanged.
        c.set_pitch(120.0);
        assert_eq!(c.pitch, MAX_PITCH);
        c.set_pitch(f64::NAN);
        assert_eq!(c.pitch, 0.0);
        let flat = Camera::new(Coord::new(0.0, 0.0), 10.0);
        assert_eq!(flat.to_screen(Coord::new(100.0, 50.0), w, h), (410.0, 295.0));
    }

    #[test]
    fn tilted_bands_cover_the_ground_nearest_and_sharpest_first() {
        let (w, h) = (800.0, 600.0);
        let mut c = Camera::new(Coord::new(1_000_000.0, 5_000_000.0), 500.0);
        c.set_bearing(30.0);
        for pitch in [10.0, 45.0, 60.0, 75.0, 85.0] {
            c.set_pitch(pitch);
            let bands = c.tilt_bands(w, h);
            assert!(!bands.is_empty() && bands.len() <= 8, "{pitch}: {}", bands.len());
            assert!(bands.windows(2).all(|b| b[0].camera.res < b[1].camera.res), "{pitch}: coarser away");
            assert!(bands.iter().all(|b| b.width <= MAX_BAND_SIDE && b.height <= MAX_BAND_SIDE));
            // Every ground point the view shows is in a band.
            for j in 0..=10 {
                for i in 0..=10 {
                    let Some(m) = c.try_to_map(w * f64::from(i) / 10.0, h * f64::from(j) / 10.0, w, h) else { continue };
                    let inside = bands.iter().any(|b| b.camera.extent(b.width, b.height).padded(0.0, 1e-6).contains(m));
                    assert!(inside, "{pitch}: ({i}, {j}) uncovered");
                }
            }
        }
        // A flat view is one band: itself.
        c.set_pitch(0.0);
        assert_eq!(c.tilt_bands(w, h), vec![Band { camera: c, width: w, height: h }]);
    }

    #[test]
    fn old_cameras_face_north() {
        let c: Camera = serde_json::from_str(r#"{"center": {"x": 1.0, "y": 2.0}, "res": 3.0}"#).unwrap();
        assert_eq!((c.bearing, c.pitch), (0.0, 0.0));
    }
}
