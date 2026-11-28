//! The AI Assistant's sheet: the conversation, drawn here, with a typing bar that opens the
//! system's text field where there is one (a phone's, with its paste, suggestions and dictation),
//! Send and Stop, approvals as finger-sized cards, and the provider's settings. It is driven by
//! [`rustgis_ui_egui::assistant`], as the desktop's pane is.
//!
//! It stands full (reading and typing) or half (the map and the conversation); lowered while the
//! Assistant works, a strip over the bar says what it is doing and has Stop. Nothing of it shows
//! until a provider is set up (a row in More sets one up), nor at all with the extension off.

use egui::{Align, Layout, RichText, Sense, Stroke, Ui, Vec2};
use rustgis_ui_egui::rustgis_assistant::{Asking, Entry, PRESETS, provider};
use rustgis_ui_egui::rustgis_engine::platform::{TextEntry, TextKind, Typed};
use rustgis_ui_egui::theme::Tokens;
use rustgis_ui_egui::{RustGisApp, assistant, i18n, tl, tl_raw};

use super::{Sheet, Swipe, TARGET, action, entry_box, heading_row, icon_button, set_sheet, sheet, toggle_chip};

/// How tall the sheet stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Height {
    /// Most of the window: for reading and typing.
    Full,
    /// About half: the map and the conversation together.
    #[default]
    Half,
}

impl Height {
    /// The share of the window the sheet takes.
    pub(crate) fn share(self) -> f32 {
        match self {
            Height::Full => super::TYPING_SHARE,
            Height::Half => super::SHEET_SHARE,
        }
    }
}

/// A field of the sheet's that is typed into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Message,
    Model,
    Address,
    Key,
}

/// What the sheet holds between frames, kept in egui's memory.
#[derive(Clone, Debug, Default)]
pub(crate) struct Chat {
    pub(crate) height: Height,
    /// The provider's settings are shown in place of the conversation.
    settings: bool,
    /// The system's text field open for one of the sheet's fields: its id, the field, and the
    /// provider it was opened for (a provider's settings never take what was typed for another's).
    entering: Option<(String, Field, String)>,
    /// What the system's text field open for the message last held.
    entered: String,
    /// How many times the system's text field was opened: each opening's id is its own.
    entries: u64,
    /// The system's text field that was open as the extension was turned off: what it still sends
    /// is not taken, and said once.
    retired: Option<String>,
    /// The approval "Allow such actions for this conversation" is ticked for.
    always: Option<Asking>,
    /// The approval card in sight, and when it came: its buttons take taps only after
    /// [`SETTLE`], so a tap meant for what was there before lands on nothing.
    card: Option<(Asking, f64)>,
}

/// How long, in seconds, an approval card is in sight before its buttons take a tap.
const SETTLE: f64 = 0.3;
/// The approval card's rows besides its details: its title, the "for this conversation" chip and
/// its buttons, counted a finger tall each.
const CARD_ROWS: f32 = 3.0;

fn chat(ctx: &egui::Context) -> Chat {
    ctx.data(|d| d.get_temp(egui::Id::unique("touch_assistant"))).unwrap_or_default()
}

fn set_chat(ctx: &egui::Context, chat: Chat) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("touch_assistant"), chat));
}

/// Raises the sheet at `height`.
pub(crate) fn raise(ctx: &egui::Context, height: Height) {
    let mut state = chat(ctx);
    state.height = height;
    set_chat(ctx, state);
    set_sheet(ctx, Sheet::Assistant);
}

/// The sheet's height while it is up.
pub(crate) fn height(ctx: &egui::Context) -> Height {
    chat(ctx).height
}

/// How far the Assistant is from being usable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Setup {
    /// Its extension is off: nothing of it shows.
    Off,
    /// Its provider or key is still being read.
    Reading,
    /// No provider is set up: More offers to set one up.
    Missing,
    /// Set up: the bar has its button.
    Ready,
}

/// Whether the AI Assistant's extension is on.
pub(crate) fn on(app: &RustGisApp) -> bool {
    app.session.extensions().is_on(rustgis_ui_egui::rustgis_engine::extensions::ASSISTANT.id)
}

/// How far the Assistant is from being usable.
pub(crate) fn setup(app: &RustGisApp) -> Setup {
    if !on(app) {
        return Setup::Off;
    }
    match app.assistant.config() {
        None => Setup::Reading,
        Some(_) if assistant::key_loading(app) => Setup::Reading,
        Some(c) if c.missing().is_some() => Setup::Missing,
        Some(_) => Setup::Ready,
    }
}

/// Each frame: with the extension on, the chosen provider is loaded the first time (its key
/// read off the frame), so the bar knows whether the Assistant is set up; with it off, its sheet
/// is lowered.
pub(crate) fn prepare(app: &mut RustGisApp, ctx: &egui::Context) {
    if on(app) {
        assistant::open(app, ctx);
        return;
    }
    if sheet(ctx) == Sheet::Assistant {
        set_sheet(ctx, Sheet::None);
    }
    let mut state = chat(ctx);
    if let Some((id, ..)) = state.entering.take() {
        state.retired = Some(id);
        set_chat(ctx, state);
    }
}

/// Whether the strip shows: the Assistant works (or a message waits for its key) and its sheet
/// isn't up.
pub(crate) fn strip_shown(app: &RustGisApp, ctx: &egui::Context) -> bool {
    on(app) && sheet(ctx) != Sheet::Assistant && (app.assistant.busy() || app.assistant.waiting_to_send())
}

/// The message `input` with the system's text field's part of it, `was`, now `text`. The field
/// sends all it holds and knows nothing of what was put in the box above it since (a message that
/// couldn't be sent), which is kept.
fn with_entered(input: &str, was: &str, text: &str) -> String {
    // A draft of nothing but spaces is replaced by what is put in the box, not kept under it.
    let above = input.strip_suffix(was).unwrap_or(if was.trim().is_empty() { input } else { "" });
    let line = if above.is_empty() || above.ends_with('\n') { "" } else { "\n" };
    format!("{above}{line}{text}")
}

/// Takes what the system's text field sent back for the sheet's fields, and gives back the rest.
pub(crate) fn take_typed(app: &mut RustGisApp, ctx: &egui::Context, typed: Vec<Typed>) -> Vec<Typed> {
    let mut state = chat(ctx);
    let mut rest = Vec::new();
    for typed in typed {
        let id = match &typed {
            Typed::Text { id, .. } | Typed::Closed { id, .. } => id,
        };
        if !id.starts_with("assistant:") {
            rest.push(typed);
            continue;
        }
        // Open as the extension was turned off: nothing it sends is taken, which is said once,
        // whether it comes while the extension is off or after it is on again.
        if state.retired.as_deref() == Some(id.as_str()) {
            if matches!(typed, Typed::Text { .. }) {
                app.session.notify(tl!("What was typed wasn't kept: the AI Assistant was turned off while it was typed."), true);
            }
            state.retired = None;
            continue;
        }
        let Some((open, field, provider)) = state.entering.clone() else { continue };
        match typed {
            Typed::Text { id, text } if id == open => {
                if field == Field::Message {
                    app.assistant.input = with_entered(&app.assistant.input, &state.entered, &text);
                    state.entered = text;
                    continue;
                }
                let Some(mut cfg) = app.assistant.config().cloned().filter(|c| c.provider == provider) else {
                    state.entering = None;
                    app.session.notify(tl!("What was typed wasn't kept: the Assistant's provider changed while it was typed."), true);
                    continue;
                };
                let before = (cfg.clone(), app.assistant.remember_key());
                match field {
                    Field::Model => cfg.model = text.trim().to_string(),
                    Field::Address => cfg.base_url = text.trim().to_string(),
                    Field::Key => cfg.key = text.trim().to_string(),
                    Field::Message => {}
                }
                assistant::apply_settings(app, cfg, before, ctx);
            }
            Typed::Closed { id, .. } if id == open => state.entering = None,
            _ => {}
        }
    }
    set_chat(ctx, state);
    rest
}

