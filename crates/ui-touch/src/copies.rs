//! The copies of picked files a phone keeps (#1902). A file picked on a phone is copied into the
//! app's storage (`picked/<pick>/…`, by Android's activity and by the iOS shell), and data and
//! projects open from the copy and are edited there. Nothing removes a copy by itself: a project
//! saved on the phone names its data by the copies' paths, so a copy that looks unused may still be
//! wanted. Remove Unused Copies, in the More sheet, removes the picks the project in the window
//! doesn't name, once the user has seen how many and how much, and agreed.
//!
//! A pick's folder is kept or removed whole: a project names a Shapefile's `.shp`, not its `.dbf`,
//! nor a GeoPackage's `-wal`, so a pick is in use when the project names any file in it.

use rustgis_ui_egui::rustgis_engine::data::dirs;
use rustgis_ui_egui::{RustGisApp, i18n, tl};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The deepest a file of a pick is (`picked/<pick>/<source folder>/<file>`), looked for a level
/// deeper: a pick holds no deeper folders, and a link out of it is never followed.
const DEPTH: usize = 3;

/// The picks the project doesn't name.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Unused {
    /// Each pick's folder (or a loose file directly in `picked/`).
    pub picks: Vec<PathBuf>,
    /// The files in them.
    pub files: usize,
    /// Their size, in bytes.
    pub bytes: u64,
}

/// What was removed: files, bytes, and what couldn't be (each in its own words).
type Removed = (usize, u64, Vec<String>);

/// Where the copies are, where the app has a folder of its own (a phone).
pub(crate) fn folder() -> Option<PathBuf> {
    dirs::root().map(|root| root.join("picked"))
}

/// The picks in `picked` none of whose files `project` (the project as it is saved, in JSON)
/// names, looking at each between picks for `cancelled`.
pub(crate) fn unused(picked: &Path, project: &str, cancelled: &AtomicBool) -> Result<Unused, String> {
    let entries = std::fs::read_dir(picked).map_err(|e| format!("{}: {e}", picked.display()))?;
    let mut found = Unused::default();
    let mut picks: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    picks.sort();
    for pick in picks {
        if cancelled.load(Ordering::Relaxed) {
            return Err(tl!("Stopped.").to_string());
        }
        let mut files = Vec::new();
        gather(&pick, DEPTH, &mut files);
        if files.iter().any(|(file, _)| named(project, file)) {
            continue;
        }
        found.files += files.len();
        found.bytes += files.iter().map(|(_, size)| size).sum::<u64>();
        found.picks.push(pick);
    }
    Ok(found)
}

/// The files at or under `path` (itself, if it is one), with their sizes, `depth` levels down at
/// most. Links aren't followed.
fn gather(path: &Path, depth: usize, files: &mut Vec<(PathBuf, u64)>) {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return };
    if meta.is_file() {
        files.push((path.to_path_buf(), meta.len()));
    } else if meta.is_dir() && depth > 0 {
        for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
            gather(&entry.path(), depth - 1, files);
        }
    }
}

/// Whether the project's JSON names `file`, written as JSON writes it.
fn named(project: &str, file: &Path) -> bool {
    let text = file.to_string_lossy();
    let quoted = serde_json::to_string(text.as_ref()).unwrap_or_default();
    let escaped = quoted.strip_prefix('"').and_then(|q| q.strip_suffix('"')).unwrap_or(&quoted);
    !escaped.is_empty() && project.contains(escaped)
}

/// Removes `picks` (folders or files), looking for `cancelled` between them.
fn remove(picks: &[PathBuf], cancelled: &AtomicBool) -> Removed {
    let (mut files, mut bytes, mut failed) = (0usize, 0u64, Vec::new());
    for pick in picks {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        let mut held = Vec::new();
        gather(pick, DEPTH, &mut held);
        let gone = if pick.is_dir() { std::fs::remove_dir_all(pick) } else { std::fs::remove_file(pick) };
        match gone {
            Ok(()) => {
                files += held.len();
                bytes += held.iter().map(|(_, size)| size).sum::<u64>();
            }
            Err(e) => failed.push(format!("{}: {e}", pick.display())),
        }
    }
    (files, bytes, failed)
}

