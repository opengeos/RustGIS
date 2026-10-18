//! World map projections computed here, on a sphere (of the WGS 84 equatorial radius unless the
//! definition gives `+R`): Robinson, Equal Earth, Natural Earth, Winkel Tripel and Eckert IV.
//!
//! The forward formulas follow the projections' published definitions. Inverses use Newton's
//! method on the forward formula, which also rejects points outside the map's outline (no
//! longitude/latitude maps to them).

use rustgis_geom::Coord;

/// A world projection (PROJ names: `robin`, `eqearth`, `natearth`, `wintri`, `eck4`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Robinson,
    EqualEarth,
    NaturalEarth,
    WinkelTripel,
    EckertIV,
}

impl Kind {
    pub fn from_proj_name(name: &str) -> Option<Kind> {
        Some(match name {
            "robin" => Kind::Robinson,
            "eqearth" => Kind::EqualEarth,
            "natearth" => Kind::NaturalEarth,
            "wintri" => Kind::WinkelTripel,
            "eck4" => Kind::EckertIV,
            _ => return None,
        })
    }

    pub fn proj_name(self) -> &'static str {
        match self {
            Kind::Robinson => "robin",
            Kind::EqualEarth => "eqearth",
            Kind::NaturalEarth => "natearth",
            Kind::WinkelTripel => "wintri",
            Kind::EckertIV => "eck4",
        }
    }
}

/// Robinson's table: (X, Y) every 5° of latitude from 0° to 90°.
const ROBINSON: [(f64, f64); 19] = [
    (1.0000, 0.0000),
    (0.9986, 0.0620),
    (0.9954, 0.1240),
    (0.9900, 0.1860),
    (0.9822, 0.2480),
    (0.9730, 0.3100),
    (0.9600, 0.3720),
    (0.9427, 0.4340),
    (0.9216, 0.4958),
    (0.8962, 0.5571),
    (0.8679, 0.6176),
    (0.8350, 0.6769),
    (0.7986, 0.7346),
    (0.7597, 0.7903),
    (0.7186, 0.8435),
    (0.6732, 0.8936),
    (0.6213, 0.9394),
    (0.5722, 0.9761),
    (0.5322, 1.0000),
];

/// Robinson's X and Y at |latitude| in degrees, by cubic (Catmull-Rom) interpolation.
fn robinson_xy(lat_deg: f64) -> (f64, f64) {
    let t = (lat_deg.abs() / 5.0).clamp(0.0, 18.0);
    let i = (t.floor() as usize).min(17);
    let f = t - i as f64;
    let at = |k: isize| {
        // Mirror at the equator and extend linearly past the pole.
        let k = k.clamp(-1, 19);
        match k {
            -1 => (ROBINSON[1].0, -ROBINSON[1].1),
            19 => (2.0 * ROBINSON[18].0 - ROBINSON[17].0, 2.0 * ROBINSON[18].1 - ROBINSON[17].1),
            k => ROBINSON[k as usize],
        }
    };
    let (p0, p1, p2, p3) = (at(i as isize - 1), at(i as isize), at(i as isize + 1), at(i as isize + 2));
    let cr = |a: f64, b: f64, c: f64, d: f64| {
        0.5 * (2.0 * b + (-a + c) * f + (2.0 * a - 5.0 * b + 4.0 * c - d) * f * f + (-a + 3.0 * b - 3.0 * c + d) * f * f * f)
    };
    (cr(p0.0, p1.0, p2.0, p3.0), cr(p0.1, p1.1, p2.1, p3.1))
}

/// Forward projection of (λ, φ) in radians, as multiples of the radius.
fn unit_forward(kind: Kind, lam: f64, phi: f64) -> (f64, f64) {
    match kind {
        Kind::Robinson => {
            let (x, y) = robinson_xy(phi.to_degrees());
            (0.8487 * x * lam, 1.3523 * y * phi.signum())
        }
        Kind::EqualEarth => {
            const A1: f64 = 1.340264;
            const A2: f64 = -0.081106;
            const A3: f64 = 0.000893;
            const A4: f64 = 0.003796;
            let m = 3.0_f64.sqrt() / 2.0;
            let th = (m * phi.sin()).asin();
            let (t2, t6, t8) = (th * th, th.powi(6), th.powi(8));
            let x = 2.0 * 3.0_f64.sqrt() * lam * th.cos() / (3.0 * (9.0 * A4 * t8 + 7.0 * A3 * t6 + 3.0 * A2 * t2 + A1));
            let y = th * (A4 * t8 + A3 * t6 + A2 * t2 + A1);
            (x, y)
        }
        Kind::NaturalEarth => {
            let p2 = phi * phi;
            let p4 = p2 * p2;
            let x = lam * (0.8707 - 0.131979 * p2 + p4 * (-0.013791 + p4 * p2 * (0.003971 - 0.001529 * p2)));
            let y = phi * (1.007226 + p2 * (0.015085 + p4 * (-0.044475 + 0.028874 * p2 - 0.005916 * p4)));
            (x, y)
        }
        Kind::WinkelTripel => {
            let phi1 = (2.0 / std::f64::consts::PI).acos();
            let alpha = (phi.cos() * (lam / 2.0).cos()).clamp(-1.0, 1.0).acos();
            let sinc = if alpha.abs() < 1e-12 { 1.0 } else { alpha.sin() / alpha };
            let x = 0.5 * (lam * phi1.cos() + 2.0 * phi.cos() * (lam / 2.0).sin() / sinc);
            let y = 0.5 * (phi + phi.sin() / sinc);
            (x, y)
        }
        Kind::EckertIV => {
            // θ + sinθ·cosθ + 2·sinθ = (2 + π/2)·sinφ, by Newton.
            let k = (2.0 + std::f64::consts::FRAC_PI_2) * phi.sin();
            let mut th = phi / 2.0;
            for _ in 0..30 {
                let f = th + th.sin() * th.cos() + 2.0 * th.sin() - k;
                let d = 1.0 + (2.0 * th).cos() + 2.0 * th.cos();
                if d.abs() < 1e-12 {
                    break;
                }
                let step = f / d;
                th -= step;
                if step.abs() < 1e-14 {
                    break;
                }
            }
            let pi = std::f64::consts::PI;
            let x = 2.0 / (pi * (4.0 + pi)).sqrt() * lam * (1.0 + th.cos());
            let y = 2.0 * (pi / (4.0 + pi)).sqrt() * th.sin();
            (x, y)
        }
    }
}

