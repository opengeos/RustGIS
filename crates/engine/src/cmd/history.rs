//! A geoprocessing run from the History pane as code (#539): a call for the `rustgis` Python
//! package, a line for the Script pane, or a `rustgis-cli` command; and the whole history as a
//! Python script.
//!
//! The parameters are as the tool ran with them, without what would give someone else the user's
//! access (`sources::redact`). Layers go by the names they had then, so a script opens the
//! project they belong to.

use serde_json::{Map, Value, json};

use super::{ps, uint};
use crate::{CmdResult, CommandSpec as C, GpRecord, Session};

pub fn commands() -> Vec<C> {
    vec![
        C::new("gp.historyCode", "Copy Run As Code", "Analyze › Geoprocessing", code)
            .view()
            .params(r#"{"run": index in the history (0 = the first run), "as"?: "python" (default)|"command" (a Script pane line)|"cli"}"#),
        C::new("gp.historyScript", "Save History As Python Script", "Analyze › Geoprocessing", script).view(),
    ]
}

fn code(s: &mut Session, p: &Value) -> CmdResult {
    let i = uint(p, "run").and_then(|i| usize::try_from(i).ok()).ok_or("run is required: a run's place in the history, from 0")?;
    let r = s.project.gp_history.get(i).ok_or("no run there in the history")?;
    let (cmd, params) = replay(r)?;
    let text = match ps(p, "as").unwrap_or("python") {
        "python" => python(&cmd, &params),
        "command" => format!("{cmd} {params}"),
        "cli" => format!("rustgis-cli run --project {} --cmd {}", shell(&project_file(s)), shell(&format!("{cmd}={params}"))),
        other => return Err(format!("unknown kind '{other}': python, command or cli")),
    };
    Ok(json!({"code": text}))
}

/// The history as a Python script that opens the project and runs each tool again, in order. A
/// run that failed, or can't be run again, is there as a comment.
fn script(s: &mut Session, _: &Value) -> CmdResult {
    let runs = &s.project.gp_history;
    if runs.is_empty() {
        return Err("the history is empty: run a tool first".into());
    }
    let name = &s.project.name;
    let mut text = format!(
        "# The geoprocessing history of the RustGIS project {}, run again in order.\n\
         # Layers are named as they were then, so this opens the project they belong to.\n",
        quoted(name)
    );
    if s.path.is_none() {
        text.push_str("# The project hasn't been saved yet: save it as this file first.\n");
    }
    text.push_str(&format!("import rustgis\n\ns = rustgis.Session({})\n", quoted(&project_file(s))));
    for (n, r) in runs.iter().enumerate() {
        let when = if r.at > 0 {
            let (date, time) = rustgis_map::graphics::civil(r.at);
            format!(" ({date} {time} UTC)")
        } else {
            String::new()
        };
        text.push_str(&format!("\n# {}. {}{when}", n + 1, line(&r.tool)));
        match replay(r) {
            Ok((cmd, params)) if r.ok => text.push_str(&format!("\n{}\n", python(&cmd, &params))),
            Ok((cmd, params)) => {
                let why = r.messages.first().map(|m| line(&crate::sources::redact_text(m))).unwrap_or_default();
                text.push_str(&format!(": failed when it ran ({why})\n"));
                for l in python(&cmd, &params).lines() {
                    text.push_str(&format!("# {l}\n"));
                }
            }
            Err(e) => text.push_str(&format!(": {}\n", line(&e))),
        }
    }
    let file = format!("{}_history.py", file_stem(name));
    Ok(json!({"fileName": file, "text": text}))
}

/// The commands other than `gp.run` a run is copied as. The history comes from the project file,
/// which may say anything: only these are written into code, so a crafted file can't pass any
/// other command off as a geoprocessing run (and an id is never text to escape).
const REPLAYED: &[&str] = &["gp.bufferPoint"];

/// The command that runs `r` again, with its parameters: `gp.run` with the tool, or the command
/// that ran it (a point buffered where the map was clicked, `gp.bufferPoint`).
pub fn replay(r: &GpRecord) -> Result<(String, Value), String> {
    if let Some((cmd, params)) = &r.command {
        let Some(cmd) = REPLAYED.iter().find(|c| **c == cmd.as_str()) else {
            return Err(format!("{} can't be run again: the history names a command that isn't a geoprocessing one", r.tool));
        };
        return Ok((cmd.to_string(), redacted(params)));
    }
    let id = tool_of(r).ok_or_else(|| format!("{} can't be run again: the history doesn't say which tool it was", r.tool))?;
    Ok(("gp.run".into(), json!({"tool": id, "params": redacted(&r.params)})))
}

/// The id of the tool `r` ran with `gp.run`: kept with the run, or, in projects from before it
/// was kept, a built-in tool found by its name. `None` for a run another command made.
pub fn tool_of(r: &GpRecord) -> Option<String> {
    if r.command.is_some() {
        return None;
    }
    if r.id.is_empty() { super::gp::tool_id(&r.tool).map(str::to_string) } else { Some(r.id.clone()) }
}

/// `v` without what would give someone else the user's access: an address's keys, tokens and
/// passwords, a connection's password. An object's keys too: the file may put anything there.
fn redacted(v: &Value) -> Value {
    match v {
        Value::String(t) => Value::String(redacted_text(t)),
        Value::Array(a) => Value::Array(a.iter().map(redacted).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (redacted_text(k), redacted(v))).collect()),
        other => other.clone(),
    }
}

