//! Model providers: building the HTTP request for a conversation and reading the reply, for the
//! three wire formats the assistant speaks (OpenAI Chat Completions, which OpenRouter, Ollama and
//! most local servers also speak; Anthropic Messages; Gemini `generateContent`).
//!
//! Nothing here does I/O: a request is plain data ([`HttpRequest`]) that the desktop sends on a
//! thread and the browser sends with its `fetch`.

use serde_json::{Map, Value, json};

/// A wire format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    /// `POST {base}/chat/completions`.
    OpenAi,
    /// `POST {base}/v1/messages`.
    Anthropic,
    /// `POST {base}/models/{model}:generateContent`.
    Gemini,
}

/// A provider the settings offer, with its defaults.
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub api: Api,
    pub base_url: &'static str,
    pub model: &'static str,
    /// Environment variables an API key is read from (desktop and command line), in order.
    pub key_env: &'static [&'static str],
    /// An environment variable that overrides the base URL.
    pub url_env: &'static str,
    /// Whether it works without a key.
    pub keyless: bool,
    /// Where to get a key.
    pub key_url: &'static str,
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "anthropic",
        label: "Anthropic",
        api: Api::Anthropic,
        base_url: "https://api.anthropic.com",
        model: "claude-opus-5-5",
        key_env: &["ANTHROPIC_API_KEY"],
        url_env: "",
        keyless: false,
        key_url: "https://console.anthropic.com/settings/keys",
    },
    Preset {
        id: "openai",
        label: "OpenAI",
        api: Api::OpenAi,
        base_url: "https://api.openai.com/v1",
        model: "gpt-5.5",
        key_env: &["OPENAI_API_KEY"],
        url_env: "",
        keyless: false,
        key_url: "https://platform.openai.com/api-keys",
    },
    Preset {
        id: "gemini",
        label: "Google Gemini",
        api: Api::Gemini,
        base_url: "https://generativelanguage.googleapis.com/v1beta",
        model: "gemini-3.8-flash",
        key_env: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        url_env: "",
        keyless: false,
        key_url: "https://aistudio.google.com/apikey",
    },
    Preset {
        id: "openrouter",
        label: "OpenRouter",
        api: Api::OpenAi,
        base_url: "https://openrouter.ai/api/v1",
        model: "openrouter/auto",
        key_env: &["OPENROUTER_API_KEY"],
        url_env: "",
        keyless: false,
        key_url: "https://openrouter.ai/keys",
    },
    Preset {
        id: "ollama",
        label: "Ollama (local)",
        api: Api::OpenAi,
        base_url: "http://localhost:11434/v1",
        model: "qwen3",
        key_env: &[],
        url_env: "OLLAMA_BASE_URL",
        keyless: true,
        key_url: "https://ollama.com/download",
    },
    Preset {
        id: "custom",
        label: "OpenAI-compatible",
        api: Api::OpenAi,
        base_url: "http://localhost:1234/v1",
        model: "",
        key_env: &["OPENAI_COMPATIBLE_API_KEY"],
        url_env: "OPENAI_COMPATIBLE_BASE_URL",
        keyless: true,
        key_url: "",
    },
];

/// The preset with this id.
pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// Which model to call, where and with what key.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// A [`Preset`] id.
    pub provider: String,
    pub base_url: String,
    pub model: String,
    pub key: String,
}

impl Config {
    /// The preset's defaults, with a key and base URL from the environment where set.
    pub fn from_env(provider: &str) -> Config {
        let p = preset(provider).unwrap_or(&PRESETS[0]);
        let env = |name: &str| std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let key = p.key_env.iter().find_map(|k| env(k)).unwrap_or_default();
        let mut base_url = if p.url_env.is_empty() { None } else { env(p.url_env) }.unwrap_or_else(|| p.base_url.to_string());
        // `OLLAMA_BASE_URL` names the server, not its OpenAI-compatible API.
        if p.id == "ollama" && !base_url.trim_end_matches('/').ends_with("/v1") {
            base_url = format!("{}/v1", base_url.trim_end_matches('/'));
        }
        Config { provider: p.id.to_string(), base_url, model: p.model.to_string(), key }
    }

