//! Colours and colour ramps.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgba(pub [u8; 4]);

impl Rgba {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Rgba([r, g, b, a])
    }
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Rgba([r, g, b, 255])
    }
    pub const TRANSPARENT: Rgba = Rgba([0, 0, 0, 0]);
    pub fn r(self) -> u8 {
        self.0[0]
    }
    pub fn g(self) -> u8 {
        self.0[1]
    }
    pub fn b(self) -> u8 {
        self.0[2]
    }
    pub fn a(self) -> u8 {
        self.0[3]
    }
    pub fn with_alpha(self, a: u8) -> Rgba {
        Rgba([self.0[0], self.0[1], self.0[2], a])
    }
    pub fn lerp(self, o: Rgba, t: f64) -> Rgba {
        let t = t.clamp(0.0, 1.0);
        let m = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t).round().clamp(0.0, 255.0) as u8;
        Rgba([m(self.0[0], o.0[0]), m(self.0[1], o.0[1]), m(self.0[2], o.0[2]), m(self.0[3], o.0[3])])
    }
    /// Darker (f < 1) or lighter (f > 1) version.
    pub fn shade(self, f: f64) -> Rgba {
        let s = |c: u8| {
            let c = c as f64;
            (if f < 1.0 { c * f } else { c + (255.0 - c) * (f - 1.0) }).round().clamp(0.0, 255.0) as u8
        };
        Rgba([s(self.0[0]), s(self.0[1]), s(self.0[2]), self.0[3]])
    }
    /// `#RRGGBB` or `#RRGGBBAA`.
    pub fn hex(self) -> String {
        if self.a() == 255 {
            format!("#{:02X}{:02X}{:02X}", self.r(), self.g(), self.b())
        } else {
            format!("#{:02X}{:02X}{:02X}{:02X}", self.r(), self.g(), self.b(), self.a())
        }
    }
    pub fn from_hex(s: &str) -> Option<Rgba> {
        let s = s.trim().trim_start_matches('#');
        let byte = |i: usize| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok());
        match s.len() {
            6 => Some(Rgba::rgb(byte(0)?, byte(2)?, byte(4)?)),
            8 => Some(Rgba::new(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        }
    }
}

/// Our own colour ramps (sequential, diverging and qualitative).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ramp {
    #[default]
    Ember,
    Lagoon,
    Forest,
    Dusk,
    Thermal,
    Diverging,
    Greys,
    Harbour,
}

impl Ramp {
    pub const ALL: [Ramp; 8] =
        [Ramp::Ember, Ramp::Lagoon, Ramp::Forest, Ramp::Dusk, Ramp::Thermal, Ramp::Diverging, Ramp::Greys, Ramp::Harbour];

    pub fn label(self) -> &'static str {
        match self {
            Ramp::Ember => "Ember",
            Ramp::Lagoon => "Lagoon",
            Ramp::Forest => "Forest",
            Ramp::Dusk => "Dusk",
            Ramp::Thermal => "Thermal",
            Ramp::Diverging => "Red–Blue",
            Ramp::Greys => "Greys",
            Ramp::Harbour => "Harbour (qualitative)",
        }
    }

    pub fn from_name(s: &str) -> Option<Ramp> {
        Ramp::ALL.into_iter().find(|r| format!("{r:?}").eq_ignore_ascii_case(s.trim()) || r.label().eq_ignore_ascii_case(s.trim()))
    }

    fn stops(self) -> &'static [[u8; 3]] {
        match self {
            Ramp::Ember => &[[0xFF, 0xF3, 0xD6], [0xFD, 0xC9, 0x7A], [0xF5, 0x8A, 0x4B], [0xD9, 0x48, 0x3B], [0x8E, 0x1B, 0x3A]],
            Ramp::Lagoon => &[[0xEE, 0xF8, 0xF6], [0xA6, 0xDD, 0xD4], [0x52, 0xB3, 0xB6], [0x22, 0x7C, 0x9D], [0x17, 0x3F, 0x6E]],
            Ramp::Forest => &[[0xF4, 0xF9, 0xE8], [0xC2, 0xE3, 0x9C], [0x7B, 0xC0, 0x6A], [0x35, 0x8E, 0x4C], [0x10, 0x55, 0x33]],
            Ramp::Dusk => &[[0xF6, 0xEF, 0xF7], [0xD4, 0xB9, 0xDA], [0xB0, 0x7A, 0xB8], [0x7E, 0x45, 0x9A], [0x45, 0x1D, 0x6B]],
            Ramp::Thermal => &[[0x2C, 0x3E, 0x8C], [0x2E, 0x9F, 0xC0], [0xA8, 0xDA, 0x6A], [0xF9, 0xC7, 0x3A], [0xD8, 0x3A, 0x2A]],
            Ramp::Diverging => &[[0xB2, 0x2B, 0x35], [0xEE, 0x9A, 0x80], [0xF5, 0xF2, 0xEF], [0x8A, 0xB8, 0xDB], [0x2A, 0x5C, 0x9C]],
            Ramp::Greys => &[[0xF4, 0xF4, 0xF4], [0xC4, 0xC4, 0xC4], [0x8E, 0x8E, 0x8E], [0x55, 0x55, 0x55], [0x22, 0x22, 0x22]],
            Ramp::Harbour => &[
                [0x3B, 0x7D, 0xD8],
                [0xE8, 0x7A, 0x41],
                [0x4C, 0xAF, 0x7A],
                [0xC9, 0x4F, 0x8C],
                [0xE3, 0xBE, 0x3C],
                [0x6F, 0x5B, 0xC9],
                [0x3F, 0xB5, 0xC2],
                [0xA6, 0x6E, 0x4E],
                [0x8D, 0x9B, 0x3A],
                [0xD9, 0x5D, 0x5D],
            ],
        }
    }

    pub fn is_qualitative(self) -> bool {
        self == Ramp::Harbour
    }

    /// Colour at `t` ∈ [0, 1] (qualitative ramps pick the nearest swatch).
    pub fn at(self, t: f64) -> Rgba {
        let s = self.stops();
        let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
        let n = s.len();
        if n == 0 {
            return Rgba::rgb(128, 128, 128);
        }
        let f = t * (n - 1) as f64;
        let i = (f.floor() as usize).min(n - 1);
        let j = (i + 1).min(n - 1);
        let c = |k: usize| s.get(k).map(|c| Rgba::rgb(c[0], c[1], c[2])).unwrap_or(Rgba::rgb(128, 128, 128));
        if self.is_qualitative() { c(f.round() as usize) } else { c(i).lerp(c(j), f - i as f64) }
    }

    /// The `k`-th of `n` evenly spaced colours (qualitative ramps cycle).
    pub fn pick(self, k: usize, n: usize) -> Rgba {
        if self.is_qualitative() {
            let s = self.stops();
            let c = s.get(k % s.len().max(1)).copied().unwrap_or([128, 128, 128]);
            return Rgba::rgb(c[0], c[1], c[2]);
        }
        if n <= 1 { self.at(0.6) } else { self.at(k as f64 / (n - 1) as f64) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_roundtrip_and_ramps() {
        let c = Rgba::new(1, 2, 255, 128);
        assert_eq!(Rgba::from_hex(&c.hex()), Some(c));
        assert_eq!(Rgba::from_hex("#zz0000"), None);
        assert_eq!(Rgba::from_hex("#ééé"), None);
        for r in Ramp::ALL {
            let _ = r.at(f64::NAN);
            let _ = r.pick(99, 3);
            let _ = r.pick(0, 0);
        }
    }
}
