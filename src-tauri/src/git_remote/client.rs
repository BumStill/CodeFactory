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

    /// Bounded read for untrusted provider data. Caller supplies a fixed-host client
    /// with redirects disabled. Never surface remote response bodies as errors.
    pub async fn get_bounded(&self, path: &str, limit: usize) -> Result<Value, String> {
        let req = self.apply_auth(self.inner.get(format!("{}{}", self.base_url, path)));
        let mut response = req
            .send()
            .await
            .map_err(|_| "provider unavailable".to_string())?;
        if !response.status().is_success() {
            return Err("provider rejected read".into());
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "provider read failed".to_string())?
        {
            if body.len().saturating_add(chunk.len()) > limit {
                return Err("provider response limit".into());
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| "invalid provider response".into())
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

    /// DELETE wrapper (e.g. removing a label from an issue/PR). An empty body is
    /// a success shape: GitHub answers 200 with the remaining labels.
    pub async fn delete(&self, path: &str) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        let req = self.apply_auth(self.inner.delete(&url));
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status();
        let text = resp.text().await.map_err(|e| e.to_string())?;
        if !status.is_success() {
            return Err(format!("HTTP {}: {}", status, text.trim()));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| format!("JSON parse error: {}", e))
    }
}