    pub fn api(&self) -> Api {
        preset(&self.provider).map_or(Api::OpenAi, |p| p.api)
    }

    /// Why this configuration can't be used yet, if it can't.
    pub fn missing(&self) -> Option<&'static str> {
        let keyless = preset(&self.provider).is_some_and(|p| p.keyless);
        if self.model.trim().is_empty() {
            Some("Choose a model.")
        } else if self.base_url.trim().is_empty() {
            Some("Enter the provider's address.")
        } else if self.key.trim().is_empty() && !keyless {
            Some("Enter an API key.")
        } else {
            None
        }
    }

    fn base(&self) -> &str {
        self.base_url.trim().trim_end_matches('/')
    }
}

/// An HTTP request to make.
#[derive(Clone, Debug, PartialEq)]
pub struct HttpRequest {
    pub method: &'static str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
}

/// What came back.
#[derive(Clone, Debug, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// The largest reply read.
pub const MAX_RESPONSE: usize = 8 << 20;

/// Whether the assistant may send a request (with an API key) to `url`: HTTPS anywhere, plain
/// HTTP only to this computer or the local network (Ollama, LM Studio).
pub fn allowed_url(url: &str) -> bool {
    let url = url.trim();
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // No credentials in the address: `http://localhost@evil.example` is not local.
    if authority.contains('@') {
        return false;
    }
    let host = if let Some(v6) = authority.strip_prefix('[') {
        v6.split(']').next().unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    }
    .to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") || host == "::1" {
        return true;
    }
    let octets: Vec<u8> = host.split('.').filter_map(|p| p.parse().ok()).collect();
    if octets.len() != 4 || host.split('.').count() != 4 {
        return false;
    }
    match (octets.first(), octets.get(1)) {
        (Some(127), _) | (Some(10), _) => true,
        (Some(192), Some(168)) => true,
        (Some(172), Some(b)) => (16..=31).contains(b),
        _ => false,
    }
}

/// A tool the model may call.
#[derive(Clone, Debug)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    /// A JSON Schema object (`{"type": "object", "properties": …}`).
    pub schema: Value,
}

/// A call the model made.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// What a call gave back.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolResult {
    pub id: String,
    pub name: String,
    pub content: String,
    pub error: bool,
}

/// One turn of the conversation, in no provider's format.
#[derive(Clone, Debug)]
pub enum Turn {
    User(String),
    Assistant {
        text: String,
        calls: Vec<ToolCall>,
        /// The reply as the provider sent it, sent back unchanged to the same provider: it may carry
        /// what the model needs to see again (Claude's thinking blocks, Gemini's thought signatures).
        native: Option<(Api, Value)>,
    },
    /// The results of the calls in the assistant turn before.
    Results(Vec<ToolResult>),
}

/// A reply read from the provider.
#[derive(Clone, Debug)]
pub struct Reply {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub native: Value,
    /// Why the reply stopped early (length limit, a refusal), if it did.
    pub cut: Option<String>,
}

/// Replies may be long (tables, explanations) but are never documents.
const MAX_TOKENS: u32 = 16_000;

/// The request asking the model for its next turn.
pub fn chat_request(cfg: &Config, system: &str, turns: &[Turn], tools: &[ToolDef]) -> HttpRequest {
    match cfg.api() {
        Api::OpenAi => openai_request(cfg, system, turns, tools),
        Api::Anthropic => anthropic_request(cfg, system, turns, tools),
        Api::Gemini => gemini_request(cfg, system, turns, tools),
    }
}