/// The strip over the bar while the Assistant works with its sheet lowered: what it is doing,
/// which raises the sheet when tapped, and Stop.
pub(crate) fn strip(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    ui.horizontal_centered(|ui| {
        ui.spinner();
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if action(ui, t, tl!("Stop"), 88.0, false, true) {
                assistant::stop(app);
            }
            let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), TARGET), Sense::click());
            let mut job = egui::text::LayoutJob::simple_singleline(doing(app), egui::FontId::proportional(14.0), t.text);
            job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width().max(0.0));
            let galley = ui.painter().layout_job(job);
            ui.painter().galley(egui::pos2(rect.min.x, rect.center().y - galley.size().y / 2.0), galley, t.text);
            if response.on_hover_text(tl!("Show the Assistant")).clicked() {
                raise(ui.ctx(), Height::Half);
            }
        });
    });
}

/// What the Assistant is doing, in a few words.
fn doing(app: &RustGisApp) -> String {
    if app.assistant.waiting_to_send() {
        return tl!("Waiting to send: the key or a picture is still being read…").to_string();
    }
    if assistant::pending(app).is_some() {
        return tl!("Waiting for your approval: tap to see it.").to_string();
    }
    if app.assistant.preview().is_some() {
        return tl!("Writing its reply…").to_string();
    }
    let messages = app.assistant.messages();
    let last = messages.iter().rev().take_while(|e| !matches!(e, Entry::User(_))).find(|e| matches!(e, Entry::Tool { .. }));
    match last {
        Some(Entry::Tool { name, summary, error, .. }) if app.assistant.waiting() => {
            let step = format!("{} {}", if *error { "✗" } else { "✓" }, plain(app, name, summary));
            i18n::fmt("{step} · thinking…", &[("step", &step)])
        }
        _ if app.assistant.waiting() => tl!("Thinking…").to_string(),
        // Waiting to try again, say: the note says why and when.
        _ => match messages.last() {
            Some(Entry::Note(n)) => i18n::note(n),
            _ => tl!("Working…").to_string(),
        },
    }
}

/// A tool call in plain words: the command's name, or what the tool does.
fn plain(app: &RustGisApp, name: &str, summary: &str) -> String {
    // A subagent's calls are marked as its.
    let (mark, summary) = match summary.strip_prefix("↳ ") {
        Some(rest) => ("↳ ", rest),
        None => ("", summary),
    };
    let first = summary.split_whitespace().next().unwrap_or_default();
    let words = match name {
        "run_command" => match app.session.registry.get(first) {
            // Without the dots of a label that opens a dialog: here it ran.
            Some(spec) => tl!(spec.label).trim_end_matches('…').to_string(),
            None => first.to_string(),
        },
        "run_tool" => {
            let args: serde_json::Value = summary.split_once(' ').and_then(|(_, json)| serde_json::from_str(json).ok()).unwrap_or_default();
            match args.get("tool").and_then(serde_json::Value::as_str) {
                Some(tool) => i18n::fmt("Ran the tool {tool}", &[("tool", tool)]),
                None => tl!("Ran a tool").to_string(),
            }
        }
        "list_layers" => tl!("Looked at the layers").to_string(),
        "view_map" => tl!("Looked at the map").to_string(),
        "search_commands" => tl!("Looked for a command").to_string(),
        "search_tools" => tl!("Looked for a tool").to_string(),
        "describe_tool" => tl!("Read about a tool").to_string(),
        "read_features" => tl!("Read features").to_string(),
        "search_docs" | "read_doc" => tl!("Read the user guide").to_string(),
        "load_skill" => tl!("Loaded a skill").to_string(),
        "delegate" => tl!("Handed a task to a subagent").to_string(),
        _ => summary.chars().take(60).collect(),
    };
    format!("{mark}{words}")
}

/// The sheet: its heading, then the provider's settings until one is set up (or while they are
/// asked for), else the conversation with its typing bar.
pub(crate) fn show(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    let ctx = ui.ctx().clone();
    let mut state = chat(&ctx);
    let set_up = setup(app);
    // Shown to set the Assistant up, the settings stay until Start: a field filled in doesn't
    // take them away while the rest is still typed.
    if set_up == Setup::Missing {
        state.settings = true;
    }
    let settings = state.settings;
    heading(app, ui, t, &mut state, set_up, settings);
    match set_up {
        Setup::Off => {}
        Setup::Reading => {
            ui.horizontal(|ui| {
                ui.set_min_height(TARGET);
                ui.spinner();
                ui.label(RichText::new(tl!("Reading the Assistant's settings…")).size(15.0).color(t.text_dim));
            });
        }
        _ if settings => settings_form(app, ui, t, &mut state),
        _ => conversation(app, ui, t, &mut state),
    }
    set_chat(&ctx, state);
}

/// The sheet's heading: the button that makes it taller or shorter, the settings, a new
/// conversation, and ×. A swipe down lowers it a step (full to half, half away), a swipe up
/// raises it.
fn heading(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, state: &mut Chat, set_up: Setup, settings: bool) {
    let has_chat = !app.assistant.messages().is_empty();
    let swipe = heading_row(ui, t, tl!("Assistant"), |ui| {
        let (icon, hint, to) = match state.height {
            Height::Half => ("up", tl!("Make the sheet taller"), Height::Full),
            Height::Full => ("down", tl!("Make the sheet shorter"), Height::Half),
        };
        if icon_button(ui, t, icon, hint) {
            state.height = to;
        }
        if set_up == Setup::Ready {
            let hint = if settings { tl!("Back to the conversation") } else { tl!("Assistant settings") };
            if icon_button(ui, t, "settings", hint) {
                state.settings = !settings;
            }
        }
        if has_chat && !settings && icon_button(ui, t, "delete", tl!("New conversation")) {
            assistant::new_conversation(app);
        }
    });
    match (swipe, state.height) {
        (Some(Swipe::Down), Height::Full) => state.height = Height::Half,
        (Some(Swipe::Down), Height::Half) => set_sheet(ui.ctx(), Sheet::None),
        (Some(Swipe::Up), _) => state.height = Height::Full,
        (None, _) => {}
    }
}

