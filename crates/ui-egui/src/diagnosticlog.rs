//! Session troubleshooting history. Capture lives below the UI and continues with this pane closed.

use egui::{RichText, Ui};
use rustgis_engine::data::diagnostics::{Filter, NetworkKind, Record, Severity};
use rustgis_engine::data::loss::LossKind;

use crate::{RustGisApp, theme::Tokens};

/// The Messages pane: its filters, Copy diagnostics and Clear, then the entries, newest first.
pub(crate) fn show(app: &mut RustGisApp, ui: &mut Ui, t: &Tokens) {
    // Background requests may finish without another UI action (including HEAD probes).
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
    let generation = app.session.diagnostics.generation();
    let mut records = match app.ui.diagnostic_snapshot.take() {
        Some((at, records)) if at == generation => records,
        _ => app.session.diagnostics.records(),
    };
    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.label(
            RichText::new(tl!("Messages and failed requests from this session. Oldest entries are discarded when the history is full."))
                .small()
                .color(t.text_dim),
        );
        ui.horizontal_wrapped(|ui| {
            // Literal labels, so each is found and translated with the interface's other strings.
            for (filter, label) in [
                (Filter::All, tl_raw!("All")),
                (Filter::Errors, tl_raw!("Errors")),
                (Filter::Warnings, tl_raw!("Warnings")),
                (Filter::Info, tl_raw!("Info")),
                (Filter::Network, tl_raw!("Network failures")),
                (Filter::Losses, tl_raw!("Data losses")),
            ] {
                let count = records.iter().filter(|r| filter.matches(r)).count();
                let text = crate::i18n::fmt("{label} ({count})", &[("label", label), ("count", &count.to_string())]);
                ui.selectable_value(&mut app.ui.diagnostic_filter, filter, text);
            }
        });
        ui.horizontal_wrapped(|ui| {
            let any = records.iter().any(|r| app.ui.diagnostic_filter.matches(r));
            if ui
                .add_enabled(any, egui::Button::new(tl!("Copy diagnostics")))
                .on_hover_text(tl!("Copy the filtered history, including technical details"))
                .clicked()
            {
                ui.ctx().copy_text(app.session.diagnostics.report(app.ui.diagnostic_filter));
            }
            if ui
                .add_enabled(!records.is_empty(), egui::Button::new(tl!("Clear")))
                .on_hover_text(tl!("Clear all diagnostic history for this session"))
                .clicked()
            {
                app.session.diagnostics.clear();
                app.ui.diagnostic_filter = Filter::All;
                records.clear();
            }
        });
    });
    ui.separator();
    egui::ScrollArea::vertical().id_salt("diagnostic_log_entries").auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
            let filter = app.ui.diagnostic_filter;
            let shown: Vec<&Record> = records.iter().rev().filter(|r| filter.matches(r)).collect();
            // The messages of one command run, newest first, under that run's heading.
            for group in shown.chunk_by(|a, b| a.run.is_some() && a.run.as_ref().map(|r| r.id) == b.run.as_ref().map(|r| r.id)) {
                let Some(first) = group.first() else { continue };
                ui.push_id(first.id, |ui| {
                    if first.run.is_some() {
                        run_heading(ui, app, first, t);
                    }
                    for record in group {
                        ui.push_id(record.id, |ui| entry(ui, record, t, first.run.is_none(), &mut app.ui.open_url));
                    }
                });
                ui.separator();
            }
            let shown = !shown.is_empty();
            if !shown {
                ui.label(tl!("No diagnostic entries match this filter."));
            }
        });
    });
    app.ui.diagnostic_snapshot = Some((generation, records));
}

/// A command run's heading over the messages it said: the command by name, what it worked on and
/// when, with Undo to before it while the history still reaches that state.
fn run_heading(ui: &mut Ui, app: &mut RustGisApp, record: &Record, t: &Tokens) {
    let Some(run) = &record.run else { return };
    let name = app.session.registry.get(record.source.as_str()).map_or(record.source.as_str(), |spec| crate::i18n::t(spec.label));
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(name).strong());
        if let Some(target) = &run.target {
            ui.label(RichText::new(crate::i18n::fmt("on {target}", &[("target", target.as_str())])).color(t.text_dim));
        }
        ui.label(RichText::new(time(record.timestamp_ms)).small().monospace().color(t.text_dim));
    });
    // Only a run that changed the project, whose state before is still in the history.
    let steps = app.session.undo_steps_to(run.before).filter(|steps| *steps > 0);
    if let Some(steps) = steps {
        let hover = if steps == 1 {
            tl!("Takes back this command's changes").to_string()
        } else {
            crate::i18n::fmt("Takes back this command's changes and the {n} after it", &[("n", &(steps - 1).to_string())])
        };
        if ui.small_button(tl!("Undo to before this")).on_hover_text(hover).clicked() {
            app.ui.history_jump = Some(crate::chrome::HistoryJump::undo(app.session.history_position(), steps));
            ui.ctx().request_repaint();
        }
    }
}

