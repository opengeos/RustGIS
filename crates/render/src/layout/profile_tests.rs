use rustgis_data::raster::{ColorHint, Crs, GeoTransform, Raster};
use rustgis_map::Projection;
use rustgis_map::layout::{FRAME_BORDER_COLOR, FRAME_BORDER_WIDTH, INDICATOR_COLOR, INDICATOR_WIDTH};

use super::*;

/// The Mantiqueira frame of #2863: 186 × 112 mm at 1:200,000 round the Agulhas Negras, in UTM 23S.
const CENTER: Coord = Coord::new(-44.715, -22.412);
const AGULHAS_NEGRAS: Coord = Coord::new(-44.6615, -22.3802);
const PEDRA_DA_MINA: Coord = Coord::new(-44.8429, -22.4282);
const UTM: &str = "EPSG:31983";
const FRAME_MM: (f64, f64, f64, f64) = (30.0, 34.0, 186.0, 112.0);

/// A 30 m grid in UTM 23S over the frame and round it, each cell `z` of its centre's easting and
/// northing.
fn dem(z: impl Fn(f64, f64) -> f64) -> Raster {
    let crs = Crs::from_epsg(31983).unwrap();
    let mid = crs.transformer().unwrap().from_lonlat(CENTER).unwrap();
    let (cols, rows, cell) = (1600usize, 1000usize, 30.0);
    let (x0, y0) = (((mid.x - 24_000.0) / cell).round() * cell, ((mid.y + 15_000.0) / cell).round() * cell);
    let data = (0..rows * cols).map(|i| z(x0 + ((i % cols) as f64 + 0.5) * cell, y0 - ((i / cols) as f64 + 0.5) * cell) as f32).collect();
    let gt = GeoTransform { origin_x: x0, origin_y: y0, pixel_width: cell, pixel_height: -cell };
    Raster::new("dem", cols as u32, rows as u32, vec![data], gt, crs, None, ColorHint::Gray, "Float32").unwrap()
}

/// A map in UTM 23S with `dem` as its layer "DEM".
fn utm_map(dem: Raster) -> Map {
    let mut map = Map::new("m");
    map.projection = Projection::Custom(UTM.into());
    map.add_raster("DEM", rustgis_map::RasterLayer::from_path(dem, "dem.tif"));
    map
}

fn frame_kind(sections: Vec<Section>, bearing: f64, pitch: f64) -> ElementKind {
    ElementKind::MapFrame {
        map: "m".into(),
        center: CENTER,
        scale: 200_000.0,
        shape: FrameShape::Rectangle,
        grid: None,
        sections,
        extent_of: None,
        extent_color: INDICATOR_COLOR,
        extent_width: INDICATOR_WIDTH,
        border_color: FRAME_BORDER_COLOR,
        border_width: FRAME_BORDER_WIDTH,
        bearing,
        pitch,
    }
}

/// A page with the Mantiqueira frame, its section line `line` named A, and a profile of "DEM" along
/// it on `side`; the profile's id.
fn page(line: SectionLine, side: ProfileSide, (bearing, pitch): (f64, f64)) -> (Layout, u32) {
    let mut l = Layout::new("L", 297.0, 210.0);
    let frame = l.add("Map", FRAME_MM, frame_kind(vec![Section::new("A".into(), line)], bearing, pitch));
    let p = Profile { title: String::new(), ..Profile::new(frame, "A".into(), "DEM".into(), side) };
    let id = l.add("Profile", p.rect(FRAME_MM), ElementKind::Profile(p));
    (l, id)
}

fn profile_of(l: &Layout, id: u32) -> &Profile {
    match l.element(id).map(|e| &e.kind) {
        Some(ElementKind::Profile(p)) => p,
        _ => panic!("a profile"),
    }
}