/// The conversation, with the approval waiting or the typing bar under it.
fn conversation(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, state: &mut Chat) {
    let approval = assistant::pending(app);
    let fresh = approval.as_ref().map(|a| &a.asked) != state.card.as_ref().map(|(asked, _)| asked);
    if fresh {
        state.card = approval.as_ref().map(|a| (a.asked.clone(), ui.input(|i| i.time)));
    }
    // What the sheet's body has: the card's details take what its heading and buttons leave.
    let room = ui.available_height();
    // A card that doesn't fit half up (a phone held sideways) stands the sheet full as it comes;
    // lowered again by hand, it stays lowered, and scrolls.
    if fresh && approval.is_some() && state.height == Height::Half && room < CARD_ROWS * (TARGET + ui.spacing().item_spacing.y) + TARGET {
        state.height = Height::Full;
        ui.ctx().request_repaint();
    }
    // The foot first, so the messages take what is left above it.
    egui::Panel::bottom("touch_assistant_foot").frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(0, 4))).show(ui, |ui| {
        match &approval {
            Some(approval) => card(app, ui, t, state, approval, room),
            None => typing(app, ui, t, state),
        }
    });
    egui::ScrollArea::vertical().id_salt("touch_assistant_messages").auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
        messages(app, ui, t);
    });
}

/// The conversation's messages, oldest first; the reply arriving, as it streams in.
fn messages(app: &RustGisApp, ui: &mut Ui, t: &Tokens) {
    if app.assistant.messages().is_empty() && !app.assistant.waiting() {
        let hint = tl!(
            "Ask about the map, or tell the Assistant what to do with it. It runs RustGIS's commands for you; every change it makes can be undone."
        );
        ui.label(RichText::new(hint).size(15.0).color(t.text_dim));
        return;
    }
    for (i, e) in app.assistant.messages().iter().enumerate() {
        match e {
            // A message of pictures alone: its notes say them.
            Entry::User(text) if text.is_empty() => {}
            Entry::User(text) => {
                ui.add_space(6.0);
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    egui::Frame::NONE.fill(t.selection).corner_radius(8).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                        ui.set_max_width(ui.available_width() * 0.85);
                        ui.add(egui::Label::new(RichText::new(text).size(15.0).color(t.text)).halign(Align::LEFT));
                    });
                });
                ui.add_space(4.0);
            }
            Entry::Assistant(text) => {
                assistant::reply(ui, text, t);
                ui.add_space(4.0);
            }
            Entry::Tool { name, summary, result, error } => {
                let (mark, ink) = if *error { ("✗", t.red) } else { ("✓", t.green) };
                let head = RichText::new(format!("{mark} {}", plain(app, name, summary))).size(14.0).color(ink);
                egui::CollapsingHeader::new(head).id_salt(("touch_assistant_tool", i)).show(ui, |ui| {
                    ui.label(RichText::new(summary).monospace().small());
                    let shown: String = result.chars().take(2000).collect();
                    ui.label(RichText::new(&shown).monospace().small().color(t.text_dim));
                    let left = result.chars().count().saturating_sub(2000);
                    if left > 0 {
                        let more = i18n::fmt("… and {n} more characters.", &[("n", &left.to_string())]);
                        ui.label(RichText::new(more).small().italics().color(t.text_dim));
                    }
                });
            }
            Entry::Error(e) => assistant::error(ui, e, t),
            Entry::Note(n) => {
                ui.label(RichText::new(i18n::note(n)).size(13.0).italics().color(t.text_dim));
            }
        }
    }
    if app.assistant.waiting() {
        match app.assistant.preview() {
            Some(text) => {
                assistant::reply(ui, text, t);
                ui.spinner();
            }
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(tl!("Thinking…")).size(14.0).color(t.text_dim));
                });
            }
        }
    }
}

/// The approval waiting, as a card: what would run, Stop, Decline and Allow a finger's height,
/// and "Allow such actions for this conversation" where the action may be. `room` is the sheet's
/// body: the details scroll in what the card's other rows leave of it.
///
/// Its widgets are its approval's own, and take taps only once it has been in sight a moment: a
/// tap begun on the card before, or meant for it, never answers this one.
fn card(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, state: &mut Chat, approval: &assistant::Approval, room: f32) {
    let ctx = ui.ctx().clone();
    let now = ui.input(|i| i.time);
    let since = state.card.as_ref().filter(|(asked, _)| *asked == approval.asked).map_or(now, |(_, at)| *at);
    let settled = now - since >= SETTLE;
    if !settled {
        ctx.request_repaint_after(web_time::Duration::from_secs_f64(SETTLE));
    }
    let frame = egui::Frame::NONE.fill(t.amber.gamma_multiply(0.15)).corner_radius(8).inner_margin(10).stroke(Stroke::new(1.0, t.amber));
    // The title, the "for this conversation" chip, the buttons and the card's margins.
    let others = CARD_ROWS * (TARGET + ui.spacing().item_spacing.y) + 24.0;
    // Too short for its details to have rows of their own beside the rest (a phone held
    // sideways, with a track's strip): the whole card scrolls, its details before its buttons.
    let cramped = room - others < 2.0 * TARGET;
    ui.push_id(("touch_assistant_card", &approval.asked), |ui| {
        let mut body = |ui: &mut Ui| {
            frame.show(ui, |ui| {
                ui.label(RichText::new(tl!("Allow this action?")).size(16.0).strong().color(t.text));
                if cramped {
                    assistant::approval_details(ui, app, approval);
                } else {
                    egui::ScrollArea::vertical().id_salt("touch_assistant_approval").max_height(room - others).show(ui, |ui| {
                        assistant::approval_details(ui, app, approval);
                    });
                }
                let width = ui.available_width();
                let mut always = approval.can_always && state.always.as_ref() == Some(&approval.asked);
                if approval.can_always && toggle_chip(ui, t, tl!("Allow such actions for this conversation"), width, always) && settled {
                    always = !always;
                    state.always = always.then(|| approval.asked.clone());
                }
                ui.horizontal(|ui| {
                    let third = (width - 2.0 * ui.spacing().item_spacing.x) / 3.0;
                    if action(ui, t, tl!("Stop"), third, false, settled) {
                        state.always = None;
                        assistant::stop(app);
                        return;
                    }
                    let decline = action(ui, t, tl!("Decline"), third, false, settled);
                    let allow = action(ui, t, tl!("Allow"), third, true, settled);
                    if allow || decline {
                        state.always = None;
                        assistant::decide(app, &approval.asked, allow, always, &ctx);
                    }
                });
            });
        };
        if cramped {
            egui::ScrollArea::vertical().id_salt("touch_assistant_card_scroll").max_height(room.max(TARGET)).show(ui, body);
        } else {
            body(ui);
        }
    });
}

