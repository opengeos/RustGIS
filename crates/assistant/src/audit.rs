//! The agents' audit log: every tool call an agent made on the open project through the desktop
//! app ([`crate::relay`]), one JSON line each, in a file a day in the app-data folder. It lets the
//! user see what an agent did, and replay or review it.
//!
//! Each line is `{ts, source, tool, command?, script?, args, success, duration_s, error}`.
//! Arguments lose passwords, keys and tokens before they are written. Files older than
//! [`KEEP_DAYS`] are removed when a new day's file starts.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

/// How many days of logs are kept.
pub const KEEP_DAYS: u64 = 30;

/// The most bytes of a call's arguments a line keeps.
const MAX_ARGS: usize = 8_000;

/// The folder the desktop app keeps the log in: `mcp-audit` in the app-data folder.
pub fn dir() -> Option<PathBuf> {
    rustgis_engine::data::dirs::app_data_dir().map(|d| d.join("mcp-audit"))
}

/// The day's log file in `dir` for `secs` since the Unix epoch (`2026-10-18.jsonl`).
pub fn file_for(dir: &Path, secs: i64) -> PathBuf {
    let day: String = crate::chatgpt::rfc3339(secs).chars().take(10).collect();
    dir.join(format!("{day}.jsonl"))
}

/// Appends a line for each tool call in `request` (a JSON-RPC message or batch) with how its
/// `reply` went. `source` says where it came from (`mcp`, `file`). Logging never fails the call:
/// a log that can't be written is skipped.
pub fn record(dir: &Path, source: &str, request: &str, reply: Option<&str>, took: Duration) {
    let lines = entries(source, request, reply, took, now());
    if lines.is_empty() {
        return;
    }
    let path = file_for(dir, now());
    let new_day = !path.exists();
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else { return };
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
    let _ = file.write_all(text.as_bytes());
    if new_day {
        prune(dir, now());
    }
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX)).unwrap_or(0)
}

/// The log lines for the tool calls in `request`, at `secs`.
pub fn entries(source: &str, request: &str, reply: Option<&str>, took: Duration, secs: i64) -> Vec<Value> {
    let Ok(msg) = serde_json::from_str::<Value>(request) else { return Vec::new() };
    let messages = msg.as_array().map(Vec::as_slice).unwrap_or_else(|| std::slice::from_ref(&msg));
    let replies: Value = reply.and_then(|r| serde_json::from_str(r).ok()).unwrap_or(Value::Null);
    let replies = replies.as_array().map(Vec::as_slice).unwrap_or_else(|| std::slice::from_ref(&replies));
    let duration_s = (took.as_secs_f64() * 1000.0).round() / 1000.0;
    messages
        .iter()
        .filter(|m| m.get("method").and_then(Value::as_str) == Some("tools/call") && m.get("id").is_some())
        .map(|m| {
            let params = m.get("params").cloned().unwrap_or(Value::Null);
            let args = redact(params.get("arguments").cloned().unwrap_or(Value::Null), 0);
            let r = replies.iter().find(|r| r.get("id") == m.get("id"));
            let failed = r.is_none_or(|r| r.get("error").is_some() || r.pointer("/result/isError") == Some(&Value::Bool(true)));
            let error = failed.then(|| {
                let text = r
                    .and_then(|r| r.pointer("/error/message").or_else(|| r.pointer("/result/content/0/text")))
                    .and_then(Value::as_str)
                    .unwrap_or("no reply");
                crate::tools::clip(&rustgis_engine::sources::redact(text), 2000)
            });
            let mut line = json!({
                "ts": crate::chatgpt::rfc3339(secs),
                "source": source,
                "tool": params.get("name"),
                "success": !failed,
                "duration_s": duration_s,
                "error": error,
            });
            if let Some(o) = line.as_object_mut() {
                for key in ["command", "script"] {
                    if let Some(v) = args.get(key).filter(|v| v.is_string()) {
                        o.insert(key.into(), v.clone());
                    }
                }
                let text = args.to_string();
                o.insert(
                    "args".into(),
                    if text.len() <= MAX_ARGS {
                        args
                    } else {
                        json!({"cut": true, "bytes": text.len(), "text": crate::tools::clip(&text, MAX_ARGS)})
                    },
                );
            }
            line
        })
        .collect()
}

