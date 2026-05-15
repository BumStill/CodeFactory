// SPDX-License-Identifier: Apache-2.0
//! Remote Git collaboration module — GitHub and GitLab integration.

pub mod client;
pub mod github;
pub mod gitlab;

use serde::{Deserialize, Serialize};

// ── Shared domain types ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteIssue {
    pub id: u64,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,       // "open" | "closed"
    pub labels: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub url: String,
    pub author: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemotePR {
    pub id: u64,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,       // "open" | "closed" | "merged"
    pub base_branch: String,
    pub head_branch: String,
    pub created_at: String,
    pub url: String,
    pub draft: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteRepo {
    pub full_name: String,   // "owner/repo"
    pub description: String,
    pub default_branch: String,
    pub url: String,
    pub private: bool,
    pub stars: u64,
}