/// The typing bar: the message being written, which opens the system's text field when tapped
/// (or is typed into here where there is none), and Send, or Stop while the Assistant works.
fn typing(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, state: &mut Chat) {
    let ctx = ui.ctx().clone();
    let busy = app.assistant.busy();
    let waiting = app.assistant.waiting_to_send();
    // The box's own message: one a command asked for waits apart.
    if app.assistant.sending_when_ready() {
        ui.label(RichText::new(tl!("It is sent once the key and any picture are read.")).size(13.0).color(t.text_dim));
    }
    let id = egui::Id::unique("touch_assistant_input");
    let mut send = false;
    ui.horizontal(|ui| {
        let gap = ui.spacing().item_spacing.x;
        let button = 88.0;
        let size = Vec2::new((ui.available_width() - button - gap).max(TARGET), TARGET);
        let hint = tl!("Ask, or say what to do…");
        if let Some(enter) = &app.services.enter_text {
            let open = state.entering.as_ref().is_some_and(|(_, f, _)| *f == Field::Message);
            if entry_box(ui, t, size, &app.assistant.input, hint, open).clicked() {
                let entry_id = format!("assistant:message:{}", state.entries);
                state.entries += 1;
                let entry = TextEntry {
                    id: entry_id.clone(),
                    value: app.assistant.input.clone(),
                    label: tl!("Message to the Assistant").to_string(),
                    kind: TextKind::Lines,
                    done: tl!("Done").to_string(),
                };
                match enter(&entry) {
                    Ok(()) => {
                        state.entering = Some((entry_id, Field::Message, String::new()));
                        state.entered = app.assistant.input.clone();
                        // Typing wants room: the conversation above, the keyboard below.
                        state.height = Height::Full;
                    }
                    Err(e) => app.session.notify(e, true),
                }
            }
        } else {
            // Enter sends; Shift+Enter starts a new line. Taken before the box sees it.
            if !busy && ui.memory(|m| m.has_focus(id)) && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                send = true;
            }
            let edit = egui::TextEdit::multiline(&mut app.assistant.input)
                .id(id)
                .desired_rows(1)
                .hint_text(hint)
                .font(egui::FontId::proportional(15.0))
                .margin(egui::Margin::symmetric(8, 10));
            let typed = ui.add_sized(size, edit);
            if typed.gained_focus() {
                state.height = Height::Full;
            }
        }
        if busy || waiting {
            if action(ui, t, tl!("Stop"), button, false, true) {
                assistant::stop(app);
            }
        } else if action(ui, t, tl!("Send"), button, true, !app.assistant.input.trim().is_empty()) {
            send = true;
        }
    });
    if send && !busy {
        // What the system's field sends from now on is of the message sent, not of the next.
        if state.entering.as_ref().is_some_and(|(_, f, _)| *f == Field::Message) {
            state.entering = None;
        }
        let text = app.assistant.input.clone();
        assistant::send(app, text, &ctx);
    }
}

/// The provider's settings: the provider, its model, its address and its key, and whether the key
/// is remembered on this device. A ChatGPT sign-in is set up in the desktop interface.
fn settings_form(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, state: &mut Chat) {
    let ctx = ui.ctx().clone();
    let Some(mut cfg) = app.assistant.config().cloned() else { return };
    let before = (cfg.clone(), app.assistant.remember_key());
    // Start the first time; Done once there is a conversation to go back to.
    let started = !app.assistant.messages().is_empty();
    egui::ScrollArea::vertical().id_salt("touch_assistant_settings").auto_shrink([false, true]).show(ui, |ui| {
        let intro = tl!("The Assistant sends your messages, and what it needs to know of the map, to the AI provider you choose.");
        ui.label(RichText::new(intro).size(14.0).color(t.text_dim));
        label(ui, t, tl!("Provider"));
        // Wrapped, so the provider chosen is in sight whichever it is.
        ui.horizontal_wrapped(|ui| {
            let chosen = cfg.provider.clone();
            for p in PRESETS.iter().filter(|p| !p.login || p.id == chosen) {
                let on = p.id == chosen;
                let text = RichText::new(tl!(p.label)).size(15.0).color(if on { t.accent_text } else { t.text });
                let chip = egui::Button::new(text).fill(if on { t.checked } else { t.input }).min_size(Vec2::new(TARGET, TARGET - 8.0));
                if ui.add(chip).clicked() && !on {
                    assistant::choose_provider(app, &mut cfg, p.id, &ctx);
                }
            }
        });
        let preset = provider::preset(&cfg.provider);
        // What the provider does with what it is sent, as the desktop's settings say it.
        if let Some(p) = preset {
            let description = if p.login || matches!(p.id, "custom" | "ollama") {
                tl!(p.description).to_owned()
            } else {
                i18n::fmt("At the default address: {description}", &[("description", tl_raw!(p.description))])
            };
            ui.label(RichText::new(description).size(13.0).color(t.text_dim));
        }
        if cfg.login() {
            ui.label(
                RichText::new(tl!("Signing in with ChatGPT is set up in the desktop interface: More › Use the Desktop Interface."))
                    .size(14.0)
                    .color(t.text),
            );
        } else {
            label(ui, t, tl!("Model"));
            field(app, ui, t, state, Field::Model, &mut cfg.model, tl!("the model's name"));
            let models: Vec<(String, String)> = app.assistant.models().iter().map(|m| (m.clone(), m.clone())).collect();
            if !models.is_empty() {
                super::chips(ui, t, "touch_assistant_models", &models, &mut cfg.model);
            }
            label(ui, t, tl!("Address"));
            field(app, ui, t, state, Field::Address, &mut cfg.base_url, tl!("the provider's address"));
            {
                // A provider that works without a key may still take one (a server of the user's own).
                label(ui, t, if preset.is_some_and(|p| p.keyless) { tl!("API key, if it needs one") } else { tl!("API key") });
                field(app, ui, t, state, Field::Key, &mut cfg.key, tl!("paste the provider's key here"));
                if let Some(url) = preset.map(|p| p.key_url).filter(|u| !u.is_empty()) {
                    ui.label(RichText::new(i18n::fmt("A key comes from {url}", &[("url", url)])).size(13.0).color(t.text_dim));
                }
                let mut remember = app.assistant.remember_key();
                if toggle_chip(ui, t, tl!("Remember the key on this device"), ui.available_width(), remember) {
                    remember = !remember;
                    app.assistant.set_remember_key(remember);
                }
            }
            if cfg.provider == "openrouter" {
                ui.label(
                    RichText::new(tl!("Ask OpenRouter to exclude endpoints that collect request data. Turning this off permits more endpoints, subject to your OpenRouter account settings."))
                        .size(13.0)
                        .color(t.text_dim),
                );
                if toggle_chip(ui, t, tl!("Private routing"), ui.available_width(), cfg.openrouter_private) {
                    cfg.openrouter_private = !cfg.openrouter_private;
                }
            }
        }
        if let Some(why) = cfg.missing() {
            ui.label(RichText::new(tl!(why)).size(14.0).color(t.amber));
        } else if action(ui, t, if started { tl!("Done") } else { tl!("Start") }, ui.available_width(), true, true) {
            state.settings = false;
        }
    });
    assistant::apply_settings(app, cfg, before, &ctx);
}

/// A label over a field of the settings.
fn label(ui: &mut Ui, t: &Tokens, text: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(text).size(13.0).color(t.text_dim));
}

