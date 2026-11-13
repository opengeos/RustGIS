//! RustGIS on Android (#1902).
//!
//! The desktop front end (`rustgis-ui-egui`) in a `NativeActivity`, with the phone's file picker
//! ([`phone`], a `rustgis_engine::platform::Platform`) standing where the browser's stands for
//! the web app. See `README.md` for how to build and install it.
//!
//! Not here yet: location and the camera; preferences and the keychain; autosave; a touch-first
//! front end.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_os = "android")]
mod phone;

#[cfg(target_os = "android")]
mod android {
    use crate::phone::Phone;
    use eframe::egui;
    use eframe::wgpu;
    use rustgis_engine::data::dirs;
    use rustgis_engine::platform::{Event, Platform};
    use rustgis_ui_egui::{Inbox, RustGisApp, i18n, tl};
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use winit::platform::android::activity::AndroidApp;

    /// What a service couldn't do, kept until the next frame says it in the window (a service has
    /// no session to tell).
    type Refusals = Rc<RefCell<Vec<String>>>;

    /// The app: the desktop front end, with the files to open once the window runs.
    struct App {
        app: RustGisApp,
        /// The files pushed to the app's folder ([`pushed`]), until the first frame takes them.
        pushed: Vec<std::path::PathBuf>,
        /// The phone's file picker, where its Java half is there.
        phone: Option<Rc<Phone>>,
        /// Picked files the front end reads as bytes (`Services::inbox`).
        inbox: Inbox,
        refusals: Refusals,
        /// Files picked for a dialog (purpose, name, copy), until their picker has answered in
        /// full: a Shapefile's parts arrive one by one and are read together.
        waiting: Vec<(String, String, PathBuf)>,
        /// What reading them off the window's thread found, until the next frame says it.
        problems: Problems,
        /// The batches being read.
        reading: Arc<AtomicUsize>,
    }

    /// Why a picked file didn't reach the dialog that asked for it.
    enum Problem {
        /// A shortcut file over `SHORTCUT_FILE_MAX_BYTES`.
        ShortcutTooLarge,
        /// The copy of the file `name` couldn't be read.
        Unread { name: String, error: String },
        /// What gathering a Shapefile's parts found, in its own words.
        Shapefile(String),
    }

    type Problems = Arc<Mutex<Vec<Problem>>>;

    impl App {
        /// Takes what the phone did since the last frame: picked files are opened, the rest is
        /// said in the window.
        fn take_phone_events(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
            for text in self.refusals.borrow_mut().drain(..) {
                self.app.session.notify(text, true);
            }
            let Some(phone) = self.phone.clone() else { return };
            for event in phone.take_events() {
                match event {
                    // Data and projects open by path, as files dropped on the desktop window do:
                    // a GeoPackage is then edited in place, in its copy.
                    Event::Picked { purpose, path, .. } if purpose == "data" || purpose == "project" => {
                        raw_input.dropped_files.push(std::sync::Arc::new(Pushed(path)));
                    }
                    // Every other picker's file goes where the browser's does: to whoever asked.
                    Event::Picked { purpose, name, path } => self.waiting.push((purpose, name, path)),
                    Event::Cancelled => {}
                    Event::Saved { name } => self.app.session.notify(i18n::fmt("Saved {name}.", &[("name", &name)]), false),
                    Event::Failed(text) => self.app.session.notify(text, true),
                }
            }
            for problem in self.problems.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
                let text = match problem {
                    Problem::ShortcutTooLarge => tl!("Shortcut file exceeds the size limit.").to_string(),
                    Problem::Unread { name, error } => i18n::fmt("{name} wasn't opened: {error}", &[("name", &name), ("error", &error)]),
                    Problem::Shapefile(text) => text,
                };
                self.app.session.notify(text, true);
            }
            // A picked file is copied on a thread of Android's: look again until it has arrived.
            let busy = phone.busy();
            if !busy && !self.waiting.is_empty() {
                self.read_waiting(ctx);
            }
            if busy || self.reading.load(Ordering::Relaxed) > 0 {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }

        /// Reads the files picked for dialogs into the inbox, off the window's thread (a file can
        /// be large), each picker's files together so a Shapefile's loose parts become one layer,
        /// as the browser's shell reads them.
        fn read_waiting(&mut self, ctx: &egui::Context) {
            let mut batches: Vec<(String, Vec<(String, PathBuf)>)> = Vec::new();
            for (purpose, name, path) in self.waiting.drain(..) {
                match batches.iter_mut().find(|(p, _)| *p == purpose) {
                    Some((_, files)) => files.push((name, path)),
                    None => batches.push((purpose, vec![(name, path)])),
                }
            }
            for (purpose, files) in batches {
                let (inbox, problems, reading, ctx) = (self.inbox.clone(), self.problems.clone(), self.reading.clone(), ctx.clone());
                reading.fetch_add(1, Ordering::Relaxed);
                let names: Vec<String> = files.iter().map(|(name, _)| name.clone()).collect();
                let counted = reading.clone();
                let started = std::thread::Builder::new().name("picked files".into()).spawn(move || {
                    read_batch(&purpose, files, &inbox, &problems);
                    reading.fetch_sub(1, Ordering::Relaxed);
                    ctx.request_repaint();
                });
                if let Err(e) = started {
                    counted.fetch_sub(1, Ordering::Relaxed);
                    let error = e.to_string();
                    for name in names {
                        self.app.session.notify(i18n::fmt("{name} wasn't opened: {error}", &[("name", &name), ("error", &error)]), true);
                    }
                }
            }
        }
    }

