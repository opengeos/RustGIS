//! RustGIS on the command line. Every command the ribbon runs is available here by id.
//!
//! ```text
//! rustgis-cli commands                                  # list command ids and parameters
//! rustgis-cli tools                                     # list geoprocessing tools
//! rustgis-cli info data.geojson                         # layer summary as JSON
//! rustgis-cli render data.geojson out.png [--basemap ocean] [--width 1600 --height 900]
//! rustgis-cli run [--sample | --project p.rgisx | --data f.geojson …] \
//!     --cmd 'gp.run={"tool":"buffer","params":{"input":"Cities","distance":100}}' \
//!     --cmd 'share.exportMap={"path":"out.png"}' [--save out.rgisx]
//! rustgis-cli parity                                    # ribbon feature coverage report
//! ```
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write;
use std::process::ExitCode;

/// `println!` that ignores a closed pipe (`rustgis-cli parity | head`).
macro_rules! out {
    ($($t:tt)*) => {{
        let _ = writeln!(std::io::stdout(), $($t)*);
    }};
}

use rustgis_engine::{Project, Session};
use serde_json::{Value, json};

fn usage() -> ExitCode {
    eprintln!(
        "usage:\n  rustgis-cli commands | tools | parity\n  rustgis-cli info <data>\n  rustgis-cli logo <out.png> [--size N]\n  rustgis-cli render <data|project> <out.png> [--basemap NAME] [--width N] [--height N] [--dpi N]\n  rustgis-cli run [--sample] [--project P] [--data F]… --cmd 'id={{json}}'… [--save P]"
    );
    ExitCode::from(2)
}

fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}

fn print(v: &Value) {
    out!("{}", serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string()));
}

fn run(s: &mut Session, id: &str, p: Value) -> Result<Value, String> {
    s.run(id, &p).map_err(|e| format!("{id}: {e}"))
}

fn is_project(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".rgisx")
}

fn num(args: &[String], name: &str) -> Option<f64> {
    opt(args, name).and_then(|v| v.parse::<f64>().ok()).filter(|v| v.is_finite())
}

fn load(s: &mut Session, path: &str) -> Result<(), String> {
    if is_project(path) {
        run(s, "project.open", json!({"path": path}))?;
    } else {
        run(s, "layer.addData", json!({"path": path}))?;
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(|s| s.as_str()) else { return usage() };
    let mut s = Session::new(Project::default());
    let res: Result<(), String> = (|| match cmd {
        "commands" => {
            print(&run(&mut s, "info.commands", Value::Null)?);
            Ok(())
        }
        "tools" => {
            print(&run(&mut s, "gp.tools", Value::Null)?);
            Ok(())
        }
        "parity" => {
            let (done, total) = rustgis_engine::catalog::coverage();
            out!("Ribbon feature coverage: {done}/{total} ({}%)", done * 100 / total.max(1));
            let mut cur = ("", "");
            for f in rustgis_engine::catalog::FEATURES {
                if (f.tab, f.group) != cur {
                    out!("\n{} › {}", f.tab, f.group);
                    cur = (f.tab, f.group);
                }
                out!(
                    "  [{}] {}{}",
                    if f.command.is_some() { "x" } else { " " },
                    f.name,
                    f.command.map(|c| format!("  ({c})")).unwrap_or_default()
                );
            }
            Ok(())
        }
        "info" => {
            let path = args.get(1).ok_or("info needs a file")?;
            load(&mut s, path)?;
            print(&run(&mut s, "info.project", Value::Null)?);
            Ok(())
        }
        "logo" => {
            // The app icon as a PNG, for installers and desktop entries.
            let out = args.get(1).ok_or("logo needs <out.png>")?;
            let size = num(&args, "--size").unwrap_or(512.0).clamp(16.0, 1024.0) as u32;
            let png =
                rustgis_engine::render::logo::pixmap(size).ok_or("could not draw the logo")?.encode_png().map_err(|e| e.to_string())?;
            std::fs::write(out, png).map_err(|e| format!("{out}: {e}"))?;
            Ok(())
        }
        "render" => {
            let (Some(input), Some(out)) = (args.get(1), args.get(2)) else { return Err("render needs <input> <out.png>".into()) };
            load(&mut s, input)?;
            if let Some(b) = opt(&args, "--basemap") {
                run(&mut s, "map.basemap", json!({"name": b}))?;
            }
            let w = num(&args, "--width").unwrap_or(1600.0).clamp(16.0, 8000.0);
            let h = num(&args, "--height").unwrap_or(900.0).clamp(16.0, 8000.0);
            s.view_size = (w, h);
            if !is_project(input) {
                run(&mut s, "map.zoomFull", Value::Null)?;
            }
            let dpi = num(&args, "--dpi").unwrap_or(96.0);
            print(&run(&mut s, "share.exportMap", json!({"path": out, "dpi": dpi}))?);
            Ok(())
        }
        "run" => {
            if args.iter().any(|a| a == "--sample") {
                run(&mut s, "project.sample", Value::Null)?;
            }
            if let Some(p) = opt(&args, "--project") {
                load(&mut s, p)?;
            }
            for (i, a) in args.iter().enumerate() {
                if a == "--data"
                    && let Some(p) = args.get(i + 1)
                {
                    load(&mut s, p)?;
                }
            }
            for (i, a) in args.iter().enumerate() {
                if a == "--cmd"
                    && let Some(c) = args.get(i + 1)
                {
                    let (id, p) = match c.split_once('=') {
                        Some((id, json)) => (id, serde_json::from_str(json).map_err(|e| format!("{id}: bad JSON: {e}"))?),
                        None => (c.as_str(), Value::Null),
                    };
                    let out = run(&mut s, id, p)?;
                    if !out.is_null() {
                        print(&out);
                    }
                }
            }
            if let Some(p) = opt(&args, "--save") {
                run(&mut s, "project.saveAs", json!({"path": p}))?;
            }
            Ok(())
        }
        _ => Err(format!("unknown subcommand '{cmd}'")),
    })();
    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rustgis-cli: {e}");
            ExitCode::FAILURE
        }
    }
}