/// A profile is read along the page's line through the frame's projection, not along a straight line
/// of longitude and latitude: on a grid whose value is its northing, a horizontal section line of a
/// UTM frame reads one value end to end, and a vertical one on a grid of eastings too. Its samples
/// run from the frame's one edge to the other, a cell of 30 m apart, their distances adding up to
/// the frame's width on the ground.
#[test]
fn a_profile_is_read_along_the_line_on_the_page() {
    let fw = FRAME_MM.2 * PT_PER_MM;
    for (line, z, side) in [
        (SectionLine::Horizontal(AGULHAS_NEGRAS), (|_, n| n - 7_500_000.0) as fn(f64, f64) -> f64, ProfileSide::Bottom),
        (SectionLine::Vertical(PEDRA_DA_MINA), |e, _| e - 500_000.0, ProfileSide::Right),
    ] {
        let maps = vec![utm_map(dem(z))];
        let (l, id) = page(line, side, (0.0, 0.0));
        let (s, section) = profile_samples(&l, &maps, profile_of(&l, id)).unwrap();
        assert_eq!(section.name, "A");
        let (lo, hi) = s.range().unwrap();
        assert!(hi - lo < 0.05, "{line:?}: {lo} to {hi}");
        let length = if side.is_horizontal() { fw } else { FRAME_MM.3 * PT_PER_MM };
        let (first, last) = (s.along.first().copied().unwrap(), s.along.last().copied().unwrap());
        assert!(first.abs() < 1e-6 && (last - length).abs() < 1e-6, "{first} {last}");
        let ground = length / PT_PER_MM * 200.0;
        let total = s.metres.last().copied().unwrap();
        assert!((total / ground - 1.0).abs() < 0.005, "{total} m for {ground} m");
        let n = s.values.len() as f64;
        assert!((n - ground / 30.0).abs() < 0.02 * n, "{n} samples");
        assert!(s.cells.is_none() && s.values.iter().all(Option::is_some));
    }
    // A turned frame reads the line the page shows: turned a quarter, its horizontal runs north to
    // south, its northings falling a metre a metre of ground where the grid (30 km from north to
    // south) has them.
    let maps = vec![utm_map(dem(|_, n| n))];
    let (l, id) = page(SectionLine::Horizontal(CENTER), ProfileSide::Bottom, (90.0, 0.0));
    let (s, _) = profile_samples(&l, &maps, profile_of(&l, id)).unwrap();
    let read: Vec<(f64, f64)> = s.metres.iter().zip(&s.values).filter_map(|(m, v)| Some((*m, (*v)?))).collect();
    assert!(read.len() > 900, "{} read", read.len());
    for w in read.windows(2) {
        let [(m0, v0), (m1, v1)] = w else { continue };
        assert!(((v0 - v1) / (m1 - m0) - 1.0).abs() < 0.01, "{m0}: {v0}, {m1}: {v1}");
    }
}

/// The page's line, not a straight line between its ends' longitudes and latitudes: between the
/// ends of the horizontal section line of the Mantiqueira, a straight line of longitude and
/// latitude strays metres off the line on the page, which a profile doesn't (#2863).
#[test]
fn the_page_line_is_not_a_straight_line_of_longitude_and_latitude() {
    let maps = vec![utm_map(dem(|_, n| n))];
    let (l, id) = page(SectionLine::Horizontal(AGULHAS_NEGRAS), ProfileSide::Bottom, (0.0, 0.0));
    let (s, _) = profile_samples(&l, &maps, profile_of(&l, id)).unwrap();
    let map = maps.first().unwrap();
    let cam = frame_camera(map, CENTER, 200_000.0, 0.0, 0.0);
    let (fw, fh) = (FRAME_MM.2 * PT_PER_MM, FRAME_MM.3 * PT_PER_MM);
    let (_, y) = cam.try_to_screen(map.to_map(AGULHAS_NEGRAS).unwrap(), fw, fh).unwrap();
    let ll = |x: f64| map.to_lonlat(cam.try_to_map(x, y, fw, fh).unwrap()).unwrap();
    let (a, b) = (ll(0.0), ll(fw));
    let r = &map.layers.first().unwrap().raster.as_ref().unwrap().data.clone().unwrap();
    let chord = (0..=100).map(|i| {
        let t = f64::from(i) / 100.0;
        r.bilinear(0, Coord::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)).unwrap()
    });
    let along = s.values.first().copied().flatten().unwrap();
    let off = chord.map(|v| (v - along).abs()).fold(0.0, f64::max);
    assert!(off > 1.0, "the chord strays {off} m");
    assert!(s.values.iter().flatten().all(|v| (v - along).abs() < 0.05));
}

