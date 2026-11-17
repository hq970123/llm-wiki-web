//! Single-account authentication.
//!
//! * First visit with no account -> the login page shows "set password",
//!   which creates the only account (argon2id hash in DATA_DIR/auth.json).
//! * `LLM_WIKI_PASSWORD` env pre-sets (and pins) the password instead.
//! * Sessions: random 256-bit token in an HttpOnly cookie; only the SHA-256
//!   of the token is stored (DATA_DIR/sessions.json) so restarts keep users
//!   signed in. Sliding 30-day expiry.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::AppState;

const COOKIE: &str = "llmwiki_session";
const SESSION_TTL_SECS: u64 = 30 * 24 * 3600;
const MIN_PASSWORD_LEN: usize = 8;

#[derive(Serialize, Deserialize, Default)]
struct AuthFile {
    password_hash: Option<String>,
}

pub struct Auth {
    file: PathBuf,
    sessions_file: PathBuf,
    env_hash: Option<String>,
    inner: Mutex<AuthInner>,
}

struct AuthInner {
    password_hash: Option<String>,
    /// sha256(token) hex -> expiry (unix secs)
    sessions: HashMap<String, u64>,
    failures: Vec<Instant>,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn hash_token(token: &str) -> String {
    let mut h = Sha256::new();
    h.update(token.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn hash_password(pw: &str) -> Result<String, String> {
    let mut salt_bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt_bytes);
    let salt = SaltString::encode_b64(&salt_bytes).map_err(|e| e.to_string())?;
    Argon2::default()
        .hash_password(pw.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

fn verify_password(pw: &str, hash: &str) -> bool {
    PasswordHash::new(hash)
        .map(|parsed| Argon2::default().verify_password(pw.as_bytes(), &parsed).is_ok())
        .unwrap_or(false)
}

fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path)
}

impl Auth {
    pub fn load(data_dir: &Path) -> Self {
        let file = data_dir.join("auth.json");
        let sessions_file = data_dir.join("sessions.json");
        // One-shot reset: when LLM_WIKI_RESET_AUTH is set to a value not seen
        // before, forget the stored password (and sessions) so the next visitor
        // runs first-time setup again. The value is remembered in
        // DATA_DIR/auth-reset.done, so restarts do not reset again.
        if let Ok(tok) = std::env::var("LLM_WIKI_RESET_AUTH") {
            let tok = tok.trim().to_string();
            let marker = data_dir.join("auth-reset.done");
            let seen = std::fs::read_to_string(&marker).unwrap_or_default();
            if !tok.is_empty() && seen.trim() != tok {
                let _ = std::fs::remove_file(&file);
                let _ = std::fs::remove_file(&sessions_file);
                let _ = std::fs::write(&marker, &tok);
                eprintln!("[auth] password reset by LLM_WIKI_RESET_AUTH");
            }
        }
        let stored: AuthFile = std::fs::read_to_string(&file)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let env_hash = std::env::var("LLM_WIKI_PASSWORD")
            .ok()
            .filter(|p| !p.is_empty())
            .map(|p| hash_password(&p).expect("hash LLM_WIKI_PASSWORD"));
        let mut sessions: HashMap<String, u64> = std::fs::read_to_string(&sessions_file)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let t = now();
        sessions.retain(|_, exp| *exp > t);
        let mut stored = stored;
        if env_hash.is_some() {
            eprintln!("[auth] password pinned by LLM_WIKI_PASSWORD");
        } else if stored.password_hash.is_none() {
            // No account yet: start with the default password "admin"
            // (change it after signing in via "Change password").
            let default_pw = std::env::var("LLM_WIKI_DEFAULT_PASSWORD")
                .ok()
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| "admin".to_string());
            match hash_password(&default_pw) {
                Ok(h) => {
                    if let Ok(data) = serde_json::to_vec_pretty(&AuthFile { password_hash: Some(h.clone()) }) {
                        let _ = write_private(&file, &data);
                    }
                    stored.password_hash = Some(h);
                    eprintln!("[auth] no account yet — default password set; change it after signing in");
                }
                Err(e) => eprintln!("[auth] cannot set default password: {e}"),
            }
        }
        Auth {
            file,
            sessions_file,
            env_hash,
            inner: Mutex::new(AuthInner {
                password_hash: stored.password_hash,
                sessions,
                failures: Vec::new(),
            }),
        }
    }

    pub fn password_from_env(&self) -> bool {
        self.env_hash.is_some()
    }

    fn effective_hash(&self, inner: &AuthInner) -> Option<String> {
        self.env_hash.clone().or_else(|| inner.password_hash.clone())
    }

    pub fn configured(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        self.effective_hash(&inner).is_some()
    }

    fn persist_sessions(&self, inner: &AuthInner) {
        if let Ok(s) = serde_json::to_vec(&inner.sessions) {
            let _ = write_private(&self.sessions_file, &s);
        }
    }

    fn new_session(&self) -> String {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let mut inner = self.inner.lock().unwrap();
        inner.sessions.insert(hash_token(&token), now() + SESSION_TTL_SECS);
        self.persist_sessions(&inner);
        token
    }

    pub fn request_has_session(&self, headers: &HeaderMap) -> bool {
        let Some(token) = cookie_value(headers, COOKIE) else {
            return false;
        };
        let key = hash_token(&token);
        let mut inner = self.inner.lock().unwrap();
        let t = now();
        match inner.sessions.get(&key).copied() {
            Some(exp) if exp > t => {
                // Sliding expiry, persisted at most ~daily per session.
                if exp < t + SESSION_TTL_SECS - 24 * 3600 {
                    inner.sessions.insert(key, t + SESSION_TTL_SECS);
                    self.persist_sessions(&inner);
                }
                true
            }
            Some(_) => {
                inner.sessions.remove(&key);
                false
            }
            None => false,
        }
    }

    fn drop_session(&self, headers: &HeaderMap) {
        if let Some(token) = cookie_value(headers, COOKIE) {
            let mut inner = self.inner.lock().unwrap();
            inner.sessions.remove(&hash_token(&token));
            self.persist_sessions(&inner);
        }
    }

    fn drop_all_sessions(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.sessions.clear();
        self.persist_sessions(&inner);
    }

    /// Crude brute-force brake: at most 10 failures per 10 minutes.
    fn throttled(&self) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let cutoff = Instant::now() - Duration::from_secs(600);
        inner.failures.retain(|t| *t > cutoff);
        inner.failures.len() >= 10
    }

