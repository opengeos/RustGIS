//! Exports that write a file from what the session shares (a layer's features, a raster's
//! pixels, a copy of the map or project to draw). A script or the command line writes it where
//! the command is called; the app writes it on a thread of its own with progress and Cancel
//! (#1291 step 3): [`export_start`] reads the session, [`ExportRun::run`] writes the file, and
//! [`export_land`] says what was written, as the command would have. The admitted operation owns
//! preparation, file commit and delivery; a completed write remains written even if delivery
//! is subsequently refused.
//!
//! The browser has no filesystem to write a path in ([`Session::downloads_files`]): there
//! [`write_here`] names the export after the path's file, its work gives the bytes back instead
//! of writing them ([`place`]), and the front end offers them as a download, as it does a tool's
//! file output (`gp.fileDelivered`). A command that saves, packages or shares a file at once
//! ([`file_command`], [`download_command`]) is downloaded the same way, and one that needs the
//! file to stay where it is refuses.

use std::borrow::Cow;
use std::io::Write;
use std::sync::Arc;

use rustgis_data::capped::Limit;
use rustgis_data::{DataError, Dataset};
use serde::Serialize;
use serde_json::Value;

use crate::{CmdResult, Session};

/// What an export wrote: the command's answer, and what it says of the file (the last says it
/// was exported, unless `then` says it).
pub struct Wrote {
    pub(crate) answer: Value,
    /// What it says once written, each loss with its code.
    pub(crate) notes: rustgis_data::loss::Notes,
    /// Drawing diagnostics available before the session finalizer, with their original severity.
    pub(crate) diagnostics: Vec<(rustgis_data::diagnostics::Severity, String)>,
    /// Source failures to report if delivery cannot update their live layers.
    pub(crate) refused_notes: Vec<String>,
    /// A physical write completed, even if authority to deliver its result is later refused.
    /// With `download` it is the download's name, and nothing is written yet.
    pub(crate) written_path: Option<String>,
    /// The file's bytes when it is downloaded rather than written ([`place`]): what it says
    /// (`notes`, `then`) is said once the front end has taken the download.
    pub(crate) download: Option<Arc<Vec<u8>>>,
    /// What it does to the session once written, after its notes, and adds to its answer (a
    /// GeoPackage table replaced: the layers reading it read it again).
    pub(crate) then: Option<Then>,
    /// Authority retained for delivery from the native export worker.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) owner: Option<crate::policy::Continuation>,
    /// The coordinate systems the worker thread used, as (definition, label), for the session to
    /// say which needed a missing datum grid (`Session::say_grid_notes`).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) crs_used: Vec<(String, String)>,
}

impl Wrote {
    /// What it will say or has said of the file, as plain text.
    pub(crate) fn completion_notes(&self) -> Vec<String> {
        self.notes
            .texts()
            .chain(self.diagnostics.iter().map(|(_, text)| text.as_str()))
            .chain(self.refused_notes.iter().map(String::as_str))
            .map(str::to_string)
            .collect()
    }
}

/// Completes an export on the live session and may update its answer. A field package's finalizer
/// checks that its home still accepts the new area or identity before installing the staged file.
pub(crate) type Then = Box<dyn FnOnce(&mut Session, &mut Value) -> Result<(), String> + Send>;

/// The writing of an export, its inputs read: it needs no session.
pub(crate) type Work = Box<dyn FnOnce() -> Result<Wrote, String> + Send>;

/// What an export reads of the session, and the work that writes its file.
type Start = fn(&Session, &Value) -> Result<Work, String>;

/// The exports the app writes on another thread ([`export_start`]), each with what it is called
/// (in its progress, and a cancelled run's error) and what it reads of the session.
pub(crate) const IN_BACKGROUND: &[(&str, &str, Start)] = &[
    ("data.convert", "Data Interop", super::workflows::convert_work),
    ("share.exportLayer", "Export Features", super::share::layer_work),
    ("share.exportTable", "Export Table", super::share::table_work),
    ("share.exportRaster", "Export Raster", super::raster::export_work),
    ("share.exportMap", "Export Map", super::share::map_work),
    ("map.exportOrbit", "Export Orbit", super::view::orbit_work),
    ("map.exportBlock", "Export Block", super::block_model::export_work),
    ("layout.export", "Export Layout", super::layout::export_work),
    ("report.export", "Export Report", super::report::export_work),
    ("source.publish", "Publish to Source", super::publish::publish_work),
    #[cfg(not(target_arch = "wasm32"))]
    ("offline.package", "Package for the Field", super::field::export_work),
];

/// `command` and what it is called, when the app writes its file on another thread (see
/// [`export_start`]).
pub fn runs_in_background(command: &str) -> Option<(&'static str, &'static str)> {
    IN_BACKGROUND.iter().find(|(c, ..)| *c == command).map(|(c, name, _)| (*c, *name))
}

/// Puts an export's finished file where it goes: written at `path`, or, when it is downloaded
/// (`download`, [`Session::downloads_files`]), given back for [`Wrote::download`]. The bytes are
/// never copied: a file a layer holds is shared with the download.
pub(crate) fn place(download: bool, path: &str, bytes: impl Into<Arc<Vec<u8>>>) -> Result<Option<Arc<Vec<u8>>>, String> {
    let bytes = bytes.into();
    if download {
        return Ok(Some(bytes));
    }
    rustgis_data::io::write_atomic(std::path::Path::new(path), &bytes).map_err(|e| format!("{path}: {e}"))?;
    Ok(None)
}