/// Reads a chat reply (any status).
pub fn parse_reply(api: Api, resp: &HttpResponse) -> Result<Reply, String> {
    let v: Value = serde_json::from_slice(&resp.body).map_err(|_| http_error(resp))?;
    if !(200..300).contains(&resp.status) {
        return Err(error_message(&v).unwrap_or_else(|| http_error(resp)));
    }
    match api {
        Api::OpenAi => openai_reply(&v),
        Api::Anthropic => anthropic_reply(&v),
        Api::Gemini => gemini_reply(&v),
    }
}

fn http_error(resp: &HttpResponse) -> String {
    let text: String = String::from_utf8_lossy(&resp.body).chars().take(300).collect();
    if (200..300).contains(&resp.status) {
        format!("the provider sent something that isn't JSON: {text}")
    } else {
        format!("HTTP {}: {}", resp.status, text.trim())
    }
}

/// The message of an error body (all three providers nest it under `error`).
fn error_message(v: &Value) -> Option<String> {
    let e = v.get("error")?;
    let msg = e.get("message").and_then(Value::as_str).or_else(|| e.as_str())?;
    Some(msg.chars().take(500).collect())
}

fn json_body(v: &Value) -> Option<String> {
    Some(v.to_string())
}

fn header(k: &str, v: &str) -> (String, String) {
    (k.to_string(), v.to_string())
}

// ---------------------------------------------------------------------------------------------
// OpenAI Chat Completions

