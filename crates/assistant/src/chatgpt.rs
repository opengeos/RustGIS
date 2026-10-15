//! Signing in with a ChatGPT account, the way the OpenAI Codex CLI does, so a ChatGPT Plus/Pro
//! subscription can drive the assistant without an API key.
//!
//! The sign-in is OAuth 2.0 authorization code with PKCE in the system browser, against
//! `https://auth.openai.com` with Codex's public client id, redirected to a short-lived server on
//! this computer; the tokens come back as a JWT access token (sent as a bearer token to the Codex
//! backend, `https://chatgpt.com/backend-api/codex`), an id token whose
//! `https://api.openai.com/auth` claim names the account's workspace and plan, and a refresh
//! token. An existing Codex CLI sign-in (`~/.codex/auth.json`) can be imported instead.
//!
//! The parameters follow the Codex CLI source (`codex-rs/login/src/server.rs`,
//! `oauth/authorization.rs`, `oauth/client.rs`, `auth/manager.rs`, `token_data.rs`). The backend
//! is OpenAI's own, not a public API: it may change, and using it is subject to OpenAI's terms
//! for ChatGPT subscriptions.
//!
//! The pure parts (reading tokens, building the refresh and exchange requests, deciding when to
//! refresh) are plain data for the tests; the file and the local callback server are desktop
//! only. Tokens are never logged, and the `Debug` form of [`Tokens`] hides them.

use base64::Engine;
use serde_json::{Value, json};

use crate::provider::{HttpRequest, HttpResponse};

/// The OpenAI sign-in service.
pub const ISSUER: &str = "https://auth.openai.com";
/// The Codex CLI's public OAuth client id.
pub const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
/// The ChatGPT backend the Codex CLI talks to (the Responses API).
pub const BACKEND: &str = "https://chatgpt.com/backend-api/codex";
/// The scopes Codex asks for (`offline_access` is the refresh token).
pub const SCOPES: &str = "openid profile email offline_access api.connectors.read api.connectors.invoke";
/// The redirect ports registered for the client id, in order of preference.
pub const PORTS: [u16; 2] = [1455, 1457];
/// Access tokens are refreshed this long before they expire (as Codex does).
pub const REFRESH_MARGIN: i64 = 5 * 60;

/// A signed-in account's tokens, in the shape of the Codex CLI's `auth.json`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Tokens {
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    /// The ChatGPT workspace (`chatgpt_account_id`), sent as `chatgpt-account-id`.
    pub account_id: String,
    /// When the tokens were last obtained (RFC 3339, informational).
    pub last_refresh: String,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens").field("account_id", &self.account_id).field("last_refresh", &self.last_refresh).finish_non_exhaustive()
    }
}

/// What the id token says about the account.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Account {
    pub email: Option<String>,
    /// The ChatGPT plan (`plus`, `pro`, `team`, `free`…).
    pub plan: Option<String>,
    pub account_id: Option<String>,
    /// When the token expires (Unix seconds).
    pub expires: Option<i64>,
}

impl Account {
    /// "name@example.com (Plus)" or what is known.
    pub fn label(&self) -> String {
        let plan = self.plan.as_deref().map(plan_label).unwrap_or_default();
        match (&self.email, plan.is_empty()) {
            (Some(e), false) => format!("{e} ({plan})"),
            (Some(e), true) => e.clone(),
            (None, false) => plan,
            (None, true) => "signed in".to_string(),
        }
    }
}

