//! The map camera: a Web Mercator centre, a resolution (metres per logical pixel) and a bearing.

use rustgis_geom::{Coord, Extent, MERCATOR_R, extent_to_merc, merc_to_lonlat, scale_denominator};
use serde::{Deserialize, Serialize};

/// Resolution limits (metres per pixel): about 1:500 to the whole world.
pub const MIN_RES: f64 = 0.15;
pub const MAX_RES: f64 = 160_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    pub center: Coord,
    pub res: f64,
    /// The compass direction the top of the view faces, in degrees clockwise from north (as in
    /// MapLibre): at 90 east is up. The globe ignores it.
    #[serde(default)]
    pub bearing: f64,
}

impl Camera {
    pub fn new(center: Coord, res: f64) -> Self {
        Camera { center, res, bearing: 0.0 }
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

    /// Screen position (logical px, origin top-left) of a Mercator point in a `w`×`h` view.
    pub fn to_screen(&self, p: Coord, w: f64, h: f64) -> (f64, f64) {
        let (ux, uy) = ((p.x - self.center.x) / self.res, (self.center.y - p.y) / self.res);
        let (c, s) = self.rotation();
        (ux * c + uy * s + w / 2.0, uy * c - ux * s + h / 2.0)
    }

    pub fn to_map(&self, sx: f64, sy: f64, w: f64, h: f64) -> Coord {
        let (ux, uy) = self.unrotate(sx - w / 2.0, sy - h / 2.0);
        Coord::new(self.center.x + ux * self.res, self.center.y - uy * self.res)
    }

    /// A screen offset (logical px) turned back to the map's north-up axes.
    fn unrotate(&self, dx: f64, dy: f64) -> (f64, f64) {
        let (c, s) = self.rotation();
        (dx * c - dy * s, dx * s + dy * c)
    }

    /// Mercator extent visible in a `w`×`h` view (around a rotated view, the extent of its
    /// corners).
    pub fn extent(&self, w: f64, h: f64) -> Extent {
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

    pub fn clamp(&mut self) {
        if !self.res.is_finite() {
            self.res = 30_000.0;
        }
        self.bearing = normalize_bearing(self.bearing);
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
        let anchor = self.to_map(sx, sy, w, h);
        self.res /= factor;
        self.clamp();
        let after = self.to_map(sx, sy, w, h);
        self.center.x += anchor.x - after.x;
        self.center.y += anchor.y - after.y;
        self.clamp();
    }

    pub fn pan_px(&mut self, dx: f64, dy: f64) {
        let (ux, uy) = self.unrotate(dx, dy);
        self.center.x -= ux * self.res;
        self.center.y += uy * self.res;
        self.clamp();
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
    fn old_cameras_face_north() {
        let c: Camera = serde_json::from_str(r#"{"center": {"x": 1.0, "y": 2.0}, "res": 3.0}"#).unwrap();
        assert_eq!(c.bearing, 0.0);
    }
}