    fn record_failure(&self) {
        self.inner.lock().unwrap().failures.push(Instant::now());
    }

    fn check(&self, pw: &str) -> bool {
        let hash = {
            let inner = self.inner.lock().unwrap();
            self.effective_hash(&inner)
        };
        hash.map(|h| verify_password(pw, &h)).unwrap_or(false)
    }

    fn set_password(&self, pw: &str, only_if_unset: bool) -> Result<(), String> {
        let hash = hash_password(pw)?;
        let mut inner = self.inner.lock().unwrap();
        if only_if_unset && self.effective_hash(&inner).is_some() {
            return Err("An account already exists".into());
        }
        let data = serde_json::to_vec_pretty(&AuthFile {
            password_hash: Some(hash.clone()),
        })
        .map_err(|e| e.to_string())?;
        write_private(&self.file, &data).map_err(|e| format!("cannot write auth.json: {e}"))?;
        inner.password_hash = Some(hash);
        Ok(())
    }
}

pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    for v in headers.get_all(header::COOKIE) {
        let Ok(s) = v.to_str() else { continue };
        for part in s.split(';') {
            let part = part.trim();
            if let Some(rest) = part.strip_prefix(name) {
                if let Some(val) = rest.strip_prefix('=') {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

/// HTTPS (directly or via a TLS-terminating proxy such as HF Spaces)?
fn is_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(',').next().unwrap_or("").trim().eq_ignore_ascii_case("https"))
        .unwrap_or(false)
        || std::env::var("LLM_WIKI_SECURE_COOKIE").map(|v| v == "1").unwrap_or(false)
}

fn session_cookie(headers: &HeaderMap, token: &str, max_age: u64) -> HeaderValue {
    // Top-level deployments (the default) get Secure + SameSite=Lax over
    // HTTPS. Only when the app must work inside a third-party iframe (e.g.
    // the Hugging Face Spaces page; LLM_WIKI_EMBED=1) is SameSite=None +
    // Partitioned used; CSRF is also covered by the mandatory X-LLM-Wiki
    // header on state-changing calls.
    let attrs = if !is_https(headers) {
        "; SameSite=Lax"
    } else if super::embed_mode() {
        "; Secure; SameSite=None; Partitioned"
    } else {
        "; Secure; SameSite=Lax"
    };
    HeaderValue::from_str(&format!(
        "{COOKIE}={token}; Path=/; HttpOnly; Max-Age={max_age}{attrs}"
    ))
    .unwrap()
}

#[derive(Deserialize)]
pub struct PasswordBody {
    password: String,
}

#[derive(Deserialize)]
pub struct ChangePasswordBody {
    current: String,
    #[serde(rename = "new")]
    new_password: String,
}

fn bad(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({ "error": msg }))).into_response()
}