fn plan_label(plan: &str) -> String {
    let mut c = plan.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The payload of a JWT, unverified (the server verifies tokens; this only reads what they say).
fn jwt_payload(jwt: &str) -> Option<Value> {
    let mut parts = jwt.split('.');
    let (h, p, s) = (parts.next()?, parts.next()?, parts.next()?);
    if h.is_empty() || p.is_empty() || s.is_empty() || parts.next().is_some() {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(p).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// What a token says about its account (an id token has the email and plan; an access token
/// the workspace and expiry).
pub fn claims(jwt: &str) -> Account {
    let Some(v) = jwt_payload(jwt) else { return Account::default() };
    let auth = v.get("https://api.openai.com/auth");
    let str_of = |x: Option<&Value>| x.and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    Account {
        email: str_of(v.get("email")).or_else(|| str_of(v.pointer("/https:~1~1api.openai.com~1profile/email"))),
        plan: str_of(auth.and_then(|a| a.get("chatgpt_plan_type"))),
        account_id: str_of(auth.and_then(|a| a.get("chatgpt_account_id"))),
        expires: v.get("exp").and_then(Value::as_i64),
    }
}

impl Tokens {
    /// Reads Codex's `auth.json` shape: `{"auth_mode": "chatgpt", "tokens": {…}, "last_refresh"}`.
    pub fn from_json(v: &Value) -> Result<Tokens, String> {
        let mode = v.get("auth_mode").and_then(Value::as_str).unwrap_or("chatgpt");
        if mode != "chatgpt" {
            return Err(format!("this is a {mode} sign-in, not a ChatGPT account"));
        }
        let t = v.get("tokens").ok_or("no tokens in the sign-in")?;
        let field = |k: &str| t.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
        let mut tokens = Tokens {
            id_token: field("id_token"),
            access_token: field("access_token"),
            refresh_token: field("refresh_token"),
            account_id: field("account_id"),
            last_refresh: v.get("last_refresh").and_then(Value::as_str).unwrap_or("").to_string(),
        };
        if tokens.access_token.is_empty() {
            return Err("the sign-in has no access token".into());
        }
        if tokens.account_id.is_empty() {
            tokens.account_id = tokens.account().account_id.unwrap_or_default();
        }
        Ok(tokens)
    }

    /// The same shape, for the file.
    pub fn to_json(&self) -> Value {
        json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "id_token": self.id_token,
                "access_token": self.access_token,
                "refresh_token": self.refresh_token,
                "account_id": self.account_id,
            },
            "last_refresh": self.last_refresh,
        })
    }

    /// The account: email and plan from the id token, workspace and expiry from the access token.
    pub fn account(&self) -> Account {
        let id = claims(&self.id_token);
        let access = claims(&self.access_token);
        Account {
            email: id.email.or(access.email),
            plan: id.plan.or(access.plan),
            account_id: access.account_id.or(id.account_id).or_else(|| Some(self.account_id.clone()).filter(|a| !a.is_empty())),
            expires: access.expires,
        }
    }

    /// Whether the access token expires within [`REFRESH_MARGIN`] of `now` (Unix seconds), or
    /// says nothing about when it does.
    pub fn expires_soon(&self, now: i64) -> bool {
        claims(&self.access_token).expires.is_none_or(|exp| exp - REFRESH_MARGIN <= now)
    }

    /// Puts this account's credentials on a request (replacing any there).
    pub fn authorize(&self, mut req: HttpRequest) -> HttpRequest {
        req.headers.retain(|(k, _)| !k.eq_ignore_ascii_case("authorization") && !k.eq_ignore_ascii_case("chatgpt-account-id"));
        req.headers.push(("Authorization".to_string(), format!("Bearer {}", self.access_token)));
        if !self.account_id.is_empty() {
            req.headers.push(("chatgpt-account-id".to_string(), self.account_id.clone()));
        }
        req
    }
}

/// The request renewing the tokens (Codex sends this grant as JSON).
pub fn refresh_request(t: &Tokens) -> HttpRequest {
    let body = json!({"grant_type": "refresh_token", "client_id": CLIENT_ID, "refresh_token": t.refresh_token});
    HttpRequest {
        method: "POST",
        url: format!("{ISSUER}/oauth/token"),
        headers: vec![("Content-Type".to_string(), "application/json".to_string())],
        body: Some(body.to_string()),
    }
}

/// The tokens a refresh gave: new ones where sent, the old otherwise.
pub fn parse_refresh(old: &Tokens, resp: &HttpResponse, now: &str) -> Result<Tokens, String> {
    let v = token_response(resp, "renewing the sign-in")?;
    let mut t = old.clone();
    let field = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    if let Some(a) = field("access_token") {
        t.access_token = a;
    }
    if let Some(i) = field("id_token") {
        t.id_token = i;
    }
    if let Some(r) = field("refresh_token") {
        t.refresh_token = r;
    }
    if let Some(a) = t.account().account_id {
        t.account_id = a;
    }
    t.last_refresh = now.to_string();
    Ok(t)
}

/// A token endpoint's reply, or why it refused.
fn token_response(resp: &HttpResponse, what: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_slice(&resp.body).unwrap_or(Value::Null);
    if !(200..300).contains(&resp.status) {
        let code = v.get("error").and_then(|e| e.as_str().or_else(|| e.get("code").and_then(Value::as_str))).unwrap_or("");
        let desc = v.get("error_description").and_then(Value::as_str).unwrap_or("");
        let why = match code {
            "invalid_grant" | "refresh_token_expired" | "refresh_token_reused" | "refresh_token_invalidated" => {
                "the sign-in has expired or was revoked; sign in again".to_string()
            }
            "" if desc.is_empty() => format!("HTTP {}", resp.status),
            _ => format!("{code} {desc}").trim().to_string(),
        };
        return Err(format!("{what}: {why}"));
    }
    if !v.is_object() {
        return Err(format!("{what}: the reply isn't JSON"));
    }
    Ok(v)
}

