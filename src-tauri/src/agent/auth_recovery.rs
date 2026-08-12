// SPDX-License-Identifier: Apache-2.0
//! Durable, secret-free authentication capability observations.
//!
//! This module does not read the Keychain itself and does not resume a model
//! call.  A callback or startup adapter probes the Keychain, passes only the
//! capability result here, then uses [`AuthRecoveryStore::observe`] to decide
//! whether the provider work may be queued.  Raw credential material is only
//! hashed in memory and is never stored.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthCapabilityStatus {
    Ready,
    Missing,
    Expired,
    Unknown,
}

impl AuthCapabilityStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Missing => "missing",
            Self::Expired => "expired",
            Self::Unknown => "unknown",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "ready" => Ok(Self::Ready),
            "missing" => Ok(Self::Missing),
            "expired" => Ok(Self::Expired),
            "unknown" => Ok(Self::Unknown),
            other => bail!("invalid auth capability status {other:?}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthObservationSource {
    Callback,
    Startup,
    Adapter,
}

impl AuthObservationSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Callback => "callback",
            Self::Startup => "startup",
            Self::Adapter => "adapter",
        }
    }
}

/// The caller retains ownership of any identity material.  It is consumed only
/// as input to SHA-256 while this call is executing.
#[derive(Clone, Copy, Debug)]
pub enum AuthCapabilityProbe<'a> {
    Ready { identity_material: &'a [u8] },
    Missing,
    Expired,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthCapabilityReceipt {
    pub id: String,
    pub objective_id: String,
    pub objective_revision: i64,
    pub request_key: String,
    pub provider: String,
    pub credential_ref: String,
    pub capability_digest: String,
    pub status: AuthCapabilityStatus,
    pub source: String,
    pub observed_at: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AuthRecoveryDisposition {
    /// A durable, current-revision receipt authorizes queueing provider work.
    QueueProvider {
        request_key: String,
        resume_cursor: String,
        capability_digest: String,
    },
    StillNeedsAuthorization {
        request_key: String,
        status: AuthCapabilityStatus,
    },
    ObserveOnlyUnknown {
        request_key: String,
    },
    NoReceipt,
}

#[derive(Clone)]
pub struct AuthRecoveryStore {
    pool: SqlitePool,
}

impl AuthRecoveryStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn record_probe(
        &self,
        objective_id: &str,
        objective_revision: i64,
        request_key: &str,
        provider: &str,
        credential_ref: &str,
        source: AuthObservationSource,
        probe: AuthCapabilityProbe<'_>,
        now: i64,
    ) -> Result<AuthCapabilityReceipt> {
        validate_public_identifier("objective_id", objective_id)?;
        validate_public_identifier("request_key", request_key)?;
        validate_public_identifier("provider", provider)?;
        validate_public_identifier("credential_ref", credential_ref)?;
        if objective_revision < 1 {
            bail!("objective revision must be positive");
        }

        let mut tx = self.pool.begin().await?;
        let objective = sqlx::query(
            "SELECT revision, domain, status, request_key, resume_cursor
             FROM objectives WHERE id=?",
        )
        .bind(objective_id)
        .fetch_optional(&mut *tx)
        .await?
        .with_context(|| format!("objective {objective_id:?} does not exist"))?;
        let current_revision: i64 = objective.get("revision");
        let domain: String = objective.get("domain");
        let objective_status: String = objective.get("status");
        let stored_request_key: Option<String> = objective.get("request_key");
        let resume_cursor: Option<String> = objective.get("resume_cursor");
        if current_revision != objective_revision {
            bail!(
                "stale auth observation revision: expected {current_revision}, got {objective_revision}"
            );
        }
        if domain != "auth" {
            bail!("objective {objective_id:?} is not an auth objective");
        }
        if !matches!(
            objective_status.as_str(),
            "waiting_authorization" | "waiting_system" | "active"
        ) {
            bail!("auth objective is not observable from status {objective_status:?}");
        }
        if stored_request_key.as_deref() != Some(request_key) {
            bail!("auth request key does not match the current objective");
        }
        if resume_cursor.as_deref().unwrap_or_default().is_empty() {
            bail!("auth objective has no resume cursor");
        }

        let (status, capability_digest) = capability_digest(provider, credential_ref, probe);
        let receipt_id = digest_text(&format!(
            "auth-capability\0{objective_id}\0{objective_revision}\0{request_key}\0{}\0{capability_digest}",
            status.as_str()
        ));
        sqlx::query(
            "INSERT INTO auth_capability_receipts
             (id, objective_id, objective_revision, request_key, provider,
              credential_ref, capability_digest, status, source, observed_at, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
               observed_at=MAX(auth_capability_receipts.observed_at, excluded.observed_at)",
        )
        .bind(&receipt_id)
        .bind(objective_id)
        .bind(objective_revision)
        .bind(request_key)
        .bind(provider)
        .bind(credential_ref)
        .bind(&capability_digest)
        .bind(status.as_str())
        .bind(source.as_str())
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        let row = sqlx::query(
            "SELECT id, objective_id, objective_revision, request_key, provider,
                    credential_ref, capability_digest, status, source, observed_at
             FROM auth_capability_receipts WHERE id=?",
        )
        .bind(&receipt_id)
        .fetch_one(&mut *tx)
        .await?;
        let receipt = receipt_from_row(&row)?;
        tx.commit().await?;
        Ok(receipt)
    }

    /// Observe the latest receipt for the *current* Objective revision.  A
    /// receipt from an older revision can never authorize provider execution.
    pub async fn observe(&self, objective_id: &str) -> Result<AuthRecoveryDisposition> {
        let row = sqlx::query(
            "SELECT r.request_key, r.capability_digest, r.status, o.resume_cursor
             FROM objectives o
             LEFT JOIN auth_capability_receipts r
               ON r.objective_id=o.id AND r.objective_revision=o.revision
             WHERE o.id=? AND o.domain='auth'
             ORDER BY r.observed_at DESC, r.created_at DESC
             LIMIT 1",
        )
        .bind(objective_id)
        .fetch_optional(&self.pool)
        .await?
        .with_context(|| format!("auth objective {objective_id:?} does not exist"))?;
        let request_key: Option<String> = row.try_get("request_key")?;
        let Some(request_key) = request_key else {
            return Ok(AuthRecoveryDisposition::NoReceipt);
        };
        let status = AuthCapabilityStatus::parse(row.get::<String, _>("status").as_str())?;
        match status {
            AuthCapabilityStatus::Ready => {
                let resume_cursor: Option<String> = row.try_get("resume_cursor")?;
                let resume_cursor = resume_cursor
                    .filter(|cursor| !cursor.is_empty())
                    .context("ready auth capability has no resume cursor")?;
                Ok(AuthRecoveryDisposition::QueueProvider {
                    request_key,
                    resume_cursor,
                    capability_digest: row.get("capability_digest"),
                })
            }
            AuthCapabilityStatus::Missing | AuthCapabilityStatus::Expired => {
                Ok(AuthRecoveryDisposition::StillNeedsAuthorization {
                    request_key,
                    status,
                })
            }
            AuthCapabilityStatus::Unknown => {
                Ok(AuthRecoveryDisposition::ObserveOnlyUnknown { request_key })
            }
        }
    }
}

fn capability_digest(
    provider: &str,
    credential_ref: &str,
    probe: AuthCapabilityProbe<'_>,
) -> (AuthCapabilityStatus, String) {
    match probe {
        AuthCapabilityProbe::Ready { identity_material } => {
            let mut hasher = Sha256::new();
            hasher.update(b"auth-capability-ready\0");
            hasher.update(provider.as_bytes());
            hasher.update(b"\0");
            hasher.update(credential_ref.as_bytes());
            hasher.update(b"\0");
            hasher.update(identity_material);
            (
                AuthCapabilityStatus::Ready,
                format!("sha256:{:x}", hasher.finalize()),
            )
        }
        AuthCapabilityProbe::Missing => {
            status_digest(provider, credential_ref, AuthCapabilityStatus::Missing)
        }
        AuthCapabilityProbe::Expired => {
            status_digest(provider, credential_ref, AuthCapabilityStatus::Expired)
        }
        AuthCapabilityProbe::Unknown => {
            status_digest(provider, credential_ref, AuthCapabilityStatus::Unknown)
        }
    }
}

fn status_digest(
    provider: &str,
    credential_ref: &str,
    status: AuthCapabilityStatus,
) -> (AuthCapabilityStatus, String) {
    (
        status,
        digest_text(&format!(
            "auth-capability-status\0{provider}\0{credential_ref}\0{}",
            status.as_str()
        )),
    )
}

fn receipt_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<AuthCapabilityReceipt> {
    Ok(AuthCapabilityReceipt {
        id: row.get("id"),
        objective_id: row.get("objective_id"),
        objective_revision: row.get("objective_revision"),
        request_key: row.get("request_key"),
        provider: row.get("provider"),
        credential_ref: row.get("credential_ref"),
        capability_digest: row.get("capability_digest"),
        status: AuthCapabilityStatus::parse(row.get::<String, _>("status").as_str())?,
        source: row.get("source"),
        observed_at: row.get("observed_at"),
    })
}

fn digest_text(value: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(value.as_bytes()))
}

fn validate_public_identifier(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        bail!("{label} must be a non-empty public identifier");
    }
    let lowercase = value.to_ascii_lowercase();
    if [
        "access_token",
        "refresh_token",
        "bearer ",
        "client_secret",
        "password=",
    ]
    .iter()
    .any(|marker| lowercase.contains(marker))
    {
        bail!("{label} appears to contain credential material");
    }
    Ok(())
}
