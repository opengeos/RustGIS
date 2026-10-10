//! The map camera: a Web Mercator centre and a resolution (metres per logical pixel).

use rustgis_geom::{Coord, Extent, MERCATOR_R, extent_to_merc, merc_to_lonlat, scale_denominator};
use serde::{Deserialize, Serialize};

/// Resolution limits (metres per pixel): about 1:500 to the whole world.
pub const MIN_RES: f64 = 0.15;
pub const MAX_RES: f64 = 160_000.0;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    pub center: Coord,
    pub res: f64,
}

impl Camera {
    pub fn world() -> Self {
        Camera { center: Coord::new(0.0, 2_000_000.0), res: 30_000.0 }
    }

    /// Screen position (logical px, origin top-left) of a Mercator point in a `w`×`h` view.
    pub fn to_screen(&self, p: Coord, w: f64, h: f64) -> (f64, f64) {
        ((p.x - self.center.x) / self.res + w / 2.0, (self.center.y - p.y) / self.res + h / 2.0)
    }

    pub fn to_map(&self, sx: f64, sy: f64, w: f64, h: f64) -> Coord {
        Coord::new(self.center.x + (sx - w / 2.0) * self.res, self.center.y - (sy - h / 2.0) * self.res)
    }

    /// Mercator extent visible in a `w`×`h` view.
    pub fn extent(&self, w: f64, h: f64) -> Extent {
        Extent::new(
            self.center.x - w / 2.0 * self.res,
            self.center.y - h / 2.0 * self.res,
            self.center.x + w / 2.0 * self.res,
            self.center.y + h / 2.0 * self.res,
        )
    }

    pub fn clamp(&mut self) {
        if !self.res.is_finite() {
            self.res = 30_000.0;
        }
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
        self.center.x -= dx * self.res;
        self.center.y += dy * self.res;
        self.clamp();
    }

    /// Fits a lon/lat extent into a `w`×`h` view with a margin.
    pub fn fit_lonlat(&mut self, e: &Extent, w: f64, h: f64) {
        if e.is_empty() || w <= 1.0 || h <= 1.0 {
            return;
        }
        let m = extent_to_merc(&e.padded(0.06, 0.02));
        self.center = m.center();
        self.res = (m.width() / w).max(m.height() / h).max(MIN_RES * 20.0);
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
}