/// Percent-encodes a query value.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decodes a query value (`+` is a space).
pub fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes.get(i) {
            Some(b'+') => out.push(b' '),
            Some(b'%') => {
                let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(b) => {
                        out.push(b);
                        i += 2;
                    }
                    None => out.push(b'%'),
                }
            }
            Some(b) => out.push(*b),
            None => break,
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The (decoded) parameters of a query string.
pub fn query_params(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| p.split_once('=').unwrap_or((p, "")))
        .map(|(k, v)| (urldecode(k), urldecode(v)))
        .collect()
}

/// The PKCE challenge (S256) of a verifier.
pub fn pkce_challenge(verifier: &str) -> String {
    use sha2::Digest;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()))
}

/// The page to open in the browser.
pub fn authorize_url(redirect_uri: &str, challenge: &str, state: &str) -> String {
    let params = [
        ("response_type", "code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", redirect_uri),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
        ("scope", SCOPES),
        // What Codex adds: the workspace in the id token, and its simplified sign-in pages.
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("originator", crate::provider::ORIGINATOR),
    ];
    let q: Vec<String> = params.iter().map(|(k, v)| format!("{k}={}", urlencode(v))).collect();
    format!("{ISSUER}/oauth/authorize?{}", q.join("&"))
}

/// The request exchanging the browser's code for tokens (form-encoded, as Codex sends it).
pub fn exchange_request(code: &str, redirect_uri: &str, verifier: &str) -> HttpRequest {
    let params = [
        ("grant_type", "authorization_code"),
        ("client_id", CLIENT_ID),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", verifier),
    ];
    let body: Vec<String> = params.iter().map(|(k, v)| format!("{k}={}", urlencode(v))).collect();
    HttpRequest {
        method: "POST",
        url: format!("{ISSUER}/oauth/token"),
        headers: vec![("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string())],
        body: Some(body.join("&")),
    }
}

/// The tokens an exchange gave.
pub fn parse_exchange(resp: &HttpResponse, now: &str) -> Result<Tokens, String> {
    let v = token_response(resp, "signing in")?;
    let field = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut t = Tokens {
        id_token: field("id_token"),
        access_token: field("access_token"),
        refresh_token: field("refresh_token"),
        account_id: String::new(),
        last_refresh: now.to_string(),
    };
    if t.access_token.is_empty() {
        return Err("signing in: the reply has no access token".into());
    }
    t.account_id = t.account().account_id.unwrap_or_default();
    Ok(t)
}

/// `secs` since the Unix epoch as RFC 3339 (`2026-10-14T13:34:16Z`).
pub fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

// ---------------------------------------------------------------------------------------------
// The desktop and the command line: the token file, the Codex import, refreshing, signing in.

#[cfg(not(target_arch = "wasm32"))]
pub use native::*;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use base64::Engine;

    use super::{
        Account, PORTS, Tokens, authorize_url, exchange_request, parse_exchange, parse_refresh, query_params, refresh_request, rfc3339,
    };
    use crate::http;
    use crate::provider::{HttpRequest, HttpResponse};

    /// The file name of the sign-in, in the app's settings folder.
    const FILE: &str = "chatgpt-auth.json";

    /// How long the browser has to come back.
    const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

    /// The settings folder the desktop app keeps its preferences in.
    pub fn settings_dir() -> Option<PathBuf> {
        if cfg!(target_os = "windows") {
            std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("RustGIS"))
        } else if cfg!(target_os = "macos") {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/RustGIS"))
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
                .map(|d| d.join("rustgis"))
        }
    }

    /// Where the sign-in is kept.
    pub fn auth_path() -> Option<PathBuf> {
        settings_dir().map(|d| d.join(FILE))
    }

    /// Where the Codex CLI keeps its sign-in (`$CODEX_HOME/auth.json`, else `~/.codex/auth.json`).
    pub fn codex_path() -> Option<PathBuf> {
        let home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(|h| PathBuf::from(h).join(".codex")));
        home.map(|d| d.join("auth.json"))
    }

    fn now() -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
    }

    fn read_tokens(path: &std::path::Path) -> Result<Tokens, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let v: serde_json::Value = serde_json::from_str(&text).map_err(|_| format!("{}: not JSON", path.display()))?;
        Tokens::from_json(&v).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The account signed in on this computer, if any.
    pub fn load() -> Option<Tokens> {
        read_tokens(&auth_path()?).ok()
    }

    /// Keeps the sign-in, readable by this user only.
    pub fn save(t: &Tokens) -> Result<(), String> {
        let path = auth_path().ok_or("no settings folder")?;
        let dir = path.parent().ok_or("no settings folder")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let tmp = dir.join(format!("{FILE}.tmp"));
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        // On Windows the settings folder (`%APPDATA%`) is private to the user already.
        let written =
            opts.open(&tmp).and_then(|mut f| f.write_all(t.to_json().to_string().as_bytes())).and_then(|_| std::fs::rename(&tmp, &path));
        written.map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Signs out: forgets the tokens on this computer (they stay valid at OpenAI until they
    /// expire; revoke them from your ChatGPT account's settings if needed).
    pub fn forget() -> Result<(), String> {
        let Some(path) = auth_path() else { return Ok(()) };
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Whether a Codex CLI sign-in exists to import.
    pub fn codex_available() -> bool {
        codex_path().is_some_and(|p| p.is_file())
    }

    /// Copies the Codex CLI's sign-in (the tokens stay Codex's too: both refresh them, and the
    /// service accepts either refresh until one is used).
    pub fn import_codex() -> Result<Tokens, String> {
        let path = codex_path().ok_or("no home folder, so no Codex sign-in to import")?;
        let t = read_tokens(&path)?;
        save(&t)?;
        Ok(t)
    }

    /// Renews the tokens with `send` and keeps the result.
    fn refresh(t: &Tokens, send: &mut dyn FnMut(&HttpRequest) -> Result<HttpResponse, String>) -> Result<Tokens, String> {
        if t.refresh_token.is_empty() {
            return Err("the sign-in can't be renewed (no refresh token); sign in again".into());
        }
        let resp = send(&refresh_request(t))?;
        let fresh = parse_refresh(t, &resp, &rfc3339(now()))?;
        save(&fresh)?;
        Ok(fresh)
    }

    /// The signed-in account's tokens, renewed with `send` when they are about to expire.
    pub fn fresh(send: &mut dyn FnMut(&HttpRequest) -> Result<HttpResponse, String>) -> Result<Tokens, String> {
        let t = load().ok_or("not signed in to ChatGPT")?;
        if t.expires_soon(now()) { refresh(&t, send) } else { Ok(t) }
    }

    /// Sends a request as the signed-in account (blocking): the tokens are renewed first when
    /// about to expire, and once more if the backend still refuses them.
    pub fn send(req: &HttpRequest) -> Result<HttpResponse, String> {
        let mut transport = http::send;
        let t = fresh(&mut transport)?;
        let resp = http::send(&t.authorize(req.clone()))?;
        if resp.status != 401 {
            return Ok(resp);
        }
        let t = refresh(&t, &mut transport)?;
        http::send(&t.authorize(req.clone()))
    }

    /// The account signed in on this computer, as its tokens describe it.
    pub fn account() -> Option<Account> {
        load().map(|t| t.account())
    }

    // -----------------------------------------------------------------------------------------
    // Signing in: the browser flow

    type Outcome = Arc<Mutex<Option<Result<Tokens, String>>>>;

    /// A sign-in under way: open [`Login::url`] in the browser, then [`Login::poll`] until the
    /// tokens arrive. Dropping it stops the callback server.
    pub struct Login {
        pub url: String,
        pub port: u16,
        outcome: Outcome,
        cancel: Arc<AtomicBool>,
    }

    impl Login {
        /// The outcome, once there is one (taken: later polls say nothing).
        pub fn poll(&self) -> Option<Result<Tokens, String>> {
            self.outcome.lock().ok().and_then(|mut o| o.take())
        }

        pub fn cancel(&self) {
            self.cancel.store(true, Ordering::Relaxed);
        }
    }

    impl Drop for Login {
        fn drop(&mut self) {
            self.cancel();
        }
    }

    fn random_token() -> Result<String, String> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|e| format!("no random numbers: {e}"))?;
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
    }

    /// Starts a sign-in: binds the callback server and makes the page to open. The tokens are
    /// saved when they arrive.
    pub fn login() -> Result<Login, String> {
        let verifier = random_token()?;
        let state = random_token()?;
        let (listener, port) = PORTS
            .iter()
            .find_map(|p| TcpListener::bind(("127.0.0.1", *p)).ok().map(|l| (l, *p)))
            .ok_or_else(|| format!("ports {} and {} are in use (another sign-in, or the Codex CLI's?)", PORTS[0], PORTS[1]))?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let redirect_uri = format!("http://127.0.0.1:{port}/auth/callback");
        let url = authorize_url(&redirect_uri, &super::pkce_challenge(&verifier), &state);
        let outcome: Outcome = Arc::default();
        let cancel = Arc::new(AtomicBool::new(false));
        let (out, stop) = (outcome.clone(), cancel.clone());
        std::thread::spawn(move || {
            let result = serve(&listener, &stop, &redirect_uri, &verifier, &state);
            if let Ok(mut o) = out.lock() {
                *o = Some(result);
            }
        });
        Ok(Login { url, port, outcome, cancel })
    }

    /// Answers requests until the callback arrives (or the sign-in is cancelled).
    fn serve(listener: &TcpListener, cancel: &AtomicBool, redirect_uri: &str, verifier: &str, state: &str) -> Result<Tokens, String> {
        let start = std::time::Instant::now();
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("the sign-in was cancelled".into());
            }
            if start.elapsed() > LOGIN_TIMEOUT {
                return Err("the sign-in timed out; try again".into());
            }
            let mut stream = match listener.accept() {
                Ok((s, _)) => s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                Err(e) => return Err(format!("the sign-in server failed: {e}")),
            };
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let Some(target) = request_target(&mut stream) else {
                respond(&mut stream, 400, "Bad request");
                continue;
            };
            let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
            if path != "/auth/callback" {
                respond(&mut stream, 404, "Not found");
                continue;
            }
            let params = query_params(query);
            let get = |k: &str| params.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str()).unwrap_or("");
            // ChatGPT may append onboarding metadata to the state.
            if !get("state").starts_with(state) || get("state").is_empty() {
                respond(&mut stream, 400, "State mismatch");
                continue;
            }
            if !get("error").is_empty() {
                let why = format!("{} {}", get("error"), get("error_description")).trim().to_string();
                respond(&mut stream, 200, &page("Sign-in failed", &why));
                return Err(format!("the sign-in was refused: {why}"));
            }
            let code = get("code");
            if code.is_empty() {
                respond(&mut stream, 400, "Missing authorization code");
                continue;
            }
            let result = http::send(&exchange_request(code, redirect_uri, verifier)).and_then(|r| parse_exchange(&r, &rfc3339(now())));
            match result {
                Ok(t) => {
                    let saved = save(&t);
                    respond(&mut stream, 200, &page("Signed in", "RustGIS is signed in to your ChatGPT account. You can close this tab."));
                    saved?;
                    return Ok(t);
                }
                Err(e) => {
                    respond(&mut stream, 200, &page("Sign-in failed", &e));
                    return Err(e);
                }
            }
        }
    }

    /// The request target (`/auth/callback?code=…`) of an HTTP request, read up to its headers' end.
    fn request_target(stream: &mut TcpStream) -> Option<String> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(chunk.get(..n)?),
                Err(_) => return None,
            }
        }
        let line = buf.split(|b| *b == b'\n').next()?;
        let line = std::str::from_utf8(line).ok()?.trim();
        let mut parts = line.split(' ');
        let (method, target) = (parts.next()?, parts.next()?);
        (method == "GET").then(|| target.to_string())
    }

    fn respond(stream: &mut TcpStream, status: u16, body: &str) {
        let reason = match status {
            200 => "OK",
            400 => "Bad Request",
            404 => "Not Found",
            _ => "Error",
        };
        let kind = if body.starts_with("<!DOCTYPE") { "text/html; charset=utf-8" } else { "text/plain; charset=utf-8" };
        let head =
            format!("HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        let _ = stream.write_all(head.as_bytes()).and_then(|_| stream.write_all(body.as_bytes())).and_then(|_| stream.flush());
    }

    fn escape(s: &str) -> String {
        s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    }

    /// The page the browser shows at the end.
    fn page(title: &str, text: &str) -> String {
        format!(
            "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>RustGIS: {t}</title></head>\
<body style=\"font-family: system-ui, sans-serif; margin: 3em auto; max-width: 32em; text-align: center\">\
<h1>{t}</h1><p>{b}</p></body></html>",
            t = escape(title),
            b = escape(text)
        )
    }
}