fn openai_request(cfg: &Config, system: &str, turns: &[Turn], tools: &[ToolDef]) -> HttpRequest {
    let mut messages = vec![json!({"role": "system", "content": system})];
    for t in turns {
        match t {
            Turn::User(text) => messages.push(json!({"role": "user", "content": text})),
            Turn::Assistant { native: Some((Api::OpenAi, m)), .. } => messages.push(m.clone()),
            Turn::Assistant { text, calls, .. } => {
                let mut m = json!({"role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) }});
                if !calls.is_empty() {
                    m["tool_calls"] = calls
                        .iter()
                        .map(|c| json!({"id": c.id, "type": "function", "function": {"name": c.name, "arguments": c.args.to_string()}}))
                        .collect();
                }
                messages.push(m);
            }
            Turn::Results(rs) => {
                for r in rs {
                    messages.push(json!({"role": "tool", "tool_call_id": r.id, "content": r.content}));
                }
            }
        }
    }
    let tools: Vec<Value> = tools
        .iter()
        .map(|t| json!({"type": "function", "function": {"name": t.name, "description": t.description, "parameters": t.schema}}))
        .collect();
    let body = json!({"model": cfg.model.trim(), "messages": messages, "tools": tools});
    let mut headers = vec![header("Content-Type", "application/json")];
    if !cfg.key.trim().is_empty() {
        headers.push(header("Authorization", &format!("Bearer {}", cfg.key.trim())));
    }
    HttpRequest { method: "POST", url: format!("{}/chat/completions", cfg.base()), headers, body: json_body(&body) }
}

fn openai_reply(v: &Value) -> Result<Reply, String> {
    let choice = v.get("choices").and_then(|c| c.get(0)).ok_or("the reply has no choices")?;
    let msg = choice.get("message").ok_or("the reply has no message")?;
    let text = msg.get("content").and_then(Value::as_str).unwrap_or("").to_string();
    let calls = msg
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, c)| {
                    let f = c.get("function");
                    let args = f.and_then(|f| f.get("arguments"));
                    ToolCall {
                        id: c.get("id").and_then(Value::as_str).map_or_else(|| format!("call_{i}"), str::to_string),
                        name: f.and_then(|f| f.get("name")).and_then(Value::as_str).unwrap_or("").to_string(),
                        // Arguments come as JSON text; some servers send an object.
                        args: match args {
                            Some(Value::String(s)) => serde_json::from_str(s).unwrap_or_else(|_| json!({"_unparsed": s})),
                            Some(other) => other.clone(),
                            None => json!({}),
                        },
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let cut = match choice.get("finish_reason").and_then(Value::as_str) {
        Some("length") => Some("the reply reached its length limit".to_string()),
        Some("content_filter") => Some("the provider's content filter stopped the reply".to_string()),
        _ => None,
    };
    // Replayed as sent, minus fields some servers refuse in requests.
    let mut native = msg.clone();
    if let Some(o) = native.as_object_mut() {
        o.retain(|k, _| matches!(k.as_str(), "role" | "content" | "tool_calls" | "reasoning_content"));
    }
    Ok(Reply { text, calls, native, cut })
}

// ---------------------------------------------------------------------------------------------
// Anthropic Messages

/// Models that take the server-side refusal fallback (Claude API only).
fn takes_fallback(model: &str) -> bool {
    ["claude-opus-5", "claude-fable-5-1", "claude-sonnet-5-5"].iter().any(|m| model.starts_with(m))
}

fn anthropic_request(cfg: &Config, system: &str, turns: &[Turn], tools: &[ToolDef]) -> HttpRequest {
    let mut messages: Vec<Value> = Vec::new();
    // Consecutive turns of one role are merged into one message.
    let mut push = |role: &str, blocks: Vec<Value>| {
        if blocks.is_empty() {
            return;
        }
        if let Some(last) = messages.last_mut()
            && last["role"] == role
            && let Some(c) = last.get_mut("content").and_then(Value::as_array_mut)
        {
            c.extend(blocks);
            return;
        }
        messages.push(json!({"role": role, "content": blocks}));
    };
    for t in turns {
        match t {
            Turn::User(text) => push("user", vec![json!({"type": "text", "text": text})]),
            Turn::Assistant { native: Some((Api::Anthropic, content)), .. } => {
                push("assistant", content.as_array().cloned().unwrap_or_default());
            }
            Turn::Assistant { text, calls, .. } => {
                let mut blocks = Vec::new();
                if !text.trim().is_empty() {
                    blocks.push(json!({"type": "text", "text": text}));
                }
                for c in calls {
                    blocks.push(json!({"type": "tool_use", "id": c.id, "name": c.name, "input": object(&c.args)}));
                }
                push("assistant", blocks);
            }
            Turn::Results(rs) => push(
                "user",
                rs.iter().map(|r| json!({"type": "tool_result", "tool_use_id": r.id, "content": r.content, "is_error": r.error})).collect(),
            ),
        }
    }
    let tools: Vec<Value> = tools.iter().map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.schema})).collect();
    let model = cfg.model.trim();
    let mut body = json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "system": system,
        "messages": messages,
        "tools": tools,
        // Caches the system prompt, tools and history between turns.
        "cache_control": {"type": "ephemeral"},
    });
    let mut headers = vec![
        header("Content-Type", "application/json"),
        header("x-api-key", cfg.key.trim()),
        header("anthropic-version", "2023-06-01"),
        // Lets the browser app call the API directly (with the user's own key).
        header("anthropic-dangerous-direct-browser-access", "true"),
    ];
    // A request a model declines is retried on another (Claude API only).
    if cfg.base() == "https://api.anthropic.com" && takes_fallback(model) {
        body["fallbacks"] = json!("default");
        headers.push(header("anthropic-beta", "server-side-fallback-2026-07-01"));
    }
    HttpRequest { method: "POST", url: format!("{}/v1/messages", cfg.base()), headers, body: json_body(&body) }
}