/// The middle of the pixels `at` (a run along the profile, its curve flat at its top), at their
/// centres.
fn middle(at: impl Iterator<Item = u32>) -> f64 {
    let at: Vec<u32> = at.collect();
    at.iter().map(|v| f64::from(*v) + 0.5).sum::<f64>() / at.len().max(1) as f64
}

/// A profile's highest point is drawn straight under the place it comes from, and straight beside it
/// for a vertical section line, whichever way the frame is turned.
#[test]
fn a_profiles_peak_is_drawn_straight_across_from_its_place() {
    let crs = Crs::from_epsg(31983).unwrap();
    let peak_at = crs.transformer().unwrap().from_lonlat(AGULHAS_NEGRAS).unwrap();
    let peak =
        move |e: f64, n: f64| 1000.0 + 1500.0 * (-((e - peak_at.x).powi(2) + (n - peak_at.y).powi(2)) / (2.0 * 900.0f64.powi(2))).exp();
    for (line, side, bearing) in [
        (SectionLine::Horizontal(AGULHAS_NEGRAS), ProfileSide::Bottom, 0.0),
        (SectionLine::Horizontal(AGULHAS_NEGRAS), ProfileSide::Top, 25.0),
        (SectionLine::Vertical(AGULHAS_NEGRAS), ProfileSide::Right, 0.0),
        (SectionLine::Vertical(AGULHAS_NEGRAS), ProfileSide::Left, -15.0),
    ] {
        let mut maps = vec![utm_map(dem(peak))];
        let (mut l, id) = page(line, side, (bearing, 0.0));
        if let Some(ElementKind::Profile(p)) = l.element_mut(id).map(|e| &mut e.kind) {
            (p.guides, p.fill) = (false, None);
        }
        // Where the frame shows the peak, in the page's pixels at 2 a point.
        let map = maps.first().unwrap();
        let (fw, fh) = (FRAME_MM.2 * PT_PER_MM, FRAME_MM.3 * PT_PER_MM);
        let cam = frame_camera(map, CENTER, 200_000.0, bearing, 0.0);
        let (px, py) = cam.try_to_screen(map.to_map(AGULHAS_NEGRAS).unwrap(), fw, fh).unwrap();
        let dpi = 192.0;
        let pm = render(&l, &mut maps, dpi, &PageInfo::default()).unwrap();
        let (_, _, k) = page_geometry(&l, dpi);
        let ppp = k / PT_PER_MM;
        let e = l.element(id).unwrap();
        let (x0, y0, x1, y1) = ((e.x * k) as u32, (e.y * k) as u32, ((e.x + e.w) * k) as u32, ((e.y + e.h) * k) as u32);
        let dark = |x: u32, y: u32| pm.pixel(x, y).is_some_and(|p| p.red() < 90 && p.green() < 90 && p.blue() < 90 && p.alpha() > 200);
        // The curve's point farthest from its base, in the middle of the profile (its numbers and
        // names are at its ends).
        let found = if side.is_horizontal() {
            let (lo, hi) = (x0 + (x1 - x0) / 5, x1 - (x1 - x0) / 5);
            let top = |x: u32| (y0..y1).find(|y| dark(x, *y)).unwrap_or(y1);
            let highest = (lo..hi).map(top).min().unwrap();
            middle((lo..hi).filter(|x| top(*x) == highest)) - FRAME_MM.0 * k - px * ppp
        } else {
            let (lo, hi) = (y0 + (y1 - y0) / 5, y1 - (y1 - y0) / 5);
            let reach = |y: u32| match side {
                ProfileSide::Right => (x0..x1).rev().find(|x| dark(*x, y)).map_or(0, |x| x - x0),
                _ => (x0..x1).find(|x| dark(*x, y)).map_or(0, |x| x1 - x),
            };
            let farthest = (lo..hi).map(reach).max().unwrap();
            middle((lo..hi).filter(|y| reach(*y) == farthest)) - FRAME_MM.1 * k - py * ppp
        };
        assert!(found.abs() <= 3.0, "{side:?} turned {bearing}: {found} pixels off");
    }
}