    /// Reads one picker's files (name, copy) into `inbox`, marked for the dialog that asked, and
    /// removes the copies: their bytes are all that was wanted of them.
    fn read_batch(purpose: &str, files: Vec<(String, PathBuf)>, inbox: &Inbox, problems: &Problems) {
        let mark = mark(purpose);
        let say = |problem| problems.lock().unwrap_or_else(|e| e.into_inner()).push(problem);
        let mut read = Vec::new();
        for (name, path) in files {
            let shortcut =
                mark == rustgis_ui_egui::SHORTCUT_PICK || (mark.is_empty() && name.to_ascii_lowercase().ends_with(".rgisshortcuts"));
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            if shortcut && size > rustgis_ui_egui::SHORTCUT_FILE_MAX_BYTES as u64 {
                say(Problem::ShortcutTooLarge);
            } else {
                match std::fs::read(&path) {
                    Ok(bytes) => read.push((name, bytes)),
                    Err(e) => say(Problem::Unread { name, error: e.to_string() }),
                }
            }
            let _ = std::fs::remove_file(&path);
            if let Some(folder) = path.parent() {
                let _ = std::fs::remove_dir(folder);
            }
        }
        let (read, found) = rustgis_engine::data::io::gather_shapefiles(read);
        for text in found {
            say(Problem::Shapefile(text));
        }
        inbox.lock().unwrap_or_else(|e| e.into_inner()).extend(read.into_iter().map(|(name, bytes)| (format!("{mark}{name}"), bytes)));
    }

