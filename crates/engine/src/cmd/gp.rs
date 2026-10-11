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
        C::new("gp.clearHistory", "Clear History", "Analysis › Geoprocessing › History", |s, _| {
            s.project.gp_history.clear();
            Ok(Value::Null)
        }),
    ]
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
    let result = match res {
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
                        let r = rustgis_data::raster::read_bytes(&file_name, &bytes).map_err(|e| format!("{name}: {e}"))?;
                        let added = super::raster::add_raster(s, &name, rustgis_map::RasterLayer::from_bytes(r, file_name, bytes))?;
                        if let Some(id) = added.get("layer").and_then(Value::as_u64).and_then(|id| u32::try_from(id).ok()) {
                            layers.push(id);
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