/// A built-in world projection with a central meridian.
#[derive(Clone, Copy, Debug)]
pub struct World {
    pub kind: Kind,
    /// Central meridian, degrees.
    pub lon0: f64,
    /// The sphere's radius, metres.
    pub radius: f64,
}

impl World {
    pub fn forward(&self, c: Coord) -> Option<Coord> {
        if !c.is_finite() {
            return None;
        }
        // Keep ±180° (and points a rounding error past it) on their own side, so rings along the
        // map's edge don't jump across the whole map.
        let d = c.x - self.lon0;
        let d = if d.abs() <= 180.0 + 1e-6 { d.clamp(-180.0, 180.0) } else { (d + 540.0).rem_euclid(360.0) - 180.0 };
        let lam = d.to_radians();
        let phi = c.y.clamp(-90.0, 90.0).to_radians();
        let (x, y) = unit_forward(self.kind, lam, phi);
        let out = Coord::new(x * self.radius, y * self.radius);
        out.is_finite().then_some(out)
    }

    /// Newton's method from a starting guess; `None` when no point maps to `m`.
    pub fn inverse(&self, m: Coord) -> Option<Coord> {
        if !m.is_finite() {
            return None;
        }
        let (tx, ty) = (m.x / self.radius, m.y / self.radius);
        let (pi, half) = (std::f64::consts::PI, std::f64::consts::FRAC_PI_2);
        let (mut lam, mut phi) = ((tx / 0.85).clamp(-pi, pi), (ty / 1.2).clamp(-half, half));
        let h = 1e-7;
        for _ in 0..40 {
            let (fx, fy) = unit_forward(self.kind, lam, phi);
            let (ex, ey) = (fx - tx, fy - ty);
            if ex.abs() + ey.abs() < 1e-12 {
                break;
            }
            // Numerical Jacobian, stepping inward near the edges.
            let dl = if lam + h > pi { -h } else { h };
            let dp = if phi + h > half { -h } else { h };
            let (ax, ay) = unit_forward(self.kind, lam + dl, phi);
            let (bx, by) = unit_forward(self.kind, lam, phi + dp);
            let (j11, j21) = ((ax - fx) / dl, (ay - fy) / dl);
            let (j12, j22) = ((bx - fx) / dp, (by - fy) / dp);
            let det = j11 * j22 - j12 * j21;
            if !det.is_finite() || det.abs() < 1e-15 {
                break;
            }
            lam = (lam - (j22 * ex - j12 * ey) / det).clamp(-pi, pi);
            phi = (phi - (-j21 * ex + j11 * ey) / det).clamp(-half, half);
        }
        let (fx, fy) = unit_forward(self.kind, lam, phi);
        // Off the outline Newton settles at the clamp, short of the target.
        if (fx - tx).abs() + (fy - ty).abs() > 1e-9 {
            return None;
        }
        let lon = (lam.to_degrees() + self.lon0 + 540.0).rem_euclid(360.0) - 180.0;
        Some(Coord::new(lon, phi.to_degrees()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_geom::MERCATOR_R;

    #[test]
    fn round_trips_and_outlines() {
        for kind in [Kind::Robinson, Kind::EqualEarth, Kind::NaturalEarth, Kind::WinkelTripel, Kind::EckertIV] {
            let w = World { kind, lon0: 0.0, radius: MERCATOR_R };
            for (lon, lat) in [(0.0, 0.0), (10.0, 50.0), (-120.0, -35.0), (179.0, 80.0), (-60.0, 10.0)] {
                let m = w.forward(Coord::new(lon, lat)).unwrap();
                let back = w.inverse(m).unwrap_or_else(|| panic!("{kind:?} {lon},{lat}"));
                assert!((back.x - lon).abs() < 1e-6 && (back.y - lat).abs() < 1e-6, "{kind:?} {lon},{lat}: {back:?}");
            }
            // Beyond the outline: nothing maps there.
            let corner =
                Coord::new(0.99 * w.forward(Coord::new(180.0, 0.0)).unwrap().x, 0.99 * w.forward(Coord::new(0.0, 90.0)).unwrap().y);
            assert!(w.inverse(corner).is_none(), "{kind:?} corner");
            assert!(w.inverse(Coord::new(f64::NAN, 0.0)).is_none());
        }
        // Robinson's published scale: the equator is 0.8487·2πR long.
        let r = World { kind: Kind::Robinson, lon0: 0.0, radius: MERCATOR_R };
        let x = r.forward(Coord::new(180.0, 0.0)).unwrap().x;
        assert!((x / (0.8487 * std::f64::consts::PI * MERCATOR_R) - 1.0).abs() < 1e-9);
        // A central meridian shifts the map.
        let shifted = World { kind: Kind::EqualEarth, lon0: 150.0, radius: MERCATOR_R };
        assert!(shifted.forward(Coord::new(150.0, 0.0)).unwrap().x.abs() < 1e-6);
    }
}
