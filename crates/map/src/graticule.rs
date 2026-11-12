//! What the map's graticule draws (#1042): latitudes and longitudes, a measured grid of eastings
//! and northings in one projected system, or the UTM zones with each zone's own grid.

use serde::{Deserialize, Serialize};

/// The lines of the map's graticule (View › Display › Graticule).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GraticuleType {
    /// Latitudes and longitudes.
    #[default]
    Degrees,
    /// Eastings and northings in one projected system ([`crate::Map::graticule_crs`]): none is
    /// the map's own when it is projected, else the UTM zone (WGS 84) at the view's centre.
    Measured,
    /// The UTM zones (WGS 84) and their latitude bands, with the Norway and Svalbard exceptions,
    /// and in each zone its own eastings and northings once few zones are in view.
    Utm,
}

impl GraticuleType {
    pub const ALL: [GraticuleType; 3] = [GraticuleType::Degrees, GraticuleType::Measured, GraticuleType::Utm];

    /// The name a command takes.
    pub fn code(self) -> &'static str {
        match self {
            GraticuleType::Degrees => "degrees",
            GraticuleType::Measured => "measured",
            GraticuleType::Utm => "utm",
        }
    }

    /// The name the interface shows.
    pub fn label(self) -> &'static str {
        match self {
            GraticuleType::Degrees => "Latitude and Longitude",
            GraticuleType::Measured => "Measured Grid",
            GraticuleType::Utm => "UTM Zones",
        }
    }

    /// The type named `s` ([`code`](GraticuleType::code), in any case).
    pub fn from_name(s: &str) -> Option<GraticuleType> {
        GraticuleType::ALL.into_iter().find(|t| t.code().eq_ignore_ascii_case(s.trim()))
    }

    pub(crate) fn is_degrees(&self) -> bool {
        *self == GraticuleType::Degrees
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graticule_types_are_named_in_any_case() {
        for t in GraticuleType::ALL {
            assert_eq!(GraticuleType::from_name(&t.code().to_uppercase()), Some(t));
        }
        assert_eq!(GraticuleType::from_name(" utm "), Some(GraticuleType::Utm));
        assert_eq!(GraticuleType::from_name("mgrs"), None);
    }
}
