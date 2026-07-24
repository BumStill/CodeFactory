// SPDX-License-Identifier: Apache-2.0
use serde::Serialize;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),

    #[error("{0}")]
    Other(String),
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

/// Transport failures cross the loop→bin boundary as `AppError::Other` with the
/// provider message verbatim (keystone slice 4.6). `TransportError::Display` is
/// already the raw message (no decoration) and `Other`'s Display is `{0}`, so a
/// switched call site's `to_string()` is byte-identical to the old direct
/// `call_openai_transport` error — and the only consumers read `Display`, never
/// the variant (the loop's context-overflow / vision greps included).
impl From<codefactory_agent_loop::transport::TransportError> for AppError {
    fn from(e: codefactory_agent_loop::transport::TransportError) -> Self {
        AppError::Other(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