fn anthropic_reply(v: &Value) -> Result<Reply, String> {
    let content = v.get("content").and_then(Value::as_array).ok_or("the reply has no content")?;
    let mut text = String::new();
    let mut calls = Vec::new();
    for b in content {
        match b.get("type").and_then(Value::as_str) {
            Some("text") => text.push_str(b.get("text").and_then(Value::as_str).unwrap_or("")),
            Some("tool_use") => calls.push(ToolCall {
                id: b.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                name: b.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                args: b.get("input").cloned().unwrap_or_else(|| json!({})),
            }),
            _ => {}
        }
    }
    let cut = match v.get("stop_reason").and_then(Value::as_str) {
        Some("max_tokens") => Some("the reply reached its length limit".to_string()),
        Some("refusal") => Some(
            v.pointer("/stop_details/explanation")
                .and_then(Value::as_str)
                .map_or_else(|| "the model declined this request".to_string(), |e| format!("the model declined this request: {e}")),
        ),
        _ => None,
    };
    // A refused reply's partial calls must not run.
    if v.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
        calls.clear();
    }
    Ok(Reply { text, calls, native: Value::Array(content.clone()), cut })
}

// ---------------------------------------------------------------------------------------------
// Gemini

/// Gemini takes a subset of JSON Schema: an object needs properties, so a free-form object
/// parameter is sent as JSON text instead (the tools accept either).
fn gemini_schema(v: &Value) -> Value {
    match v {
        Value::Object(o) => {
            let is_object = o.get("type").and_then(Value::as_str) == Some("object");
            let has_props = o.get("properties").and_then(Value::as_object).is_some_and(|p| !p.is_empty());
            if is_object && !has_props {
                let desc = o.get("description").and_then(Value::as_str).unwrap_or("A JSON object");
                return json!({"type": "string", "description": format!("{desc} (as JSON text)")});
            }
            let mut out = Map::new();
            for (k, v) in o {
                if k == "additionalProperties" || k == "$schema" {
                    continue;
                }
                let v = if k == "properties" {
                    Value::Object(v.as_object().map(|p| p.iter().map(|(k, v)| (k.clone(), gemini_schema(v))).collect()).unwrap_or_default())
                } else if k == "items" {
                    gemini_schema(v)
                } else {
                    v.clone()
                };
                out.insert(k.clone(), v);
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

/// The prefix of call ids made up for Gemini calls that came without one.
const MADE_UP: &str = "rustgis-call-";

fn gemini_request(cfg: &Config, system: &str, turns: &[Turn], tools: &[ToolDef]) -> HttpRequest {
    let mut contents = Vec::new();
    for t in turns {
        match t {
            Turn::User(text) => contents.push(json!({"role": "user", "parts": [{"text": text}]})),
            Turn::Assistant { native: Some((Api::Gemini, c)), .. } => contents.push(c.clone()),
            Turn::Assistant { text, calls, .. } => {
                let mut parts = Vec::new();
                if !text.trim().is_empty() {
                    parts.push(json!({"text": text}));
                }
                for c in calls {
                    parts.push(json!({"functionCall": {"name": c.name, "args": object(&c.args)}}));
                }
                if !parts.is_empty() {
                    contents.push(json!({"role": "model", "parts": parts}));
                }
            }
            Turn::Results(rs) => {
                let parts: Vec<Value> = rs
                    .iter()
                    .map(|r| {
                        let response = if r.error { json!({"error": r.content}) } else { json!({"result": r.content}) };
                        let mut f = json!({"name": r.name, "response": response});
                        // Ids Gemini gave (not the ones made up for it) go back with the result.
                        if !r.id.starts_with(MADE_UP) {
                            f["id"] = json!(r.id);
                        }
                        json!({"functionResponse": f})
                    })
                    .collect();
                contents.push(json!({"role": "user", "parts": parts}));
            }
        }
    }
    let decls: Vec<Value> =
        tools.iter().map(|t| json!({"name": t.name, "description": t.description, "parameters": gemini_schema(&t.schema)})).collect();
    let body = json!({
        "systemInstruction": {"parts": [{"text": system}]},
        "contents": contents,
        "tools": [{"functionDeclarations": decls}],
    });
    let model = cfg.model.trim().trim_start_matches("models/");
    HttpRequest {
        method: "POST",
        url: format!("{}/models/{}:generateContent", cfg.base(), model),
        headers: vec![header("Content-Type", "application/json"), header("x-goog-api-key", cfg.key.trim())],
        body: json_body(&body),
    }
}

fn gemini_reply(v: &Value) -> Result<Reply, String> {
    let Some(cand) = v.get("candidates").and_then(|c| c.get(0)) else {
        let why = v.pointer("/promptFeedback/blockReason").and_then(Value::as_str).unwrap_or("no reason given");
        return Err(format!("Gemini sent no answer ({why})"));
    };
    let content = cand.get("content").cloned().unwrap_or_else(|| json!({"role": "model", "parts": []}));
    let mut text = String::new();
    let mut calls = Vec::new();
    for (i, p) in content.get("parts").and_then(Value::as_array).into_iter().flatten().enumerate() {
        if p.get("thought").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if let Some(t) = p.get("text").and_then(Value::as_str) {
            text.push_str(t);
        }
        if let Some(f) = p.get("functionCall") {
            calls.push(ToolCall {
                id: f.get("id").and_then(Value::as_str).map_or_else(|| format!("{MADE_UP}{i}"), str::to_string),
                name: f.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                args: f.get("args").cloned().unwrap_or_else(|| json!({})),
            });
        }
    }
    let cut = match cand.get("finishReason").and_then(Value::as_str) {
        Some("MAX_TOKENS") => Some("the reply reached its length limit".to_string()),
        Some(r @ ("SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII")) => {
            Some(format!("Gemini stopped the reply ({r})"))
        }
        _ => None,
    };
    let mut native = content;
    if native.get("role").is_none() {
        native["role"] = json!("model");
    }
    Ok(Reply { text, calls, native, cut })
}

/// `v` if it is an object, else an empty one (providers require objects for tool input).
fn object(v: &Value) -> Value {
    if v.is_object() { v.clone() } else { json!({}) }
}

// ---------------------------------------------------------------------------------------------
// Model lists

/// The request listing the provider's models.
pub fn models_request(cfg: &Config) -> HttpRequest {
    let key = cfg.key.trim();
    match cfg.api() {
        Api::OpenAi => {
            let mut headers = Vec::new();
            if !key.is_empty() {
                headers.push(header("Authorization", &format!("Bearer {key}")));
            }
            HttpRequest { method: "GET", url: format!("{}/models", cfg.base()), headers, body: None }
        }
        Api::Anthropic => HttpRequest {
            method: "GET",
            url: format!("{}/v1/models?limit=1000", cfg.base()),
            headers: vec![
                header("x-api-key", key),
                header("anthropic-version", "2023-06-01"),
                header("anthropic-dangerous-direct-browser-access", "true"),
            ],
            body: None,
        },
        Api::Gemini => HttpRequest {
            method: "GET",
            url: format!("{}/models?pageSize=1000", cfg.base()),
            headers: vec![header("x-goog-api-key", key)],
            body: None,
        },
    }
}

/// The model ids in a model list, sorted.
pub fn parse_models(api: Api, resp: &HttpResponse) -> Result<Vec<String>, String> {
    let v: Value = serde_json::from_slice(&resp.body).map_err(|_| http_error(resp))?;
    if !(200..300).contains(&resp.status) {
        return Err(error_message(&v).unwrap_or_else(|| http_error(resp)));
    }
    let mut ids: Vec<String> = match api {
        Api::OpenAi | Api::Anthropic => v
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| m.get("id").and_then(Value::as_str))
            .map(str::to_string)
            .collect(),
        Api::Gemini => v
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|m| {
                m.get("supportedGenerationMethods")
                    .and_then(Value::as_array)
                    .is_some_and(|a| a.iter().any(|x| x.as_str() == Some("generateContent")))
            })
            .filter_map(|m| m.get("name").and_then(Value::as_str))
            .map(|n| n.trim_start_matches("models/").to_string())
            .collect(),
    };
    ids.sort();
    ids.dedup();
    ids.truncate(2000);
    Ok(ids)
}
