//! Pictures attached to the AI Assistant's next message: picked with the composer's picture
//! button (the system's dialog on a computer, the browser's or the phone's picker elsewhere) or
//! pasted into the message box. Each is read by [`rustgis_assistant::attach::prepare`], off the
//! window's thread on the desktop, and shown above the box until it is sent or taken off.

use egui::{RichText, Ui};
use rustgis_assistant::attach::{self, Picture};
use rustgis_assistant::provider::Note;

use crate::RustGisApp;
use crate::theme::Tokens;

/// The pictures of the message being written.
#[derive(Default)]
pub struct Pictures {
    /// Ready to go with the message.
    ready: Vec<Picture>,
    /// Being read, with their names.
    #[cfg(not(target_arch = "wasm32"))]
    reading: Vec<(String, crate::Slot<Result<Picture, Note>>)>,
}

impl Pictures {
    /// Whether a picture is still being read: the message waits for it.
    pub fn reading(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            !self.reading.is_empty()
        }
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    }

    /// Whether the message has a picture ready.
    pub fn any(&self) -> bool {
        !self.ready.is_empty()
    }

    /// The pictures ready, taken for the message sent.
    pub fn take(&mut self) -> Vec<Picture> {
        std::mem::take(&mut self.ready)
    }

    /// How many the message has or is reading.
    fn count(&self) -> usize {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.ready.len() + self.reading.len()
        }
        #[cfg(target_arch = "wasm32")]
        {
            self.ready.len()
        }
    }
}

/// The picker purpose of the composer's picture button ([`crate::ASSISTANT_PICK`]).
pub const PURPOSE: &str = "assistantPicture";

/// The composer's picture button: picks pictures for the message.
pub fn button(app: &mut RustGisApp, ui: &mut Ui) {
    let full = app.assistant.pictures.count() >= attach::MAX_PER_MESSAGE;
    let hover = if full {
        crate::i18n::fmt("A message takes at most {n} pictures", &[("n", &attach::MAX_PER_MESSAGE.to_string())])
    } else {
        tl!("Attach a picture (PNG, JPEG or WebP), or paste one into the message box").to_string()
    };
    let clicked = ui.add_enabled_ui(!full, |ui| crate::widgets::icon_button(ui, "picture", &hover).clicked()).inner;
    if !clicked {
        return;
    }
    if let Some(open) = &app.services.open_async {
        open(PURPOSE);
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    for path in pick_files().unwrap_or_default() {
        match attach::read(&path) {
            Ok(bytes) => received(app, &path, bytes),
            Err(e) => app.session.notify(e, true),
        }
    }
}

/// The pictures the system's dialog gives (none in a unit test, or cancelled).
#[cfg(all(not(target_arch = "wasm32"), not(test)))]
fn pick_files() -> Option<Vec<String>> {
    crate::filedialog::FileDialog::new()
        .add_filter(tl!("Pictures"), attach::EXTENSIONS)
        .pick_files()
        .map(|ps| ps.iter().map(|p| p.display().to_string()).collect())
}

#[cfg(all(not(target_arch = "wasm32"), test))]
fn pick_files() -> Option<Vec<String>> {
    None
}

/// A notice for a picture that can't be attached, in the interface's language.
fn refused(app: &mut RustGisApp, why: &Note) {
    app.session.notify(crate::i18n::note(why), true);
}

/// Whether the message has room for another picture; says so when it hasn't.
fn room(app: &mut RustGisApp, name: &str) -> bool {
    if app.assistant.pictures.count() < attach::MAX_PER_MESSAGE {
        return true;
    }
    let text = crate::i18n::fmt(
        "{name} wasn't attached: a message takes at most {n} pictures",
        &[("name", name), ("n", &attach::MAX_PER_MESSAGE.to_string())],
    );
    app.session.notify(text, true);
    false
}

/// A picture file for the message (from the dialog, or the browser's or phone's picker), read
/// and made ready to send. One past [`attach::MAX_PER_MESSAGE`], or one that can't be sent, is
/// said in a notice.
pub fn received(app: &mut RustGisApp, name: &str, bytes: Vec<u8>) {
    if !room(app, name) {
        return;
    }
    let name = name.to_string();
    start(app, name.clone(), move || attach::prepare(&name, &bytes));
}

/// A pasted picture's pixels for the message. Its size is checked before its pixels are copied.
fn pasted(app: &mut RustGisApp, image: &egui::ColorImage) {
    let name = tl!("pasted picture").to_string();
    if !room(app, &name) {
        return;
    }
    let [w, h] = image.size;
    let (w, h) = (u32::try_from(w).unwrap_or(u32::MAX), u32::try_from(h).unwrap_or(u32::MAX));
    if !attach::fits(w, h) {
        refused(app, &attach::too_many_pixels(name));
        return;
    }
    let rgba: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_srgba_unmultiplied()).collect();
    start(app, name.clone(), move || attach::from_rgba(&name, w, h, rgba));
}

/// Reads a picture: on a worker on the desktop (decoding a large photo takes a moment), at once
/// in the browser, which has one thread. A worker the system refuses reads it here. Decoding is
/// one step with no check for Cancel; taking the picture off while it is read drops it when the
/// worker is done.
fn start(app: &mut RustGisApp, name: String, read: impl FnOnce() -> Result<Picture, Note> + Send + 'static) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let slot: crate::Slot<Result<Picture, Note>> = Default::default();
        let out = slot.clone();
        let read = std::sync::Arc::new(std::sync::Mutex::new(Some(read)));
        let work = read.clone();
        let spawned = std::thread::Builder::new().name("assistant picture".into()).spawn(move || {
            let Some(read) = work.lock().ok().and_then(|mut r| r.take()) else { return };
            let picture = read();
            if let Ok(mut out) = out.lock() {
                *out = Some(picture);
            }
        });
        if spawned.is_ok() {
            app.assistant.pictures.reading.push((name, slot));
            return;
        }
        if let Some(read) = read.lock().ok().and_then(|mut r| r.take()) {
            finished(app, read());
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = name;
        finished(app, read());
    }
}

