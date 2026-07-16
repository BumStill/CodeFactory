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
use serde::{Deserialize, Serialize};

use crate::config::settings::{CustomModel, ReasoningEffort};
use crate::errors::{AppError, Result};

// ── Published OAuth client parameters (from openai/codex) ────────────────────
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const ISSUER: &str = "https://auth.openai.com";
/// Fixed because the redirect URI must match OpenAI's allow-list.
const REDIRECT_PORT: u16 = 1455;
const SCOPE: &str = "openid profile email offline_access api.connectors.read api.connectors.invoke";

/// ChatGPT backend the access token is spent against (Responses API), used by
/// the request layer. Public so the agent can build the endpoint URL.
pub const CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api/codex";

/// Keychain account under which the whole token blob is stored.
pub const SECRET_REF: &str = "codefactory.oauth.chatgpt";

/// Refresh when the access token is within this many seconds of expiry.
const REFRESH_SKEW_SECS: i64 = 300;

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
    crate::secrets::delete_key(SECRET_REF)
}

pub fn current_account() -> Result<Option<CodexAccount>> {
    Ok(load_tokens()?.as_ref().map(CodexAccount::from))
}

/// Return a non-expired access token (refreshing if needed) along with the
/// ChatGPT account id to send as the `chatgpt-account-id` header. Used by the
/// request layer when an endpoint is backed by ChatGPT login.
pub async fn valid_access_token() -> Result<(String, Option<String>)> {
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
    Ok((tokens.access_token, tokens.account_id))
}

#[derive(Deserialize)]
struct CodexModelsResponse {
    #[serde(default)]
    models: Vec<CodexModelEntry>,
}

#[derive(Deserialize)]
struct CodexModelEntry {
    slug: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    context_window: Option<i64>,
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
            default_reasoning_effort: model
                .default_reasoning_level
                .as_deref()
                .and_then(ReasoningEffort::parse),
            supported_reasoning_efforts: model.supported_reasoning_levels.and_then(|levels| {
                let efforts: Vec<_> = levels
                    .into_iter()
                    .filter_map(|level| ReasoningEffort::parse(&level.effort))
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
    store_tokens(&tokens)?;
    Ok(CodexAccount::from(&tokens))
}

// ── Tauri commands ───────────────────────────────────────────────────────────
#[tauri::command]
pub async fn codex_login(app: tauri::AppHandle) -> Result<CodexAccount> {
    login(app).await
}

#[tauri::command]
pub async fn codex_logout() -> Result<()> {
    logout()
}

#[tauri::command]
pub async fn codex_account() -> Result<Option<CodexAccount>> {
    current_account()
}

#[tauri::command]
pub async fn codex_models() -> Result<Vec<CustomModel>> {
    let (access_token, account_id) = valid_access_token().await?;
    fetch_codex_models(
        &reqwest::Client::new(),
        &access_token,
        account_id.as_deref(),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::settings::ReasoningEffort;

    #[test]
    fn catalog_filters_unlisted_or_non_api_models_and_maps_capabilities() {
        let models = parse_codex_models(
            r#"{
                "models": [
                    {
                        "slug": "gpt-5.6-sol",
                        "display_name": "GPT-5.6 Sol",
                        "context_window": 272000,
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
                ReasoningEffort::Ultra,
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