/// A field of the settings: a box the system's text field opens on, where there is one, else
/// typed into here. The key shows as dots.
fn field(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens, state: &mut Chat, field: Field, value: &mut String, hint: &str) {
    let size = Vec2::new(ui.available_width(), TARGET - 6.0);
    let secret = field == Field::Key;
    let Some(enter) = &app.services.enter_text else {
        // The desktop pane's ids: its settings are saved once none of them is typed in.
        let id = match field {
            Field::Model => "assistant_model",
            Field::Address => "assistant_url",
            _ => "assistant_key",
        };
        let edit = egui::TextEdit::singleline(value)
            .id(egui::Id::unique(id))
            .password(secret)
            .hint_text(hint)
            .font(egui::FontId::proportional(15.0))
            .margin(egui::Margin::symmetric(8, 10));
        ui.add_sized(size, edit);
        return;
    };
    let open = state.entering.as_ref().is_some_and(|(_, f, _)| *f == field);
    let shown = if secret && !value.is_empty() { "•".repeat(12) } else { value.clone() };
    if entry_box(ui, t, size, &shown, hint, open).clicked() {
        let id = format!("assistant:setting:{}", state.entries);
        state.entries += 1;
        let (label, kind) = match field {
            Field::Model => (tl!("The model's name"), TextKind::Plain),
            Field::Address => (tl!("The provider's address"), TextKind::Plain),
            _ => (tl!("The provider's API key"), TextKind::Secret),
        };
        let provider = app.assistant.config().map(|c| c.provider.clone()).unwrap_or_default();
        let entry = TextEntry { id: id.clone(), value: value.clone(), label: label.to_string(), kind, done: tl!("Done").to_string() };
        match enter(&entry) {
            Ok(()) => state.entering = Some((id, field, provider)),
            Err(e) => app.session.notify(e, true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::Held;
    use egui::Rect;
    use rustgis_ui_egui::rustgis_assistant::HttpResponse;
    use serde_json::{Value, json};
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    /// The provider's requests answered in turn by `replies`, and once they are used up never
    /// answered; gives how many were sent.
    fn answering(held: &mut Held, replies: Vec<Value>) -> Arc<Mutex<usize>> {
        let sent = Arc::new(Mutex::new(0));
        let count = sent.clone();
        let replies = Mutex::new(VecDeque::from(replies));
        held.app.services.http = Some(Box::new(move |_, slot| {
            *count.lock().unwrap() += 1;
            if let Some(reply) = replies.lock().unwrap().pop_front() {
                *slot.lock().unwrap() = Some(Ok(HttpResponse { status: 200, body: reply.to_string().into_bytes(), headers: vec![] }));
            }
        }));
        sent
    }

    fn say(text: &str) -> Value {
        json!({"choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]})
    }

    /// A reply calling one command.
    fn command(id: &str, command: &str, params: Value) -> Value {
        let args = json!({"command": command, "params": params}).to_string();
        json!({"choices": [{"message": {"role": "assistant", "content": null,
            "tool_calls": [{"id": id, "type": "function", "function": {"name": "run_command", "arguments": args}}]},
            "finish_reason": "tool_calls"}]})
    }

    /// Chooses `provider` with `model` and no key: an OpenAI-compatible one needs none, so a model
    /// sets it up and none leaves it not set up.
    fn provider(held: &mut Held, provider: &str, model: &str) {
        let ctx = held.ctx.clone();
        assistant::open(&mut held.app, &ctx);
        let mut cfg = held.app.assistant.config().cloned().unwrap();
        let before = (cfg.clone(), held.app.assistant.remember_key());
        assistant::choose_provider(&mut held.app, &mut cfg, provider, &ctx);
        cfg.model = model.into();
        cfg.key.clear();
        assistant::apply_settings(&mut held.app, cfg, before, &ctx);
        held.frame(Vec::new());
        held.frame(Vec::new());
    }

    /// The bar's labels, left to right.
    fn bar(held: &mut Held) -> Vec<String> {
        let foot = held.screen.y - crate::BAR;
        let mut words: Vec<(String, Rect)> = held.frame(Vec::new()).into_iter().filter(|(_, at)| at.min.y >= foot).collect();
        words.sort_by(|a, b| a.1.min.x.total_cmp(&b.1.min.x));
        words.into_iter().map(|(word, _)| word).collect()
    }

    /// Where `word` is on the bar.
    fn on_bar(held: &mut Held, word: &str) -> egui::Pos2 {
        let foot = held.screen.y - crate::BAR;
        held.frame(Vec::new()).into_iter().find(|(w, at)| w == word && at.min.y >= foot).map(|(_, at)| at.center()).unwrap()
    }

    /// The system's text field, as a phone's shell opens it: what was asked for is kept.
    fn native(held: &mut Held) -> Rc<RefCell<Vec<TextEntry>>> {
        let asked: Rc<RefCell<Vec<TextEntry>>> = Rc::default();
        let asking = asked.clone();
        held.app.services.enter_text = Some(Box::new(move |entry| {
            asking.borrow_mut().push(entry.clone());
            Ok(())
        }));
        asked
    }

    fn said(held: &Held, mark: u64) -> Vec<String> {
        held.app.session.notices_since(mark).iter().map(|n| n.text.clone()).collect()
    }

    #[test]
    fn nothing_of_the_assistant_shows_until_it_is_set_up_nor_with_its_extension_off() {
        let mut held = Held::new();
        // Tall enough for the whole of More.
        held.screen = Vec2::new(411.0, 1600.0);
        provider(&mut held, "custom", "");
        assert_eq!(setup(&held.app), Setup::Missing);
        assert_eq!(bar(&mut held), ["Add Data", "Capture", "More"]);
        assert!(!strip_shown(&held.app, &held.ctx));
        // The only trace: a row in More, which opens the settings.
        held.raise(Sheet::More, "More");
        let row = held.find("Set Up the Assistant").expect("the row in More").center();
        held.touch(&[row]);
        assert_eq!(sheet(&held.ctx), Sheet::Assistant);
        assert!(held.find("Choose a model.").is_some(), "the settings say what is missing");
        // Set up: its button on the bar, and no row in More.
        provider(&mut held, "custom", "test");
        assert_eq!(setup(&held.app), Setup::Ready);
        assert_eq!(bar(&mut held), ["Add Data", "Capture", "Assistant", "More"]);
        held.raise(Sheet::More, "More");
        assert!(held.find("Set Up the Assistant").is_none());
        // The extension off: none of it, and its sheet is lowered.
        held.raise(Sheet::Assistant, "Assistant");
        crate::run(&mut held.app, "extension.disable", json!({"id": "assistant"}));
        held.frame(Vec::new());
        assert_eq!(sheet(&held.ctx), Sheet::None);
        assert_eq!(bar(&mut held), ["Add Data", "Capture", "More"]);
        provider(&mut held, "custom", "");
        held.raise(Sheet::More, "More");
        assert!(held.find("Set Up the Assistant").is_none());
    }

    #[test]
    fn layers_is_a_button_on_the_map_and_the_assistant_takes_its_place_on_the_bar() {
        let mut held = Held::new();
        provider(&mut held, "custom", "test");
        // A finger's size at the map's top-right corner: it raises the Layers sheet and lowers it,
        // and the map under it takes neither tap.
        let map = held.app.map.view_rect.unwrap().0;
        let corner = egui::pos2(map.max.x - 8.0 - TARGET / 2.0, map.min.y + 8.0 + TARGET / 2.0);
        let camera = held.app.session.map().camera;
        held.touch(&[corner + Vec2::new(TARGET / 2.0 - 4.0, TARGET / 2.0 - 4.0)]);
        assert_eq!(sheet(&held.ctx), Sheet::Layers);
        held.touch(&[corner]);
        assert_eq!(sheet(&held.ctx), Sheet::None);
        assert_eq!(held.app.session.map().camera, camera);
        // ModelBuilder in the map's place has its own buttons there: Layers is on the bar then.
        crate::run(&mut held.app, "ui.modelBuilder", json!({}));
        assert_eq!(bar(&mut held), ["Layers", "Add Data", "Capture", "Assistant", "More"]);
        let back = held.find("Back to the map").unwrap().center();
        held.touch(&[back]);
        assert_eq!(bar(&mut held), ["Add Data", "Capture", "Assistant", "More"]);
        // The bar's Assistant raises its sheet and lowers it.
        let button = on_bar(&mut held, "Assistant");
        held.touch(&[button]);
        assert_eq!(sheet(&held.ctx), Sheet::Assistant);
        held.touch(&[button]);
        assert_eq!(sheet(&held.ctx), Sheet::None);
    }

    #[test]
    fn a_message_typed_in_the_systems_text_field_is_sent_and_the_reply_drawn() {
        let mut held = Held::new();
        provider(&mut held, "custom", "test");
        let sent = answering(&mut held, vec![say("Tokyo is in view.")]);
        let asked = native(&mut held);
        raise(&held.ctx, Height::Half);
        held.frame(Vec::new());
        let bar = held.find("Ask, or say what to do…").unwrap().center();
        held.touch(&[bar]);
        let entry = asked.borrow().last().cloned().expect("the system's text field");
        assert_eq!((entry.kind, entry.value.as_str()), (TextKind::Lines, ""));
        assert_eq!(height(&held.ctx), Height::Full, "typing raises the sheet");
        held.app.typed.push(Typed::Text { id: entry.id.clone(), text: "Zoom to Tokyo".into() });
        held.frame(Vec::new());
        assert_eq!(held.app.assistant.input, "Zoom to Tokyo");
        let send = held.find("Send").unwrap().center();
        held.touch(&[send]);
        assert_eq!(*sent.lock().unwrap(), 1);
        assert!(held.app.assistant.messages().iter().any(|e| matches!(e, Entry::User(t) if t == "Zoom to Tokyo")));
        assert!(held.find("Tokyo is in view.").is_some(), "the reply is drawn");
        // What the field still sends after the message went is the message's, not the next one's.
        held.app.typed.push(Typed::Text { id: entry.id.clone(), text: "Zoom to Tokyo".into() });
        held.app.typed.push(Typed::Closed { id: entry.id, failed: None });
        held.frame(Vec::new());
        assert_eq!(held.app.assistant.input, "");
    }

    /// Of several messages a command asks for in one frame, the first goes and, the Assistant busy
    /// with it, the rest are put in the box in the order asked, said once, and aren't sent later.
    #[test]
    fn messages_a_command_asks_for_while_one_is_out_go_into_the_box_in_order() {
        let mut held = Held::new();
        provider(&mut held, "custom", "test");
        // Requests whose replies are held back.
        let out = Arc::new(Mutex::new(Vec::new()));
        let asking = out.clone();
        held.app.services.http = Some(Box::new(move |_, slot| asking.lock().unwrap().push(slot)));
        let mark = held.app.session.notice_mark();
        for ask in ["first", "second", "third"] {
            crate::run(&mut held.app, "ui.pane.assistant", json!({"ask": ask}));
        }
        for _ in 0..3 {
            held.frame(Vec::new());
        }
        let asked = |held: &Held| -> Vec<String> {
            held.app.assistant.messages().iter().filter_map(|e| if let Entry::User(t) = e { Some(t.clone()) } else { None }).collect()
        };
        assert_eq!(asked(&held), ["first"]);
        assert_eq!(held.app.assistant.input, "second\nthird");
        assert_eq!(said(&held, mark).iter().filter(|n| n.contains("busy")).count(), 1, "{:?}", said(&held, mark));
        let slot = out.lock().unwrap().remove(0);
        *slot.lock().unwrap() = Some(Ok(HttpResponse { status: 200, body: say("Done.").to_string().into_bytes(), headers: vec![] }));
        for _ in 0..3 {
            held.frame(Vec::new());
        }
        assert!(out.lock().unwrap().is_empty(), "nothing more is sent by itself");
        assert_eq!(asked(&held), ["first"]);
        assert_eq!(held.app.assistant.input, "second\nthird");
    }

    /// A message put in the box while the system's text field is open on it stays, above what is
    /// typed: the field sends all it holds and doesn't have it.
    #[test]
    fn a_message_put_in_the_box_while_the_systems_text_field_is_open_is_kept() {
        for draft in ["", " ", "a draft"] {
            let mut held = Held::new();
            provider(&mut held, "custom", "test");
            held.app.services.http = Some(Box::new(|_, _| {}));
            let asked = native(&mut held);
            raise(&held.ctx, Height::Half);
            held.frame(Vec::new());
            let bar = held.find("Ask, or say what to do…").unwrap().center();
            held.touch(&[bar]);
            let id = asked.borrow().last().cloned().expect("the system's text field").id;
            held.app.typed.push(Typed::Text { id: id.clone(), text: draft.into() });
            held.frame(Vec::new());
            for ask in ["first", "second"] {
                crate::run(&mut held.app, "ui.pane.assistant", json!({"ask": ask}));
            }
            held.frame(Vec::new());
            held.frame(Vec::new());
            for text in ["a draft, longer", "", "another"] {
                held.app.typed.push(Typed::Text { id: id.clone(), text: text.into() });
                held.frame(Vec::new());
                assert_eq!(held.app.assistant.input, format!("second\n{text}"), "after the draft {draft:?}");
            }
        }
    }

    /// The Layers button isn't on a strip of map too short for it, and is back as the sheet lowers.
    #[test]
    fn layers_is_not_on_a_strip_of_map_too_short_for_it() {
        let mut held = Held::new();
        held.screen = Vec2::new(800.0, 360.0);
        provider(&mut held, "custom", "test");
        let layers = egui::pos2(800.0 - 8.0 - TARGET / 2.0, 16.0);
        raise(&held.ctx, Height::Full);
        held.frame(Vec::new());
        held.touch(&[layers]);
        assert_eq!(sheet(&held.ctx), Sheet::None, "a tap on the map above the sheet lowers it");
        held.touch(&[layers]);
        assert_eq!(sheet(&held.ctx), Sheet::Layers);
    }

    #[test]
    fn without_a_system_text_field_the_message_is_typed_here() {
        let mut held = Held::new();
        provider(&mut held, "custom", "test");
        answering(&mut held, vec![say("Done.")]);
        raise(&held.ctx, Height::Half);
        held.frame(Vec::new());
        let bar = held.find("Ask, or say what to do…").unwrap().center();
        held.touch(&[bar]);
        held.frame(vec![egui::Event::Text("Label the cities".into())]);
        assert_eq!(held.app.assistant.input, "Label the cities");
        let enter =
            egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        held.frame(vec![enter]);
        held.frame(Vec::new());
        assert!(held.app.assistant.messages().iter().any(|e| matches!(e, Entry::User(t) if t == "Label the cities")));
        assert_eq!(held.app.assistant.input, "", "Enter sent it, adding no line");
    }

    #[test]
    fn stop_ends_the_run_from_the_sheet_and_from_the_strip_left_while_it_works() {
        let mut held = Held::new();
        provider(&mut held, "custom", "test");
        // Never answered: the Assistant works until it is stopped.
        answering(&mut held, Vec::new());
        raise(&held.ctx, Height::Half);
        held.app.assistant.input = "Buffer the cities".into();
        let send = held.find("Send").unwrap().center();
        held.touch(&[send]);
        assert!(held.app.assistant.busy());
        let stop = held.find("Stop").unwrap().center();
        held.touch(&[stop]);
        assert!(!held.app.assistant.busy());
        assert!(held.find("Stopped.").is_some());
        // Again, and the sheet lowered: the strip says what it is doing and has Stop.
        held.app.assistant.input = "Buffer the cities again".into();
        let send = held.find("Send").unwrap().center();
        held.touch(&[send]);
        set_sheet(&held.ctx, Sheet::None);
        assert!(held.find("Thinking…").is_some(), "the strip");
        assert!(strip_shown(&held.app, &held.ctx));
        // Its words raise the sheet, half up.
        let words = held.find("Thinking…").unwrap().center();
        held.touch(&[words]);
        assert_eq!((sheet(&held.ctx), height(&held.ctx)), (Sheet::Assistant, Height::Half));
        set_sheet(&held.ctx, Sheet::None);
        let stop = held.find("Stop").unwrap().center();
        held.touch(&[stop]);
        assert!(!held.app.assistant.busy());
        assert!(!strip_shown(&held.app, &held.ctx));
        assert!(held.find("Thinking…").is_none());
    }

    #[test]
    fn an_approval_is_a_card_and_one_no_longer_waiting_answers_nothing() {
        let mut held = Held::new();
        provider(&mut held, "custom", "test");
        let path = std::env::temp_dir().join(format!("rustgis-touch-never-written-{}.png", std::process::id()));
        let export = || command("c0", "share.exportMap", json!({"path": path}));
        // The second reply calls the same id again, as some providers' replies do.
        answering(&mut held, vec![export(), export(), say("Nothing exported.")]);
        raise(&held.ctx, Height::Half);
        held.app.assistant.input = "Export the map".into();
        let send = held.find("Send").unwrap().center();
        held.touch(&[send]);
        held.frame(Vec::new());
        let shown = assistant::pending(&held.app).expect("an approval");
        assert!(held.find("Allow this action?").is_some());
        let decline = held.find("Decline").unwrap();
        let mark = held.app.session.notice_mark();
        held.touch(&[decline.center()]);
        // Answered, and the next reply asks again. A second tap there as its card comes (a double
        // tap meant for the first) answers nothing.
        let next = assistant::pending(&held.app).expect("the next approval");
        assert!(next.asked != shown.asked);
        held.touch(&[decline.center()]);
        assert!(assistant::pending(&held.app).is_some_and(|p| p.asked == next.asked), "the second tap answered nothing");
        // Nor does the first card's answer, given late.
        assert!(said(&held, mark).iter().all(|n| !n.contains("no longer waiting")), "{:?}", said(&held, mark));
        let entries = held.app.assistant.messages().len();
        let ctx = held.ctx.clone();
        assistant::decide(&mut held.app, &shown.asked, true, false, &ctx);
        assert_eq!(held.app.assistant.messages().len(), entries);
        assert!(assistant::pending(&held.app).is_some_and(|p| p.asked == next.asked));
        let notices = said(&held, mark);
        assert_eq!(notices.iter().filter(|n| n.contains("no longer waiting")).count(), 1, "{notices:?}");
        // In sight a moment, its Decline answers it.
        let decline = held.find("Decline").unwrap().center();
        held.touch(&[decline]);
        held.frame(Vec::new());
        assert!(assistant::pending(&held.app).is_none());
        assert!(!path.exists());
        assert!(held.find("Nothing exported.").is_some());
    }

    #[test]
    fn the_sheet_stands_full_or_half_by_its_button_or_a_swipe_and_a_swipe_lowers_it() {
        let mut held = Held::new();
        provider(&mut held, "custom", "test");
        let title = |held: &mut Held| held.raise(Sheet::Assistant, "Assistant");
        raise(&held.ctx, Height::Half);
        let half = title(&mut held);
        // The button beside × makes it taller, and shorter again.
        let spacing = held.ctx.global_style().spacing.item_spacing.x;
        let button = egui::pos2(held.screen.x - 8.0 - TARGET - spacing - TARGET / 2.0, half.y);
        held.touch(&[button]);
        assert_eq!(height(&held.ctx), Height::Full);
        let full = title(&mut held);
        assert!(full.y < half.y - 100.0, "{full:?} over {half:?}");
        held.touch(&[egui::pos2(button.x, full.y)]);
        assert_eq!(height(&held.ctx), Height::Half);
        // Up a step by a swipe up; down a step by each swipe down, the last lowering it.
        let swipe =
            |held: &mut Held, from: egui::Pos2, dy: f32| held.touch(&[from, from + Vec2::new(0.0, dy / 2.0), from + Vec2::new(0.0, dy)]);
        let from = egui::pos2(160.0, half.y);
        swipe(&mut held, from, -120.0);
        assert_eq!(height(&held.ctx), Height::Full);
        let full = title(&mut held);
        swipe(&mut held, egui::pos2(160.0, full.y), 120.0);
        assert_eq!((sheet(&held.ctx), height(&held.ctx)), (Sheet::Assistant, Height::Half));
        let half = title(&mut held);
        swipe(&mut held, egui::pos2(160.0, half.y), 120.0);
        assert_eq!(sheet(&held.ctx), Sheet::None);
        // Back lowers it as it does every sheet.
        let back = egui::Event::Key {
            key: egui::Key::BrowserBack,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        title(&mut held);
        held.frame(vec![back]);
        assert_eq!(sheet(&held.ctx), Sheet::None);
    }

    #[test]
    fn the_settings_take_a_key_from_the_systems_text_field_for_its_provider_only() {
        let mut held = Held::new();
        // Tall enough for the whole form, which scrolls on a phone.
        held.screen = Vec2::new(411.0, 1600.0);
        let asked = native(&mut held);
        provider(&mut held, "openai", "gpt-test");
        assert_eq!(setup(&held.app), Setup::Missing);
        raise(&held.ctx, Height::Full);
        held.frame(Vec::new());
        assert!(held.find("Enter an API key.").is_some());
        let key = held.find("paste the provider's key here").unwrap().center();
        held.touch(&[key]);
        let entry = asked.borrow().last().cloned().expect("the system's text field");
        assert_eq!(entry.kind, TextKind::Secret);
        let mark = held.app.session.notice_mark();
        held.app.typed.push(Typed::Text { id: entry.id.clone(), text: " synthetic-key ".into() });
        held.frame(Vec::new());
        assert_eq!(held.app.assistant.config().map(|c| c.key.as_str()), Some("synthetic-key"));
        assert_eq!(setup(&held.app), Setup::Ready);
        assert!(said(&held, mark).is_empty());
        // Set up, the settings stay until Start, and the key shows as dots.
        assert!(held.find("synthetic-key").is_none());
        let start = held.find("Start").expect("the settings stay up").center();
        held.touch(&[start]);
        assert!(held.find("Ask, or say what to do…").is_some(), "the conversation");
        // Typed for one provider's key while another was chosen: not kept, and said once.
        provider(&mut held, "openai", "gpt-test");
        raise(&held.ctx, Height::Full);
        held.frame(Vec::new());
        let key = held.find("paste the provider's key here").unwrap().center();
        held.touch(&[key]);
        let entry = asked.borrow().last().cloned().unwrap();
        provider(&mut held, "gemini", "gemini-test");
        held.app.typed.push(Typed::Text { id: entry.id.clone(), text: "for openai".into() });
        held.app.typed.push(Typed::Text { id: entry.id, text: "for openai, longer".into() });
        held.frame(Vec::new());
        assert_eq!(held.app.assistant.config().map(|c| c.key.as_str()), Some(""));
        let notices = said(&held, mark);
        assert_eq!(notices.iter().filter(|n| n.contains("wasn't kept")).count(), 1, "{notices:?}");
    }

    /// The settings say what the provider does with what it is sent, and for OpenRouter offer
    /// private routing, as the desktop's do; a tap turns it off.
    #[test]
    fn the_settings_describe_the_provider_and_offer_openrouters_private_routing() {
        let mut held = Held::new();
        // Tall enough for the whole form, which scrolls on a phone.
        held.screen = Vec2::new(411.0, 1600.0);
        provider(&mut held, "openai", "gpt-test");
        raise(&held.ctx, Height::Full);
        held.frame(Vec::new());
        assert!(held.find("Private routing").is_none(), "only for OpenRouter");
        provider(&mut held, "openrouter", "m");
        raise(&held.ctx, Height::Full);
        held.frame(Vec::new());
        let description = provider::preset("openrouter").map(|p| p.description).unwrap();
        assert!(held.find(&format!("At the default address: {description}")).is_some());
        assert_eq!(held.app.assistant.config().map(|c| c.openrouter_private), Some(true));
        let chip = held.find("Private routing").expect("the choice").center();
        held.touch(&[chip]);
        assert_eq!(held.app.assistant.config().map(|c| c.openrouter_private), Some(false));
    }

    /// What the system's text field sends for the settings after the extension is turned off is
    /// not taken, and that is said once; a provider that needs no key still offers its key field.
    #[test]
    fn typing_for_the_settings_after_the_assistant_is_turned_off_is_not_kept() {
        let mut held = Held::new();
        let asked = native(&mut held);
        provider(&mut held, "custom", "");
        raise(&held.ctx, Height::Full);
        held.frame(Vec::new());
        assert!(held.find("API key, if it needs one").is_some());
        let model = held.find("the model's name").unwrap().center();
        held.touch(&[model]);
        let entry = asked.borrow().last().cloned().expect("the system's text field");
        crate::run(&mut held.app, "extension.disable", json!({"id": "assistant"}));
        let mark = held.app.session.notice_mark();
        held.app.typed.push(Typed::Text { id: entry.id.clone(), text: "test".into() });
        held.app.typed.push(Typed::Text { id: entry.id, text: "test model".into() });
        held.frame(Vec::new());
        assert_eq!(held.app.assistant.config().map(|c| c.model.as_str()), Some(""));
        let notices = said(&held, mark);
        assert_eq!(notices.iter().filter(|n| n.contains("turned off while it was typed")).count(), 1, "{notices:?}");
        // Nor what it sends only once the extension is on again.
        crate::run(&mut held.app, "extension.enable", json!({"id": "assistant"}));
        raise(&held.ctx, Height::Full);
        held.frame(Vec::new());
        let model = held.find("the model's name").unwrap().center();
        held.touch(&[model]);
        let entry = asked.borrow().last().cloned().expect("the system's text field");
        crate::run(&mut held.app, "extension.disable", json!({"id": "assistant"}));
        held.frame(Vec::new());
        crate::run(&mut held.app, "extension.enable", json!({"id": "assistant"}));
        let mark = held.app.session.notice_mark();
        held.app.typed.push(Typed::Text { id: entry.id.clone(), text: "late".into() });
        held.app.typed.push(Typed::Closed { id: entry.id, failed: None });
        held.frame(Vec::new());
        assert_eq!(held.app.assistant.config().map(|c| c.model.as_str()), Some(""));
        let notices = said(&held, mark);
        assert_eq!(notices.iter().filter(|n| n.contains("turned off while it was typed")).count(), 1, "{notices:?}");
    }

    /// In a short window (a phone held sideways) the approval card raises the sheet as it comes and
    /// scrolls whole, what would run before its buttons; lowered by hand, the sheet stays lowered.
    #[test]
    fn an_approval_card_in_a_short_window_scrolls_whole_and_its_height_is_the_users() {
        let mut held = Held::new();
        held.screen = Vec2::new(800.0, 360.0);
        provider(&mut held, "custom", "test");
        let path = std::env::temp_dir().join(format!("rustgis-touch-short-never-written-{}.png", std::process::id()));
        answering(&mut held, vec![command("c0", "share.exportMap", json!({"path": path})), say("Not exported.")]);
        raise(&held.ctx, Height::Half);
        held.app.assistant.input = "Export the map".into();
        let send = held.find("Send").unwrap().center();
        held.touch(&[send]);
        held.frame(Vec::new());
        assert!(assistant::pending(&held.app).is_some());
        assert_eq!(height(&held.ctx), Height::Full, "raised as the card came");
        let words = held.frame(Vec::new());
        let title = words.iter().find(|(w, _)| w == "Allow this action?").map(|(_, at)| *at).expect("the card's title in sight");
        let what = words.iter().find(|(w, _)| w == "Export Map").map(|(_, at)| *at).expect("what would run, in sight");
        assert!(title.max.y <= what.min.y);
        // Its buttons are reached by scrolling the card.
        let over = egui::pos2(400.0, what.center().y);
        let mut decline = None;
        for _ in 0..20 {
            let wheel = egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -60.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
            };
            let words = held.frame(vec![egui::Event::PointerMoved(over), wheel]);
            decline = words.into_iter().find(|(w, at)| w == "Decline" && at.max.y <= held.screen.y - crate::BAR).map(|(_, at)| at.center());
            if decline.is_some() {
                break;
            }
        }
        held.touch(&[decline.expect("Decline scrolled into sight")]);
        held.frame(Vec::new());
        assert!(assistant::pending(&held.app).is_none());
        assert!(!path.exists());
        // Lowered by hand while a card is up, it stays lowered.
        held.app.assistant.input = "Export it again".into();
        answering(&mut held, vec![command("c0", "share.exportMap", json!({"path": path}))]);
        let send = held.find("Send").unwrap().center();
        held.touch(&[send]);
        held.frame(Vec::new());
        assert_eq!(height(&held.ctx), Height::Full);
        let full = held.raise(Sheet::Assistant, "Assistant");
        held.touch(&[full, full + Vec2::new(0.0, 60.0), full + Vec2::new(0.0, 120.0)]);
        held.frame(Vec::new());
        assert_eq!((sheet(&held.ctx), height(&held.ctx)), (Sheet::Assistant, Height::Half));
    }
}