fn logged_in(headers: &HeaderMap, st: &AppState) -> Response {
    let token = st.auth.new_session();
    let mut res = Json(json!({ "ok": true })).into_response();
    res.headers_mut()
        .append(header::SET_COOKIE, session_cookie(headers, &token, SESSION_TTL_SECS));
    res
}

pub async fn handle_status(State(st): State<AppState>, headers: HeaderMap) -> Response {
    Json(json!({
        "configured": st.auth.configured(),
        "authenticated": st.auth.request_has_session(&headers),
        "passwordFromEnv": st.auth.password_from_env(),
    }))
    .into_response()
}

pub async fn handle_setup(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PasswordBody>,
) -> Response {
    if !headers.contains_key("x-llm-wiki") {
        return bad(StatusCode::FORBIDDEN, "missing X-LLM-Wiki header");
    }
    if st.auth.configured() {
        return bad(StatusCode::CONFLICT, "An account already exists");
    }
    if body.password.chars().count() < MIN_PASSWORD_LEN {
        return bad(StatusCode::BAD_REQUEST, "Password must be at least 8 characters");
    }
    if let Err(e) = st.auth.set_password(&body.password, true) {
        return bad(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }
    eprintln!("[auth] account created");
    logged_in(&headers, &st)
}

pub async fn handle_login(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<PasswordBody>,
) -> Response {
    if !headers.contains_key("x-llm-wiki") {
        return bad(StatusCode::FORBIDDEN, "missing X-LLM-Wiki header");
    }
    if !st.auth.configured() {
        return bad(StatusCode::CONFLICT, "No account yet — set a password first");
    }
    if st.auth.throttled() {
        return bad(StatusCode::TOO_MANY_REQUESTS, "Too many failed attempts, try again in a few minutes");
    }
    let auth = st.auth.clone();
    let pw = body.password.clone();
    let ok = tokio::task::spawn_blocking(move || auth.check(&pw)).await.unwrap_or(false);
    if !ok {
        st.auth.record_failure();
        tokio::time::sleep(Duration::from_millis(700)).await;
        return bad(StatusCode::UNAUTHORIZED, "Wrong password");
    }
    logged_in(&headers, &st)
}

fn clear_cookie(headers: &HeaderMap) -> HeaderValue {
    session_cookie(headers, "", 0)
}

pub async fn handle_logout(State(st): State<AppState>, headers: HeaderMap) -> Response {
    st.auth.drop_session(&headers);
    let mut res = Json(json!({ "ok": true })).into_response();
    res.headers_mut().append(header::SET_COOKIE, clear_cookie(&headers));
    res
}

pub async fn handle_logout_get(State(st): State<AppState>, headers: HeaderMap) -> Response {
    st.auth.drop_session(&headers);
    let mut res = Redirect::to("/login").into_response();
    res.headers_mut().append(header::SET_COOKIE, clear_cookie(&headers));
    res
}

pub async fn handle_change_password(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ChangePasswordBody>,
) -> Response {
    if st.auth.password_from_env() {
        return bad(StatusCode::CONFLICT, "Password is set by LLM_WIKI_PASSWORD and cannot be changed here");
    }
    if body.new_password.chars().count() < MIN_PASSWORD_LEN {
        return bad(StatusCode::BAD_REQUEST, "Password must be at least 8 characters");
    }
    if !st.auth.check(&body.current) {
        st.auth.record_failure();
        return bad(StatusCode::FORBIDDEN, "Current password is wrong");
    }
    if let Err(e) = st.auth.set_password(&body.new_password, false) {
        return bad(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }
    st.auth.drop_all_sessions();
    logged_in(&headers, &st)
}
