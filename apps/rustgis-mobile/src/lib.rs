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

/// What starts the name of a file from the picker for `purpose` in the inbox, as the browser's
/// shell marks them: the front end sends a marked file to the dialog that asked for it. A DXF
/// request (`dxf:<id>`) carries its id, as Import DXF matches its answer by it.
#[cfg(any(target_os = "android", target_os = "ios", test))]
fn mark(purpose: &str) -> String {
    if let Some(id) = purpose.strip_prefix("dxf:") {
        return format!("{}{id}:", rustgis_ui_egui::DXF_PICK);
    }
    match purpose {
        "shortcuts" => rustgis_ui_egui::SHORTCUT_PICK,
        "text" => rustgis_ui_egui::DELIMITED_PICK,
        "shapefile" => rustgis_ui_egui::SHAPEFILE_PICK,
        p if p.starts_with(rustgis_ui_egui::TOOL_FILE_PURPOSE) => rustgis_ui_egui::TOOL_FILE_PICK,
        _ => "",
    }
    .to_string()
}

/// What the phone reported, oldest first, until the front end takes it: at most `most`
/// positions. Past that the oldest position goes, and how many went is said when they are
/// taken; nothing else is dropped (a picked file, a failure), as positions are what pile up, out
/// of sight for days.
#[cfg(any(target_os = "ios", test))]
struct Queue {
    events: std::collections::VecDeque<rustgis_engine::platform::Event>,
    most: usize,
    positions: usize,
    dropped: usize,
}

#[cfg(any(target_os = "ios", test))]
impl Queue {
    fn new(most: usize) -> Queue {
        Queue { events: Default::default(), most: most.max(1), positions: 0, dropped: 0 }
    }

    fn add(&mut self, event: rustgis_engine::platform::Event) {
        use rustgis_engine::platform::Event;
        if matches!(event, Event::Position(_)) {
            if self.positions >= self.most
                && let Some(oldest) = self.events.iter().position(|e| matches!(e, Event::Position(_)))
            {
                self.events.remove(oldest);
                self.positions -= 1;
                self.dropped += 1;
            }
            self.positions += 1;
        }
        self.events.push_back(event);
    }

    fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Everything waiting, oldest first, after a failure that says how many positions were
    /// dropped, when any were.
    fn take(&mut self) -> Vec<rustgis_engine::platform::Event> {
        let mut taken = Vec::with_capacity(self.events.len() + 1);
        if self.dropped > 0 {
            let text = rustgis_ui_egui::i18n::fmt(
                "{count} positions found while RustGIS was out of sight were dropped: more were waiting than it keeps.",
                &[("count", &self.dropped.to_string())],
            );
            taken.push(rustgis_engine::platform::Event::Failed(text));
        }
        taken.extend(self.events.drain(..));
        self.positions = 0;
        self.dropped = 0;
        taken
    }
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
    fn a_dxf_request_keeps_its_id_in_its_mark() {
        assert_eq!(super::mark("dxf:7"), format!("{}7:", rustgis_ui_egui::DXF_PICK));
        assert_eq!(super::mark("shapefile"), rustgis_ui_egui::SHAPEFILE_PICK);
        assert_eq!(super::mark("data"), "");
    }

    #[test]
    fn the_oldest_positions_go_past_the_limit_and_are_counted() {
        use rustgis_engine::platform::{Event, Fix};
        let at = |lon: f64| Event::Position(Fix { lon, lat: 0.0, accuracy: None, time: None });
        let mut queue = super::Queue::new(2);
        queue.add(Event::Cancelled);
        for lon in [1.0, 2.0, 3.0] {
            queue.add(at(lon));
        }
        let taken = queue.take();
        // The dropped position is said first; the picker's answer is kept.
        assert!(matches!(&taken[0], Event::Failed(text) if text.starts_with("1 positions")), "{taken:?}");
        assert_eq!(&taken[1..], &[Event::Cancelled, at(2.0), at(3.0)]);
        // Taken, the queue starts again: under the limit, nothing is said.
        queue.add(at(4.0));
        assert_eq!(queue.take(), vec![at(4.0)]);
        assert!(queue.is_empty());
    }

    #[test]
    fn a_part_picked_alone_is_kept_to_be_refused_by_name() {
        assert_eq!(kept(&["roads.dbf", "lakes.shp", "notes.prj"]), ["roads.dbf", "lakes.shp", "notes.prj"]);
        // Other files are never parts.
        assert_eq!(kept(&["a.gpkg", "a.geojson"]), ["a.gpkg", "a.geojson"]);
    }
}
