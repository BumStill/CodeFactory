// SPDX-License-Identifier: Apache-2.0
//! "Sign in with ChatGPT" — OpenAI Codex OAuth (PKCE loopback) login.
//!
//! Mirrors the official OpenAI Codex CLI sign-in flow so ChatGPT Plus/Pro
//! users can authenticate with their account instead of pasting an API key.
//! The constants below (client id, issuer, redirect port, scopes) are taken
//! verbatim from the public `openai/codex` source — they are NOT secrets;
//! they are the published parameters of OpenAI's OAuth client. The redirect
//! URI is on OpenAI's allow-list, so the loopback port is fixed at 1455.
//!
//! What this module owns:
//!   * the PKCE loopback authorization-code flow (browser + localhost:1455),
//!   * token storage in the OS keychain (via `secrets`), and
//!   * token refresh + a `valid_access_token()` accessor the request layer
//!     uses to talk to the ChatGPT backend Responses API on the user's
//!     subscription.

use base64::Engine;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::State;

use crate::config::settings::{
    self as settings_config, ApiStyle, CustomModel, Endpoint, ReasoningEffort, Settings,
};
use crate::errors::{AppError, Result};
use crate::AppState;

// ── Published OAuth client parameters (from openai/codex) ────────────────────
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const ISSUER: &str = "https://auth.openai.com";
/// Fixed because the redirect URI must match OpenAI's allow-list.
const REDIRECT_PORT: u16 = 1455;
const SCOPE: &str = "openid profile email offline_access api.connectors.read api.connectors.invoke";

/// ChatGPT backend the access token is spent against (Responses API), used by
/// the request layer. Public so the agent can build the endpoint URL.
pub const CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";
pub(crate) const CHATGPT_ENDPOINT_KEY: &str = "chatgpt";
const CHATGPT_DEFAULT_MODEL: &str = "gpt-5.6-sol";

/// Keychain account under which the whole token blob is stored.
pub const SECRET_REF: &str = "codefactory.oauth.chatgpt";

/// Refresh when the access token is within this many seconds of expiry.
const REFRESH_SKEW_SECS: i64 = 300;

// Refresh and logout both mutate the same keychain record. Holding one lock
// across refresh prevents an in-flight refresh from restoring credentials
// after the user has signed out.
static AUTH_MUTATION_LOCK: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));
static AUTH_REVISION: AtomicU64 = AtomicU64::new(1);
static LAST_CATALOG_FETCH: Lazy<tokio::sync::Mutex<Option<CatalogFetch>>> =
    Lazy::new(|| tokio::sync::Mutex::new(None));

#[derive(Clone)]
struct CatalogFetch {
    auth_revision: u64,
    models: Vec<CustomModel>,
}

fn catalog_snapshot_is_current(
    snapshot: &CatalogFetch,
    models: &[CustomModel],
    auth_revision: u64,
) -> bool {
    snapshot.auth_revision == auth_revision && snapshot.models == models
}

// ── Stored token blob (serialized as JSON into the OS keychain) ──────────────
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct CodexTokens {
    pub access_token: String,
    pub refresh_token: String,
    pub id_token: String,
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub plan: Option<String>,
    /// Unix seconds of the last successful token grant/refresh.
    pub last_refresh: i64,
}

/// Account info surfaced to the frontend — never includes raw tokens.
#[derive(Serialize, Clone, Debug)]
pub struct CodexAccount {
    pub email: Option<String>,
    pub plan: Option<String>,
    pub account_id: Option<String>,
}

impl From<&CodexTokens> for CodexAccount {
    fn from(t: &CodexTokens) -> Self {
        CodexAccount {
            email: t.email.clone(),
            plan: t.plan.clone(),
            account_id: t.account_id.clone(),
        }
    }
}

// ── PKCE ─────────────────────────────────────────────────────────────────────
struct Pkce {
    verifier: String,
    challenge: String,
}

fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut buf = [0u8; N];
    getrandom::getrandom(&mut buf).map_err(|e| AppError::Other(format!("无法获取随机数：{e}")))?;
    Ok(buf)
}

fn generate_pkce() -> Result<Pkce> {
    use sha2::{Digest, Sha256};
    let seed = random_bytes::<64>()?;
    // Verifier: URL-safe base64 without padding (43..128 chars).
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(seed);
    // Challenge (S256): BASE64URL(SHA256(verifier)) without padding.
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    Ok(Pkce {
        verifier,
        challenge,
    })
}

