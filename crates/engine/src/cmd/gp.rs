//! Analysis tab: run geoprocessing tools and keep their history.

use rustgis_analysis::{Inputs, Output, OutputLayer, RasterInput};
use rustgis_data::Dataset;
use serde_json::{Value, json};

use super::{need_layers, ps};
use crate::{CmdResult, CommandSpec as C, GpRecord, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("gp.run", "Run Tool", "Analysis › Geoprocessing", run).when(need_layers).params(r#"{"tool": tool id, "params": {...}}"#),
        C::new("gp.tools", "Tools", "Analysis › Geoprocessing", |_, _| {
            Ok(Value::Array(
                rustgis_analysis::tools()
                    .map(|t| {
                        json!({"id": t.id, "name": t.name, "toolbox": t.toolbox, "summary": t.summary,
                            "params": t.params.iter().map(|p| json!({"name": p.name, "label": p.label, "required": p.required, "kind": format!("{:?}", p.kind)})).collect::<Vec<_>>()})
                    })
                    .collect(),
            ))
        })
        .view(),
        C::new("gp.geolibre", "GeoLibre Tools", "Analysis › Geoprocessing", |_, _| Ok(geolibre_status())).view(),
        C::new("gp.installGeolibre", "Install GeoLibre Tools", "Analysis › Geoprocessing", |_, _| {
            let path = install_geolibre(&|_, _| {})?;
            #[cfg(all(feature = "geolibre", not(target_arch = "wasm32")))]
            rustgis_geolibre::set_runner(Some(std::path::PathBuf::from(&path)))?;
            Ok(json!({"path": path, "status": geolibre_status()}))
        })
        .view(),
        C::new("gp.finish", "Finish Tool Run", "Analysis › Geoprocessing", finish)
            .params(r#"{"id": a finished run (the browser's GeoLibre tools run asynchronously)}"#),
        C::new("gp.setGeolibre", "Locate GeoLibre Runner", "Analysis › Geoprocessing", set_geolibre)
            .view()
            .params(r#"{"path"?: "the geolibre executable (empty: look in the usual places)"}"#),
        C::new("gp.clearHistory", "Clear History", "Analysis › Geoprocessing › History", |s, _| {
            s.project.gp_history.clear();
            Ok(Value::Null)
        }),
    ]
}

/// Whether the GeoLibre tools are available. Natively: the runner in use, its version and tool
/// count. In the browser (`web`): whether the WASI runner's tools have been loaded.
pub fn geolibre_status() -> Value {
    #[cfg(all(feature = "geolibre", not(target_arch = "wasm32")))]
    {
        let found = rustgis_geolibre::runner::find();
        json!({
            "supported": true,
            "web": false,
            "available": found.is_some(),
            "path": found.as_ref().map(|(p, _)| p.display().to_string()),
            "version": found.map(|(_, v)| v),
            "tools": rustgis_analysis::external_tools().len(),
            "download": rustgis_geolibre::runner::DOWNLOAD_URL,
        })
    }
    #[cfg(all(feature = "geolibre", target_arch = "wasm32"))]
    {
        let n = rustgis_analysis::external_tools().len();
        json!({"supported": true, "web": true, "available": n > 0, "tools": n})
    }
    #[cfg(not(feature = "geolibre"))]
    json!({"supported": false, "available": false})
}

/// Downloads and installs the latest GeoLibre runner for this platform; returns the program's
/// path. `progress` gets the bytes received and the expected total. Blocks: the app calls it
/// from a background thread, then picks the runner with `gp.setGeolibre`. Native only.
pub fn install_geolibre(progress: &dyn Fn(u64, Option<u64>)) -> Result<String, String> {
    #[cfg(all(feature = "geolibre", not(target_arch = "wasm32")))]
    {
        rustgis_geolibre::install::install(progress).map(|p| p.display().to_string())
    }
    #[cfg(not(all(feature = "geolibre", not(target_arch = "wasm32"))))]
    {
        let _ = progress;
        Err("this build can't install the GeoLibre runner".into())
    }
}

/// Chooses the GeoLibre runner (an optional download), or looks for it again. Native only.
fn set_geolibre(_s: &mut Session, p: &Value) -> CmdResult {
    #[cfg(all(feature = "geolibre", not(target_arch = "wasm32")))]
    {
        let path = ps(p, "path").map(str::trim).filter(|p| !p.is_empty()).map(std::path::PathBuf::from);
        rustgis_geolibre::set_runner(path)?;
        Ok(geolibre_status())
    }
    #[cfg(not(all(feature = "geolibre", not(target_arch = "wasm32"))))]
    {
        let _ = p;
        Err("this build has no GeoLibre runner to choose".into())
    }
}

/// Loads the GeoLibre tool list from manifests (the JSON array the runner lists), as the
/// browser does from its WASI runner; returns how many tools there are.
pub fn external_load_manifests(json: &str) -> Result<usize, String> {
    #[cfg(feature = "geolibre")]
    {
        rustgis_geolibre::set_manifests(json)
    }
    #[cfg(not(feature = "geolibre"))]
    {
        let _ = json;
        Err("this build doesn't include the GeoLibre tools".into())
    }
}

/// A GeoLibre run to carry out asynchronously (the browser's WASI runner): run `tool` with
/// `argv` in a working folder `/work` holding `files`, then pass the new files and the printed
/// lines to [`external_complete`] with `id`.
#[derive(Clone, Debug)]
pub struct ExternalRun {
    pub id: u64,
    pub tool: String,
    pub argv: Vec<String>,
    pub files: Vec<(String, Vec<u8>)>,
}

/// The new files and printed lines of a finished run, or the runner's error.
pub type ExternalOutput = Result<(Vec<(String, Vec<u8>)>, Vec<String>), String>;

/// Prepares a run of a GeoLibre tool on the active map, checking its parameters as `gp.run`
/// does; the session keeps it until [`external_complete`].
pub fn external_start(s: &mut Session, tool: &str, params: &Value) -> Result<ExternalRun, String> {
    #[cfg(feature = "geolibre")]
    {
        let spec = rustgis_analysis::tool(tool)
            .filter(|t| rustgis_analysis::is_external(t))
            .ok_or_else(|| format!("no GeoLibre tool '{tool}'"))?;
        let params = rustgis_analysis::resolve_params(spec, params)?;
        let job = rustgis_geolibre::prepare(spec, &MapInputs(s.map(), &s.project.tables), &params)?;
        s.next_pending = s.next_pending.wrapping_add(1);
        let id = s.next_pending;
        let run = ExternalRun { id, tool: job.tool.clone(), argv: job.argv("/work"), files: job.files.clone() };
        s.started.push((id, job, params, now_ms()));
        Ok(run)
    }
    #[cfg(not(feature = "geolibre"))]
    {
        let _ = (s, tool, params);
        Err("this build doesn't include the GeoLibre tools".into())
    }
}

/// Finishes a run from [`external_start`]: turns the runner's files and lines into a result
/// that `gp.finish {"id"}` then applies like `gp.run` would, with undo and history.
pub fn external_complete(s: &mut Session, id: u64, ran: ExternalOutput) -> Result<(), String> {
    #[cfg(feature = "geolibre")]
    {
        let i = s.started.iter().position(|(n, ..)| *n == id).ok_or("no run with that id")?;
        let (_, job, params, t0) = s.started.remove(i);
        let result =
            ran.map(|(files, lines)| rustgis_geolibre::finish(&job, &files, &lines, None)).map_err(|e| format!("{}: {e}", job.name));
        s.pending.push(crate::PendingRun { id, tool: job.name.clone(), params, result, millis: now_ms().saturating_sub(t0) });
        Ok(())
    }
    #[cfg(not(feature = "geolibre"))]
    {
        let _ = (s, id, ran);
        Err("this build doesn't include the GeoLibre tools".into())
    }
}

/// Applies a finished run stashed by [`external_complete`].
fn finish(s: &mut Session, p: &Value) -> CmdResult {
    let id = p.get("id").and_then(Value::as_u64).ok_or("id is required")?;
    let i = s.pending.iter().position(|r| r.id == id).ok_or("no finished run with that id")?;
    let r = s.pending.remove(i);
    apply(s, r.tool, r.params, r.result, r.millis)
}

struct MapInputs<'a>(&'a rustgis_map::Map, &'a [Dataset]);

impl Inputs for MapInputs<'_> {
    fn dataset(&self, layer: &str) -> Option<&Dataset> {
        self.0.layer_by_name(layer).map(|l| &l.dataset).or_else(|| self.1.iter().find(|t| t.name.eq_ignore_ascii_case(layer)))
    }
    fn selection(&self, layer: &str) -> &[usize] {
        self.0.layer_by_name(layer).map(|l| l.selection.as_slice()).unwrap_or(&[])
    }
    fn raster(&self, layer: &str) -> Option<RasterInput<'_>> {
        let rl = self.0.layer_by_name(layer)?.raster.as_ref()?;
        Some(RasterInput {
            raster: rl.data.as_deref()?,
            path: rl.path.as_deref(),
            embedded: rl.embedded.as_deref().map(|b| (rl.file_name.as_deref().unwrap_or("raster.tif"), b.as_slice())),
        })
    }
}

