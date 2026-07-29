// SPDX-License-Identifier: Apache-2.0
//! Persistence + budget seams (keystone slice 4.2, refined in 4.4a).
//!
//! [`Persistence`] is a write-only journal so the loop never touches `sqlx`
//! directly. The desktop `SqlitePersistence` owns the pool AND the `anonymous`
//! flag — every DB write returns early when anonymous, moving the scattered
//! `if self.anonymous` checks into one place. The sidecar's `NullPersistence`
//! no-ops everything.
//!
//! The method shapes mirror the desktop loop's real inherent helpers so two
//! load-bearing properties survive by construction (see the adversarial map for
//! slice 4.4):
//! - [`Persistence::persist_message`] returns `Option<String>` — the id, or
//!   `None` when NOT written (anonymous). Callers key `mark_rejected_candidate`
//!   and tool-start off that id, so an anonymous impl MUST return `Ok(None)`.
//! - [`Persistence::persist_message`] (redacted) and
//!   [`Persistence::persist_gate_message`] (RAW) stay DISTINCT: gate content
//!   (e.g. `gate_ready`) must byte-match the replayed provider turn, so it is
//!   never blanket-redacted.
//!
//! NOT modelled here (deliberately): `turn_error` (written only in
//! `commands/chat.rs`, not by any loop helper), and the three NON-DB anonymous
//! guards (KB-tool strip, hook disabling) which stay literal in the loop —
//! centralizing them here would silently re-enable KB tools / hooks in
//! anonymous runs.
//!
//! [`Budget`] abstracts the run's stopping condition (iteration ceiling /
//! wall-clock reserve). Consumed by the desktop impls in slice 4.4b-c.

use crate::types::ToolCall;

/// A persistence write failure. Best-effort from the loop's view; the desktop
/// impl maps `sqlx`/app errors into this so the trait stays tauri-free.
#[derive(Debug, Clone)]
pub struct PersistError {
    pub message: String,
}

impl std::fmt::Display for PersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PersistError {}

pub type PersistResult<T> = Result<T, PersistError>;

/// One `usage_events` row. Primitive/borrowed fields only — the bin decomposes
/// its `Usage` into these so the trait carries no bin-specific type.
#[derive(Debug, Clone)]
pub struct UsageRow<'a> {
    pub request_id: String,
    pub session_id: &'a str,
    pub task_id: Option<String>,
    pub surface: &'a str,
    pub provider: String,
    pub endpoint: &'a str,
    pub model: &'a str,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cached_tokens: i64,
    pub actual_cost_usd: Option<f64>,
    pub cost_source: String,
}

#[derive(Debug, Clone)]
pub struct TurnActivityUpdate {
    pub root_turn_id: String,
    pub phase: String,
    pub status: String,
    pub recent_activity_kind: String,
    pub recent_activity_label: String,
    pub waiting_reason: Option<String>,
    pub terminal_reason: Option<String>,
}

/// Write-only persistence. Every method no-ops (returning the "not written"
/// value) when the run is anonymous, inside the impl.
#[async_trait::async_trait]
pub trait Persistence: Send + Sync {
    async fn update_turn_activity(&self, _update: &TurnActivityUpdate) -> PersistResult<i64> {
        Ok(0)
    }

    /// A redacted user/assistant/tool message. Returns the new id, or `None`
    /// when not written (anonymous) — the `None` is control-flow load-bearing.
    #[allow(clippy::too_many_arguments)]
    async fn persist_message(
        &self,
        role: &str,
        content: &str,
        input_tokens: Option<i64>,
        output_tokens: Option<i64>,
        tool_calls: Option<&[ToolCall]>,
        reasoning_content: Option<&str>,
        endpoint_id: Option<&str>,
        model_id: Option<&str>,
        usage_request_id: Option<&str>,
    ) -> PersistResult<Option<String>>;