fn random_state() -> Result<String> {
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_bytes::<24>()?))
}

fn authorize_url(challenge: &str, state: &str, redirect_uri: &str) -> String {
    let params = [
        ("response_type", "code"),
        ("client_id", CLIENT_ID),
        ("redirect_uri", redirect_uri),
        ("scope", SCOPE),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
    ];
    let qs = params
        .iter()
        .map(|(k, v)| format!("{}={}", k, urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{ISSUER}/oauth/authorize?{qs}")
}

/// Minimal application/x-www-form-urlencoded percent-encoding (RFC 3986
/// unreserved set kept literal). Avoids pulling in an extra crate.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ── id_token claim parsing (no signature check — token comes over TLS from
// the token endpoint, we only read claims we need) ───────────────────────────
#[derive(Deserialize, Default)]
struct IdClaims {
    #[serde(default)]
    email: Option<String>,
    #[serde(rename = "https://api.openai.com/profile", default)]
    profile: Option<ProfileClaims>,
    #[serde(rename = "https://api.openai.com/auth", default)]
    auth: Option<AuthClaims>,
}

#[derive(Deserialize, Default)]
struct ProfileClaims {
    #[serde(default)]
    email: Option<String>,
}

#[derive(Deserialize, Default)]
struct AuthClaims {
    #[serde(default)]
    chatgpt_plan_type: Option<String>,
    #[serde(default)]
    chatgpt_account_id: Option<String>,
}

struct ParsedId {
    email: Option<String>,
    plan: Option<String>,
    account_id: Option<String>,
}

fn decode_jwt_payload(jwt: &str) -> Option<serde_json::Value> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn parse_id_token(id_token: &str) -> ParsedId {
    let claims: IdClaims = decode_jwt_payload(id_token)
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    let email = claims
        .email
        .or_else(|| claims.profile.as_ref().and_then(|p| p.email.clone()));
    let (plan, account_id) = claims
        .auth
        .map(|a| (a.chatgpt_plan_type, a.chatgpt_account_id))
        .unwrap_or((None, None));
    ParsedId {
        email,
        plan,
        account_id,
    }
}

/// Unix-seconds `exp` claim of a JWT, if present.
fn jwt_exp(jwt: &str) -> Option<i64> {
    decode_jwt_payload(jwt)?
        .get("exp")
        .and_then(serde_json::Value::as_i64)
}

// ── Token endpoint exchanges ─────────────────────────────────────────────────
#[derive(Deserialize)]
struct TokenResponse {
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
}

async fn post_token(body: String) -> Result<TokenResponse> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{ISSUER}/oauth/token"))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await?;
    if !status.is_success() {
        return Err(AppError::Other(format!(
            "OAuth 令牌请求失败（{status}）：{text}"
        )));
    }
    serde_json::from_str(&text).map_err(AppError::from)
}

async fn exchange_code(code: &str, verifier: &str, redirect_uri: &str) -> Result<CodexTokens> {
    let body = format!(
        "grant_type=authorization_code&code={}&redirect_uri={}&client_id={}&code_verifier={}",
        urlencode(code),
        urlencode(redirect_uri),
        urlencode(CLIENT_ID),
        urlencode(verifier),
    );
    let tr = post_token(body).await?;
    let id_token = tr.id_token.unwrap_or_default();
    let parsed = parse_id_token(&id_token);
    Ok(CodexTokens {
        access_token: tr
            .access_token
            .ok_or_else(|| AppError::Other("令牌响应缺少 access_token".into()))?,
        refresh_token: tr.refresh_token.unwrap_or_default(),
        id_token,
        account_id: parsed.account_id,
        email: parsed.email,
        plan: parsed.plan,
        last_refresh: now_secs(),
    })
}

/// Refresh the access token using the stored refresh token, persisting the new
/// blob. Returns the refreshed tokens.
async fn refresh(mut tokens: CodexTokens) -> Result<CodexTokens> {
    if tokens.refresh_token.is_empty() {
        return Err(AppError::Other(
            "没有可用的 refresh_token，请重新登录 ChatGPT".into(),
        ));
    }
    let body = format!(
        "client_id={}&grant_type=refresh_token&refresh_token={}",
        urlencode(CLIENT_ID),
        urlencode(&tokens.refresh_token),
    );
    let tr = post_token(body).await?;
    if let Some(at) = tr.access_token {
        tokens.access_token = at;
    }
    if let Some(rt) = tr.refresh_token {
        // OpenAI may rotate the refresh token; keep the newest.
        tokens.refresh_token = rt;
    }
    if let Some(it) = tr.id_token {
        let parsed = parse_id_token(&it);
        tokens.id_token = it;
        // Refresh identity fields when present (account/plan can change).
        if parsed.email.is_some() {
            tokens.email = parsed.email;
        }
        if parsed.plan.is_some() {
            tokens.plan = parsed.plan;
        }
        if parsed.account_id.is_some() {
            tokens.account_id = parsed.account_id;
        }
    }
    tokens.last_refresh = now_secs();
    store_tokens(&tokens)?;
    Ok(tokens)
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

// ── Storage (OS keychain via `secrets`) ──────────────────────────────────────
fn store_tokens(tokens: &CodexTokens) -> Result<()> {
    let json = serde_json::to_string(tokens)?;
    crate::secrets::set_key(SECRET_REF, &json)
}

pub fn load_tokens() -> Result<Option<CodexTokens>> {
    let Some(json) = crate::secrets::get_key(SECRET_REF)? else {
        return Ok(None);
    };
    Ok(serde_json::from_str(&json).ok())
}

pub fn logout() -> Result<()> {
    crate::secrets::delete_key(SECRET_REF)?;
    AUTH_REVISION.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

pub fn current_account() -> Result<Option<CodexAccount>> {
    Ok(load_tokens()?.as_ref().map(CodexAccount::from))
}

/// Return a non-expired access token (refreshing if needed) along with the
/// ChatGPT account id to send as the `chatgpt-account-id` header. Used by the
/// request layer when an endpoint is backed by ChatGPT login.
async fn valid_tokens_locked() -> Result<CodexTokens> {
    let tokens = load_tokens()?.ok_or_else(|| AppError::Other("尚未登录 ChatGPT".into()))?;
    let needs_refresh = match jwt_exp(&tokens.access_token) {
        Some(exp) => now_secs() + REFRESH_SKEW_SECS >= exp,
        // Unknown expiry → refresh if it's been a while since the last grant.
        None => now_secs() - tokens.last_refresh >= 25 * 60,
    };
    let tokens = if needs_refresh {
        refresh(tokens).await?
    } else {
        tokens
    };
    Ok(tokens)
}

pub async fn valid_access_token() -> Result<(String, Option<String>)> {
    let _auth_guard = AUTH_MUTATION_LOCK.lock().await;
    let tokens = valid_tokens_locked().await?;
    Ok((tokens.access_token, tokens.account_id))
}

async fn valid_access_token_snapshot() -> Result<(String, Option<String>, u64)> {
    let _auth_guard = AUTH_MUTATION_LOCK.lock().await;
    let tokens = valid_tokens_locked().await?;
    Ok((
        tokens.access_token,
        tokens.account_id,
        AUTH_REVISION.load(Ordering::SeqCst),
    ))
}

#[derive(Deserialize)]
struct CodexModelsResponse {
    #[serde(default)]
    models: Vec<CodexModelEntry>,
}

const fn default_effective_context_window_percent() -> i64 {
    95
}

#[derive(Deserialize)]
struct CodexModelEntry {
    slug: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    context_window: Option<i64>,
    #[serde(default)]
    max_context_window: Option<i64>,
    // Match the official Codex ModelInfo contract: the backend may omit this
    // field, in which case clients reserve 5% for prompts, tools, and output.
    #[serde(default = "default_effective_context_window_percent")]
    effective_context_window_percent: i64,
    #[serde(default)]
    default_reasoning_level: Option<String>,
    #[serde(default)]
    supported_reasoning_levels: Option<Vec<CodexReasoningLevel>>,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(default)]
    supported_in_api: bool,
}

#[derive(Deserialize)]
struct CodexReasoningLevel {
    effort: String,
}

fn parse_codex_models(body: &str) -> Result<Vec<CustomModel>> {
    let catalog: CodexModelsResponse = serde_json::from_str(body)?;
    Ok(catalog
        .models
        .into_iter()
        .filter(|model| model.visibility.as_deref() == Some("list") && model.supported_in_api)
        .map(|model| CustomModel {
            id: model.slug,
            name: model.display_name.filter(|name| !name.trim().is_empty()),
            context_length: model
                .context_window
                .and_then(|length| u32::try_from(length).ok()),
            max_context_length: model
                .max_context_window
                .and_then(|length| u32::try_from(length).ok()),
            effective_context_window_percent: u8::try_from(model.effective_context_window_percent)
                .ok()
                .filter(|percent| (1..=100).contains(percent)),
            default_reasoning_effort: model
                .default_reasoning_level
                .as_deref()
                .and_then(ReasoningEffort::parse),
            supported_reasoning_efforts: model.supported_reasoning_levels.and_then(|levels| {
                let efforts: Vec<_> = levels
                    .into_iter()
                    .filter_map(|level| ReasoningEffort::parse(&level.effort))
                    // The Codex catalog can advertise the client-side `ultra`
                    // label even while the ChatGPT Responses transport rejects
                    // it. `max` is the highest value accepted on this route.
                    .filter(|effort| *effort != ReasoningEffort::Ultra)
                    .collect();
                (!efforts.is_empty()).then_some(efforts)
            }),
        })
        .collect())
}

fn build_codex_models_request(
    client: &reqwest::Client,
    base_url: &str,
    access_token: &str,
    account_id: Option<&str>,
) -> reqwest::RequestBuilder {
    let url = format!(
        "{}/models?client_version={}",
        base_url.trim_end_matches('/'),
        env!("CARGO_PKG_VERSION")
    );
    let mut request = client
        .get(url)
        .bearer_auth(access_token)
        .header("Accept", "application/json")
        .header("originator", "codex_cli_rs");
    if let Some(account_id) = account_id {
        request = request.header("chatgpt-account-id", account_id);
    }
    request
}

async fn fetch_codex_models(
    client: &reqwest::Client,
    access_token: &str,
    account_id: Option<&str>,
) -> Result<Vec<CustomModel>> {
    let response = build_codex_models_request(client, CHATGPT_BASE_URL, access_token, account_id)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(AppError::Other(format!(
            "ChatGPT 模型目录请求失败（{status}）：{body}"
        )));
    }
    parse_codex_models(&body)
}

fn apply_codex_models_to_settings(settings: &mut Settings, models: Vec<CustomModel>) {
    let existing = settings.endpoints.get(CHATGPT_ENDPOINT_KEY).cloned();
    let valid_ids: Vec<_> = models.iter().map(|model| model.id.clone()).collect();
    let fallback_model = if valid_ids.iter().any(|model| model == CHATGPT_DEFAULT_MODEL) {
        CHATGPT_DEFAULT_MODEL.to_string()
    } else {
        models
            .first()
            .map(|model| model.id.clone())
            .unwrap_or_default()
    };
    let active_model = existing
        .as_ref()
        .and_then(|endpoint| endpoint.active_model.as_ref())
        .filter(|model| valid_ids.contains(model))
        .cloned()
        .unwrap_or(fallback_model);

    settings.endpoints.insert(
        CHATGPT_ENDPOINT_KEY.to_string(),
        Endpoint {
            base_url: CHATGPT_BASE_URL.to_string(),
            key_ref: None,
            api_style: ApiStyle::Chatgpt,
            custom_models: models,
            active_model: Some(active_model.clone()),
        },
    );

    if existing.is_none() {
        settings.default_endpoint = CHATGPT_ENDPOINT_KEY.to_string();
        settings.default_model = active_model;
    } else if settings.default_endpoint == CHATGPT_ENDPOINT_KEY
        && !valid_ids.contains(&settings.default_model)
    {
        settings.default_model = active_model;
    }
}

fn remove_chatgpt_from_settings(settings: &mut Settings) {
    if settings.endpoints.remove(CHATGPT_ENDPOINT_KEY).is_none() {
        return;
    }
    if settings.default_endpoint != CHATGPT_ENDPOINT_KEY {
        return;
    }

    let mut endpoint_names: Vec<_> = settings.endpoints.keys().cloned().collect();
    endpoint_names.sort();
    let next_endpoint = endpoint_names.into_iter().next().unwrap_or_default();
    let next_model = settings
        .endpoints
        .get(&next_endpoint)
        .and_then(|endpoint| {
            endpoint
                .active_model
                .clone()
                .or_else(|| endpoint.custom_models.first().map(|model| model.id.clone()))
        })
        .unwrap_or_default();
    settings.default_endpoint = next_endpoint;
    settings.default_model = next_model;
}

/// Whole-settings saves originate from a frontend snapshot. Reconcile the
/// backend-owned subscription endpoint with the latest locked state so a
/// concurrent theme/hook save cannot roll back a catalog refresh or resurrect
/// the endpoint after logout. The active model remains user-editable when it
/// exists in the current catalog.
pub(crate) fn reconcile_chatgpt_settings(current: &Settings, incoming: &mut Settings) {
    let Some(current_endpoint) = current.endpoints.get(CHATGPT_ENDPOINT_KEY) else {
        remove_chatgpt_from_settings(incoming);
        return;
    };

    let requested_active = incoming
        .endpoints
        .get(CHATGPT_ENDPOINT_KEY)
        .and_then(|endpoint| endpoint.active_model.as_ref())
        .filter(|model| {
            current_endpoint
                .custom_models
                .iter()
                .any(|candidate| candidate.id == **model)
        })
        .cloned();
    let fallback_active = current_endpoint
        .active_model
        .as_ref()
        .filter(|model| {
            current_endpoint
                .custom_models
                .iter()
                .any(|candidate| candidate.id == **model)
        })
        .cloned()
        .or_else(|| {
            current_endpoint
                .custom_models
                .first()
                .map(|model| model.id.clone())
        });
    let mut endpoint = current_endpoint.clone();
    endpoint.active_model = requested_active.or(fallback_active);
    incoming
        .endpoints
        .insert(CHATGPT_ENDPOINT_KEY.to_string(), endpoint.clone());

    if incoming.default_endpoint == CHATGPT_ENDPOINT_KEY
        && !endpoint
            .custom_models
            .iter()
            .any(|model| model.id == incoming.default_model)
    {
        incoming.default_model = endpoint.active_model.unwrap_or_default();
    }
}

// ── Loopback authorization-code flow ─────────────────────────────────────────
/// Blocking single-shot loopback server: accept connections until one carries
/// a `/auth/callback` with a matching `state`, then return its `code`. Responds
/// to the browser with a small success page.
fn wait_for_callback(listener: std::net::TcpListener, expected_state: &str) -> Result<String> {
    use std::io::{Read, Write};
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(s) => s,
            Err(_) => continue,
        };
        let mut buf = [0u8; 4096];
        let n = stream.read(&mut buf).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]);
        // First line: "GET /auth/callback?code=...&state=... HTTP/1.1"
        let Some(path) = req.split_whitespace().nth(1) else {
            continue;
        };
        if !path.starts_with("/auth/callback") {
            // Ignore favicon/other probes; keep listening.
            let _ = stream.write_all(simple_http(404, "Not found").as_bytes());
            continue;
        }
        let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
        let params = parse_query(query);
        if let Some(err) = params.get("error") {
            let desc = params.get("error_description").cloned().unwrap_or_default();
            let _ = stream
                .write_all(simple_http(200, "登录失败，可关闭此页并返回 CodeFactory。").as_bytes());
            return Err(AppError::Other(format!("OAuth 授权被拒绝：{err} {desc}")));
        }
        match (params.get("code"), params.get("state")) {
            (Some(code), Some(state)) if state == expected_state => {
                let _ = stream.write_all(
                    simple_http(200, "登录成功，请返回 CodeFactory，可关闭此页。").as_bytes(),
                );
                return Ok(code.clone());
            }
            (_, Some(state)) if state != expected_state => {
                let _ = stream.write_all(simple_http(400, "状态校验失败，请重试登录。").as_bytes());
                return Err(AppError::Other(
                    "OAuth state 校验失败（可能的 CSRF）".into(),
                ));
            }
            _ => {
                let _ = stream.write_all(simple_http(400, "缺少授权码。").as_bytes());
                continue;
            }
        }
    }
    Err(AppError::Other("本地回调服务器意外关闭".into()))
}

