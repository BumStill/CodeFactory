// SPDX-License-Identifier: Apache-2.0
//! Fail-closed restart admission for in-app updates.
//!
//! Downloading an updater payload is harmless, but installing it replaces the
//! running bundle and `relaunch()` terminates every in-process agent future.
//! A single backend snapshot keeps all update entry points on the same rule:
//! no install or relaunch while CodeFactory owns live work.

use serde::Serialize;
use std::sync::atomic::Ordering;
use tauri::State;

use crate::commands::tasks::SchedulerHandles;
use crate::commands::terminal::TerminalState;
use crate::errors::AppError;
use crate::AppState;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct UpdateSafetyStatus {
    pub safe_to_restart: bool,
    pub restart_reserved: bool,
    pub active_chat_turns: usize,
    pub active_task_schedulers: usize,
    pub active_delivery_leases: i64,
    pub pending_permissions: usize,
    pub managed_browser_sessions: usize,
    pub terminal_sessions: usize,
}

impl UpdateSafetyStatus {
    fn evaluate(mut self) -> Self {
        self.safe_to_restart = self.active_chat_turns == 0
            && self.active_task_schedulers == 0
            && self.active_delivery_leases == 0
            && self.pending_permissions == 0
            && self.managed_browser_sessions == 0
            && self.terminal_sessions == 0;
        self
    }
}

async fn count_active_delivery_leases(
    pool: &sqlx::SqlitePool,
    now: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_runs
         WHERE status <> 'completed'
           AND lease_owner IS NOT NULL
           AND lease_expires_at IS NOT NULL
           AND lease_expires_at > ?",
    )
    .bind(now)
    .fetch_one(pool)
    .await
}

#[tauri::command]
pub async fn reserve_update_install(
    state: State<'_, AppState>,
    schedulers: State<'_, SchedulerHandles>,
    terminals: State<'_, TerminalState>,
) -> Result<UpdateSafetyStatus, AppError> {
    // Hold every admission map until the reservation bit is set. Each producer
    // rechecks that bit while holding its own map, closing the check/install
    // race instead of relying on a best-effort snapshot.
    let chat_turns = state.chat_cancels.lock().await;
    let task_schedulers = schedulers.lock().await;
    let permissions = state.pending_permissions.lock().await;
    let terminal_map = terminals.0.lock().await;
    let active_chat_turns = chat_turns.len();
    let active_task_schedulers = task_schedulers.len();
    let pending_permissions = permissions.len();
    let terminal_sessions = terminal_map.len();
    let managed_browser_sessions = crate::tools::browser_session::list_managed_sessions().len();

    // A delivery worker can outlive the chat future that started it. Its
    // unexpired durable lease is therefore independent restart-blocking work.
    let now = chrono::Utc::now().timestamp_millis();
    let pool = state.db.read().await;
    let active_delivery_leases = count_active_delivery_leases(&pool, now).await?;

    let mut status = UpdateSafetyStatus {
        safe_to_restart: false,
        restart_reserved: false,
        active_chat_turns,
        active_task_schedulers,
        active_delivery_leases,
        pending_permissions,
        managed_browser_sessions,
        terminal_sessions,
    }
    .evaluate();
    if status.safe_to_restart {
        status.restart_reserved = state
            .update_restart_reserved
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
        status.safe_to_restart = status.restart_reserved;
    }
    Ok(status)
}

#[tauri::command]
pub async fn release_update_install_reservation(
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    state.update_restart_reserved.store(false, Ordering::SeqCst);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{count_active_delivery_leases, UpdateSafetyStatus};

    #[test]
    fn restart_is_safe_only_when_every_runtime_owner_is_idle() {
        assert!(UpdateSafetyStatus::default().evaluate().safe_to_restart);

        for active_owner in [
            UpdateSafetyStatus {
                active_chat_turns: 1,
                ..UpdateSafetyStatus::default()
            },
            UpdateSafetyStatus {
                active_task_schedulers: 1,
                ..UpdateSafetyStatus::default()
            },
            UpdateSafetyStatus {
                active_delivery_leases: 1,
                ..UpdateSafetyStatus::default()
            },
            UpdateSafetyStatus {
                pending_permissions: 1,
                ..UpdateSafetyStatus::default()
            },
            UpdateSafetyStatus {
                managed_browser_sessions: 1,
                ..UpdateSafetyStatus::default()
            },
            UpdateSafetyStatus {
                terminal_sessions: 1,
                ..UpdateSafetyStatus::default()
            },
        ] {
            assert!(!active_owner.evaluate().safe_to_restart);
        }
    }

    #[tokio::test]
    async fn only_unexpired_nonterminal_delivery_leases_block_restart() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE delivery_runs (
                id TEXT PRIMARY KEY,
                status TEXT NOT NULL,
                lease_owner TEXT,
                lease_expires_at INTEGER
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        for (id, status, owner, expiry) in [
            ("active", "waiting", Some("worker"), Some(2_000_i64)),
            ("expired", "waiting", Some("worker"), Some(999_i64)),
            ("done", "completed", Some("worker"), Some(2_000_i64)),
            ("unowned", "waiting", None, Some(2_000_i64)),
        ] {
            sqlx::query(
                "INSERT INTO delivery_runs (id,status,lease_owner,lease_expires_at)
                 VALUES (?,?,?,?)",
            )
            .bind(id)
            .bind(status)
            .bind(owner)
            .bind(expiry)
            .execute(&pool)
            .await
            .unwrap();
        }

        assert_eq!(count_active_delivery_leases(&pool, 1_000).await.unwrap(), 1);
    }
}