    /// A RAW (un-redacted) gate/notice row, role `user`, tagged with a
    /// `completion_state` (`gate_recovery` / `gate_ready` / `gate_warning` /
    /// `gate_blocked` / `turn_notice`). DISTINCT from `persist_message` so gate
    /// content is never redacted.
    async fn persist_gate_message(&self, content: &str, state: &str) -> PersistResult<()>;

    /// Dedup-by-marker wrapper around `persist_gate_message`; the anonymous
    /// short-circuit sits BEFORE the dedup read so anonymous runs stay
    /// read-free too.
    async fn persist_gate_message_once(
        &self,
        marker: &str,
        content: &str,
        state: &str,
    ) -> PersistResult<()>;

    /// Collapse the most recent assistant draft to a rejected gate candidate
    /// (`completion_state='rejected_candidate'`). No-ops on `None` id or
    /// anonymous.
    async fn mark_rejected_candidate(&self, message_id: Option<&str>) -> PersistResult<()>;

    /// Record that a tool call was STARTED (paired with the persisted assistant
    /// message). The impl parses `tool_call.function.arguments` internally. No-op
    /// when anonymous.
    async fn record_tool_call_started(
        &self,
        message_id: &str,
        tool_call: &ToolCall,
    ) -> PersistResult<()>;

    /// Record one terminal tool outcome into the trajectory.
    async fn record_tool_call_outcome(
        &self,
        tool_call: &ToolCall,
        status: &str,
        result: Option<&str>,
        error: Option<&str>,
        duration_ms: u64,
    ) -> PersistResult<()>;

    /// Persist a cancelled tool batch. The per-item DB write is anonymous-gated;
    /// the content strings are returned UNCONDITIONALLY (the event/UI path needs
    /// them even in anonymous runs).
    async fn persist_cancelled_tool_batch(
        &self,
        remaining: &[ToolCall],
    ) -> PersistResult<Vec<String>>;

    /// Insert one `usage_events` row. Returns `true` when a NEW row was written
    /// (so the caller can gate its usage-recorded emits); anonymous → `Ok(false)`.
    async fn record_usage(&self, row: UsageRow<'_>) -> PersistResult<bool>;
}

/// The run's stopping condition. `may_continue` is polled between rounds.
pub trait Budget: Send + Sync {
    /// True while another round is permitted. The desktop uses the iteration
    /// ceiling; the sidecar also stops when the wall-clock reserve is hit.
    fn may_continue(&self, iteration: usize) -> bool;

    /// `(remaining_secs, total_secs)` of the run's wall-clock budget, when the
    /// surface has one (keystone slice 4.8c b3). The budget owns the clock, so
    /// the loop can hand it to the completion-policy evaluator — which uses it
    /// for the convergence / time-finalization / delivery-checkpoint windows.
    /// Desktop has no wall clock and keeps the default `None`, which makes the
    /// evaluator behave exactly as before.
    fn wall_time(&self) -> Option<(u64, u64)> {
        None
    }

