//! Asynchronous Zarr slices for imported projects, independent of the Add Zarr dialog.
//! Uses the same metadata, planning and assembly functions as the dialog, keeping each import's
//! state separate so simultaneous stores and a user-operated picker cannot overwrite each other.
use crate::{RustGisApp, Slot};
use rustgis_engine::data::raster::{geotiff, zarr};
use rustgis_engine::render::rustgis_tiles::cache;
use serde_json::{Value, json};
use std::collections::HashMap;

type Answer = Slot<Result<Vec<u8>, String>>;

/// One active imported store shares this budget between retained bytes and its download batch.
const MAX_IMPORT_BYTES: usize = 128 << 20;

/// One bounded metadata/chunk batch, followed by a single layer download completion.
pub struct Read {
    id: u64,
    params: Value,
    root: String,
    queue: Vec<String>,
    waiting: Vec<(String, Answer)>,
    got: HashMap<String, Vec<u8>>,
    phase: Option<(zarr::Store, zarr::Plan)>,
    bytes: usize,
}

/// Starts an import read; the engine has already reserved its layer and drawing position.
pub fn start(app: &mut RustGisApp, p: &Value) -> Result<(), String> {
    if app.ui.geolibre_zarr.len() >= 16 {
        return Err(tl!("16 imported Zarr stores are already queued").into());
    }
    let root = p.get("url").and_then(Value::as_str).ok_or(tl!("Zarr URL is required"))?.trim_end_matches('/').to_string();
    let id = app.session.download_started();
    app.ui.geolibre_zarr.push(Read {
        id,
        params: p.clone(),
        root,
        queue: zarr::METADATA_KEYS.iter().map(|s| s.to_string()).collect(),
        waiting: Vec::new(),
        got: HashMap::new(),
        phase: None,
        bytes: 0,
    });
    Ok(())
}

impl Read {
    /// Polls one batch. Missing chunks retain the format's fill values; other errors stop it.
    fn poll(&mut self) -> Option<Result<Vec<u8>, String>> {
        let mut error = None;
        self.waiting.retain(|(key, slot)| match slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
            Some(Ok(bytes)) => {
                self.bytes = self.bytes.saturating_add(bytes.len());
                if self.bytes > MAX_IMPORT_BYTES {
                    error = Some(tl!("Imported Zarr slice exceeds the 128 MiB download limit").into());
                }
                self.got.insert(key.clone(), bytes);
                false
            }
            Some(Err(e)) => {
                if !rustgis_engine::cmd::zarr::is_absent(&e) || cache::http_status(&e) == Some(403) {
                    error = Some(e);
                }
                false
            }
            None => true,
        });
        if let Some(e) = error {
            return Some(Err(e));
        }
        // Wait for the entire batch before reserving another: partially finished batches
        // still own their byte allowance in callback slots, even before we consume them.
        if !self.waiting.is_empty() {
            return None;
        }
        let count = self.queue.len().min(6);
        let allowance = MAX_IMPORT_BYTES.saturating_sub(self.bytes) / count.max(1);
        if count > 0 && allowance == 0 {
            return Some(Err(tl!("Imported Zarr slice exceeds the 128 MiB download limit").into()));
        }
        while self.waiting.len() < count
            && let Some(key) = self.queue.pop()
        {
            let slot: Answer = Slot::default();
            let land = slot.clone();
            let file_limit = if self.phase.is_none() { zarr::MAX_METADATA } else { zarr::MAX_CHUNK_FILE as usize };
            let limit = file_limit.min(allowance);
            cache::download_limited(
                &format!("{}/{key}", self.root),
                limit,
                Box::new(move |r| *land.lock().unwrap_or_else(|e| e.into_inner()) = Some(r)),
            );
            self.waiting.push((key, slot));
        }
        if !self.queue.is_empty() || !self.waiting.is_empty() {
            return None;
        }
        if let Some((store, plan)) = self.phase.take() {
            let name = self.params.get("name").and_then(Value::as_str).unwrap_or("Zarr");
            return Some(
                zarr::assemble(&store, &plan, &self.got, name, None)
                    .map_err(|e| e.to_string())
                    .and_then(|r| geotiff::write(&r, 0).map_err(|e| e.to_string())),
            );
        }
        let docs = std::mem::take(&mut self.got).into_iter().collect::<Vec<_>>();
        let result = (|| {
            let store = zarr::parse_store(&docs).map_err(|e| e.to_string())?;
            let mut sel = rustgis_engine::cmd::zarr::selection(&self.params);
            rustgis_engine::cmd::zarr::pick_variable(&store, &mut sel)?;
            let plan = zarr::plan(&store, &sel).map_err(|e| e.to_string())?;
            self.queue = plan.keys.clone();
            self.phase = Some((store, plan));
            Ok(())
        })();
        result.err().map(Err)
    }
}