/// `v` without what would give someone else the user's access: values under keys that name a
/// secret, and the passwords, keys and tokens in addresses ([`rustgis_engine::sources::redact`]).
fn redact(v: Value, depth: usize) -> Value {
    let secret = |k: &str| {
        let k = k.to_ascii_lowercase();
        ["password", "passwd", "secret", "token", "apikey", "api_key", "key", "authorization"].iter().any(|s| k == *s || k.ends_with(s))
    };
    match v {
        Value::String(s) => Value::String(rustgis_engine::sources::redact(&s)),
        Value::Array(a) if depth < 16 => Value::Array(a.into_iter().map(|x| redact(x, depth + 1)).collect()),
        Value::Object(o) if depth < 16 => Value::Object(
            o.into_iter().map(|(k, x)| if secret(&k) { (k, json!("[redacted]")) } else { (k, redact(x, depth + 1)) }).collect(),
        ),
        Value::Array(_) | Value::Object(_) => json!("[too deep]"),
        other => other,
    }
}

/// Removes day files in `dir` older than [`KEEP_DAYS`] before `secs`.
fn prune(dir: &Path, secs: i64) {
    let oldest = file_for(dir, secs - i64::try_from(KEEP_DAYS * 86_400).unwrap_or(0));
    let Some(oldest) = oldest.file_name().and_then(|n| n.to_str()).map(str::to_string) else { return };
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for e in read.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        // Only our own day files: `YYYY-MM-DD.jsonl`, compared as text.
        let ours = name.len() == oldest.len() && name.ends_with(".jsonl") && name.chars().take(10).all(|c| c.is_ascii_digit() || c == '-');
        if ours && name < oldest {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line for each tool call: what ran, whether it worked, how long it took; secrets in its
    /// arguments are not written.
    #[test]
    fn entries_describe_each_call_without_secrets() {
        let request = json!([
            {"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "run_command", "arguments": {"command": "layer.addData", "params": {"path": "postgresql://ann:hunter2@db.example.com/gis", "token": "abc"}}}},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "run", "arguments": {"script": "map.zoomIn"}}},
            {"jsonrpc": "2.0", "id": 3, "method": "ping"}
        ]);
        let reply = json!([
            {"jsonrpc": "2.0", "id": 1, "result": {"isError": true, "content": [{"type": "text", "text": "could not connect"}]}},
            {"jsonrpc": "2.0", "id": 2, "result": {"isError": false, "content": [{"type": "text", "text": "{}"}]}},
            {"jsonrpc": "2.0", "id": 3, "result": {}}
        ]);
        let lines = entries("mcp", &request.to_string(), Some(&reply.to_string()), Duration::from_millis(1234), 1_792_000_000);
        assert_eq!(lines.len(), 2, "{lines:?}");
        let (a, b) = (&lines[0], &lines[1]);
        assert_eq!(
            (a["tool"].clone(), a["command"].clone(), a["success"].clone()),
            (json!("run_command"), json!("layer.addData"), json!(false))
        );
        assert_eq!(a["error"], "could not connect");
        assert_eq!((a["source"].clone(), a["duration_s"].clone()), (json!("mcp"), json!(1.234)));
        assert!(a["ts"].as_str().is_some_and(|t| t.ends_with('Z')));
        let args = a["args"].to_string();
        assert!(!args.contains("hunter2") && !args.contains("abc") && args.contains("db.example.com"), "{args}");
        assert_eq!((b["script"].clone(), b["success"].clone(), b["error"].clone()), (json!("map.zoomIn"), json!(true), Value::Null));
        // No reply (the connection gave up): failed.
        let lone = json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "list_layers"}});
        let lines = entries("file", &lone.to_string(), None, Duration::ZERO, 0);
        assert_eq!((lines[0]["success"].clone(), lines[0]["error"].clone()), (json!(false), json!("no reply")));
        // Not JSON: nothing to log.
        assert!(entries("mcp", "not json", None, Duration::ZERO, 0).is_empty());
    }

    /// Lines go to the day's file; old days are removed, other files are left.
    #[test]
    fn records_append_to_the_days_file_and_old_days_go() {
        let dir = tempfile::tempdir().unwrap();
        let old = file_for(dir.path(), now() - 40 * 86_400);
        let recent = file_for(dir.path(), now() - 2 * 86_400);
        std::fs::write(&old, "{}\n").unwrap();
        std::fs::write(&recent, "{}\n").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "mine").unwrap();
        let call =
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "list_layers", "arguments": {}}}).to_string();
        let ok = json!({"jsonrpc": "2.0", "id": 1, "result": {"isError": false, "content": []}}).to_string();
        record(dir.path(), "mcp", &call, Some(&ok), Duration::from_millis(5));
        record(dir.path(), "mcp", &call, Some(&ok), Duration::from_millis(5));
        let today = std::fs::read_to_string(file_for(dir.path(), now())).unwrap();
        assert_eq!(today.lines().count(), 2, "{today}");
        assert!(today.lines().all(|l| serde_json::from_str::<Value>(l).is_ok_and(|v| v["tool"] == "list_layers")));
        assert!(!old.exists(), "a day past the limit is removed");
        assert!(recent.exists() && dir.path().join("notes.txt").exists());
    }
}