/// What keeps a profile from being drawn is said, one notice with each profile and why: a tilted
/// frame, a section line running the other way, a band the raster hasn't, a raster read in place, a
/// section line that isn't there, and no data along the line. A drawable one says nothing.
#[test]
fn a_profile_not_drawn_says_why() {
    let notes = |l: &Layout, maps: &[Map]| {
        let n = profiles_not_drawn(l, maps);
        profiles_notice(&n.into_iter().map(|n| (n, 1)).collect(), 1)
    };
    let maps = vec![utm_map(dem(|_, n| n))];
    let (l, _) = page(SectionLine::Horizontal(CENTER), ProfileSide::Bottom, (0.0, 0.0));
    assert!(notes(&l, &maps).is_empty(), "{:?}", notes(&l, &maps));
    let (tilted, _) = page(SectionLine::Horizontal(CENTER), ProfileSide::Bottom, (0.0, 40.0));
    let said = notes(&tilted, &maps);
    assert!(said.len() == 1 && said[0].starts_with("1 profile(s) not drawn: 'Profile' (2): its frame is tilted"), "{said:?}");
    let (across, _) = page(SectionLine::Horizontal(CENTER), ProfileSide::Right, (0.0, 0.0));
    assert!(notes(&across, &maps)[0].contains("a horizontal section line's profile goes under or over its frame, not right"));
    let change = |f: &dyn Fn(&mut Profile)| {
        let (mut l, id) = page(SectionLine::Horizontal(CENTER), ProfileSide::Bottom, (0.0, 0.0));
        if let Some(ElementKind::Profile(p)) = l.element_mut(id).map(|e| &mut e.kind) {
            f(p);
        }
        notes(&l, &maps).join(" | ")
    };
    assert!(change(&|p| p.band = 2).contains("its layer 'DEM' has no band 2 (it has 1)"));
    assert!(change(&|p| p.section = "B".into()).contains("its frame has no section line 'B'"));
    assert!(change(&|p| p.layer = "Other".into()).contains("the frame's map has no raster layer 'Other'"));
    let mut in_place = utm_map(dem(|_, n| n));
    if let Some(r) = in_place.layers.first_mut().and_then(|l| l.raster.as_mut()) {
        r.in_place = true;
    }
    assert!(notes(&l, &[in_place]).join("").contains("is read in place, a part at a time, not held whole"));
    let empty = vec![utm_map(dem(|_, _| f64::NAN))];
    assert!(notes(&l, &empty).join("").contains("has no data along section line 'A'"));
    // Drawn, it draws its placeholder otherwise.
    let mut maps = maps;
    assert!(render(&tilted, &mut maps, 96.0, &PageInfo::default()).is_ok());
}

/// A gap in the data breaks the curve, and a value axis given keeps to it: the samples' own ends
/// otherwise, out to round numbers.
#[test]
fn a_profiles_value_axis_is_round_or_given() {
    let base = Profile::new(1, "A".into(), "DEM".into(), ProfileSide::Bottom);
    assert_eq!(value_axis(&base, (412.0, 2_791.0)), (0.0, 3_000.0, 1_000.0));
    let (lo, hi, step) = value_axis(&base, (12.3, 12.9));
    assert!((lo - 12.2).abs() < 1e-9 && (hi - 13.0).abs() < 1e-9 && (step - 0.2).abs() < 1e-12, "{lo} {hi} {step}");
    let given = Profile { min: Some(400.0), max: Some(2_900.0), ..base.clone() };
    assert_eq!(value_axis(&given, (412.0, 2_791.0)).0, 400.0);
    assert_eq!(value_axis(&given, (412.0, 2_791.0)).1, 2_900.0);
    // Flat, it still has a span.
    let (lo, hi, _) = value_axis(&base, (5.0, 5.0));
    assert!(lo < 5.0 && hi > 5.0);
    // A gap of no data along the line: two runs, each drawn.
    let mid = Crs::from_epsg(31983).unwrap().transformer().unwrap().from_lonlat(CENTER).unwrap().x;
    let maps = vec![utm_map(dem(move |e, _| if (e - mid).abs() < 2_000.0 { f64::NAN } else { 100.0 }))];
    let (l, id) = page(SectionLine::Horizontal(CENTER), ProfileSide::Bottom, (0.0, 0.0));
    let (s, _) = profile_samples(&l, &maps, profile_of(&l, id)).unwrap();
    let runs = s.values.windows(2).filter(|w| w[0].is_some() != w[1].is_some()).count();
    assert_eq!(runs, 2, "a gap in the middle");
}
