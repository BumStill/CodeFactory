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

/// One session's steering state.
///
/// `delivered_ids` is CF-STOP-R4 (M63) idempotency: a client message id that has
/// already been handed to this session can never be handed over a second time.
/// M63's shape was: the loop was parked inside a tool call, the steer sat
/// unread, the user stopped the session, and the continuation re-sent the same
/// message — the same line landed twice. Identity now travels with the message
/// instead of being inferred from its text.
#[derive(Debug, Default)]
pub struct SessionInterjections {
    pub pending: Vec<Interjection>,
    pub delivered_ids: std::collections::HashSet<String>,
}

/// session_id → pending interjections + the ids already delivered.
pub type InterjectionQueue = Arc<Mutex<HashMap<String, SessionInterjections>>>;

/// `true` = queued, `false` = this exact client message was already delivered
/// to this session (a duplicate must not be handed over twice).
#[command]
pub async fn queue_interjection(
    session_id: String,
    message: String,
    client_message_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<bool, AppError> {
    enqueue_interjection(&state.interjections, &session_id, &message, client_message_id.as_deref()).await
}

/// The queueing rule itself, free of `AppState` so it stays directly testable.
pub async fn enqueue_interjection(
    queue: &InterjectionQueue,
    session_id: &str,
    message: &str,
    client_message_id: Option<&str>,
) -> Result<bool, AppError> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Err(AppError::Other("interjection cannot be empty".into()));
    }
    let entry = Interjection {
        session_id: session_id.to_string(),
        message: trimmed.into(),
        at: chrono::Utc::now().timestamp_millis(),
    };
    let mut q = queue.lock().await;
    let session = q.entry(session_id.to_string()).or_default();
    if let Some(id) = client_message_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        if !session.delivered_ids.insert(id.to_string()) {
            return Ok(false);
        }
    }
    session.pending.push(entry);
    Ok(true)
}

/// Drain (return + clear) the pending queue for a session. Used by the
/// scheduler at the start of each task dispatch. The delivered-id set stays:
/// draining means "the message was handed over", not "it may come again".
pub async fn drain_for_session(
    queue: &InterjectionQueue,
    session_id: &str,
) -> Vec<Interjection> {
    let mut q = queue.lock().await;
    let Some(session) = q.get_mut(session_id) else {
        return Vec::new();
    };
    std::mem::take(&mut session.pending)
}

/// The chat loop's view of the same queue. Both consumers drain at their own
/// safe boundary — the scheduler before dispatching the next task, the agent
/// loop before its next model request — so one `queue_interjection` call means
/// the same thing to the user no matter which is running.
pub struct SessionSteerInbox {
    pub queue: InterjectionQueue,
    pub session_id: String,
    /// The session's standing delivery grant, read once when the loop starts.
    /// `capability_override` is synchronous, so it cannot query the row itself,
    /// and the flag is settled at turn entry before the loop is built.
    pub delivery_authorized: bool,
}

#[async_trait::async_trait]
impl codefactory_agent_loop::services::SteerInbox for SessionSteerInbox {
    async fn drain(&self) -> Vec<String> {
        drain_for_session(&self.queue, &self.session_id)
            .await
            .into_iter()
            .map(|item| item.message)
            .collect()
    }

    fn capability_override(
        &self,
        content: &str,
    ) -> Option<codefactory_agent_loop::run::TurnCapability> {
        crate::agent::steer_capability_override_with_authorization(
            content,
            self.delivery_authorized,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn drain_clears_queue() {
        let q: InterjectionQueue = Arc::new(Mutex::new(HashMap::new()));
        {
            let mut g = q.lock().await;
            g.entry("s1".into()).or_default().pending.push(Interjection {
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

    /// CF-STOP-R4 最低证据（幂等的一半）：同一条客户端消息 id 只会被交付一次——
    /// 即使它在被读到之前又被送了一次，也只会落一条。
    #[tokio::test]
    async fn the_same_client_message_is_never_handed_over_twice() {
        let q: InterjectionQueue = Arc::new(Mutex::new(HashMap::new()));
        assert!(enqueue_interjection(&q, "s1", "改用 chrome channel", Some("m-1"))
            .await
            .unwrap());
        assert!(
            !enqueue_interjection(&q, "s1", "改用 chrome channel", Some("m-1"))
                .await
                .unwrap(),
            "同一条消息第二次必须被拒绝"
        );
        // 换一条消息（新 id）当然可以进。
        assert!(enqueue_interjection(&q, "s1", "然后跑测试", Some("m-2"))
            .await
            .unwrap());

        let drained = drain_for_session(&q, "s1").await;
        assert_eq!(
            drained.iter().map(|i| i.message.as_str()).collect::<Vec<_>>(),
            vec!["改用 chrome channel", "然后跑测试"],
            "重复的一条只能落一次"
        );

        // 「读到之后又续跑」也不能把同一条再送一次：id 集合不随 drain 清空。
        assert!(
            !enqueue_interjection(&q, "s1", "改用 chrome channel", Some("m-1"))
                .await
                .unwrap(),
            "已经交付过的消息在续跑时仍然必须被拒绝"
        );
        assert!(drain_for_session(&q, "s1").await.is_empty());
        // 另一个会话不受影响：id 是会话内身份，不是全局身份。
        assert!(enqueue_interjection(&q, "s2", "改用 chrome channel", Some("m-1"))
            .await
            .unwrap());
    }

    /// The inbox is where the 2026-09-08 regression actually bit: it asked
    /// "what does this message mean on its own" and threw away what the
    /// session already held, so `继续` demoted a delivery-authorized session to
    /// local-only work and every commit, PR and release was then refused.
    #[tokio::test]
    async fn a_continuation_steer_does_not_revoke_the_sessions_delivery_grant() {
        use codefactory_agent_loop::services::SteerInbox;
        let authorized = SessionSteerInbox {
            queue: Arc::new(Mutex::new(HashMap::new())),
            session_id: "s1".into(),
            delivery_authorized: true,
        };
        assert_eq!(
            authorized.capability_override("继续"),
            Some(codefactory_agent_loop::run::TurnCapability::Deliver),
        );
        let unauthorized = SessionSteerInbox {
            queue: Arc::new(Mutex::new(HashMap::new())),
            session_id: "s1".into(),
            delivery_authorized: false,
        };
        assert_eq!(
            unauthorized.capability_override("继续"),
            Some(codefactory_agent_loop::run::TurnCapability::Implement),
        );
    }
}