/// The most files that wait for download at once.
pub(crate) const MAX_DOWNLOADS: usize = 8;

/// The room for one more download: what the files waiting leave of the most bytes they may hold
/// in all ([`Session::download_room`]).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Room {
    pub(crate) bytes: usize,
    /// What the files already waiting hold.
    pub(crate) waiting: usize,
}

impl Room {
    /// Why `path`'s file, stopped as it passed the room, can't be downloaded, and what to do.
    pub(crate) fn refusal(self, path: &str) -> String {
        let cap = mib(self.bytes.saturating_add(self.waiting));
        if self.waiting == 0 {
            format!("{path}: stopped once the file passed the {cap} MiB a command can download in the browser; {SMALLER}")
        } else {
            format!(
                "{path}: stopped once the file passed what is left of the {cap} MiB that downloads can hold in the browser, {} MiB of it taken by those already waiting; split the task, model, or script into smaller runs, {SMALLER}",
                mib(self.waiting)
            )
        }
    }

    /// `path`'s file as `make` encodes it, stopped as soon as it passes the room, so no more of
    /// it is ever held.
    pub(crate) fn encode<T>(self, path: &str, make: impl FnOnce(&Limit) -> rustgis_data::Result<T>) -> Result<T, String> {
        let limit = Limit::new(self.bytes);
        make(&limit).map_err(|e| if limit.passed() { self.refusal(path) } else { format!("{path}: {e}") })
    }
}

/// What to do about a file too large to download.
pub(crate) const SMALLER: &str =
    "make a smaller file (fewer features or layers, a smaller size or fewer pages), use the ribbon's buttons, or use the desktop app";

/// The file a command makes at once ([`file_command`]): the path it is for, and, where it is
/// downloaded, the room it has. A file that is written has no limit here but memory.
#[derive(Clone, Copy)]
pub(crate) struct Output<'a> {
    pub(crate) path: &'a str,
    room: Option<Room>,
}

impl<'a> Output<'a> {
    /// A file that is written, or kept in the session: nothing limits it here.
    pub(crate) const UNLIMITED: Output<'static> = Output { path: "", room: None };

    /// Whether it is made to be downloaded.
    pub(crate) fn downloads(self) -> bool {
        self.room.is_some()
    }

    /// The most bytes it may come to.
    pub(crate) fn most(self) -> usize {
        self.room.map_or(usize::MAX, |room| room.bytes)
    }

    /// Why it can't be downloaded, having passed the room.
    pub(crate) fn refusal(self) -> String {
        self.room.map_or_else(|| format!("{}: the file would be too large", self.path), |room| room.refusal(self.path))
    }

    /// Refuses once `held` bytes of the file, or of what it is made of, pass the room.
    pub(crate) fn check(self, held: usize) -> Result<(), String> {
        if held > self.most() { Err(self.refusal()) } else { Ok(()) }
    }

    /// The file `write` makes, stopped as soon as it passes the room.
    pub(crate) fn file(self, write: impl FnOnce(&mut dyn Write) -> Result<(), String>) -> Result<Vec<u8>, String> {
        self.file_up_to(usize::MAX, || self.refusal(), write)
    }

    /// [`Output::file`] for a kind of file that holds `most` bytes, refused past them with
    /// `too_large`; where the room is less, past that, with its refusal.
    pub(crate) fn file_up_to(
        self,
        most: usize,
        too_large: impl FnOnce() -> String,
        write: impl FnOnce(&mut dyn Write) -> Result<(), String>,
    ) -> Result<Vec<u8>, String> {
        let limit = Limit::new(most.min(self.most()));
        let made = made_within(&limit, write);
        if limit.passed() {
            return Err(if self.most() < most { self.refusal() } else { too_large() });
        }
        made
    }

    /// `value` as compact JSON, stopped at the room.
    pub(crate) fn json<T: Serialize + ?Sized>(self, value: &T) -> Result<Vec<u8>, String> {
        self.file(|out| serde_json::to_writer(out, value).map_err(|e| e.to_string()))
    }

    /// `value` as indented JSON, stopped at the room.
    pub(crate) fn json_pretty<T: Serialize + ?Sized>(self, value: &T) -> Result<Vec<u8>, String> {
        self.file(|out| serde_json::to_writer_pretty(out, value).map_err(|e| e.to_string()))
    }

    /// A zip archive whose entries are made one at a time ([`Parts`]).
    pub(crate) fn parts(self) -> Parts<'a> {
        Parts { out: self, held: 0, entries: Vec::new() }
    }
}

/// The file `write` makes within `limit` ([`Limit::file`]), or why it didn't, in its own words.
fn made_within(limit: &Limit, write: impl FnOnce(&mut dyn Write) -> Result<(), String>) -> Result<Vec<u8>, String> {
    let mut failed = None;
    let made = limit.file(|out| {
        write(out).map_err(|why| {
            failed = Some(why);
            DataError::Format(String::new())
        })
    });
    made.map_err(|e| failed.unwrap_or_else(|| e.to_string()))
}

/// The entries of a zip archive as they are made, together held to its file's room, then the
/// archive made within what they leave of it ([`rustgis_data::zip::write_within`]).
pub(crate) struct Parts<'a> {
    out: Output<'a>,
    held: usize,
    entries: Vec<(String, Vec<u8>)>,
}