    /// True while another TOOL CALL may start, checked before each call inside
    /// a batch. A model response can carry several calls, and a wall-clock
    /// surface must be able to stop part-way through one rather than run the
    /// whole batch past its reserve. Desktop bounds itself by iterations only,
    /// so the default never interrupts a batch.
    fn may_start_tool(&self) -> bool {
        true
    }
}

/// Swallows every write. The eval sidecar has no database and the desktop's
/// anonymous runs have nothing to write either, so both carry this rather than
/// branching inside the shared loop (slice 4.8).
pub struct NullPersistence;

#[async_trait::async_trait]
impl Persistence for NullPersistence {
    async fn persist_message(
        &self,
        _role: &str,
        _content: &str,
        _input_tokens: Option<i64>,
        _output_tokens: Option<i64>,
        _tool_calls: Option<&[ToolCall]>,
        _reasoning_content: Option<&str>,
        _endpoint_id: Option<&str>,
        _model_id: Option<&str>,
        _usage_request_id: Option<&str>,
    ) -> PersistResult<Option<String>> {
        Ok(None)
    }
    async fn persist_gate_message(&self, _content: &str, _state: &str) -> PersistResult<()> {
        Ok(())
    }
    async fn persist_gate_message_once(
        &self,
        _marker: &str,
        _content: &str,
        _state: &str,
    ) -> PersistResult<()> {
        Ok(())
    }
    async fn mark_rejected_candidate(&self, _id: Option<&str>) -> PersistResult<()> {
        Ok(())
    }
    async fn record_tool_call_started(
        &self,
        _message_id: &str,
        _tool_call: &ToolCall,
    ) -> PersistResult<()> {
        Ok(())
    }
    async fn record_tool_call_outcome(
        &self,
        _tc: &ToolCall,
        _status: &str,
        _result: Option<&str>,
        _error: Option<&str>,
        _duration_ms: u64,
    ) -> PersistResult<()> {
        Ok(())
    }
    async fn persist_cancelled_tool_batch(
        &self,
        remaining: &[ToolCall],
    ) -> PersistResult<Vec<String>> {
        // Content is returned even by the null impl — the UI path needs it.
        Ok(remaining
            .iter()
            .map(|tc| format!("Cancelled: {}", tc.function.name))
            .collect())
    }
    async fn record_usage(&self, _row: UsageRow<'_>) -> PersistResult<bool> {
        Ok(false)
    }
}

/// Always permits another round — the desktop loop bounds itself with the
/// iteration ceiling (`for iteration in 0..max_iterations`), so it carries this
/// for the shared `LoopServices` contract without consuming it (slice 4.6b).
pub struct NullBudget;

impl Budget for NullBudget {
    fn may_continue(&self, _iteration: usize) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CeilingBudget(usize);
    impl Budget for CeilingBudget {
        fn may_continue(&self, iteration: usize) -> bool {
            iteration < self.0
        }
    }

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: "t".into(),
            r#type: "function".into(),
            function: crate::types::FunctionCall {
                name: name.into(),
                arguments: "{}".into(),
            },
        }
    }

    #[tokio::test]
    async fn null_persistence_is_object_safe_and_never_writes() {
        let p: std::sync::Arc<dyn Persistence> = std::sync::Arc::new(NullPersistence);
        // persist_message returns None (the load-bearing anonymous sentinel).
        assert_eq!(
            p.persist_message(
                "assistant",
                "hi",
                Some(1),
                Some(2),
                None,
                None,
                Some("chatgpt"),
                Some("gpt-5.5"),
                Some("r"),
            )
            .await
            .unwrap(),
            None
        );
        p.persist_gate_message("recover now", "gate_recovery")
            .await
            .unwrap();
        p.mark_rejected_candidate(Some("m1")).await.unwrap();
        p.record_tool_call_outcome(&call("bash"), "done", Some("ok"), None, 5)
            .await
            .unwrap();
        // Cancelled content still comes back even from a no-op journal.
        let cancelled = p
            .persist_cancelled_tool_batch(&[call("read_file"), call("bash")])
            .await
            .unwrap();
        assert_eq!(cancelled.len(), 2);
        // record_usage reports "no new row" so a caller suppresses its emits.
        assert!(!p.record_usage(usage_row("req-1")).await.unwrap());
    }

    fn usage_row(request_id: &str) -> UsageRow<'static> {
        UsageRow {
            request_id: request_id.to_string(),
            session_id: "s1",
            task_id: None,
            surface: "interactive",
            provider: "p".into(),
            endpoint: "e",
            model: "m",
            input_tokens: 10,
            output_tokens: 20,
            reasoning_tokens: 0,
            cached_tokens: 0,
            actual_cost_usd: None,
            cost_source: "unknown".into(),
        }
    }

    #[test]
    fn budget_ceiling_stops_at_the_limit() {
        let b: std::sync::Arc<dyn Budget> = std::sync::Arc::new(CeilingBudget(3));
        assert!(b.may_continue(2));
        assert!(!b.may_continue(3));
    }

    #[test]
    fn persist_error_is_a_std_error() {
        let e: Box<dyn std::error::Error> = Box::new(PersistError {
            message: "db down".into(),
        });
        assert_eq!(e.to_string(), "db down");
    }
}