/// Where Remove Unused Copies is.
#[derive(Clone, Default)]
pub(crate) enum Job {
    #[default]
    Idle,
    /// Looking for the unused copies, off the window's thread.
    Scanning(Arc<Mutex<Option<Result<Unused, String>>>>, Arc<AtomicBool>),
    /// Found: waiting for the user to agree.
    Asking(Unused),
    /// Removing them, off the window's thread.
    Removing(Arc<Mutex<Option<Removed>>>, Arc<AtomicBool>),
}

fn id() -> egui::Id {
    egui::Id::unique("touch_copies")
}

pub(crate) fn job(ctx: &egui::Context) -> Job {
    ctx.data(|d| d.get_temp::<Job>(id())).unwrap_or_default()
}

fn set_job(ctx: &egui::Context, job: Job) {
    ctx.data_mut(|d| d.insert_temp(id(), job));
}

/// Starts looking for the copies the project doesn't name. The project is copied on the window's
/// thread (one step, without Cancel); it is written out and the copies looked at off it.
pub(crate) fn start(app: &mut RustGisApp, ctx: &egui::Context) {
    let Some(picked) = folder() else { return };
    let project = app.session.project.clone();
    let (result, cancelled) = (Arc::new(Mutex::new(None)), Arc::new(AtomicBool::new(false)));
    let (out, stop, wake) = (result.clone(), cancelled.clone(), ctx.clone());
    let started = std::thread::Builder::new().name("unused copies".into()).spawn(move || {
        let found = serde_json::to_string(&project).map_err(|e| e.to_string()).and_then(|text| unused(&picked, &text, &stop));
        *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(found);
        wake.request_repaint();
    });
    match started {
        Ok(_) => set_job(ctx, Job::Scanning(result, cancelled)),
        Err(e) => app.session.notify(i18n::fmt("The unused copies weren't looked for: {error}", &[("error", &e.to_string())]), true),
    }
}

/// Removes the copies the user agreed to, off the window's thread.
pub(crate) fn confirm(app: &mut RustGisApp, ctx: &egui::Context, found: Unused) {
    let (result, cancelled) = (Arc::new(Mutex::new(None)), Arc::new(AtomicBool::new(false)));
    let (out, stop, wake) = (result.clone(), cancelled.clone(), ctx.clone());
    let started = std::thread::Builder::new().name("removing copies".into()).spawn(move || {
        let removed = remove(&found.picks, &stop);
        *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(removed);
        wake.request_repaint();
    });
    match started {
        Ok(_) => set_job(ctx, Job::Removing(result, cancelled)),
        Err(e) => {
            set_job(ctx, Job::Idle);
            app.session.notify(i18n::fmt("The unused copies weren't removed: {error}", &[("error", &e.to_string())]), true);
        }
    }
}

/// Lets go of what was found, or stops what is running.
pub(crate) fn cancel(ctx: &egui::Context) {
    match job(ctx) {
        Job::Scanning(_, stop) | Job::Removing(_, stop) => stop.store(true, Ordering::Relaxed),
        Job::Asking(_) | Job::Idle => set_job(ctx, Job::Idle),
    }
}

/// A size for people: megabytes with one decimal, or kilobytes under a tenth of a megabyte.
pub(crate) fn size(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 0.1 {
        i18n::fmt("{n} MB", &[("n", &format!("{mb:.1}"))])
    } else {
        i18n::fmt("{n} KB", &[("n", &bytes.div_ceil(1024).to_string())])
    }
}

