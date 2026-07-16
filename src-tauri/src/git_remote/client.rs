// SPDX-License-Identifier: Apache-2.0
//! Thin HTTP client wrapper around `reqwest::Client` for GitHub / GitLab APIs.

use reqwest::Client;
use serde_json::Value;

use crate::config::settings::GitProvider;

pub struct RemoteGitClient {
    pub inner: Client,
    pub base_url: String,
    pub provider: GitProvider,
    token: String,
}

impl RemoteGitClient {
    pub fn new(base_url: &str, token: &str, provider: GitProvider) -> Self {
        Self {
            inner: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            provider,
            token: token.to_string(),
        }
    }

    fn apply_auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.provider {
            GitProvider::Github => req
                .header("Authorization", format!("Bearer {}", self.token))
                .header("Accept", "application/vnd.github.v3+json")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .header("User-Agent", "CodeFactory/1.0"),
            GitProvider::Gitlab => req
                .header("PRIVATE-TOKEN", &self.token)
                .header("User-Agent", "CodeFactory/1.0"),
        }
    }

    pub async fn get(&self, path: &str) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let req = self.apply_auth(self.inner.get(&url));
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("HTTP {}: {}", status, text.trim()));
        }
        serde_json::from_str(&text)
            .map_err(|e| format!("JSON parse error: {}: {}", e, &text[..text.len().min(200)]))
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let req = self.apply_auth(self.inner.post(&url)).json(&body);
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("HTTP {}: {}", status, text.trim()));
        }
        serde_json::from_str(&text).map_err(|e| format!("JSON parse error: {}", e))
    }

    // Scaffolding: HTTP PATCH wrapper paired with get()/post(); reserved for
    // git-remote API calls (e.g. editing PRs/issues) that aren't wired yet.
    #[allow(dead_code)]
    pub async fn patch(&self, path: &str, body: Value) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let req = self.apply_auth(self.inner.patch(&url)).json(&body);
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("HTTP {}: {}", status, text.trim()));
        }
        serde_json::from_str(&text).map_err(|e| format!("JSON parse error: {}", e))
    }

    pub async fn put(&self, path: &str, body: Value) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let req = self.apply_auth(self.inner.put(&url)).json(&body);
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("HTTP {}: {}", status, text.trim()));
        }
        // Some PUT endpoints (e.g. an empty 200) return no JSON body.
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| format!("JSON parse error: {}", e))
    }
}