/// `t` without its secrets. A connection is taken whole, though a libpq one (`PG:host=db
/// password=…`) has spaces in it: `redact` takes its password out. Other text with spaces may
/// name addresses among its words; a single word may be one.
fn redacted_text(t: &str) -> String {
    if connection(t) || !t.contains(char::is_whitespace) { crate::sources::redact(t) } else { crate::sources::redact_text(t) }
}

/// Whether `t` is a database connection `sources::redact` reads as one: PostgreSQL's (a URI, or
/// libpq's `key=value` words after `postgres:`, `postgresql:` or GDAL's `PG:`), MySQL's or
/// MariaDB's.
fn connection(t: &str) -> bool {
    let lower = t.to_ascii_lowercase();
    ["postgres:", "postgresql:", "pg:", "mysql:", "mariadb:"].iter().any(|p| lower.starts_with(p))
}

/// The project file a script opens: the one it was opened from or saved to, else one named
/// after it (to save it as, first).
fn project_file(s: &Session) -> String {
    match &s.path {
        Some(p) => p.display().to_string(),
        None => format!("{}.rgis", file_stem(&s.project.name)),
    }
}

/// `name` as a file name on every system: letters, digits, `-` and `_` kept, anything else (a
/// separator, `* ? " < > |`, a control character) `_`, as `model.export` names its file.
fn file_stem(name: &str) -> String {
    let stem: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    if stem.is_empty() { "project".into() } else { stem }
}

/// A call of the `rustgis` package's `Session.run`, one parameter a line (as ArcGIS Pro copies a
/// tool's Python): keywords where the names allow, `gp.run`'s tool before its parameters.
pub fn python(cmd: &str, params: &Value) -> String {
    let mut out = format!("s.run(\n    {},\n", quoted(cmd));
    let empty = Map::new();
    let o = params.as_object().unwrap_or(&empty);
    if o.keys().all(|k| identifier(k)) {
        // `tool` first: the JSON map keeps its keys sorted.
        let mut keys: Vec<&String> = o.keys().collect();
        keys.sort_by_key(|k| (k.as_str() != "tool", k.as_str() == "params"));
        for k in keys {
            if let Some(v) = o.get(k) {
                out.push_str(&format!("    {k}={},\n", py(v, 1)));
            }
        }
    } else {
        out.push_str(&format!("    {},\n", py(params, 1)));
    }
    out.push(')');
    out
}

/// `v` as a Python literal, objects one key a line at `indent` levels.
fn py(v: &Value, indent: usize) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => (if *b { "True" } else { "False" }).into(),
        Value::Number(n) => n.to_string(),
        Value::String(t) => quoted(t),
        Value::Array(a) => format!("[{}]", a.iter().map(|x| py(x, indent)).collect::<Vec<_>>().join(", ")),
        Value::Object(o) if o.is_empty() => "{}".into(),
        Value::Object(o) => {
            let pad = "    ".repeat(indent);
            let items: String = o.iter().map(|(k, x)| format!("{pad}    {}: {},\n", quoted(k), py(x, indent + 1))).collect();
            format!("{{\n{items}{pad}}}")
        }
    }
}

/// A string literal Python reads back as `t`: JSON's escapes are Python's too.
fn quoted(t: &str) -> String {
    serde_json::to_string(t).unwrap_or_else(|_| "\"\"".into())
}

/// Whether `k` can be a keyword argument: a Python identifier, not one of its reserved words.
fn identifier(k: &str) -> bool {
    const RESERVED: &[&str] = &[
        "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else",
        "except", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise",
        "return", "try", "while", "with", "yield",
    ];
    let mut chars = k.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !RESERVED.contains(&k)
}

/// `t` quoted for a POSIX shell: in single quotes, each of its own closing them for a moment.
fn shell(t: &str) -> String {
    format!("'{}'", t.replace('\'', "'\\''"))
}

/// `t` on one line, for a comment. What may end a line (a lone `\r` does, for Python), or any
/// other control character, becomes a space: the text comes from the project file, and a line
/// break in it would leave the rest of it outside the comment, as code.
fn line(t: &str) -> String {
    t.chars().map(|c| if c.is_control() || c == '\u{2028}' || c == '\u{2029}' { ' ' } else { c }).take(300).collect()
}
