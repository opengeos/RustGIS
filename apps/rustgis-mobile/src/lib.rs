//! RustGIS on Android and iOS (#1902).
//!
//! The desktop front end (`rustgis-ui-egui`) with the touch interface, in a `NativeActivity` on
//! Android and a UIKit window on iOS, with the phone (a `rustgis_engine::platform::Platform`)
//! standing where the browser stands for the web app: the system's file picker, the device's
//! position, the camera. `shell` is what both share. See `README.md` for how to build and install
//! them.
//!
//! Not here yet: preferences and the keychain; autosave.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(any(target_os = "android", target_os = "ios"))]
mod shell;

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
mod phone;

#[cfg(target_os = "ios")]
mod ios;
#[cfg(target_os = "ios")]
pub use ios::main as ios_main;

/// The files of `picked` to open, each by its path: all but the parts another of them reads from
/// beside it (a Shapefile's `.dbf`, `.shx`, `.prj` and `.cpg`, a MapInfo file's `.mid`), matched
/// by name whatever the case. A part picked without its main file stays, and is refused by name.
#[cfg_attr(not(any(target_os = "android", target_os = "ios", test)), expect(dead_code, reason = "only a phone picks files"))]
fn without_sidecars(picked: Vec<std::path::PathBuf>) -> Vec<std::path::PathBuf> {
    let lower = |part: Option<&std::ffi::OsStr>| part.map(|p| p.to_string_lossy().to_lowercase()).unwrap_or_default();
    let named: Vec<(String, String)> = picked.iter().map(|p| (lower(p.file_stem()), lower(p.extension()))).collect();
    let has = |stem: &str, extension: &str| named.iter().any(|(s, e)| s == stem && e == extension);
    let mut kept = Vec::new();
    for (path, (stem, extension)) in picked.iter().zip(&named) {
        let read_by = match extension.as_str() {
            "dbf" | "shx" | "prj" | "cpg" => "shp",
            "mid" => "mif",
            _ => "",
        };
        if read_by.is_empty() || !has(stem, read_by) {
            kept.push(path.clone());
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::without_sidecars;
    use std::path::PathBuf;

    fn kept(names: &[&str]) -> Vec<String> {
        without_sidecars(names.iter().map(|n| PathBuf::from("/picked/1").join(n)).collect())
            .iter()
            .map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
            .collect()
    }

    #[test]
    fn parts_picked_with_their_file_are_left_to_it() {
        assert_eq!(kept(&["roads.shp", "roads.dbf", "roads.shx", "roads.prj", "roads.cpg"]), ["roads.shp"]);
        assert_eq!(kept(&["ROADS.SHP", "Roads.DBF", "parcels.mif", "parcels.MID"]), ["ROADS.SHP", "parcels.mif"]);
    }

    #[test]
    fn a_part_picked_alone_is_kept_to_be_refused_by_name() {
        assert_eq!(kept(&["roads.dbf", "lakes.shp", "notes.prj"]), ["roads.dbf", "lakes.shp", "notes.prj"]);
        // Other files are never parts.
        assert_eq!(kept(&["a.gpkg", "a.geojson"]), ["a.gpkg", "a.geojson"]);
    }
}
