//! A raster's profile beside its map frame (#2863 part 2): along one of the frame's section lines,
//! on the side the line runs along, lined up with the map so that each point of the curve is
//! straight across from the place it comes from.

use super::*;

/// The side of its map frame a profile lies along: under or over it for a horizontal section line,
/// left or right of it for a vertical one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProfileSide {
    #[default]
    Bottom,
    Top,
    Left,
    Right,
}

impl ProfileSide {
    pub const ALL: [ProfileSide; 4] = [ProfileSide::Bottom, ProfileSide::Top, ProfileSide::Left, ProfileSide::Right];

    pub fn code(self) -> &'static str {
        match self {
            ProfileSide::Bottom => "bottom",
            ProfileSide::Top => "top",
            ProfileSide::Left => "left",
            ProfileSide::Right => "right",
        }
    }

    pub fn from_name(s: &str) -> Option<ProfileSide> {
        ProfileSide::ALL.into_iter().find(|v| v.code().eq_ignore_ascii_case(s.trim()))
    }

    /// Whether it runs across the page: under or over the frame.
    pub fn is_horizontal(self) -> bool {
        matches!(self, ProfileSide::Bottom | ProfileSide::Top)
    }

    /// Whether a profile of `line` can lie on this side: one that runs the same way as the line.
    pub fn suits(self, line: SectionLine) -> bool {
        self.is_horizontal() == matches!(line, SectionLine::Horizontal(_))
    }

    /// The side a profile of `line` takes unless given one: under a horizontal line's frame, right
    /// of a vertical one's.
    pub fn default_for(line: SectionLine) -> ProfileSide {
        match line {
            SectionLine::Horizontal(_) => ProfileSide::Bottom,
            SectionLine::Vertical(_) => ProfileSide::Right,
        }
    }
}

/// A profile element: the values of a raster's band along a section line of its map frame, drawn
/// beside the frame as a line, filled under it or not, with a value axis, a distance axis and the
/// section's names at its ends. Its box isn't its own: it takes the frame's side along its length
/// ([`Profile::rect`]), so it follows the frame when the frame is moved or resized.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// The map frame, and its section line (by name, any case).
    pub frame: u32,
    pub section: String,
    /// The raster layer of the frame's map (by name, any case), and its band from 1.
    pub layer: String,
    #[serde(default = "first_band", skip_serializing_if = "is_first_band")]
    pub band: u32,
    #[serde(default)]
    pub side: ProfileSide,
    /// Millimetres between the frame and the profile, and across the profile.
    #[serde(default = "profile_gap")]
    pub gap: f64,
    #[serde(default = "profile_thickness")]
    pub thickness: f64,
    /// Written over the curve ("Elevation"), and after the value axis' numbers ("m"). Left out
    /// when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unit: String,
    /// The curve's colour and width (points), and the colour under it; none, no fill.
    #[serde(default = "profile_line")]
    pub line_color: Rgba,
    #[serde(default = "profile_line_width")]
    pub line_width: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Rgba>,
    /// The value axis' ends; none, the samples' own, out to round numbers. Two profiles given the
    /// same share a scale. Read only when below the other end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// What the distance axis counts in, as a scale bar does.
    #[serde(default, skip_serializing_if = "is_default")]
    pub units: ScaleUnits,
    /// Lines across the curve at the value axis' numbers.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub guides: bool,
    /// The colour of its text and axes (#2257); none, the dark grey of [`INK`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<Rgba>,
}

/// A new profile's gap and thickness (mm), and the least and most each may be.
pub const PROFILE_GAP: f64 = 3.0;
pub const PROFILE_GAPS: (f64, f64) = (0.0, 100.0);
pub const PROFILE_THICKNESS: f64 = 30.0;
pub const PROFILE_THICKNESSES: (f64, f64) = (10.0, 300.0);
/// A new profile's curve: its colour, width (points) and fill; and the least and most the width may be.
pub const PROFILE_LINE: Rgba = Rgba::rgb(0x3C, 0x32, 0x28);
pub const PROFILE_LINE_WIDTH: f64 = 1.0;
pub const PROFILE_LINE_WIDTHS: (f64, f64) = (0.1, 10.0);
pub const PROFILE_FILL: Rgba = Rgba::rgb(0xD8, 0xC8, 0xA8);
/// The longest title and unit (characters), and the highest band a profile names.
pub const MAX_PROFILE_TEXT: usize = 100;
pub const MAX_PROFILE_BAND: u32 = 65_535;
/// The most samples a profile reads along its section, as the `profile.*` commands.
pub const MAX_PROFILE_SAMPLES: usize = 10_000;

