//! Presentations: pages shown one after another, each a map view, text, an image or a video,
//! with a title, speaker notes and a duration.

use rustgis_geom::Coord;
use serde::{Deserialize, Serialize};

use crate::Rgba;

/// Most pages a presentation keeps (files are hostile).
pub const MAX_PAGES: usize = 500;
/// The default page: 16:9, 254 × 142.875 mm (10 × 5.625 in).
pub const WIDTH: f64 = 254.0;
pub const HEIGHT: f64 = 142.875;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Presentation {
    pub name: String,
    /// Page size in millimetres.
    pub width: f64,
    pub height: f64,
    pub pages: Vec<Page>,
}

impl Default for Presentation {
    fn default() -> Self {
        Presentation { name: "Presentation".into(), width: WIDTH, height: HEIGHT, pages: Vec::new() }
    }
}

impl Presentation {
    /// Makes a presentation read from a file safe to use.
    pub fn clamp(&mut self) {
        let side = |v: f64, d: f64| if v.is_finite() { v.clamp(50.0, 2000.0) } else { d };
        self.width = side(self.width, WIDTH);
        self.height = side(self.height, HEIGHT);
        self.name = self.name.chars().take(200).collect();
        self.pages.truncate(MAX_PAGES);
        for p in &mut self.pages {
            p.title = p.title.chars().take(500).collect();
            p.notes = p.notes.chars().take(20_000).collect();
            p.seconds = if p.seconds.is_finite() { p.seconds.clamp(0.5, 3600.0) } else { 5.0 };
            if let PageKind::Map { center, width_m, .. } = &mut p.kind {
                if !(center.x.is_finite() && center.y.is_finite()) {
                    *center = Coord::new(0.0, 0.0);
                }
                center.x = center.x.clamp(-180.0, 180.0);
                center.y = center.y.clamp(-85.0, 85.0);
                *width_m = if width_m.is_finite() { width_m.clamp(10.0, 4e7) } else { 4e7 };
            }
        }
    }
}

/// One page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    #[serde(default)]
    pub title: String,
    pub kind: PageKind,
    /// Speaker notes (not drawn).
    #[serde(default)]
    pub notes: String,
    /// How long the page shows when the presentation plays.
    #[serde(default = "five")]
    pub seconds: f64,
}

fn five() -> f64 {
    5.0
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PageKind {
    /// A map at a place: its centre (longitude/latitude) and the ground width shown, in metres.
    #[serde(rename_all = "camelCase")]
    Map { map: String, center: Coord, width_m: f64 },
    /// Text on a coloured page.
    Blank {
        #[serde(default)]
        text: String,
        background: Rgba,
    },
    /// A picture (base64 PNG/JPEG) with a caption.
    Image {
        image: String,
        #[serde(default)]
        caption: String,
    },
    /// A video (a file path or URL), shown by its poster picture until played.
    Video {
        source: String,
        #[serde(default)]
        poster: Option<String>,
        #[serde(default)]
        caption: String,
    },
}

impl PageKind {
    pub fn code(&self) -> &'static str {
        match self {
            PageKind::Map { .. } => "map",
            PageKind::Blank { .. } => "blank",
            PageKind::Image { .. } => "image",
            PageKind::Video { .. } => "video",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_and_round_trips() {
        let mut p = Presentation {
            width: f64::NAN,
            pages: vec![Page {
                title: "Rome".into(),
                kind: PageKind::Map { map: "World".into(), center: Coord::new(f64::NAN, 95.0), width_m: -1.0 },
                notes: String::new(),
                seconds: f64::INFINITY,
            }],
            ..Presentation::default()
        };
        p.clamp();
        assert_eq!(p.width, WIDTH);
        assert_eq!(p.pages[0].seconds, 5.0);
        assert!(matches!(p.pages[0].kind, PageKind::Map { center, width_m, .. } if center.y == 0.0 && width_m == 10.0));
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Presentation>(&json).unwrap(), p);
    }
}