/// Finishes through the same target-layer replacement as other imported URL data.
pub fn poll(app: &mut RustGisApp, ctx: &egui::Context) {
    let mut done = Vec::new();
    // Serial reads bound aggregate buffers and assembly to one store. Queued jobs hold
    // parameters only; they cannot start downloads until earlier stores complete.
    if let Some(job) = app.ui.geolibre_zarr.first_mut()
        && let Some(result) = job.poll()
    {
        done.push((job.id, job.root.clone(), job.params.clone(), result));
        app.ui.geolibre_zarr.remove(0);
    }
    for (id, url, mut params, result) in done {
        match result {
            Ok(bytes) => {
                params["geolibreZarr"] = json!(true);
                app.add_landed(crate::serviceui::Landed { id, url, params, origin: None }, bytes);
            }
            Err(e) => {
                app.session.download_failed(id);
                rustgis_engine::cmd::geolibre::discard_pending(&mut app.session, &params);
                app.session.notify(format!("{url}: {e}"), true);
            }
        }
    }
    if !app.ui.geolibre_zarr.is_empty() {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geolibre_zarr_completion_replaces_reserved_layer_after_map_switch() {
        let array = |shape: Value| json!({"zarr_format":2,"shape":shape,"chunks":shape,"dtype":"<f8","compressor":null,"filters":null,"order":"C","fill_value":null});
        let metadata = json!({"zarr_consolidated_format":1,"metadata":{
            ".zgroup":{"zarr_format":2},
            "value/.zarray":array(json!([2,2])),"value/.zattrs":{"_ARRAY_DIMENSIONS":["lat","lon"]},
            "lat/.zarray":array(json!([2])),"lat/.zattrs":{"_ARRAY_DIMENSIONS":["lat"],"units":"degrees_north"},
            "lon/.zarray":array(json!([2])),"lon/.zattrs":{"_ARRAY_DIMENSIONS":["lon"],"units":"degrees_east"}
        }});
        let store = zarr::parse_store(&[(".zmetadata".into(), metadata.to_string().into_bytes())]).unwrap();
        let plan = zarr::plan(&store, &zarr::Selection { variable: "value".into(), index: Vec::new() }).unwrap();
        let mut got = HashMap::new();
        for (key, values) in [("value/0.0", vec![1.0f64, 2.0, 3.0, 4.0]), ("lat/0", vec![0.5, 1.5]), ("lon/0", vec![0.5, 1.5])] {
            got.insert(key.into(), values.iter().flat_map(|v| v.to_le_bytes()).collect());
        }
        let mut app = RustGisApp::with_sample();
        let target = app.session.project.active_map;
        let mut ds = rustgis_engine::data::Dataset::new("Grid", rustgis_engine::data::GeometryType::Point, Vec::new());
        ds.source = Some("geolibre-pending:zarr-test".into());
        let layer = app.session.map_mut().add_dataset(ds);
        app.run("map.new", json!({"name":"Other"})).unwrap();
        let active = app.session.project.active_map;
        let id = app.session.download_started();
        let params = json!({"name":"Grid","geolibreLayer":{"token":"geolibre-pending:zarr-test","record":{"name":"Grid","opacity":0.4}}});
        app.ui.geolibre_zarr.push(Read {
            id,
            params,
            root: "https://example.org/grid.zarr".into(),
            queue: Vec::new(),
            waiting: Vec::new(),
            got,
            phase: Some((store, plan)),
            bytes: 0,
        });
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.logic(ui.ctx()));
        // Logic-only frames do not paint; explicitly discard their font texture updates.
        output.textures_delta.clear();
        assert!(app.ui.geolibre_zarr.is_empty());
        assert_eq!(app.session.project.active_map, active);
        let loaded = app.session.project.maps[target].layer(layer).unwrap();
        assert!(loaded.is_raster());
        assert_eq!(loaded.transparency, 60.0);
        assert!(app.session.map().layers.is_empty());
    }

    #[test]
    fn geolibre_zarr_start_failure_removes_reservation_through_app_frame() {
        let mut app = RustGisApp::with_sample();
        // Keep sixteen reads pending without issuing network requests.
        for i in 0..16 {
            app.ui.geolibre_zarr.push(Read {
                id: i,
                params: json!({}),
                root: "https://example.org/grid.zarr".into(),
                queue: Vec::new(),
                waiting: vec![(".zmetadata".into(), Slot::default())],
                got: HashMap::new(),
                phase: None,
                bytes: 0,
            });
        }
        let target = app.session.project.active_map;
        let token = "geolibre-pending:overflow-test";
        let mut ds = rustgis_engine::data::Dataset::new("Overflow", rustgis_engine::data::GeometryType::Point, Vec::new());
        ds.source = Some(token.into());
        let id = app.session.map_mut().add_dataset(ds);
        let mut group = rustgis_engine::map::LayerGroup::new("Folder");
        group.children.push(id);
        app.session.map_mut().add_group(group);
        app.session
            .request_url(&json!({"url":"https://example.org/overflow.zarr","geolibreZarr":true,"geolibreLayer":{"token":token}}))
            .unwrap();
        app.run("map.new", json!({"name":"Other"})).unwrap();
        let active = app.session.project.active_map;
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.logic(ui.ctx()));
        // Logic-only frames do not paint; explicitly discard their font texture updates.
        output.textures_delta.clear();
        assert_eq!(app.session.project.active_map, active);
        assert!(app.session.project.maps[target].layer(id).is_none());
        assert!(app.session.project.maps[target].groups.iter().all(|g| !g.children.contains(&id)));
        assert_eq!(app.ui.geolibre_zarr.len(), 16);
        assert!(app.session.take_url_requests().is_empty());
    }

    /// A full active reader must not let queued stores issue downloads or retain chunk buffers.
    #[test]
    fn geolibre_zarr_waiting_store_does_not_multiply_the_active_byte_budget() {
        let mut app = RustGisApp::with_sample();
        app.ui.geolibre_zarr.push(Read {
            id: 0,
            params: json!({}),
            root: "https://example.org/active.zarr".into(),
            queue: vec!["next".into()],
            waiting: vec![("pending".into(), Slot::default())],
            got: HashMap::new(),
            phase: None,
            bytes: MAX_IMPORT_BYTES,
        });
        start(&mut app, &json!({"url":"https://example.org/queued.zarr"})).unwrap();
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.logic(ui.ctx()));
        output.textures_delta.clear();
        assert_eq!(app.ui.geolibre_zarr.len(), 2);
        let queued = &app.ui.geolibre_zarr[1];
        assert!(queued.waiting.is_empty());
        assert!(queued.got.is_empty());
        assert_eq!(queued.bytes, 0);
        assert_eq!(queued.queue.len(), zarr::METADATA_KEYS.len());
        // Once the active batch lands, an exhausted budget fails before another request.
        let active = &mut app.ui.geolibre_zarr[0];
        active.waiting.clear();
        assert!(active.poll().unwrap().is_err());
        assert!(active.waiting.is_empty());
    }
}