/// A picture read: ready for the message, or said in a notice when it can't be sent.
fn finished(app: &mut RustGisApp, picture: Result<Picture, Note>) {
    match picture {
        Ok(p) => app.assistant.pictures.ready.push(p),
        Err(why) => refused(app, &why),
    }
}

/// Takes in the pictures read since the last frame, and a picture pasted into the focused message
/// box (`focused`).
pub fn poll(app: &mut RustGisApp, ui: &Ui, focused: bool) {
    if take_read(app) {
        ui.ctx().request_repaint_after(web_time::Duration::from_millis(100));
    }
    if focused {
        let images: Vec<std::sync::Arc<egui::ColorImage>> = ui.input(|i| {
            i.events.iter().filter_map(|e| if let egui::Event::PasteImage(img) = e { Some(img.clone()) } else { None }).collect()
        });
        for image in images {
            pasted(app, &image);
        }
    }
}

/// Takes in the pictures read since it was last asked; says whether others are still being read.
fn take_read(app: &mut RustGisApp) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut done = Vec::new();
        app.assistant.pictures.reading.retain(|(_, slot)| match slot.lock().ok().and_then(|mut s| s.take()) {
            Some(r) => {
                done.push(r);
                false
            }
            None => true,
        });
        for r in done {
            finished(app, r);
        }
    }
    app.assistant.pictures.reading()
}

/// The message's pictures above the box: each with its size, those still being read with a
/// spinner, and a button on each that takes it off.
pub fn chips(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    if app.assistant.pictures.count() == 0 {
        return;
    }
    let off = |ui: &mut Ui| ui.small_button("×").on_hover_text(tl!("Take this picture off the message")).clicked();
    let mut removed = None;
    #[cfg(not(target_arch = "wasm32"))]
    let mut dropped = None;
    ui.horizontal_wrapped(|ui| {
        for (i, p) in app.assistant.pictures.ready.iter().enumerate() {
            let label = format!("{} · {} × {}", p.name, p.width, p.height);
            ui.label(RichText::new(label).small().color(t.text));
            if off(ui) {
                removed = Some(i);
            }
            ui.add_space(6.0);
        }
        #[cfg(not(target_arch = "wasm32"))]
        for (i, (name, _)) in app.assistant.pictures.reading.iter().enumerate() {
            ui.spinner();
            ui.label(RichText::new(name).small().color(t.text_dim));
            if off(ui) {
                dropped = Some(i);
            }
            ui.add_space(6.0);
        }
    });
    if let Some(i) = removed
        && i < app.assistant.pictures.ready.len()
    {
        app.assistant.pictures.ready.remove(i);
    }
    // Its worker finishes, and what it read goes nowhere.
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(i) = dropped
        && i < app.assistant.pictures.reading.len()
    {
        app.assistant.pictures.reading.remove(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> Vec<u8> {
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 255])))
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    /// Takes in the pictures being read, once each is (a worker reads it).
    fn read_all(app: &mut RustGisApp) {
        for _ in 0..1_000_000 {
            if !take_read(app) {
                return;
            }
            std::thread::yield_now();
        }
        panic!("the picture was never read");
    }

    #[test]
    fn a_picked_picture_goes_with_the_next_message() {
        let mut app = RustGisApp::with_sample();
        let cfg = rustgis_assistant::Config {
            provider: "custom".into(),
            base_url: "http://localhost:9/v1".into(),
            model: "test".into(),
            key: String::new(),
            account: String::new(),
            client_id: String::new(),
            session: String::new(),
            reasoning: None,
            tool_output_token_limit: rustgis_assistant::tools::DEFAULT_OUTPUT_TOKENS,
        };
        app.assistant.agent = Some(rustgis_assistant::Agent::new(cfg, &app.session));
        let layers = app.session.map().layers.len();
        // As the browser's or the phone's picker answers: a picture, and a cancelled pick.
        let _ = app.open_picked(&format!("{}map.png", crate::ASSISTANT_PICK), &png());
        let _ = app.open_picked(crate::ASSISTANT_PICK, &[]);
        read_all(&mut app);
        assert!(app.assistant.pictures.any());
        assert_eq!(app.assistant.pictures.ready.len(), 1, "a cancelled pick attaches nothing");
        assert_eq!(app.session.map().layers.len(), layers, "a picture for the assistant isn't opened on the map");
        // A file that isn't a picture is said, and attaches nothing.
        received(&mut app, "notes.txt", b"text".to_vec());
        read_all(&mut app);
        assert_eq!(app.assistant.pictures.ready.len(), 1);
        assert!(app.session.notices_since(0).iter().any(|n| n.text.contains("PNG, JPEG or WebP")));
        // Sent with the message, and gone from the composer.
        let agent = app.assistant.agent.as_mut().unwrap();
        let step = agent.ask_with("like this", app.assistant.pictures.take(), &app.session);
        assert!(matches!(step, rustgis_assistant::Step::Send(_)));
        assert!(!app.assistant.pictures.any());
        let Some(rustgis_assistant::provider::Turn::User(_, sent)) = app.assistant.agent.as_ref().unwrap().turns().last() else {
            panic!("message")
        };
        assert_eq!(sent.iter().map(|p| (p.name.as_str(), p.width, p.height)).collect::<Vec<_>>(), [("map.png", 3, 2)]);
    }
}
