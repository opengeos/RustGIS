//! The `geolibre` runner: a separate executable (an optional download) that runs the tools.
//!
//! RustGIS doesn't link the suite (it would triple the app's size). It runs the runner as a
//! subprocess instead: `geolibre manifests` for the tool forms, `geolibre <tool> --k=v …` for a
//! run. A tool that crashes or panics takes only the subprocess down.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;

/// Where releases with the runner are published.
pub const DOWNLOAD_URL: &str = "https://github.com/opengeos/geolibre-rust/releases/latest";

/// The environment variable that names the runner, ahead of every other place.
pub const ENV: &str = "RUSTGIS_GEOLIBRE";

fn exe_name() -> &'static str {
    if cfg!(windows) { "geolibre.exe" } else { "geolibre" }
}

/// A runner the user chose (Locate…), ahead of the usual places.
static CHOSEN: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The runner found, with its version, cached until the choice changes.
static FOUND: Mutex<Option<Option<(PathBuf, String)>>> = Mutex::new(None);

/// A command for the runner, without a console window on Windows.
fn command(exe: &Path) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// Runs `c` to completion, or kills it after `limit`. For the quick queries (version,
/// manifests): a program that only happens to be called `geolibre` mustn't hang the app. Tool
/// runs have no limit, since a large raster can legitimately take a long time.
fn output_within(mut c: Command, limit: Duration) -> Option<Output> {
    let mut child = c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    // Drain both pipes on threads while the process runs: a full pipe (large manifests, or
    // many warnings) would otherwise block it until the time limit.
    fn drain(mut pipe: impl std::io::Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            buf
        })
    }
    let (Some(out), Some(err)) = (child.stdout.take(), child.stderr.take()) else {
        // Not expected with piped output, but never leave the process running.
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };
    let (out_reader, err_reader) = (drain(out), drain(err));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let stdout = out_reader.join().ok()?;
    let stderr = err_reader.join().unwrap_or_default();
    Some(Output { status, stdout, stderr })
}

/// The runner's version, if `exe` is a GeoLibre runner (not some other program called geolibre).
pub fn version_of(exe: &Path) -> Option<String> {
    if !exe.is_file() {
        return None;
    }
    let mut c = command(exe);
    c.arg("version");
    let out = output_within(c, Duration::from_secs(10))?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && text.starts_with("geolibre-cli")).then_some(text)
}

/// The places a runner is looked for, in order.
fn candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(p) = CHOSEN.lock()
        && let Some(p) = p.clone()
    {
        v.push(p);
    }
    if let Some(p) = std::env::var_os(ENV) {
        v.push(PathBuf::from(p));
    }
    // Where one-click install puts it.
    if let Some(dir) = crate::install::install_dir() {
        v.push(dir.join(exe_name()));
    }
    // Beside the app (a bundle that ships it), or in a `geolibre` folder there.
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        v.push(dir.join(exe_name()));
        v.push(dir.join("geolibre").join(exe_name()));
    }
    if let Some(path) = std::env::var_os("PATH") {
        v.extend(std::env::split_paths(&path).map(|d| d.join(exe_name())));
    }
    v
}

/// The runner to use and its version, or `None` when there isn't one.
///
/// The answer is kept until the choice changes (Locate…). Probing runs without holding the
/// cache's lock, so a slow candidate never blocks another caller.
pub fn find() -> Option<(PathBuf, String)> {
    if let Ok(found) = FOUND.lock()
        && let Some(f) = found.as_ref()
    {
        return f.clone();
    }
    let probed = candidates().into_iter().find_map(|p| version_of(&p).map(|v| (p, v)));
    if let Ok(mut found) = FOUND.lock() {
        // Another caller may have stored an answer meanwhile: the first one stays.
        return found.get_or_insert(probed).clone();
    }
    probed
}

/// Uses `exe` as the runner (`None`: look in the usual places again). Fails, keeping the
/// current choice, when `exe` isn't a GeoLibre runner.
pub fn choose(exe: Option<PathBuf>) -> Result<Option<(PathBuf, String)>, String> {
    if let Some(p) = &exe
        && version_of(p).is_none()
    {
        return Err(format!("{} is not a GeoLibre runner", p.display()));
    }
    if let Ok(mut c) = CHOSEN.lock() {
        *c = exe;
    }
    if let Ok(mut f) = FOUND.lock() {
        *f = None;
    }
    Ok(find())
}

/// Every tool's manifest, from `geolibre manifests`.
pub fn manifests() -> Result<Vec<Value>, String> {
    let (exe, _) = find().ok_or("the GeoLibre runner isn't installed")?;
    let mut c = command(&exe);
    c.arg("manifests");
    let out = output_within(c, Duration::from_secs(60)).ok_or_else(|| format!("{}: no answer to `manifests`", exe.display()))?;
    if !out.status.success() {
        return Err(format!("{}: {}", exe.display(), String::from_utf8_lossy(&out.stderr).trim()));
    }
    match serde_json::from_slice::<Value>(&out.stdout) {
        Ok(Value::Array(v)) => Ok(v),
        _ => Err(format!("{}: unexpected manifests output", exe.display())),
    }
}

/// A parameter value as the runner's `--key=value` text.
pub fn arg_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// What a run printed: its messages, and the structured outputs (the last line, JSON).
pub struct Ran {
    pub messages: Vec<String>,
    pub outputs: serde_json::Map<String, Value>,
}

/// Runs one tool in `dir`. The error carries what the tool wrote to stderr.
pub fn run(id: &str, args: &[(String, Value)], dir: &Path) -> Result<Ran, String> {
    let (exe, _) = find().ok_or("the GeoLibre runner isn't installed")?;
    let mut c = command(&exe);
    c.current_dir(dir).arg(id);
    for (k, v) in args {
        c.arg(format!("--{k}={}", arg_text(v)));
    }
    let out = c.output().map_err(|e| format!("{}: {e}", exe.display()))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        let err = if err.is_empty() { "the tool stopped unexpectedly" } else { err };
        return Err(err.lines().take(20).collect::<Vec<_>>().join("\n"));
    }
    let mut lines: Vec<&str> = stdout.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let outputs = match lines.last().map(|l| serde_json::from_str::<Value>(l)) {
        Some(Ok(Value::Object(m))) => {
            lines.pop();
            m
        }
        _ => serde_json::Map::new(),
    };
    Ok(Ran { messages: lines.into_iter().take(40).map(str::to_string).collect(), outputs })
}
