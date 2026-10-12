use std::collections::VecDeque;

use rustgis_engine::{Project, Session};
use serde_json::{Value, json};

use crate::agent::{Agent, Entry, MAX_STEPS, Step, run_blocking};
use crate::provider::*;
use crate::tools;

fn sample() -> Session {
    let mut s = Session::new(Project::default());
    let _ = s.run("project.sample", &Value::Null);
    s
}

fn cfg(provider: &str) -> Config {
    let p = preset(provider).unwrap_or(&PRESETS[0]);
    Config {
        provider: provider.into(),
        base_url: p.base_url.into(),
        model: if p.model.is_empty() { "m".into() } else { p.model.into() },
        key: "k".into(),
    }
}

fn ok(v: Value) -> Result<HttpResponse, String> {
    Ok(HttpResponse { status: 200, body: v.to_string().into_bytes() })
}

/// An OpenAI reply calling one tool.
fn call(id: &str, name: &str, args: Value) -> Value {
    json!({"choices": [{"message": {"role": "assistant", "content": null,
        "tool_calls": [{"id": id, "type": "function", "function": {"name": name, "arguments": args.to_string()}}]},
        "finish_reason": "tool_calls"}]})
}

fn say(text: &str) -> Value {
    json!({"choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]})
}

fn body(req: &HttpRequest) -> Value {
    serde_json::from_str(req.body.as_deref().unwrap_or("null")).unwrap_or(Value::Null)
}

#[test]
fn urls_allowed() {
    assert!(allowed_url("https://api.openai.com/v1/chat/completions"));
    assert!(allowed_url("http://localhost:11434/v1/models"));
    assert!(allowed_url("http://127.0.0.1:1234/v1"));
    assert!(allowed_url("http://192.168.1.20:11434/v1"));
    assert!(allowed_url("http://[::1]:8080/v1"));
    assert!(allowed_url("http://172.20.0.2/v1"));
    assert!(!allowed_url("http://example.com/v1"));
    assert!(!allowed_url("http://localhost@example.com/v1"));
    assert!(!allowed_url("http://172.40.0.2/v1"));
    assert!(!allowed_url("http://10.example.com/v1"));
    assert!(!allowed_url("file:///etc/passwd"));
    assert!(!allowed_url("https://user:key@api.example.com/v1"));
    assert!(!allowed_url("https://"));
    assert!(!allowed_url("http://010.0.0.1/v1"));
    assert!(!allowed_url("http://0177.0.0.1/v1"));
    assert!(allowed_url("http://10.0.0.1/v1"));
    assert!(!allowed_url(""));
}

#[test]
fn openai_round_trip() {
    let s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(req) = a.ask("Zoom to Paris", &s) else { panic!("expected a request") };
    assert_eq!(req.url, "https://api.openai.com/v1/chat/completions");
    assert!(req.headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer k"));
    let b = body(&req);
    assert_eq!(b["messages"][0]["role"], "system");
    let user = b["messages"][1]["content"].as_str().unwrap_or("");
    assert!(user.contains("<map>") && user.contains("\"Cities\"") && user.ends_with("Zoom to Paris"));
    assert!(b["tools"].as_array().is_some_and(|t| t.iter().any(|t| t["function"]["name"] == "run_command")));
}

#[test]
fn agent_runs_commands_and_answers() {
    let mut s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let mut replies: VecDeque<Value> = VecDeque::from(vec![
        call("c1", "run_command", json!({"command": "map.goToXY", "params": {"lon": 2.35, "lat": 48.85, "scale": 5000000}})),
        say("Done: the map is centred on **Paris**."),
    ]);
    let mut requests = Vec::new();
    run_blocking(
        &mut a,
        "Zoom to Paris",
        &mut s,
        |req| {
            requests.push(body(req));
            replies.pop_front().map(ok).unwrap_or_else(|| Err("no more replies".into()))
        },
        |_, _, _| true,
    );
    let c = s.map().center_lonlat();
    assert!((c.x - 2.35).abs() < 0.01 && (c.y - 48.85).abs() < 0.01, "map moved to {c:?}");
    assert!(matches!(a.transcript.get(1), Some(Entry::Tool { error: false, .. })), "{:?}", a.transcript);
    assert!(matches!(a.transcript.last(), Some(Entry::Assistant(t)) if t.contains("Paris")));
    // The second request carries the call and its result.
    let msgs = requests.get(1).and_then(|b| b["messages"].as_array().cloned()).unwrap_or_default();
    assert!(msgs.iter().any(|m| m["role"] == "tool" && m["tool_call_id"] == "c1"));
    assert!(!a.busy());
    // The context is sent again only when the map changed: it did (the camera moved).
    let Step::Send(req) = a.ask("And now?", &s) else { panic!("expected a request") };
    let b = body(&req);
    let last = b["messages"].as_array().and_then(|m| m.last().cloned()).unwrap_or(Value::Null);
    assert!(last["content"].as_str().is_some_and(|t| t.starts_with("<map>")));
}

#[test]
fn unchanged_context_is_not_repeated() {
    let s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(_) = a.ask("hi", &s) else { panic!("expected a request") };
    let mut s2 = sample();
    let _ = a.receive(a.generation(), ok(say("Hello")), &mut s2);
    let Step::Send(req) = a.ask("what layers?", &s) else { panic!("expected a request") };
    let b = body(&req);
    let last = b["messages"].as_array().and_then(|m| m.last().cloned()).unwrap_or(Value::Null);
    assert_eq!(last["content"], "what layers?");
}

#[test]
fn file_access_needs_approval() {
    let mut s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(_) = a.ask("export the map", &s) else { panic!("expected a request") };
    let path = std::env::temp_dir().join("rustgis-assistant-never-written.png");
    let step =
        a.receive(a.generation(), ok(call("c1", "run_command", json!({"command": "share.exportMap", "params": {"path": path}}))), &mut s);
    assert_eq!(step, Step::Confirm);
    let (what, why, _) = a.pending(&s).unwrap_or_default();
    assert!(what.starts_with("share.exportMap") && why.contains("files"), "{what} / {why}");
    let Step::Send(req) = a.decide(false, false, &mut s) else { panic!("expected a request") };
    assert!(!path.exists());
    let b = body(&req);
    let tool_msg = b["messages"].as_array().and_then(|m| m.iter().find(|m| m["role"] == "tool").cloned()).unwrap_or(Value::Null);
    assert!(tool_msg["content"].as_str().is_some_and(|t| t.contains("declined")));
}

#[test]
fn gate_rules() {
    let s = sample();
    let g = |name: &str, args: Value| tools::gate(&s, &ToolCall { id: "x".into(), name: name.into(), args });
    assert!(g("run_command", json!({"command": "map.zoomFull"})).is_none());
    assert!(g("run_command", json!({"command": "edit.undo"})).is_none());
    assert!(g("run_command", json!({"command": "layer.addData", "params": {"path": "/etc/passwd"}})).is_some());
    assert!(g("run_command", json!({"command": "project.new"})).is_some());
    assert!(g("run_command", json!({"command": "script.run", "params": {"script": "map.zoomFull {}"}})).is_some());
    assert!(g("run_command", json!({"command": "plugin.install", "params": {"path": "x.rgplugin"}})).is_some());
    assert!(g("run_command", json!({"command": "map.addTiles", "params": {"url": "https://tile.example/{z}/{x}/{y}.png"}})).is_some());
    assert!(g("run_command", json!({"command": "plugin:x/y"})).is_some());
    assert!(g("run_tool", json!({"tool": "buffer", "params": {"input": "Cities", "distance": 100}})).is_none());
    assert!(g("list_layers", json!({})).is_none());
}

#[test]
fn geoprocessing_and_reading_data() {
    let mut s = sample();
    let t = |name: &str, args: Value| ToolCall { id: "x".into(), name: name.into(), args };
    let found = tools::run(&mut s, &t("search_tools", json!({"query": "buffer"}))).unwrap_or_default();
    assert!(found.contains("\"buffer\""), "{found}");
    let desc = tools::run(&mut s, &t("describe_tool", json!({"tool": "buffer"}))).unwrap_or_default();
    assert!(desc.contains("distance"), "{desc}");
    let before = s.map().layers.len();
    let ran = tools::run(&mut s, &t("run_tool", json!({"tool": "buffer", "params": {"input": "Cities", "distance": 100}})));
    assert!(ran.as_deref().is_ok_and(|r| r.contains("layersAdded")), "{ran:?}");
    assert_eq!(s.map().layers.len(), before + 1);
    let rows = tools::run(
        &mut s,
        &t("read_features", json!({"layer": "Cities", "where": "pop_max > 10000000", "fields": ["name", "pop_max"], "order_by": "pop_max", "descending": true, "limit": 3})),
    )
    .unwrap_or_default();
    let v: Value = serde_json::from_str(&rows).unwrap_or(Value::Null);
    assert!(v["matched"].as_u64().is_some_and(|n| n > 3), "{rows}");
    let r = v["rows"].as_array().cloned().unwrap_or_default();
    assert_eq!(r.len(), 3);
    assert!(r[0]["pop_max"].as_i64() >= r[1]["pop_max"].as_i64());
    assert!(tools::run(&mut s, &t("read_features", json!({"layer": "Cities", "fields": ["nope"]}))).is_err());
    assert!(tools::run(&mut s, &t("read_features", json!({"layer": "Cities", "fields": [1, null]}))).is_err());
    let cmds = tools::run(&mut s, &t("search_commands", json!({"query": "basemap"}))).unwrap_or_default();
    assert!(cmds.contains("map.basemap"), "{cmds}");
}

#[test]
fn hostile_arguments_never_panic() {
    let mut s = sample();
    let junk = [
        Value::Null,
        json!({}),
        json!([1, 2]),
        json!("text"),
        json!({"command": 5, "params": "not json", "tool": [], "layer": {"a": 1}, "query": null, "limit": -4}),
        json!({"command": "map.goToXY", "params": "{\"lon\": 1e309}"}),
        json!({"command": "ui.about"}),
        json!({"command": "nope.nope"}),
        json!({"tool": "nope", "params": {"input": "Nope"}}),
        json!({"layer": "Cities", "where": "pop_max >", "limit": 1e20, "order_by": "", "fields": [1, null]}),
        json!({"layer": 99999, "selected": true}),
        json!({"query": "∑≈ç√ 🙂 a b c d e f g h i j k l m n o p", "limit": 100000}),
        json!({"tool": "buffer", "params": {"input": "Cities", "distance": "far"}}),
    ];
    for def in tools::defs() {
        for args in &junk {
            let c = ToolCall { id: "x".into(), name: def.name.into(), args: args.clone() };
            let _ = tools::gate(&s, &c);
            let _ = tools::summary(&c);
            if let Ok(r) = tools::run(&mut s, &c) {
                assert!(r.chars().count() <= tools::MAX_RESULT + 60);
            }
        }
    }
    let _ = tools::run(&mut s, &ToolCall { id: "x".into(), name: "rm_rf".into(), args: Value::Null });
}

#[test]
fn hostile_replies_never_panic() {
    let bodies = [
        "",
        "null",
        "[]",
        "{\"choices\": []}",
        "{\"choices\": [{\"message\": {\"tool_calls\": [{\"function\": {\"arguments\": \"{oops\"}}]}}]}",
        "{\"content\": [{\"type\": \"tool_use\"}], \"stop_reason\": \"refusal\"}",
        "{\"candidates\": [{\"content\": {\"parts\": [{\"functionCall\": {}}, {\"text\": 5}]}}]}",
        "{\"error\": {\"message\": \"Invalid API key\"}}",
        "<html>bad gateway</html>",
    ];
    for api in [Api::OpenAi, Api::Anthropic, Api::Gemini] {
        for b in bodies {
            for status in [200, 401, 500] {
                let _ = parse_reply(api, &HttpResponse { status, body: b.as_bytes().to_vec() });
                let _ = parse_models(api, &HttpResponse { status, body: b.as_bytes().to_vec() });
            }
        }
    }
    let e = parse_reply(Api::OpenAi, &HttpResponse { status: 401, body: b"{\"error\": {\"message\": \"Invalid API key\"}}".to_vec() });
    assert_eq!(e.err().as_deref(), Some("Invalid API key"));
    // A bad reply ends the run with an error the user sees.
    let mut s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(_) = a.ask("hi", &s) else { panic!("expected a request") };
    assert_eq!(a.receive(a.generation(), Err("offline".into()), &mut s), Step::Done);
    assert!(matches!(a.transcript.last(), Some(Entry::Error(e)) if e == "offline"));
    assert!(!a.busy());
}

#[test]
fn anthropic_format() {
    let mut s = sample();
    let mut a = Agent::new(cfg("anthropic"), &s);
    let Step::Send(req) = a.ask("list layers", &s) else { panic!("expected a request") };
    assert_eq!(req.url, "https://api.anthropic.com/v1/messages");
    assert!(req.headers.iter().any(|(k, v)| k == "x-api-key" && v == "k"));
    assert!(req.headers.iter().any(|(k, _)| k == "anthropic-version"));
    let b = body(&req);
    assert!(b["system"].as_str().is_some_and(|t| t.contains("RustGIS")));
    assert_eq!(b["fallbacks"], "default");
    assert!(b["tools"][0]["input_schema"].is_object());
    let reply = json!({"content": [
        {"type": "thinking", "thinking": "", "signature": "sig"},
        {"type": "text", "text": "Let me look."},
        {"type": "tool_use", "id": "toolu_1", "name": "list_layers", "input": {}}
    ], "stop_reason": "tool_use"});
    let Step::Send(req) = a.receive(a.generation(), ok(reply), &mut s) else { panic!("expected a request") };
    let b = body(&req);
    let msgs = b["messages"].as_array().cloned().unwrap_or_default();
    assert_eq!(msgs.len(), 3);
    // The thinking block goes back unchanged; the result follows as a user message.
    assert_eq!(msgs[1]["content"][0]["signature"], "sig");
    assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
    assert_eq!(msgs[2]["content"][0]["tool_use_id"], "toolu_1");
    // The next user message joins the results' user message.
    let mut a2 = Agent::new(cfg("anthropic"), &s);
    let Step::Send(_) = a2.ask("x", &s) else { panic!("expected a request") };
    let _ =
        a2.receive(a2.generation(), ok(json!({"content": [{"type": "tool_use", "id": "t", "name": "list_layers", "input": {}}]})), &mut s);
    a2.stop();
    let Step::Send(req) = a2.ask("y", &s) else { panic!("expected a request") };
    let msgs = body(&req)["messages"].as_array().cloned().unwrap_or_default();
    assert!(msgs.windows(2).all(|w| w[0]["role"] != w[1]["role"]), "{msgs:?}");
    // A refusal runs nothing.
    let mut a3 = Agent::new(cfg("anthropic"), &s);
    let Step::Send(_) = a3.ask("x", &s) else { panic!("expected a request") };
    let refused = json!({"content": [{"type": "tool_use", "id": "t", "name": "run_command", "input": {"command": "layer.remove", "params": {"layer": "Cities"}}}], "stop_reason": "refusal"});
    assert_eq!(a3.receive(a3.generation(), ok(refused), &mut s), Step::Done);
    assert!(s.map().layer_by_name("Cities").is_some());
}

#[test]
fn gemini_format() {
    let mut s = sample();
    let mut a = Agent::new(cfg("gemini"), &s);
    let Step::Send(req) = a.ask("hide the rivers", &s) else { panic!("expected a request") };
    assert!(req.url.ends_with("/models/gemini-3.8-flash:generateContent"), "{}", req.url);
    let b = body(&req);
    let decls = b["tools"][0]["functionDeclarations"].as_array().cloned().unwrap_or_default();
    let run = decls.iter().find(|d| d["name"] == "run_command").cloned().unwrap_or(Value::Null);
    // A free-form object parameter goes as JSON text.
    assert_eq!(run["parameters"]["properties"]["params"]["type"], "string");
    let reply = json!({"candidates": [{"content": {"role": "model", "parts": [
        {"functionCall": {"name": "run_command", "args": {"command": "layer.visibility", "params": "{\"layer\": \"Rivers\", \"visible\": false}"}}, "thoughtSignature": "abc"}
    ]}, "finishReason": "STOP"}]});
    let Step::Send(req) = a.receive(a.generation(), ok(reply), &mut s) else { panic!("expected a request") };
    assert!(s.map().layer_by_name("Rivers").is_some_and(|l| !l.visible));
    let b = body(&req);
    let contents = b["contents"].as_array().cloned().unwrap_or_default();
    assert_eq!(contents[1]["parts"][0]["thoughtSignature"], "abc");
    assert_eq!(contents[2]["parts"][0]["functionResponse"]["name"], "run_command");
    assert!(contents[2]["parts"][0]["functionResponse"].get("id").is_none());
}

#[test]
fn steps_are_bounded_and_stop_works() {
    let mut s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let mut n = 0;
    run_blocking(
        &mut a,
        "loop forever",
        &mut s,
        |_| {
            n += 1;
            ok(call(&format!("c{n}"), "list_layers", json!({})))
        },
        |_, _, _| true,
    );
    assert_eq!(n, MAX_STEPS as usize);
    assert!(matches!(a.transcript.last(), Some(Entry::Note(_))));
    // A stale response (after a stop) is dropped.
    let Step::Send(_) = a.ask("again", &s) else { panic!("expected a request") };
    let g = a.generation();
    a.stop();
    assert_eq!(a.receive(g, ok(say("late")), &mut s), Step::Done);
    assert!(!a.transcript.iter().any(|e| matches!(e, Entry::Assistant(t) if t == "late")));
}

#[test]
fn missing_settings_are_reported() {
    let s = sample();
    let mut c = cfg("openai");
    c.key.clear();
    let mut a = Agent::new(c, &s);
    assert_eq!(a.ask("hi", &s), Step::Done);
    assert!(matches!(a.transcript.last(), Some(Entry::Error(_))));
    let mut c = cfg("ollama");
    c.key.clear();
    assert!(c.missing().is_none());
}

#[test]
fn model_lists() {
    let r = |v: Value| HttpResponse { status: 200, body: v.to_string().into_bytes() };
    assert_eq!(parse_models(Api::OpenAi, &r(json!({"data": [{"id": "b"}, {"id": "a"}]}))), Ok(vec!["a".to_string(), "b".to_string()]));
    let g = json!({"models": [
        {"name": "models/gemini-x", "supportedGenerationMethods": ["generateContent"]},
        {"name": "models/embed", "supportedGenerationMethods": ["embedContent"]}
    ]});
    assert_eq!(parse_models(Api::Gemini, &r(g)), Ok(vec!["gemini-x".to_string()]));
    let req = models_request(&cfg("anthropic"));
    assert_eq!(req.method, "GET");
    assert!(req.url.starts_with("https://api.anthropic.com/v1/models"));
}

#[test]
fn empty_replies_are_reported() {
    let mut s = sample();
    let mut a = Agent::new(cfg("ollama"), &s);
    let Step::Send(_) = a.ask("hi", &s) else { panic!("expected a request") };
    let empty = json!({"choices": [{"message": {"role": "assistant", "content": "", "reasoning": "…"}, "finish_reason": "stop"}]});
    assert_eq!(a.receive(a.generation(), ok(empty), &mut s), Step::Done);
    assert!(matches!(a.transcript.last(), Some(Entry::Note(n)) if n.contains("empty")));
}

#[test]
fn ordering_mixed_columns_never_panics() {
    use rustgis_engine::data::Value as V;
    let mut s = sample();
    let id = s.layer_id(&json!({"layer": "Cities"})).unwrap_or(0);
    // A column of mixed numbers, numeric text, words, NaN and nulls.
    let mixed = [V::Int(5), V::Text("abc".into()), V::Text("10".into()), V::Float(f64::NAN), V::Null, V::Text("9".into()), V::Bool(true)];
    if let Some(l) = s.map_mut().layer_mut(id) {
        let fi = l.dataset.fields.iter().position(|f| f.name == "name").unwrap_or(0);
        for (i, f) in l.dataset.features.iter_mut().enumerate() {
            if let Some(a) = f.attrs.get_mut(fi) {
                *a = mixed[i % mixed.len()].clone();
            }
        }
    }
    let t = ToolCall {
        id: "x".into(),
        name: "read_features".into(),
        args: json!({"layer": "Cities", "order_by": "name DESC", "fields": ["name"], "limit": 30}),
    };
    let r = tools::run(&mut s, &t);
    let v: Value = r.as_deref().ok().and_then(|r| serde_json::from_str(r).ok()).unwrap_or(Value::Null);
    let rows = v["rows"].as_array().cloned().unwrap_or_default();
    assert_eq!(rows.len(), 30, "{r:?}");
    // Numbers (numeric text too) first, largest first; empty values last.
    assert_eq!(rows[0]["name"], "10", "{r:?}");
    assert!(rows.iter().all(|r| !r["name"].is_null()), "{r:?}");
}

#[test]
fn always_allow_is_per_command_and_never_for_reserved_actions() {
    let mut s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(_) = a.ask("x", &s) else { panic!("expected a request") };
    let dir = std::env::temp_dir();
    let export = |n: u32| json!({"command": "share.exportLayer", "params": {"layer": "Lakes", "path": dir.join(format!("rustgis-assistant-test-{n}.geojson"))}});
    let reply = json!({"choices": [{"message": {"role": "assistant", "content": null, "tool_calls": [
        {"id": "c0", "type": "function", "function": {"name": "run_command", "arguments": export(1).to_string()}},
        {"id": "c1", "type": "function", "function": {"name": "run_command", "arguments": export(1).to_string()}},
        {"id": "c2", "type": "function", "function": {"name": "run_command", "arguments": export(2).to_string()}},
        {"id": "c3", "type": "function", "function": {"name": "run_command", "arguments": json!({"command": "map.addTiles", "params": {"url": "https://t.example/{z}/{x}/{y}.png"}}).to_string()}},
        {"id": "c4", "type": "function", "function": {"name": "run_command", "arguments": json!({"command": "project.new"}).to_string()}}
    ]}, "finish_reason": "tool_calls"}]});
    assert_eq!(a.receive(a.generation(), ok(reply), &mut s), Step::Confirm);
    assert!(a.pending(&s).is_some_and(|p| p.2));
    // Allowing the export for the conversation allows that file only: the next export (another
    // file) asks again, and so do the tiles.
    assert_eq!(a.decide(true, true, &mut s), Step::Confirm);
    assert!(a.pending(&s).is_some_and(|p| p.0.starts_with("share.exportLayer") && p.2));
    assert_eq!(a.decide(false, false, &mut s), Step::Confirm);
    assert!(a.pending(&s).is_some_and(|p| p.0.starts_with("map.addTiles")));
    assert_eq!(a.decide(false, false, &mut s), Step::Confirm);
    // Replacing the project is never allowed for good.
    assert!(a.pending(&s).is_some_and(|p| p.0.starts_with("project.new") && !p.2));
    let _ = a.decide(false, false, &mut s);
    for n in [1, 2] {
        let _ = std::fs::remove_file(dir.join(format!("rustgis-assistant-test-{n}.geojson")));
    }
}

#[test]
fn stopping_a_request_drops_its_message() {
    let s = sample();
    let mut a = Agent::new(cfg("openai"), &s);
    let Step::Send(_) = a.ask("first", &s) else { panic!("expected a request") };
    a.stop();
    let Step::Send(req) = a.ask("second", &s) else { panic!("expected a request") };
    let msgs = body(&req)["messages"].as_array().cloned().unwrap_or_default();
    let users: Vec<&Value> = msgs.iter().filter(|m| m["role"] == "user").collect();
    assert_eq!(users.len(), 1);
    assert!(users[0]["content"].as_str().is_some_and(|t| t.starts_with("<map>") && t.ends_with("second")));
}

#[test]
fn malformed_params_are_refused() {
    let mut s = sample();
    let n = s.map().layers.len();
    for params in [json!("{\"layer\": "), json!("[1]"), json!(5)] {
        let c = ToolCall { id: "x".into(), name: "run_command".into(), args: json!({"command": "layer.remove", "params": params}) };
        assert!(tools::run(&mut s, &c).is_err());
    }
    assert_eq!(s.map().layers.len(), n, "no layer may be removed by a call whose parameters didn't parse");
    let c = ToolCall { id: "x".into(), name: "run_command".into(), args: json!({"command": "map.zoomFull", "params": "{}"}) };
    assert!(tools::run(&mut s, &c).is_ok());
}

#[test]
fn stopping_a_follow_up_keeps_the_history_valid() {
    let mut s = sample();
    for provider in ["anthropic", "openai", "gemini"] {
        let mut a = Agent::new(cfg(provider), &s);
        let Step::Send(_) = a.ask("x", &s) else { panic!("expected a request") };
        let reply = match provider {
            "anthropic" => json!({"content": [{"type": "tool_use", "id": "t", "name": "list_layers", "input": {}}]}),
            "gemini" => {
                json!({"candidates": [{"content": {"role": "model", "parts": [{"functionCall": {"name": "list_layers", "args": {}}}]}}]})
            }
            _ => call("t", "list_layers", json!({})),
        };
        // The calls ran and the follow-up request is out when the user stops.
        let Step::Send(_) = a.receive(a.generation(), ok(reply), &mut s) else { panic!("expected a follow-up") };
        a.stop();
        let Step::Send(req) = a.ask("y", &s) else { panic!("expected a request") };
        let b = body(&req);
        match provider {
            // Results and the new text share one user message.
            "anthropic" => {
                let msgs = b["messages"].as_array().cloned().unwrap_or_default();
                assert!(msgs.windows(2).all(|w| w[0]["role"] != w[1]["role"]), "{msgs:?}");
                let last = msgs.last().cloned().unwrap_or(Value::Null);
                assert_eq!(last["content"][0]["type"], "tool_result");
                assert_eq!(last["content"][1]["type"], "text");
            }
            // Tool results, then the user's message.
            "openai" => {
                let msgs = b["messages"].as_array().cloned().unwrap_or_default();
                let roles: Vec<&str> = msgs.iter().filter_map(|m| m["role"].as_str()).collect();
                assert_eq!(roles, ["system", "user", "assistant", "tool", "user"]);
            }
            _ => {
                let c = b["contents"].as_array().cloned().unwrap_or_default();
                assert_eq!(c.len(), 4);
                assert!(c[2]["parts"][0].get("functionResponse").is_some());
            }
        }
    }
}