impl Parts<'_> {
    /// Adds the entry `name` that `write` makes, stopped as the entries pass the room.
    pub(crate) fn make(&mut self, name: String, write: impl FnOnce(&mut dyn Write) -> Result<(), String>) -> Result<(), String> {
        let limit = Limit::new(self.out.most().saturating_sub(self.held));
        let made = made_within(&limit, write);
        if limit.passed() {
            return Err(self.out.refusal());
        }
        self.push(name, made?)
    }

    /// Adds the entry `name`, made already, refused once the entries pass the room.
    pub(crate) fn push(&mut self, name: String, mut bytes: Vec<u8>) -> Result<(), String> {
        // Spare room counts against the archive's limit while it is made.
        bytes.shrink_to_fit();
        self.held = self.held.saturating_add(bytes.len());
        self.out.check(self.held)?;
        self.entries.push((name, bytes));
        Ok(())
    }

    /// The archive of the entries.
    pub(crate) fn zip(self) -> Result<Vec<u8>, String> {
        match self.out.room {
            None => rustgis_data::zip::write(&self.entries),
            Some(room) => room.encode(self.out.path, |limit| rustgis_data::zip::write_within(&self.entries, limit)),
        }
    }
}

/// `bytes` in MiB, rounded up.
pub(crate) fn mib(bytes: usize) -> usize {
    bytes.div_ceil(1 << 20)
}

/// The most feature storage a layer read in place may take while its download is made, and the
/// most the files waiting for download may hold. The features and the file are held at once, in
/// the browser's memory. A fixed figure: the browser can't say what memory it has free, so
/// `rustgis_data::memory::planned` would give its floor there.
pub(crate) const MAX_DOWNLOAD_BYTES: usize = 256 << 20;

/// Every feature of layer `l`, read in place, for a file made in memory to be downloaded: refused
/// past [`MAX_DOWNLOAD_BYTES`] of features, before any is read when the source's count alone says
/// so. A definition query is applied after the read, so it doesn't make the read smaller.
pub(super) fn read_for_download(l: &rustgis_map::Layer) -> Result<(Dataset, rustgis_map::Unreadable), String> {
    read_for_download_after(l, 0)
}

/// [`read_for_download`] for a file made of several layers, `held` bytes of whose features were
/// read before this one's: together they are held to [`MAX_DOWNLOAD_BYTES`].
pub(super) fn read_for_download_after(l: &rustgis_map::Layer, held: usize) -> Result<(Dataset, rustgis_map::Unreadable), String> {
    let needs = if held == 0 { "the layer needs" } else { "with the layers read before it, the layer needs" };
    let refusal = format!(
        "{}: {needs} more than the {} MiB of features the web app reads for a download; zoom in, select the features you need, use Make Layer From Selected Features and export that layer, or use the desktop app",
        l.name,
        MAX_DOWNLOAD_BYTES >> 20
    );
    l.load_all_counted_up_to(MAX_DOWNLOAD_BYTES.saturating_sub(held) as u64, &refusal)
}

/// Export `command`'s parameters when its file is downloaded: `path` cut to the download's name,
/// with what to say of that once it is downloaded. A Shapefile, several files, is downloaded as
/// a `.zip`. Refused here, before anything is read, for a path with no file name and for the
/// formats the browser can't write.
fn for_download<'a>(command: &str, p: &'a Value) -> Result<(Cow<'a, Value>, Vec<String>), String> {
    // No path, or parameters that aren't an object: the export says what it needs.
    let Some(path) = super::ps(p, "path") else { return Ok((Cow::Borrowed(p), Vec::new())) };
    let mut name = path.rsplit(['/', '\\']).next().unwrap_or_default().to_string();
    if matches!(name.as_str(), "" | "." | "..") {
        return Err(format!(
            "{path}: the browser downloads a file under the name its path ends in, and this one has none; end it in a file name, such as map.png"
        ));
    }
    let folder = name != path;
    let mut notes = Vec::new();
    if matches!(command, "share.exportLayer" | "share.exportTable") {
        let named = std::path::Path::new(&name);
        match named.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
            "gpkg" | "fgb" => {
                return Err(format!(
                    "{path}: the web app can't write GeoPackage or FlatGeobuf files; export to .geojson, .csv, .parquet, .zip (a Shapefile), .kml, .kmz or .gpx, which it downloads, or use the desktop app"
                ));
            }
            "shp" => {
                name = named.with_extension("zip").to_string_lossy().into_owned();
                notes.push(format!("{name}: a Shapefile is several files, so the browser downloads them in one .zip"));
            }
            _ => {}
        }
    }
    if folder {
        notes.push(format!("{name}: the browser chooses where downloads go, so the folder in '{path}' wasn't used"));
    }
    let mut p = p.clone();
    if let Some(params) = p.as_object_mut() {
        params.insert("path".into(), Value::String(name));
    }
    Ok((Cow::Owned(p), notes))
}

/// Exports that render map, layout, report, presentation or scene text with installed fonts.
fn uses_fonts(command: &str) -> bool {
    matches!(command, "share.exportMap" | "layout.export" | "report.export" | "presentation.export" | "map.exportOrbit" | "map.exportBlock")
}

/// Why a font-drawing export is refused while the CJK font folders are still being scanned.
const CJK_PENDING: &str = "CJK font folder scan is still running; try exporting again in a moment";

/// What is said of an export during which that scan started.
const CJK_STARTED: &str = "CJK font folder scan started during this export; re-run it once the scan finishes if CJK text is missing.";