fn now_ms() -> u64 {
    web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn run(s: &mut Session, p: &Value) -> CmdResult {
    let tool = ps(p, "tool").ok_or("tool is required")?.to_string();
    let params = p.get("params").cloned().unwrap_or(json!({}));
    let t0 = now_ms();
    let res = rustgis_analysis::run(&tool, &MapInputs(s.map(), &s.project.tables), &params);
    let millis = now_ms().saturating_sub(t0);
    let name = rustgis_analysis::tool(&tool).map(|t| t.name).unwrap_or("Tool").to_string();
    apply(s, name, params, res, millis)
}

/// Adds a tool's result to the map and the history.
fn apply(s: &mut Session, name: String, params: Value, res: Result<rustgis_analysis::ToolResult, String>, millis: u64) -> CmdResult {
    let mut result = match res {
        Err(e) => {
            s.project.gp_history.push(GpRecord { tool: name, params, ok: false, messages: vec![e.clone()], millis });
            return Err(e);
        }
        Ok(r) => r,
    };
    let out = match result.output {
        Output::Layer(ds) => {
            let id = super::layer::add_dataset(s, ds);
            json!({"layer": id})
        }
        Output::Selection { layer, rows } => {
            let id = s.map().layer_by_name(&layer).map(|l| l.id).ok_or("the input layer disappeared")?;
            let n = rows.len();
            super::select::set(s, id, rows, super::select::Mode::New);
            json!({"selected": n})
        }
        Output::Update { layer, dataset } => {
            let l = s.map_mut().layers.iter_mut().find(|l| l.name.eq_ignore_ascii_case(&layer)).ok_or("the input layer disappeared")?;
            l.dataset = dataset;
            l.touch_geometry();
            json!({"updated": layer})
        }
        Output::Table(t) => {
            let n = t.name.clone();
            s.project.tables.retain(|x| x.name != n);
            s.project.tables.push(t);
            json!({"table": n})
        }
        Output::Layers(outputs) => {
            let mut layers = Vec::new();
            let mut tables = Vec::new();
            for o in outputs {
                match o {
                    OutputLayer::Vector(ds) => layers.push(super::layer::add_dataset(s, ds)),
                    OutputLayer::Raster { name, file_name, bytes } => {
                        // One unreadable output doesn't undo the others: it's reported instead.
                        let added = rustgis_data::raster::read_bytes(&file_name, &bytes)
                            .map_err(|e| format!("{name}: {e}"))
                            .and_then(|r| super::raster::add_raster(s, &name, rustgis_map::RasterLayer::from_bytes(r, file_name, bytes)));
                        match added {
                            Ok(v) => {
                                if let Some(id) = v.get("layer").and_then(Value::as_u64).and_then(|id| u32::try_from(id).ok()) {
                                    layers.push(id);
                                }
                            }
                            Err(e) => result.messages.push(e),
                        }
                    }
                    OutputLayer::Table(t) => {
                        let n = t.name.clone();
                        s.project.tables.retain(|x| x.name != n);
                        s.project.tables.push(t);
                        tables.push(n);
                    }
                }
            }
            json!({"layers": layers, "tables": tables})
        }
    };
    for m in &result.messages {
        s.notify(m.clone(), false);
    }
    s.project.gp_history.push(GpRecord { tool: name, params, ok: true, messages: result.messages, millis });
    if s.project.gp_history.len() > 500 {
        s.project.gp_history.remove(0);
    }
    Ok(out)
}