fn simple_http(status: u16, body_text: &str) -> String {
    let reason = if status == 200 { "OK" } else { "Error" };
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>CodeFactory</title></head>\
         <body style=\"font-family:system-ui;background:#0b0b0c;color:#eaeaea;display:flex;\
         align-items:center;justify-content:center;height:100vh;margin:0\">\
         <p style=\"font-size:15px\">{body_text}</p></body></html>"
    );
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.as_bytes().len()
    )
}

fn parse_query(query: &str) -> std::collections::HashMap<String, String> {
    query
        .split('&')
        .filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            Some((k.to_string(), urldecode(v)))
        })
        .collect()
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// Opens the system browser via the shell plugin (already a dependency, with
// the `shell:allow-open` capability granted). `Shell::open` is deprecated in
// favour of tauri-plugin-opener; migrating to that plugin is a future cleanup
// that would add a new dependency + capability, so we keep shell.open here.
#[allow(deprecated)]
fn open_browser(app: &tauri::AppHandle, url: &str) -> Result<()> {
    use tauri_plugin_shell::ShellExt;
    app.shell()
        .open(url, None)
        .map_err(|e| AppError::Other(format!("无法打开浏览器：{e}")))
}

/// Run the full interactive login: open the browser, capture the loopback
/// callback, exchange the code, and persist the tokens. Returns account info.
pub async fn login(app: tauri::AppHandle) -> Result<CodexAccount> {
    let pkce = generate_pkce()?;
    let state = random_state()?;
    let redirect_uri = format!("http://localhost:{REDIRECT_PORT}/auth/callback");

    // Bind first so we fail fast (and before opening the browser) if the port
    // is busy — the redirect URI is allow-listed at :1455 only.
    let listener = std::net::TcpListener::bind(("127.0.0.1", REDIRECT_PORT)).map_err(|e| {
        AppError::Other(format!(
            "无法监听本地端口 {REDIRECT_PORT}（可能已有 Codex/ChatGPT 登录在进行）：{e}"
        ))
    })?;

    let url = authorize_url(&pkce.challenge, &state, &redirect_uri);
    open_browser(&app, &url)?;

    let expected = state.clone();
    let code = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        tokio::task::spawn_blocking(move || wait_for_callback(listener, &expected)),
    )
    .await
    .map_err(|_| AppError::Other("登录超时：5 分钟内未完成授权".into()))?
    .map_err(|e| AppError::Other(format!("回调任务失败：{e}")))??;

    let tokens = exchange_code(&code, &pkce.verifier, &redirect_uri).await?;
    let _auth_guard = AUTH_MUTATION_LOCK.lock().await;
    store_tokens(&tokens)?;
    AUTH_REVISION.fetch_add(1, Ordering::SeqCst);
    *LAST_CATALOG_FETCH.lock().await = None;
    Ok(CodexAccount::from(&tokens))
}