#[cfg(test)]
thread_local! {
    static TEST_CJK_SCAN_PENDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn cjk_scan_pending() -> bool {
    #[cfg(test)]
    if TEST_CJK_SCAN_PENDING.get() {
        return true;
    }
    rustgis_render::fonts::cjk_roots_pending()
}

/// `command`'s export written here: it says what it wrote. Where files are downloaded
/// ([`Session::downloads_files`]) it answers that the download is pending instead, and says what
/// it wrote when the front end acknowledges it.
pub(crate) fn write_here(s: &mut Session, p: &Value, command: &str) -> CmdResult {
    let &(_, name, start) = IN_BACKGROUND.iter().find(|(c, ..)| *c == command).ok_or_else(|| format!("{command} isn't an export"))?;
    if uses_fonts(command) && cjk_scan_pending() {
        return Err(CJK_PENDING.into());
    }
    // Only an export that takes a path is named after it (Publish to Source writes the layer's
    // own source, and says itself what the browser can't do).
    let takes_path = s.registry.get(command).and_then(|c| c.param_keys()).is_some_and(|keys| keys.contains(&"path"));
    let (p, renamed) = if s.downloads_files() && takes_path { for_download(command, p)? } else { (Cow::Borrowed(p), Vec::new()) };
    let owner = s.continuation()?;
    owner.check()?;
    let current = owner.clone();
    let still = std::sync::Arc::new(move || current.check().is_ok());
    // The probe keeps authority checks at file commit even when the user cannot cancel this run.
    let control = if s.stoppable { rustgis_analysis::control::Control::stoppable() } else { rustgis_analysis::control::Control::new() };
    let (mut wrote, pending_cjk) = crate::render::rustgis_tiles::cache::with_still(still, || {
        let work = start(s, &p)?;
        if uses_fonts(command) {
            let (res, hit) = rustgis_render::fonts::track_pending_cjk(|| written_under(name, &control, work));
            Ok::<_, String>((res?, hit))
        } else {
            Ok((written_under(name, &control, work)?, false))
        }
    })?;
    if pending_cjk {
        wrote.diagnostics.push((rustgis_data::diagnostics::Severity::Warning, CJK_STARTED.into()));
    }
    s.check_continuation(&owner).map_err(|why| {
        // A download not yet offered wrote nothing.
        let written = wrote.written_path.as_deref().filter(|_| wrote.download.is_none());
        completion_with_notes(command, written, &wrote.completion_notes(), why)
    })?;
    if wrote.download.is_some() {
        for (i, note) in renamed.into_iter().enumerate() {
            wrote.notes.insert(i, note);
        }
        return super::gp::queue_export(s, wrote);
    }
    said(s, wrote)
}

/// A file a command makes at once, on the thread that runs it ([`file_command`]).
pub(crate) struct Made {
    pub(crate) bytes: Vec<u8>,
    /// What the command answers once the file is written, or its download is taken.
    pub(crate) answer: Value,
    /// What it says once the file is out, after what making it said.
    pub(crate) notes: rustgis_data::loss::Notes,
    /// The line saying the file is out. Without one, a download says it was offered.
    pub(crate) done: Option<String>,
    /// What a download does to the session once taken.
    pub(crate) taken: Option<Box<dyn FnOnce(&mut Session) + Send>>,
}

impl Made {
    /// `bytes`, answered with `answer`, with nothing more to say of them.
    pub(crate) fn new(bytes: Vec<u8>, answer: Value) -> Self {
        Made { bytes, answer, notes: Default::default(), done: None, taken: None }
    }
}

/// `command`'s file, made by `build` for the path it is given: written there, or, where files are
/// downloaded ([`Session::downloads_files`]), offered as a download ([`download_command`]).
pub(crate) fn file_command(
    s: &mut Session,
    command: &str,
    p: &Value,
    build: impl FnOnce(&mut Session, Output<'_>) -> Result<Made, String>,
) -> CmdResult {
    if s.downloads_files() {
        return download_command(s, command, p, build);
    }
    let mark = s.notice_mark();
    let path = super::ps(p, "path").ok_or("path is required")?;
    let made = build(s, Output { path, room: None })?;
    rustgis_data::io::write_atomic(std::path::Path::new(path), &made.bytes).map_err(|e| format!("{path}: {e}"))?;
    for note in made.notes {
        s.say(note);
    }
    s.retain_written_notices(mark);
    if let Some(done) = made.done {
        s.notify_written(done);
    }
    Ok(made.answer)
}

/// Queues the file `build` makes for the front end to offer as a download, under the file name
/// `p`'s path ends in. `build` is given that name and the room the downloads waiting leave
/// ([`Session::download_room`], refused before anything is made when no more can wait), and
/// makes the file within it. The answer says it is pending, with what will be said of it; that
/// is said, and `command`'s answer given, once the front end has taken the download
/// (`gp.fileDelivered`). A download not taken says only why.
pub(crate) fn download_command(
    s: &mut Session,
    command: &str,
    p: &Value,
    build: impl FnOnce(&mut Session, Output<'_>) -> Result<Made, String>,
) -> CmdResult {
    if uses_fonts(command) && cjk_scan_pending() {
        return Err(CJK_PENDING.into());
    }
    let (p, renamed) = for_download(command, p)?;
    let name = super::ps(&p, "path").ok_or("path is required")?.to_string();
    let owner = s.continuation()?;
    owner.check()?;
    let out = Output { path: &name, room: s.download_room()? };
    // What making the file says waits with it.
    let (made, held) = s.holding_notices(|s| {
        let (made, pending_cjk) =
            if uses_fonts(command) { rustgis_render::fonts::track_pending_cjk(|| build(s, out)) } else { (build(s, out), false) };
        let mut made = made?;
        for note in std::mem::take(&mut made.notes) {
            s.say(note);
        }
        if pending_cjk {
            s.warn(CJK_STARTED);
        }
        Ok::<_, String>(made)
    });
    let made = match made.and_then(|made| s.check_continuation(&owner).map(|()| made)) {
        Ok(made) => made,
        // No file: said as a command that fails says it.
        Err(why) => {
            for notice in held {
                s.relay_notice(notice);
            }
            return Err(why);
        }
    };
    let said: Vec<String> = held.iter().map(|notice| notice.text.clone()).collect();
    let Made { bytes, answer, done, taken, .. } = made;
    let done = done.unwrap_or_else(|| format!("Download offered: {name}"));
    let then: Then = Box::new(move |s: &mut Session, _: &mut Value| {
        if let Some(taken) = taken {
            taken(s);
        }
        s.notify_written(done);
        let mark = s.notice_mark();
        for notice in held {
            s.relay_notice(notice);
        }
        s.retain_written_notices(mark);
        Ok(())
    });
    let wrote = Wrote {
        answer,
        notes: renamed.into_iter().collect(),
        diagnostics: Vec::new(),
        refused_notes: Vec::new(),
        written_path: Some(name),
        download: Some(Arc::new(bytes)),
        then: Some(then),
        #[cfg(not(target_arch = "wasm32"))]
        owner: None,
        #[cfg(not(target_arch = "wasm32"))]
        crs_used: Vec::new(),
    };
    let mut out = super::gp::queue_export(s, wrote)?;
    if let Some(messages) = out["messages"].as_array_mut() {
        messages.extend(said.into_iter().map(Value::String));
    }
    Ok(out)
}

/// Refuses, where files are downloaded, a `path` a command would write for a layer to read its
/// pixels from: a download can't be read back.
pub(crate) fn no_layer_file(s: &Session, p: &Value) -> Result<(), String> {
    match super::ps(p, "path").map(str::trim).filter(|path| !path.is_empty()) {
        Some(path) if s.downloads_files() => Err(format!(
            "{path}: the web app can't write a file for the layer to read; leave out path (the project keeps the result), then download it with share.exportRaster"
        )),
        _ => Ok(()),
    }
}

/// Keeps a completed file visible when its result cannot be delivered to the session.
pub(crate) fn completion_refused(written_path: Option<&str>, why: String) -> String {
    match written_path {
        Some(path) => format!("Exported {path}; completion was refused: {why}"),
        None => why,
    }
}

/// A committed export's format notices remain part of its refusal diagnostic; its finalizer
/// stays blocked, so reporting the write cannot update a revoked layer's record.
fn completion_with_notes(command: &str, written_path: Option<&str>, notes: &[String], why: String) -> String {
    let Some(path) = written_path else { return why };
    let mut message = if command == "source.publish" {
        format!("Published {path}; completion was refused: {why}; the layer's record wasn't updated; add the file again to compare it")
    } else {
        completion_refused(Some(path), why)
    };
    for note in notes {
        message.push('\n');
        message.push_str(note);
    }
    message
}

/// `work` under `control`, as [`rustgis_analysis::run_under`] runs it, except that a file once
/// placed is written: a Cancel that comes after the last step doesn't turn it into a failure.
fn written_under(name: &str, control: &std::sync::Arc<rustgis_analysis::control::Control>, work: Work) -> Result<Wrote, String> {
    let mut wrote = None;
    let res = rustgis_analysis::run_under(name, control, || work().map(|w| wrote = Some(w)));
    match wrote {
        Some(w) => Ok(w),
        None => Err(res.err().unwrap_or_else(|| format!("{name} wrote nothing"))),
    }
}

/// Says what an export wrote, and gives its answer (or the failure its `then` says).
pub(super) fn said(s: &mut Session, wrote: Wrote) -> CmdResult {
    let Wrote { mut answer, notes, diagnostics, then, written_path, .. } = wrote;
    let mark = s.notice_mark();
    say(s, diagnostics);
    if written_path.is_some() {
        s.retain_written_notices(mark);
    }
    for note in notes {
        if written_path.is_some() {
            s.say_written(note);
        } else {
            s.say(note);
        }
    }
    if let Some(then) = then {
        rustgis_data::provenance::without(|| then(s, &mut answer))?;
    }
    Ok(answer)
}

/// Lands an export drawn on a copy of the map or project (Export Map, Export Layout): what
/// drawing it said, with each notice's severity; the streamed layers whose source failed on the
/// copy, reported for this export even when the screen already said them. Matching live layers
/// keep the failure marked said so subsequent screen frames do not repeat it;
/// then that it was written to `path`, or why not. A cancelled one says only that it was cancelled.
pub(crate) fn drawn(
    maps: &[rustgis_map::Map],
    mut notices: Vec<(rustgis_data::diagnostics::Severity, String)>,
    written: Result<Value, String>,
    path: String,
) -> Result<Wrote, String> {
    if let (Err(e), Err(_)) = (&written, rustgis_data::progress::check()) {
        return Err(e.clone());
    }
    // Held layers' places the drawing couldn't give in longitude/latitude ([`rustgis_map::Layer::unplaced`]):
    // drawn without them, and said of this export, as the screen says them of the map.
    let (mut unplaced, mut partly, mut undrawn) = (0usize, Vec::new(), Vec::new());
    for m in maps {
        for l in &m.layers {
            let named = format!("map '{}', layer '{}'", m.name, l.name);
            // Only the layers this export drew: a hidden one, or a map the layout doesn't
            // show, keeps a count from an earlier drawing.
            match l.unplaced_drawn() {
                Some(Ok(n)) if *n > 0 => {
                    unplaced += n;
                    partly.push(format!("{named} ({n})"));
                }
                Some(Err(e)) => undrawn.push(format!("{named}: {e}")),
                _ => {}
            }
        }
    }
    if written.is_ok() {
        let warn = rustgis_data::diagnostics::Severity::Warning;
        if unplaced > 0 {
            notices.push((
                warn,
                format!(
                    "{path}: {unplaced} feature(s) have a place that can't be given in longitude/latitude, drawn without it ({})",
                    crate::listed(&partly)
                ),
            ));
        }
        if !undrawn.is_empty() {
            notices.push((
                warn,
                format!(
                    "{path}: {} layer(s) aren't drawn, as their system can't be given in longitude/latitude: {}",
                    undrawn.len(),
                    crate::listed(&undrawn)
                ),
            ));
        }
    }
    // Each by its live layer, with the source it failed on: one the user removed, or opened
    // afresh meanwhile (`SourceLayer::same_opening`), is left as it is.
    let failed: Vec<_> = maps
        .iter()
        .flat_map(|m| &m.layers)
        .filter_map(|l| {
            let src = l.source.as_ref()?;
            Some((l.instance_id(), l.name.clone(), src.error.clone()?, src.clone()))
        })
        .collect();
    // Every output needs its own counted notice, independent of screen suppression.
    use crate::notice_text::NoticeText;
    let mut source_errors = Vec::new();
    let mut linked = 0;
    for (_, name, error, source) in &failed {
        let path = source.linked_file();
        linked += usize::from(path.is_some());
        if source_errors.len() < 5 {
            source_errors.push(match path {
                Some(path) => NoticeText::format(
                    "{name}: {path} ({error})",
                    [("name", name.clone().into()), ("path", path.to_string().into()), ("error", error.clone().into())],
                ),
                None => NoticeText::format("{name}: {error}", [("name", name.clone().into()), ("error", error.clone().into())]),
            });
        }
    }
    let source_notice = NoticeText::source_errors(failed.len(), source_errors, linked)
        .map(|message| NoticeText::format("{path}: {message}", [("path", path.clone().into()), ("message", message)]));
    let refused_notes = source_notice.iter().map(NoticeText::english).collect();
    let written_path = written.is_ok().then(|| path.clone());
    let answer = written.as_ref().map_or(Value::Null, Value::clone);
    let then: Then = Box::new(move |s: &mut Session, _: &mut Value| {
        for (instance, _, error, copied) in failed {
            let live = s.project.maps.iter_mut().flat_map(|m| m.layers.iter_mut()).find(|l| l.instance_id() == instance);
            if let Some(src) = live.and_then(|l| l.source.as_mut())
                && src.same_opening(&copied)
            {
                src.error.get_or_insert(error);
                // The export reports this failure below; the screen need not say it again.
                if src.error == copied.error {
                    let _ = src.error_unsaid();
                }
            }
        }
        let mark = s.notice_mark();
        if let Some(message) = source_notice {
            s.notify_text(message, true);
        }
        if written.is_ok() {
            s.retain_written_notices(mark);
        }
        // Background exports land without Session::run's display diagnostics. Source failures
        // already reported above retain their screen markers, so this cannot duplicate them.
        s.notice_landed_pixels();
        written?;
        s.notify_written(format!("Exported {path}"));
        Ok(())
    });
    Ok(Wrote {
        answer,
        notes: Default::default(),
        diagnostics: notices,
        refused_notes,
        written_path,
        // Its caller's, which placed the file ([`place`]).
        download: None,
        then: Some(then),
        #[cfg(not(target_arch = "wasm32"))]
        owner: None,
        #[cfg(not(target_arch = "wasm32"))]
        crs_used: Vec::new(),
    })
}

/// [`drawn`] for an export drawn on `copy`, a copy of the session: what drawing it said, a loss
/// kept with its code and said with the export's notes ([`copy_notices_coded`]).
pub(crate) fn drawn_on(copy: &mut Session, written: Result<Value, String>, path: String) -> Result<Wrote, String> {
    let (coded, said): (Vec<_>, Vec<_>) = copy_notices_coded(copy, &path).into_iter().partition(|(_, note)| note.loss.is_some());
    let said = said.into_iter().map(|(severity, note)| (severity, note.text)).collect();
    let mut wrote = drawn(&copy.project.maps, said, written, path)?;
    wrote.notes.extend(coded.into_iter().map(|(_, note)| note));
    Ok(wrote)
}

/// What drawing an export on `copy` (a copy of the session) said, each notice with its severity
/// and its loss code, to say again where the export lands; past the notices a session keeps, how
/// many earlier ones there were. `path` names the file in that count.
pub(crate) fn copy_notices_coded(copy: &mut Session, path: &str) -> Vec<(rustgis_data::diagnostics::Severity, rustgis_data::loss::Note)> {
    let kept = std::mem::take(&mut copy.notices);
    let earlier = copy.notices_given().saturating_sub(kept.len() as u64);
    let mut said = Vec::new();
    if earlier > 0 {
        said.push((
            rustgis_data::diagnostics::Severity::Warning,
            rustgis_data::loss::Note::plain(format!("{path}: {earlier} earlier notices from drawing it aren't shown")),
        ));
    }
    said.extend(kept.into_iter().map(|n| (n.severity, rustgis_data::loss::Note { loss: n.loss(), text: n.text, tally: None })));
    said
}

/// [`say`] for notices with their loss codes: one with a code is said as that loss.
pub(crate) fn say_coded(s: &mut Session, notices: Vec<(rustgis_data::diagnostics::Severity, rustgis_data::loss::Note)>) {
    use rustgis_data::diagnostics::Severity;
    for (severity, note) in notices {
        if note.loss.is_some() {
            s.say(note);
            continue;
        }
        match severity {
            Severity::Error => s.notify(note.text, true),
            Severity::Warning => s.warn(note.text),
            Severity::Info => s.notify(note.text, false),
        }
    }
}

/// Says `notices` (what an export said on a copy of the session) in `s`, each as it was said.
pub(crate) fn say(s: &mut Session, notices: Vec<(rustgis_data::diagnostics::Severity, String)>) {
    use rustgis_data::diagnostics::Severity;
    for (severity, text) in notices {
        match severity {
            Severity::Error => s.notify(text, true),
            Severity::Warning => s.warn(text),
            Severity::Info => s.notify(text, false),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// An export ([`IN_BACKGROUND`]) started to write its file on another thread ([`ExportRun::run`])
/// while the session goes on: its inputs are taken now (cheaply: the features and pixels are
/// shared, and a streamed layer's source is read on the export's thread), and [`export_land`]
/// says what it wrote.
pub struct ExportRun {
    /// What the export is called, for its progress and a cancelled run's error.
    pub name: &'static str,
    started: Started,
    work: Work,
}

#[cfg(not(target_arch = "wasm32"))]
/// The command an export started on another thread runs, with what and when: [`export_land`]
/// says what it wrote as that command, and puts it in the session's log as `Session::run` does.
#[derive(Clone)]
pub struct Started {
    pub(crate) command: &'static str,
    pub(crate) params: Value,
    pub(crate) at: u64,
    pub(crate) start: web_time::Instant,
    /// The admitted invocation; cloned metadata never selects replacement authority.
    pub(crate) owner: crate::policy::Continuation,
    /// The project it started in, its name as the log keeps it and its `Session::opened`: the
    /// log's entry names that one, not one opened while it ran.
    pub(crate) project: (String, u64),
    /// Shared with the worker so failed exports retain the reads preceding the error, too.
    inputs: std::sync::Arc<std::sync::Mutex<Vec<rustgis_data::provenance::Fingerprint>>>,
    capture_inputs: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl ExportRun {
    /// What [`export_land`] needs of it once it has run.
    pub fn started(&self) -> Started {
        self.started.clone()
    }

    /// Writes the file, or stages it if installing it needs the live session. `control` stops the
    /// work and counts its progress (see [`rustgis_analysis::control`]). A cancelled export leaves
    /// no file, or the one there before.
    ///
    /// On Linux, font-drawing exports wait for any in-flight background CJK font-folder scan off
    /// the UI thread so exported files do not omit CJK glyphs; that one-time bounded scan
    /// (`MAX_DIRS = 2_000`, `MAX_FILES = 20_000`) does not observe `control` Cancel while waiting.
    pub fn run(self, control: &std::sync::Arc<rustgis_analysis::control::Control>) -> Result<Wrote, String> {
        let inputs = self.started.capture_inputs.then(rustgis_data::provenance::Capture::start);
        let ExportRun { name, work, started } = self;
        let command = started.command;
        let owner = started.owner;
        owner.check()?;
        let current = owner.clone();
        let still = std::sync::Arc::new(move || current.check().is_ok());
        // A bug in a writer must not take the app down.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::render::rustgis_tiles::cache::with_still(still, || {
                if uses_fonts(command) {
                    rustgis_render::fonts::with_blocking_cjk_scan(|| written_under(name, control, work))
                } else {
                    written_under(name, control, work)
                }
            })
        }))
        .unwrap_or_else(|payload| {
            let incident = rustgis_data::incident::caught(payload.as_ref());
            Err(rustgis_data::incident::message(name, false, &incident.id))
        })
        .map(|w| Wrote { owner: Some(owner), crs_used: rustgis_data::raster::crs::take_used(), ..w });
        if let Some(inputs) = inputs {
            started.inputs.lock().unwrap_or_else(|e| e.into_inner()).extend(inputs.inputs());
        }
        result
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Starts `command` (one of [`IN_BACKGROUND`]'s) with `p`, to write its file on another thread:
/// its parameters checked and its inputs taken now.
pub fn export_start(s: &mut Session, command: &str, p: &Value) -> Result<ExportRun, String> {
    s.start_export(command, p)
}

#[cfg(not(target_arch = "wasm32"))]
/// Prepares only a registered, admitted export and seals its initiating authority.
pub(crate) fn export_start_raw(s: &mut Session, op: crate::policy::Authorized<crate::policy::CommandOp>) -> Result<ExportRun, String> {
    let crate::policy::CommandOp::Registered { spec, params } = op.op() else { return Err("not a registered export".into()) };
    let inputs = s.log.on().then(rustgis_data::provenance::Capture::start);
    let (command, p) = (spec.id, params);
    let &(_, name, start) =
        IN_BACKGROUND.iter().find(|(c, ..)| *c == command).ok_or_else(|| format!("{command} doesn't run in the background"))?;
    let owner = s.continuation()?;
    owner.check()?;
    let current = owner.clone();
    let still = std::sync::Arc::new(move || current.check().is_ok());
    let project = (crate::project_log::kept_name(&s.title()), s.opened);
    let started = Started {
        command,
        params: p.clone(),
        at: crate::project_log::now_ms(),
        start: web_time::Instant::now(),
        owner,
        project,
        inputs: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        capture_inputs: s.log.on(),
    };
    let work = crate::render::rustgis_tiles::cache::with_still(still, || start(s, p))?;
    if let Some(inputs) = inputs {
        *started.inputs.lock().unwrap_or_else(|e| e.into_inner()) = inputs.inputs();
    }
    Ok(ExportRun { name, started, work })
}

#[cfg(not(target_arch = "wasm32"))]
/// Says what [`ExportRun::run`] wrote (or why it failed), as its command would have, and gives
/// its answer: the troubleshooting history files what it says (its failure too) under that
/// command, and the session's log has its entry, as when it runs here.
pub fn export_land(s: &mut Session, started: Started, done: Result<Wrote, String>) -> CmdResult {
    let landed = export_land_floored(s, started, done);
    // A landed export (a field package's) may record ids packages hold: kept as any command's are.
    s.keep_field_floors();
    landed
}

/// [`export_land`], before field packages' id floors are kept.
#[cfg(not(target_arch = "wasm32"))]
fn export_land_floored(s: &mut Session, started: Started, done: Result<Wrote, String>) -> CmdResult {
    let _inputs = s.log.on().then(rustgis_data::provenance::Capture::start);
    rustgis_data::provenance::record(started.inputs.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned());
    let Started { command, params, at, start, owner, project, .. } = started;
    let before = s.diagnostic_source.replace(command);
    let mark = s.notice_mark();
    let landed: CmdResult = (|| {
        if let Ok(wrote) = &done {
            let completed = wrote.owner.as_ref().ok_or("the export has no initiating authority")?;
            if !owner.same_run(completed) {
                return Err("this export result belongs to another run".into());
            }
        }
        let written_path = done.as_ref().ok().and_then(|wrote| wrote.written_path.clone());
        if let Ok(wrote) = &done {
            s.crs_used_elsewhere.extend(wrote.crs_used.iter().cloned());
        }
        let notes = done.as_ref().ok().map(Wrote::completion_notes).unwrap_or_default();
        s.finish_export(&owner, |s| done.and_then(|wrote| said(s, wrote)))
            .map_err(|why| completion_with_notes(command, written_path.as_deref(), &notes, why))?
    })();
    // The coordinate systems its worker used that needed a missing datum grid, as the command
    // run here says them after it.
    s.say_grid_notes();
    // Its failure said once, under the command, as `Session::run` says one.
    if let Err(e) = &landed {
        let text = Session::redacted(e);
        if !s.notices_since(mark).iter().any(|n| n.error && n.text == text.as_str()) {
            s.notify_redacted(text, true);
        }
    }
    s.diagnostic_source = before;
    if s.log.on() && crate::project_log::is_logged(s, command, &params) {
        let (kept, partial) = crate::project_log::kept_and_partial(&params);
        // The project it started in, not one opened while it ran.
        s.log_entry(crate::project_log::entry(project, command, kept, partial, &landed, at, start.elapsed()), false);
    }
    landed
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustgis_analysis::control::Control;

    #[test]
    fn a_cancel_after_the_file_is_placed_leaves_it_written() {
        let control = Control::new();
        let late = control.clone();
        // The work places its file, then Cancel comes before the run returns.
        let work: Work = Box::new(move || {
            late.cancel();
            Ok(Wrote {
                answer: serde_json::json!({"path": "x"}),
                diagnostics: Vec::new(),
                refused_notes: Vec::new(),
                notes: ["Exported x"].into_iter().collect(),
                written_path: Some("x".into()),
                download: None,
                then: None,
                #[cfg(not(target_arch = "wasm32"))]
                owner: None,
                #[cfg(not(target_arch = "wasm32"))]
                crs_used: Vec::new(),
            })
        });
        let wrote = written_under("Export Raster", &control, work).unwrap();
        assert_eq!(wrote.notes, ["Exported x"]);
        // A writer that stopped for the cancel is the cancelled run's error.
        let control = Control::new();
        let stopped = control.clone();
        let work: Work = Box::new(move || {
            stopped.cancel();
            Err("cancelled".into())
        });
        assert!(written_under("Export Raster", &control, work).err().unwrap().contains("was cancelled"));
    }

    #[test]
    fn write_here_rejects_font_exports_while_cjk_scan_is_pending() {
        struct PendingGuard;
        impl Drop for PendingGuard {
            fn drop(&mut self) {
                TEST_CJK_SCAN_PENDING.set(false);
            }
        }

        let mut s = Session::new(crate::Project::default());
        TEST_CJK_SCAN_PENDING.set(true);
        let _guard = PendingGuard;

        // Font-drawing synchronous exports fail fast rather than blocking the UI thread or writing missing glyphs.
        let err = write_here(&mut s, &serde_json::json!({"path": "out.png"}), "share.exportMap").unwrap_err();
        assert!(err.contains("CJK font folder scan is still running"), "{err}");

        // Non-font exports (e.g. tables) are unaffected by a pending CJK scan and proceed to normal validation.
        let table_err =
            write_here(&mut s, &serde_json::json!({"layer": "NoSuchLayer", "path": "out.csv"}), "share.exportTable").unwrap_err();
        assert!(!table_err.contains("CJK font folder scan"), "{table_err}");
    }
}