    /// What starts the name of a file from the picker for `purpose` in the inbox, as the browser's
    /// shell marks them: the front end sends a marked file to the dialog that asked for it.
    fn mark(purpose: &str) -> &'static str {
        match purpose {
            "shortcuts" => rustgis_ui_egui::SHORTCUT_PICK,
            "text" => rustgis_ui_egui::DELIMITED_PICK,
            "shapefile" => rustgis_ui_egui::SHAPEFILE_PICK,
            p if p.starts_with(rustgis_ui_egui::TOOL_FILE_PURPOSE) => rustgis_ui_egui::TOOL_FILE_PICK,
            _ => "",
        }
    }

    /// Gives the front end the phone's file picker, the way the browser's shell gives its own:
    /// Open and Add Data ask through `open_async`, and what is saved or exported is offered
    /// through `download`.
    fn use_picker(app: &mut RustGisApp, phone: &Rc<Phone>, inbox: &Inbox, refusals: &Refusals) {
        let (picker, refused) = (phone.clone(), refusals.clone());
        app.services.open_async = Some(Box::new(move |purpose: &str| {
            log::info!("asking for files: {purpose}");
            if let Err(e) = picker.pick_files(purpose) {
                log::warn!("{e}");
                refused.borrow_mut().push(e);
            }
        }));
        let saver = phone.clone();
        app.services.download = Some(Box::new(move |name: &str, bytes: Vec<u8>| saver.save_file(name, bytes)));
        app.services.inbox = Some(inbox.clone());
    }

    impl eframe::App for App {
        fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
            // The GPU map backend draws inside egui's wgpu pass; hand over the render state.
            self.app.set_render_state(frame.wgpu_render_state().cloned());
            self.app.logic(ctx);
        }

        fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
            self.app.ui(ui);
        }

        fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
            self.take_phone_events(ctx, raw_input);
            // The pushed files open as files dropped on the desktop window do: once the window
            // runs, read off its thread with progress and Cancel, each failure said in the window.
            // Opened before the event loop, a large one would hold the activity's start.
            for path in self.pushed.drain(..) {
                raw_input.dropped_files.push(std::sync::Arc::new(Pushed(path)));
            }
            self.app.drop_stale_clicks(raw_input);
        }
    }

    /// A pushed or picked file, handed to the front end as a file dropped on the window: by its path.
    #[derive(Debug)]
    struct Pushed(std::path::PathBuf);

    impl egui::DroppedFile for Pushed {
        fn path(&self) -> &std::path::Path {
            &self.0
        }

        fn bytes(&self) -> Result<Vec<u8>, String> {
            std::fs::read(&self.0).map_err(|e| e.to_string())
        }
    }

    /// Points the app's folders (`rustgis_data::dirs`) at the app's own storage: Android has no
    /// `HOME`, so without this there are no settings, no tile cache and no recovery folder.
    /// Returns whether there is such storage.
    ///
    /// Run first in [`android_main`], before anything asks for a folder.
    fn set_folders(app: &AndroidApp) -> bool {
        match app.internal_data_path() {
            Some(data) => dirs::set_root(data),
            None => false,
        }
    }

    /// The files and folders in the app's external files folder
    /// (`/sdcard/Android/data/org.rustgis.mobile/files`, where `adb push` can put them), in name
    /// order: a way in for tests, beside the file picker. A folder or an entry that can't be read is
    /// said in the window.
    fn pushed(android: &AndroidApp, app: &mut RustGisApp) -> Vec<std::path::PathBuf> {
        let Some(dir) = android.external_data_path() else {
            app.session.warn("This device gives RustGIS no external files folder: there are no pushed files to open.");
            return Vec::new();
        };
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                app.session.notify(format!("The files in {} weren't opened: {e}", dir.display()), true);
                return Vec::new();
            }
        };
        let mut files = Vec::new();
        let mut unread = 0usize;
        let mut first = None;
        for entry in entries {
            match entry {
                // Folders too (a Zarr store is one): what the front end can't open, it says.
                Ok(entry) => files.push(entry.path()),
                Err(e) => {
                    unread += 1;
                    first.get_or_insert(e);
                }
            }
        }
        if let Some(e) = first {
            app.session.notify(format!("{unread} entries of {} couldn't be read and weren't opened: {e}", dir.display()), true);
        }
        files.sort();
        // One project: each one opened would replace the one before it, and nothing would say so.
        let mut projects = 0usize;
        files.retain(|f| {
            let project = rustgis_engine::project::open_command(&f.display().to_string()) != "layer.addData";
            projects += usize::from(project);
            !project || projects == 1
        });
        if projects > 1 {
            app.session
                .notify(format!("{} other projects in {} weren't opened: only the first project is.", projects - 1, dir.display()), false);
        }
        files
    }

    /// Android's entry point for a `NativeActivity` (`android-activity` calls it on its own thread).
    // `no_mangle` is the one unsafe attribute here: the name is what `android-activity` calls.
    #[allow(unsafe_code)]
    #[unsafe(no_mangle)]
    fn android_main(android: AndroidApp) {
        android_logger::init_once(android_logger::Config::default().with_max_level(log::LevelFilter::Info).with_tag("rustgis"));
        let folders = set_folders(&android);
        let mut app = RustGisApp::with_sample();
        if !folders {
            app.session.warn("This device gives RustGIS no folder of its own: settings, the tile cache and recovery copies are off.");
        }
        // As on the desktop: commands run on the window's thread must not hold it.
        app.session.in_place_wait = false;
        app.session.on_window_thread = true;
        app.reads_off_frame = true;
        if let Err(e) = app.start(&rustgis_ui_egui::Start { blank: Some(false), ..Default::default() }) {
            log::warn!("start-up: {e}");
            app.session.notify(e, true);
        }
        let (inbox, refusals) = (Inbox::default(), Refusals::default());
        let phone = match Phone::new(&android) {
            Ok(phone) => {
                let phone = Rc::new(phone);
                use_picker(&mut app, &phone, &inbox, &refusals);
                Some(phone)
            }
            Err(e) => {
                app.session.warn(i18n::fmt("{error}. Every button that asks for a file does nothing.", &[("error", &e)]));
                None
            }
        };
        let pushed = pushed(&android, &mut app);
        app.session.mark_saved();
        let mut wgpu_options = eframe::egui_wgpu::WgpuConfiguration::default();
        if let eframe::egui_wgpu::WgpuSetup::CreateNew(setup) = &mut wgpu_options.wgpu_setup {
            // Vulkan where the phone has it, OpenGL ES otherwise (older phones, the emulator).
            setup.instance_descriptor.backends = wgpu::Backends::VULKAN | wgpu::Backends::GL;
        }
        let options = eframe::NativeOptions { android_app: Some(android), wgpu_options, ..Default::default() };
        if let Err(e) = eframe::run_native(
            "RustGIS",
            options,
            Box::new(move |_cc| {
                Ok(Box::new(App {
                    app,
                    pushed,
                    phone,
                    inbox,
                    refusals,
                    waiting: Vec::new(),
                    problems: Problems::default(),
                    reading: Arc::default(),
                }))
            }),
        ) {
            log::error!("the app stopped: {e}");
        }
        // The activity is gone (Back, or the system destroyed it). winit's event loop can't be
        // made twice in a process, so a process left alive would answer the next launch with a
        // blank window: end it, and the next launch starts afresh. Phase 1 keeps the session
        // across this instead (`onSaveInstanceState`).
        std::process::exit(0);
    }
}
