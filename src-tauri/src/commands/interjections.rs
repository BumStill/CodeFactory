// SPDX-License-Identifier: Apache-2.0
//! Mid-execution interjections — short user notes the scheduler picks up
//! before dispatching the **next** pending task. Honest scope: we can't
//! safely surgery into a sub-agent that's already mid-tool-call, but
//! redirecting before the next task starts catches most "wait, change X"
//! moments cheaply and predictably.
//!
//! Queue lives in-memory per session — interjections are transient by
//! design. The scheduler drains the queue when it begins dispatching a
//! task and appends the notes to that task's `SubagentBrief.parent_summary`.
//!
//! If you find yourself wanting durable interjections, write them to
//! memory.md or as a preference instead — that's the persistent channel.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{command, State};
use tokio::sync::Mutex;

use crate::errors::AppError;
use crate::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interjection {
    pub session_id: String,
    pub message: String,
    pub at: i64, // unix ms
}

/// session_id → queue of pending interjections. Drained by the scheduler.
pub type InterjectionQueue = Arc<Mutex<HashMap<String, Vec<Interjection>>>>;

#[command]
pub async fn queue_interjection(
    session_id: String,
    message: String,
    state: State<'_, AppState>,
) -> Result<(), AppError> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Err(AppError::Other("interjection cannot be empty".into()));
    }
    let entry = Interjection {
        session_id: session_id.clone(),
        message: trimmed.into(),
        at: chrono::Utc::now().timestamp_millis(),
    };
    let mut q = state.interjections.lock().await;
    q.entry(session_id).or_default().push(entry);
    Ok(())
}

#[command]
pub async fn list_interjections(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<Interjection>, AppError> {
    let q = state.interjections.lock().await;
    Ok(q.get(&session_id).cloned().unwrap_or_default())
}

/// Drain (return + clear) the queue for a session. Used by the scheduler
/// at the start of each task dispatch.
pub async fn drain_for_session(
    queue: &InterjectionQueue,
    session_id: &str,
) -> Vec<Interjection> {
    let mut q = queue.lock().await;
    q.remove(session_id).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn drain_clears_queue() {
        let q: InterjectionQueue = Arc::new(Mutex::new(HashMap::new()));
        {
            let mut g = q.lock().await;
            g.entry("s1".into()).or_default().push(Interjection {
                session_id: "s1".into(),
                message: "hi".into(),
                at: 0,
            });
        }
        let drained = drain_for_session(&q, "s1").await;
        assert_eq!(drained.len(), 1);
        let drained_again = drain_for_session(&q, "s1").await;
        assert!(drained_again.is_empty());
    }
}