/// One entry, laid out as a compiler message: its severity, and its loss code when it reports lost
/// data; the message; then what the code means (`note`) and what to do (`help`), from text built
/// into the app so they read offline, with the guide's entry for more. A message no command said
/// shows its time and source itself (`standalone`).
fn entry(ui: &mut Ui, record: &Record, t: &Tokens, standalone: bool, open_url: &mut Option<String>) {
    // Literal labels, so each is found and translated with the interface's other strings.
    let (severity, color) = match record.severity {
        Severity::Error => (tl!("Error"), t.red),
        Severity::Warning => (tl!("Warning"), t.amber),
        Severity::Info => (tl!("Info"), t.accent_text),
    };
    ui.horizontal_wrapped(|ui| {
        if standalone {
            ui.label(RichText::new(time(record.timestamp_ms)).small().monospace().color(t.text_dim));
        }
        ui.label(RichText::new(severity).small().strong().color(color));
        if let Some(code) = record.loss {
            // The code is an identifier, never translated: scripts and the guide match on it.
            ui.label(RichText::new(format!("[{}]", code.spec().id)).small().monospace().color(color));
            let kind = match code.spec().kind {
                LossKind::Dropped => tl!("data left out"),
                LossKind::Changed => tl!("data changed"),
                LossKind::Approximated => tl!("data approximated"),
                LossKind::Assumed => tl!("something assumed"),
            };
            ui.label(RichText::new(kind).small().color(t.text_dim));
        }
        if standalone {
            // A source is a command id (kept as written) or one of the history's own words.
            let source = match record.source.as_str() {
                "Application" => tl!("Application"),
                "Network" => tl!("Network"),
                "Plugin" => tl!("Plugin"),
                command => command,
            };
            ui.label(RichText::new(source).small().color(t.text_dim));
        }
    });
    ui.add(egui::Label::new(&record.message).wrap().selectable(true));
    if let Some(code) = record.loss {
        let spec = code.spec();
        let line = |ui: &mut Ui, label: &str, text: &str| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(label).small().strong().color(t.text_dim));
                ui.add(egui::Label::new(RichText::new(text).small()).wrap());
            });
        };
        line(ui, tl!("= note:"), crate::i18n::t(spec.meaning));
        line(ui, tl!("= help:"), crate::i18n::t(spec.action));
        if ui.link(RichText::new(tl!("More in the guide")).small()).on_hover_text(code.doc_url()).clicked() {
            *open_url = Some(code.doc_url());
        }
    }
    if !record.details.is_empty() || record.network.is_some() {
        egui::CollapsingHeader::new(tl!("Technical details")).show(ui, |ui| {
            if let Some(network) = &record.network {
                let kind = match network.kind {
                    NetworkKind::Http => tl!("HTTP failure"),
                    NetworkKind::Connection => tl!("Connection failure"),
                    NetworkKind::Timeout => tl!("Timeout"),
                    NetworkKind::Other => tl!("Request failure"),
                };
                ui.label(kind);
                ui.add(
                    egui::Label::new(RichText::new(format!("{} {}", network.method, network.url)).monospace().small())
                        .wrap()
                        .selectable(true),
                );
                if let Some(status) = network.status {
                    ui.label(crate::i18n::fmt("HTTP status: {status}", &[("status", &status.to_string())]));
                }
                if let Some(duration) = network.duration_ms {
                    ui.label(crate::i18n::fmt("Duration: {duration} ms", &[("duration", &duration.to_string())]));
                }
            }
            if !record.details.is_empty() {
                ui.add(egui::Label::new(RichText::new(&record.details).monospace().small()).wrap().selectable(true));
            }
        });
    }
}

/// The entry's UTC date and time: with the date, a session past UTC midnight reads in order.
fn time(timestamp_ms: u64) -> String {
    let seconds = timestamp_ms / 1000;
    let (date, minute) = rustgis_engine::map::graphics::civil(seconds);
    format!("{date} {minute}:{:02} UTC", seconds % 60)
}

#[cfg(test)]
mod tests {
    #[test]
    fn entry_times_carry_their_utc_date() {
        assert_eq!(super::time(0), "1970-01-01 00:00:00 UTC");
        // A leap day, and the instant a day ends.
        assert_eq!(super::time(951_782_400_000), "2000-02-29 00:00:00 UTC");
        assert_eq!(super::time(951_868_799_999), "2000-02-29 23:59:59 UTC");
        assert_eq!(super::time(4_107_542_400_000), "2100-03-01 00:00:00 UTC");
    }
}
