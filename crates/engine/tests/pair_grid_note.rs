//! A conversion between two systems whose direct operation needs a datum grid RustGIS doesn't have
//! is said as a system's is: by both systems' names, pointing to the setting that downloads it.

use rustgis_data::raster::Crs;
use rustgis_engine::{Project, Session};

/// LV03 to LV95's best transformation is by swisstopo's CHENyx06 grid, which neither system's own
/// conversion to WGS 84 needs; without it the next is used, and the session says so once, after
/// the command that converted.
#[test]
fn a_pairs_missing_grid_is_said_once() {
    for offers in [true, false] {
        let mut s = Session::new(Project::default());
        s.offers_grid_downloads = offers;
        // What a command converts on the session's thread (an export to another system).
        Crs::Epsg(21781).to(Crs::Epsg(2056)).unwrap();
        s.notice_landed_rasters();
        let texts: Vec<&String> = s.notices.iter().map(|n| &n.text).collect();
        let notes: Vec<&&String> = texts.iter().filter(|t| t.contains("EPSG:21781 to EPSG:2056")).collect();
        assert_eq!(notes.len(), 1, "{texts:?}");
        assert!(notes[0].contains("needs a grid"), "{texts:?}");
        assert_eq!(notes[0].contains("Settings › Datum Grids › Download Datum Grids"), offers, "{texts:?}");
        // Not again for the same pair.
        Crs::Epsg(21781).to(Crs::Epsg(2056)).unwrap();
        s.notice_landed_rasters();
        assert_eq!(s.notices.iter().filter(|n| n.text.contains("EPSG:21781 to EPSG:2056")).count(), 1);
    }
}