fn first_band() -> u32 {
    1
}

fn is_first_band(b: &u32) -> bool {
    *b == 1
}

fn is_true(v: &bool) -> bool {
    *v
}

fn profile_gap() -> f64 {
    PROFILE_GAP
}

fn profile_thickness() -> f64 {
    PROFILE_THICKNESS
}

fn profile_line() -> Rgba {
    PROFILE_LINE
}

fn profile_line_width() -> f64 {
    PROFILE_LINE_WIDTH
}

impl Profile {
    /// A profile of `layer`'s first band along `section` of `frame`, on `side`, in the default look.
    pub fn new(frame: u32, section: String, layer: String, side: ProfileSide) -> Profile {
        Profile {
            frame,
            section,
            layer,
            band: 1,
            side,
            gap: PROFILE_GAP,
            thickness: PROFILE_THICKNESS,
            title: String::new(),
            unit: String::new(),
            line_color: PROFILE_LINE,
            line_width: PROFILE_LINE_WIDTH,
            fill: Some(PROFILE_FILL),
            min: None,
            max: None,
            units: ScaleUnits::default(),
            guides: true,
            color: None,
        }
    }

    /// Its box (x, y, w, h in mm) beside a frame whose box is `frame`: the frame's length along its
    /// side, `gap` from it and `thickness` across.
    pub fn rect(&self, (fx, fy, fw, fh): (f64, f64, f64, f64)) -> (f64, f64, f64, f64) {
        let (g, t) = (self.gap, self.thickness);
        match self.side {
            ProfileSide::Bottom => (fx, fy + fh + g, fw, t),
            ProfileSide::Top => (fx, fy - g - t, fw, t),
            ProfileSide::Right => (fx + fw + g, fy, t, fh),
            ProfileSide::Left => (fx - g - t, fy, t, fh),
        }
    }

    /// The gap and thickness that put it at `rect` (x, y, w, h in mm), beside a frame whose box is
    /// `frame`, as far as they can: what is along the frame stays the frame's ([`Profile::rect`]).
    /// Whether either was held to its limits.
    pub fn take_rect(&mut self, (fx, fy, fw, fh): (f64, f64, f64, f64), (x, y, w, h): (f64, f64, f64, f64)) -> bool {
        let (gap, thickness) = match self.side {
            ProfileSide::Bottom => (y - (fy + fh), h),
            ProfileSide::Top => (fy - (y + h), h),
            ProfileSide::Right => (x - (fx + fw), w),
            ProfileSide::Left => (fx - (x + w), w),
        };
        self.gap = finite_or(gap, self.gap);
        self.thickness = finite_or(thickness, self.thickness);
        let (g, t) = (self.gap, self.thickness);
        self.clamp();
        (g, t) != (self.gap, self.thickness)
    }

    /// Its numbers and texts brought into range (a layout file is hostile input). Whether a text was
    /// cut.
    pub(super) fn clamp(&mut self) -> bool {
        self.gap = finite_or(self.gap, PROFILE_GAP).clamp(PROFILE_GAPS.0, PROFILE_GAPS.1);
        self.thickness = finite_or(self.thickness, PROFILE_THICKNESS).clamp(PROFILE_THICKNESSES.0, PROFILE_THICKNESSES.1);
        self.line_width = finite_or(self.line_width, PROFILE_LINE_WIDTH).clamp(PROFILE_LINE_WIDTHS.0, PROFILE_LINE_WIDTHS.1);
        self.band = self.band.clamp(1, MAX_PROFILE_BAND);
        self.min = self.min.filter(|v| v.is_finite());
        self.max = self.max.filter(|v| v.is_finite());
        let mut cut = false;
        for (text, most) in [(&mut self.title, MAX_PROFILE_TEXT), (&mut self.unit, MAX_PROFILE_TEXT), (&mut self.section, MAX_SECTION_NAME)]
        {
            // One line: a break in a hand-edited file is a space.
            let one = text.replace("\r\n", " ").replace(['\n', '\r'], " ");
            cut |= one.trim().chars().nth(most).is_some();
            *text = one.trim().chars().take(most).collect::<String>().trim_end().to_string();
        }
        cut
    }

    /// The value axis' ends given, when they make one (the least below the most).
    pub fn given_range(&self) -> (Option<f64>, Option<f64>) {
        match (self.min, self.max) {
            (Some(lo), Some(hi)) if lo >= hi => (None, None),
            given => given,
        }
    }
}