/// Takes what the threads found, each frame: what is found is asked about, what was removed is
/// said.
pub(crate) fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
    match job(ctx) {
        Job::Idle | Job::Asking(_) => {}
        Job::Scanning(result, stop) => match result.lock().unwrap_or_else(|e| e.into_inner()).take() {
            None => ctx.request_repaint_after(std::time::Duration::from_millis(250)),
            Some(Ok(found)) if found.picks.is_empty() => {
                set_job(ctx, Job::Idle);
                app.session.notify(tl!("No copies of picked files are unused: the project names every one."), false);
            }
            Some(Ok(found)) => set_job(ctx, Job::Asking(found)),
            Some(Err(_)) if stop.load(Ordering::Relaxed) => set_job(ctx, Job::Idle),
            Some(Err(error)) => {
                set_job(ctx, Job::Idle);
                app.session.notify(i18n::fmt("The unused copies weren't looked for: {error}", &[("error", &error)]), true);
            }
        },
        Job::Removing(result, stop) => match result.lock().unwrap_or_else(|e| e.into_inner()).take() {
            None => ctx.request_repaint_after(std::time::Duration::from_millis(250)),
            Some((files, bytes, failed)) => {
                set_job(ctx, Job::Idle);
                let (count, size) = (files.to_string(), size(bytes));
                let template = if files == 1 {
                    "Removed 1 copy of a picked file ({size})."
                } else {
                    "Removed {count} copies of picked files ({size})."
                };
                let mut text = i18n::fmt(template, &[("count", &count), ("size", &size)]);
                if stop.load(Ordering::Relaxed) {
                    text.push(' ');
                    text.push_str(tl!("Stopped before the rest."));
                }
                if let Some(first) = failed.first() {
                    let (n, first) = (failed.len().to_string(), first.clone());
                    text.push(' ');
                    text.push_str(&i18n::fmt("{n} couldn't be removed, the first: {first}", &[("n", &n), ("first", &first)]));
                }
                app.session.notify(text, !failed.is_empty());
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pick_is_kept_whole_when_the_project_names_any_file_in_it() {
        let root = std::env::temp_dir().join(format!("rustgis-copies-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let picked = root.join("picked");
        // A Shapefile the project names by its .shp (its .dbf beside it), and a GeoPackage it
        // doesn't name, from another pick; a loose file too.
        let shapes = picked.join("1/0");
        let unused_pick = picked.join("2/0");
        std::fs::create_dir_all(&shapes).unwrap();
        std::fs::create_dir_all(&unused_pick).unwrap();
        std::fs::write(shapes.join("roads.shp"), b"shp").unwrap();
        std::fs::write(shapes.join("roads.dbf"), b"dbf").unwrap();
        std::fs::write(unused_pick.join("parcels.gpkg"), b"0123456789").unwrap();
        std::fs::write(picked.join("loose.csv"), b"a,b").unwrap();
        let project = serde_json::json!({"layers": [{"source": shapes.join("roads.shp").to_string_lossy()}]}).to_string();
        let found = unused(&picked, &project, &AtomicBool::new(false)).unwrap();
        assert_eq!(found.picks, vec![picked.join("2"), picked.join("loose.csv")]);
        assert_eq!((found.files, found.bytes), (2, 13));
        // Removed: the named pick stays, sidecar and all.
        let (files, bytes, failed) = remove(&found.picks, &AtomicBool::new(false));
        assert_eq!((files, bytes, failed.len()), (2, 13, 0));
        assert!(shapes.join("roads.dbf").exists());
        assert!(!picked.join("2").exists() && !picked.join("loose.csv").exists());
        // Cancelled, nothing is looked at.
        assert!(unused(&picked, &project, &AtomicBool::new(true)).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_path_is_matched_as_json_writes_it() {
        let file = Path::new("/data/picked/1/0/a \"quoted\" name.geojson");
        let project = serde_json::json!({"path": file.to_string_lossy()}).to_string();
        assert!(named(&project, file));
        assert!(!named(&project, Path::new("/data/picked/1/0/other.geojson")));
    }

    #[test]
    fn sizes_are_said_in_megabytes_or_kilobytes() {
        assert_eq!(size(420), "1 KB");
        assert_eq!(size(50 * 1024), "50 KB");
        assert_eq!(size(3 * 1024 * 1024 + 300 * 1024), "3.3 MB");
    }
}
