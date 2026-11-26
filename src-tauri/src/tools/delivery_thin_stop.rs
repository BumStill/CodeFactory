// SPDX-License-Identifier: Apache-2.0
//! Read the existing cancellation authorities; this gate grants no permission.
use crate::agent::stop_fence::{DeliveryStopGate, SessionStopGate};
use std::sync::Arc;

#[derive(Debug)]
pub(super) struct ThinStopGate {
    db: sqlx::SqlitePool,
    session: String,
    objective: String,
    root: Option<String>,
    foreground: Option<Result<String, String>>,
}
impl ThinStopGate {
    pub(super) fn new(db: sqlx::SqlitePool, session: String, objective: String, root: Option<String>) -> Arc<Self> {
        Arc::new(Self { db, session, objective, root, foreground: None })
    }
    pub(super) async fn foreground(db: sqlx::SqlitePool, session: String, objective: String, root: Option<String>) -> Arc<Self> {
        let mut gate = Self { db, session, objective, root, foreground: None };
        let rows: Result<Vec<(String, Option<String>, Option<String>)>, _> = sqlx::query_as(
            "SELECT run_instance_id,root_turn_id,objective_id FROM chat_run_controls WHERE session_id=? AND status IN ('active','cancel_requested')")
            .bind(&gate.session).fetch_all(&gate.db).await;
        gate.foreground = Some(match rows {
            Ok(rows) if rows.len() == 1 && gate.root.is_some() && rows[0].1 == gate.root
                && rows[0].2.as_ref().is_none_or(|id| id == &gate.objective) => Ok(rows[0].0.clone()),
            Ok(_) => Err("foreground chat run missing, ambiguous, or bound to another task".into()),
            Err(error) => Err(format!("foreground chat identity unreadable: {error}")),
        });
        Arc::new(gate)
    }
    async fn check(&self) -> Result<Option<String>, sqlx::Error> {
        if let Some(reason) = SessionStopGate::new(self.db.clone(), &self.session).stop_reason().await {
            return Ok(Some(reason));
        }
        // Requested is a session-level admission fence. Settled historical
        // intents do not veto a newly admitted Objective/root.
        let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM chat_session_cancel_intents WHERE session_id=? AND status='requested'")
            .bind(&self.session).fetch_one(&self.db).await?;
        if pending != 0 { return Ok(Some("durable session cancellation requested".into())); }
        let current: Option<(Option<String>, String, Option<String>)> = sqlx::query_as(
            "SELECT session_id,status,COALESCE(NULLIF(resume_cursor,''),root_turn_id) FROM objectives WHERE id=?")
            .bind(&self.objective).fetch_optional(&self.db).await?;
        let Some((session, status, root)) = current else { return Ok(Some("delivery Objective missing".into())); };
        if session.as_deref() != Some(&self.session) || self.root.as_ref().is_some_and(|expected| root.as_ref() != Some(expected)) {
            return Ok(Some("delivery Objective/session/root binding changed".into()));
        }
        if status == "cancelled" { return Ok(Some("delivery Objective cancelled".into())); }
        if let Some(binding) = &self.foreground {
            let id = match binding { Ok(id) => id, Err(reason) => return Ok(Some(reason.clone())) };
            let row: Option<(String, Option<String>, Option<String>, Option<i64>)> = sqlx::query_as(
                "SELECT status,root_turn_id,objective_id,cancel_requested_at FROM chat_run_controls WHERE run_instance_id=? AND session_id=?")
                .bind(id).bind(&self.session).fetch_optional(&self.db).await?;
            match row {
                Some((status, root, objective, None)) if status == "active" && root == self.root
                    && objective.as_ref().is_none_or(|id| id == &self.objective) => {},
                _ => return Ok(Some("foreground chat run cancelled, fenced, or replaced".into())),
            }
        }
        // Background local receipt reconciliation is owned by its durable
        // mutation permit, not a new foreground run. Session/Objective/root
        // cancellation checks above still apply; an internal old run fence
        // alone does not discard the system-owned recovery transaction.
        Ok(None)
    }
}
#[async_trait::async_trait]
impl DeliveryStopGate for ThinStopGate {
    async fn stop_reason(&self) -> Option<String> {
        match self.check().await {
            Ok(reason) => reason,
            Err(error) => Some(format!("delivery cancellation state unreadable: {error}")),
        }
    }
}
