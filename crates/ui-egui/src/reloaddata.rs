//! Manual local-layer reload: capture the target before asking to discard edits.
use crate::{RustGisApp, dialogs::Dialogs, theme::Tokens};
use rustgis_engine::{Session, map::Layer};
use serde_json::{Value, json};

pub(crate) const HELP: &str = "Read the selected layer's local source again. Folders reload only when you choose Reload Data; they are not watched. Undo restores in-memory data and configuration, not overwritten files or historical streamed contents.";

fn source(layer: &Layer) -> Option<(&str, Option<&str>, bool)> {
    if let Some(edit) = &layer.gpkg {
        rustgis_engine::map::paths::is_local(&edit.path).then_some((edit.path.as_str(), Some(edit.table.as_str()), true))
    } else {
        layer.linked_file().map(|path| (path, layer.linked_table(), false))
    }
}

pub(crate) fn supported(layer: &Layer) -> bool {
    source(layer).is_some()
}

pub struct Form {
    opened: u64,
    map: u64,
    layer: u32,
    instance: u64,
    path: String,
    table: Option<String>,
    gpkg: bool,
    error: Option<String>,
}

impl Form {
    fn new(s: &Session, id: u32) -> Result<Self, String> {
        let layer = s.map().layer(id).ok_or_else(|| tl!("Layer not found").to_string())?;
        let (path, table, gpkg) = source(layer)
            .ok_or_else(|| tl!("Select a layer linked to a local file or folder, or an editable GeoPackage layer.").to_string())?;
        Ok(Self {
            opened: s.opened,
            map: s.map().instance_id(),
            layer: id,
            instance: layer.instance_id(),
            path: path.into(),
            table: table.map(String::from),
            gpkg,
            error: None,
        })
    }

    fn target<'a>(&self, s: &'a Session) -> Result<&'a Layer, String> {
        if self.opened == s.opened
            && self.map == s.map().instance_id()
            && let Some(layer) = s.map().layer(self.layer)
            && layer.instance_id() == self.instance
            && source(layer) == Some((self.path.as_str(), self.table.as_deref(), self.gpkg))
        {
            return Ok(layer);
        }
        Err(tl!("The project, map, layer or data source changed. Reopen Reload Data for the intended layer.").into())
    }

    fn pending<'a>(&self, s: &'a Session) -> Result<(&'a str, [usize; 3]), String> {
        let layer = self.target(s)?;
        let counts = layer.gpkg.as_ref().map_or([0; 3], |edit| {
            let changes = edit.versions.changes(&layer.dataset.features);
            [changes.inserts.len(), changes.updates.len(), changes.deletes.len()]
        });
        Ok((layer.name.as_str(), counts))
    }

    /// Validate identity and recount immediately before invoking the engine mutation.
    fn dispatch(&self, app: &mut RustGisApp, discard: bool) -> Result<Value, String> {
        let (_, counts) = self.pending(&app.session)?;
        if !discard && counts != [0; 3] {
            return Err(tl!("This layer has pending edits. Confirm Discard Edits and Reload first.").into());
        }
        let mut params = json!({"layer": self.layer});
        if discard {
            params["discardEdits"] = json!(true);
        }
        app.run("layer.reloadData", params)
    }
}

pub(crate) fn request(app: &mut RustGisApp, params: &Value) -> Result<Value, String> {
    let result = (|| {
        let form = Form::new(&app.session, app.session.layer_id(params)?)?;
        let (_, counts) = form.pending(&app.session)?;
        if counts == [0; 3] {
            return form.dispatch(app, false);
        }
        app.dialogs = Dialogs { open: Some("reloadData"), reload: Some(form), ..Default::default() };
        Ok(Value::Null)
    })();
    if let Err(error) = &result {
        app.session.notify(error.clone(), true);
    }
    result
}

pub(crate) fn show(app: &mut RustGisApp, ctx: &egui::Context) {
    let Some(mut form) = app.dialogs.reload.take() else {
        app.dialogs.open = None;
        return;
    };
    let pending = form.pending(&app.session);
    let t = Tokens::get(ctx);
    let mut cancel = false;
    let mut discard = false;
    let modal = egui::Modal::new(egui::Id::unique("reload_data")).show(ctx, |ui| {
        ui.set_width(460.0);
        ui.heading(tl!("Reload Data"));
        match &pending {
            Ok((name, counts)) => {
                ui.label(crate::i18n::fmt("Reload {name} and discard its pending edits?", &[("name", name)]));
                ui.label(crate::i18n::fmt(
                    "Inserts: {inserts}; updates: {updates}; deletes: {deletes}.",
                    &[("inserts", &counts[0].to_string()), ("updates", &counts[1].to_string()), ("deletes", &counts[2].to_string())],
                ));
                ui.add(egui::Label::new(egui::RichText::new(&form.path).monospace()).wrap().selectable(true));
            }
            Err(error) => {
                ui.colored_label(t.red, error);
            }
        }
        ui.label(tl!("Folders reload only when you choose Reload Data; they are not watched."));
        ui.label(tl!("Undo restores in-memory data and configuration, not overwritten files or historical streamed contents."));
        if let Some(error) = &form.error {
            ui.colored_label(t.red, error);
        }
        ui.separator();
        ui.horizontal(|ui| {
            cancel = crate::widgets::dialog_button(ui, tl!("Cancel"), None, egui::vec2(84.0, 28.0)).clicked();
            discard = ui
                .add_enabled_ui(pending.is_ok(), |ui| {
                    crate::widgets::dialog_button(ui, tl!("Discard Edits and Reload"), Some(t.red), egui::vec2(230.0, 28.0)).clicked()
                })
                .inner;
        });
        cancel |= ui.input(|input| input.key_pressed(egui::Key::Escape));
    });
    if cancel || modal.backdrop_response.clicked() {
        app.dialogs.open = None;
    } else if discard {
        match form.dispatch(app, true) {
            Ok(_) => app.dialogs.open = None,
            Err(error) => {
                form.error = Some(error);
                app.dialogs.reload = Some(form);
            }
        }
    } else {
        app.dialogs.reload = Some(form);
    }
}