// ── Tauri commands ───────────────────────────────────────────────────────────
#[tauri::command]
pub async fn codex_login(app: tauri::AppHandle) -> Result<CodexAccount> {
    login(app).await
}

#[tauri::command]
pub async fn codex_logout(state: State<'_, AppState>) -> Result<()> {
    let _auth_guard = AUTH_MUTATION_LOCK.lock().await;
    let mut current = state.settings.write().await;
    logout()?;
    *LAST_CATALOG_FETCH.lock().await = None;
    let mut next = current.clone();
    remove_chatgpt_from_settings(&mut next);
    settings_config::save(&next)?;
    *current = next;
    Ok(())
}

#[tauri::command]
pub async fn codex_account() -> Result<Option<CodexAccount>> {
    current_account()
}

#[tauri::command]
pub async fn codex_models() -> Result<Vec<CustomModel>> {
    let (access_token, account_id, auth_revision) = valid_access_token_snapshot().await?;
    let models = fetch_codex_models(
        &reqwest::Client::new(),
        &access_token,
        account_id.as_deref(),
    )
    .await?;
    *LAST_CATALOG_FETCH.lock().await = Some(CatalogFetch {
        auth_revision,
        models: models.clone(),
    });
    Ok(models)
}

#[tauri::command]
pub async fn apply_codex_models(
    models: Vec<CustomModel>,
    state: State<'_, AppState>,
) -> Result<()> {
    if models.is_empty() {
        return Err(AppError::Other("ChatGPT 模型目录为空".into()));
    }

    let _auth_guard = AUTH_MUTATION_LOCK.lock().await;
    if current_account()?.is_none() {
        return Ok(());
    }
    let auth_revision = AUTH_REVISION.load(Ordering::SeqCst);
    let mut current = state.settings.write().await;
    let snapshot = LAST_CATALOG_FETCH.lock().await.clone();
    let can_apply = snapshot
        .as_ref()
        .is_some_and(|fetch| catalog_snapshot_is_current(fetch, &models, auth_revision))
        || (snapshot.is_none() && !current.endpoints.contains_key(CHATGPT_ENDPOINT_KEY));
    if !can_apply {
        tracing::warn!("ignoring stale ChatGPT model catalog response");
        return Ok(());
    }

    let mut next = current.clone();
    apply_codex_models_to_settings(&mut next, models);
    settings_config::save(&next)?;
    *current = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::{DeliveryCeiling, ReasoningEffort};

    fn catalog_model(id: &str) -> CustomModel {
        CustomModel {
            id: id.into(),
            name: None,
            context_length: Some(272000),
            max_context_length: Some(272000),
            effective_context_window_percent: Some(95),
            default_reasoning_effort: Some(ReasoningEffort::Medium),
            supported_reasoning_efforts: Some(vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::High,
            ]),
        }
    }

    #[test]
    fn catalog_snapshot_rejects_stale_auth_revision_or_different_payload() {
        let snapshot = CatalogFetch {
            auth_revision: 7,
            models: vec![catalog_model("gpt-5.6-sol")],
        };

        assert!(catalog_snapshot_is_current(&snapshot, &snapshot.models, 7));
        assert!(!catalog_snapshot_is_current(&snapshot, &snapshot.models, 8));
        assert!(!catalog_snapshot_is_current(
            &snapshot,
            &[catalog_model("gpt-5.5")],
            7
        ));
    }

    #[test]
    fn catalog_patch_preserves_unrelated_settings_and_repairs_default_model() {
        let mut settings = Settings::default();
        settings.delivery_ceiling = DeliveryCeiling::ThroughRelease;
        settings.delivery_ci_timeout_secs = 777;
        settings.default_endpoint = CHATGPT_ENDPOINT_KEY.into();
        settings.default_model = "retired-model".into();
        settings.endpoints.insert(
            CHATGPT_ENDPOINT_KEY.into(),
            Endpoint {
                base_url: CHATGPT_BASE_URL.into(),
                key_ref: None,
                api_style: ApiStyle::Chatgpt,
                custom_models: vec![catalog_model("retired-model")],
                active_model: Some("retired-model".into()),
            },
        );

        apply_codex_models_to_settings(&mut settings, vec![catalog_model("future-model")]);

        assert_eq!(settings.delivery_ceiling, DeliveryCeiling::ThroughRelease);
        assert_eq!(settings.delivery_ci_timeout_secs, 777);
        assert_eq!(settings.default_model, "future-model");
        assert_eq!(
            settings.endpoints[CHATGPT_ENDPOINT_KEY]
                .active_model
                .as_deref(),
            Some("future-model")
        );
    }

    #[test]
    fn logout_patch_removes_chatgpt_and_selects_a_valid_remaining_model() {
        let mut settings = Settings::default();
        settings.endpoints.insert(
            CHATGPT_ENDPOINT_KEY.into(),
            Endpoint {
                base_url: CHATGPT_BASE_URL.into(),
                key_ref: None,
                api_style: ApiStyle::Chatgpt,
                custom_models: vec![catalog_model("gpt-5.6-sol")],
                active_model: Some("gpt-5.6-sol".into()),
            },
        );
        settings.endpoints.insert(
            "deepseek".into(),
            Endpoint {
                base_url: "https://api.deepseek.com".into(),
                key_ref: None,
                api_style: ApiStyle::Openai,
                custom_models: vec![catalog_model("deepseek-chat")],
                active_model: Some("deepseek-chat".into()),
            },
        );
        settings.default_endpoint = CHATGPT_ENDPOINT_KEY.into();
        settings.default_model = "gpt-5.6-sol".into();

        remove_chatgpt_from_settings(&mut settings);

        assert!(!settings.endpoints.contains_key(CHATGPT_ENDPOINT_KEY));
        assert_eq!(settings.default_endpoint, "deepseek");
        assert_eq!(settings.default_model, "deepseek-chat");
    }

    #[test]
    fn stale_whole_settings_save_preserves_current_catalog_and_requested_model() {
        let current_catalog = vec![catalog_model("gpt-5.6-sol"), catalog_model("gpt-5.5")];
        let mut current = Settings::default();
        apply_codex_models_to_settings(&mut current, current_catalog.clone());

        let mut incoming = current.clone();
        incoming.theme = crate::config::settings::Theme::Light;
        incoming
            .endpoints
            .get_mut(CHATGPT_ENDPOINT_KEY)
            .unwrap()
            .custom_models = vec![catalog_model("retired-model")];
        incoming
            .endpoints
            .get_mut(CHATGPT_ENDPOINT_KEY)
            .unwrap()
            .active_model = Some("gpt-5.5".into());

        reconcile_chatgpt_settings(&current, &mut incoming);

        assert_eq!(incoming.theme, crate::config::settings::Theme::Light);
        assert_eq!(
            incoming.endpoints[CHATGPT_ENDPOINT_KEY]
                .custom_models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            current_catalog
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            incoming.endpoints[CHATGPT_ENDPOINT_KEY]
                .active_model
                .as_deref(),
            Some("gpt-5.5")
        );
    }

    #[test]
    fn stale_whole_settings_save_cannot_resurrect_logged_out_endpoint() {
        let current = Settings::default();
        let mut incoming = current.clone();
        incoming.endpoints.insert(
            CHATGPT_ENDPOINT_KEY.into(),
            Endpoint {
                base_url: CHATGPT_BASE_URL.into(),
                key_ref: None,
                api_style: ApiStyle::Chatgpt,
                custom_models: vec![catalog_model("gpt-5.6-sol")],
                active_model: Some("gpt-5.6-sol".into()),
            },
        );
        incoming.default_endpoint = CHATGPT_ENDPOINT_KEY.into();
        incoming.default_model = "gpt-5.6-sol".into();

        reconcile_chatgpt_settings(&current, &mut incoming);

        assert!(!incoming.endpoints.contains_key(CHATGPT_ENDPOINT_KEY));
        assert_ne!(incoming.default_endpoint, CHATGPT_ENDPOINT_KEY);
    }

    #[test]
    fn catalog_filters_unlisted_or_non_api_models_and_maps_capabilities() {
        let models = parse_codex_models(
            r#"{
                "models": [
                    {
                        "slug": "gpt-5.6-sol",
                        "display_name": "GPT-5.6 Sol",
                        "context_window": 272000,
                        "max_context_window": 1050000,
                        "default_reasoning_level": "low",
                        "supported_reasoning_levels": [
                            {"effort": "low", "description": "fast"},
                            {"effort": "medium", "description": "balanced"},
                            {"effort": "xhigh", "description": "deep"},
                            {"effort": "max", "description": "deeper"},
                            {"effort": "ultra", "description": "deepest"}
                        ],
                        "visibility": "list",
                        "supported_in_api": true
                    },
                    {
                        "slug": "hidden",
                        "visibility": "hide",
                        "supported_in_api": true
                    },
                    {
                        "slug": "not-in-api",
                        "visibility": "list",
                        "supported_in_api": false
                    }
                ]
            }"#,
        )
        .expect("catalog should parse");

        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-5.6-sol");
        assert_eq!(models[0].name.as_deref(), Some("GPT-5.6 Sol"));
        assert_eq!(models[0].context_length, Some(272000));
        assert_eq!(models[0].max_context_length, Some(1050000));
        assert_eq!(models[0].effective_context_window_percent, Some(95));
        assert_eq!(
            models[0].default_reasoning_effort,
            Some(ReasoningEffort::Low)
        );
        assert_eq!(
            models[0].supported_reasoning_efforts,
            Some(vec![
                ReasoningEffort::Low,
                ReasoningEffort::Medium,
                ReasoningEffort::XHigh,
                ReasoningEffort::Max,
            ])
        );
    }

    #[test]
    fn catalog_request_uses_package_version_and_oauth_account_headers() {
        let client = reqwest::Client::new();
        let request = build_codex_models_request(
            &client,
            "https://chatgpt.com/backend-api/codex",
            "access-token",
            Some("account-123"),
        )
        .build()
        .expect("request should build");

        assert_eq!(request.url().path(), "/backend-api/codex/models");
        assert_eq!(
            request
                .url()
                .query_pairs()
                .find(|(key, _)| key == "client_version")
                .map(|(_, value)| value.into_owned()),
            Some(env!("CARGO_PKG_VERSION").to_string())
        );
        assert_eq!(
            request.headers().get("authorization").unwrap(),
            "Bearer access-token"
        );
        assert_eq!(
            request.headers().get("chatgpt-account-id").unwrap(),
            "account-123"
        );
    }

    #[test]
    fn catalog_empty_efforts_become_none_without_reordering_server_priority() {
        let models = parse_codex_models(
            r#"{
                "models": [
                    {
                        "slug": "server-first",
                        "display_name": "Server First",
                        "supported_reasoning_levels": [],
                        "visibility": "list",
                        "supported_in_api": true,
                        "priority": 20
                    },
                    {
                        "slug": "server-second",
                        "display_name": "Server Second",
                        "supported_reasoning_levels": [{"effort": "medium"}],
                        "visibility": "list",
                        "supported_in_api": true,
                        "priority": 0
                    }
                ]
            }"#,
        )
        .expect("catalog should parse");

        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec!["server-first", "server-second"]
        );
        assert_eq!(models[0].supported_reasoning_efforts, None);
    }
}
