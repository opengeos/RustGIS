//! Display units for live map measurements and the scale bar. Geometry stays in SI units.
use rustgis_engine::geom;

pub fn distance(m: f64, imperial: bool) -> String {
    if !imperial {
        return geom::format_distance(m);
    }
    let ft = m / 0.3048;
    if ft.round() < 5280.0 { format!("{ft:.0} ft") } else { format!("{:.1} mi", m / 1609.344) }
}

pub fn area(m2: f64, imperial: bool) -> String {
    if !imperial {
        return geom::format_area(m2);
    }
    if m2 < 4046.8564224 {
        format!("{:.0} ft²", m2 / 0.09290304)
    } else if m2 < 2_589_988.110336 {
        crate::i18n::fmt("{area} acres", &[("area", &format!("{:.1}", m2 / 4046.8564224))])
    } else {
        format!("{:.1} mi²", m2 / 2_589_988.110336)
    }
}

pub fn scale_distance(max_m: f64, imperial: bool) -> f64 {
    let unit = if !imperial {
        1.0
    } else if max_m < 1609.344 {
        0.3048
    } else {
        1609.344
    };
    geom::nice_distance(max_m / unit) * unit
}
