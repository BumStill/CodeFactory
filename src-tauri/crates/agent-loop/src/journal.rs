// SPDX-License-Identifier: Apache-2.0
//! Persistence + budget seams (keystone slice 4.2).
//!
//! [`Persistence`] is a write-only journal so the loop never touches `sqlx`
//! directly. The desktop `SqlitePersistence` owns the pool AND the `anonymous`
//! flag — every method returns early when anonymous, moving the no-DB-trace
//! guarantee from ~6 scattered `if self.anonymous` checks into one place. The
//! sidecar's `NullPersistence` no-ops everything (the Python bridge owns
//! `trajectory.json`).
//!
//! [`Budget`] abstracts the run's stopping condition — the desktop uses an
//! iteration ceiling, the sidecar adds a wall-clock reserve.
//!
//! These signatures are PROVISIONAL: nothing consumes them yet, so they firm
//! up with zero call-site churn when the desktop impls (slice 4.4) and the
//! loop body (slice 4.6) land. They exist now to lock the seam shape and prove
//! object-safety early.

/// A single tool outcome as the loop hands it to the journal. Deliberately
/// primitive so the trait carries no desktop-specific types.
#[derive(Debug, Clone, Default)]
pub struct JournaledTool {
    pub request_id: String,
    pub command: String,
    pub status: String,
    pub return_code: Option<i32>,
    pub content: Option<String>,
    pub duration_ms: u64,
}

/// Write-only persistence. All methods are best-effort from the loop's view;
/// the desktop impl no-ops them when the run is anonymous.
#[async_trait::async_trait]
pub trait Persistence: Send + Sync {
    /// A user/assistant/tool message. `completion_state` tags gate boundaries
    /// (`gate_recovery` / `gate_ready` / `rejected_candidate` / …).
    async fn persist_message(&self, role: &str, content: &str, completion_state: Option<&str>);

    /// Mark the most recent assistant draft as a rejected gate candidate.
    async fn mark_rejected_candidate(&self);

    /// Record one executed tool call for the trajectory.
    async fn record_tool_call(&self, tool: &JournaledTool);

    /// Record a usage/cost event for one provider round.
    async fn record_usage(&self, input_tokens: u64, output_tokens: u64);
}

/// The run's stopping condition. `may_continue` is polled between rounds.
pub trait Budget: Send + Sync {
    /// True while another round is permitted. The desktop uses the iteration
    /// ceiling; the sidecar also stops when the wall-clock reserve is hit.
    fn may_continue(&self, iteration: usize) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NullPersistence;

    #[async_trait::async_trait]
    impl Persistence for NullPersistence {
        async fn persist_message(&self, _r: &str, _c: &str, _s: Option<&str>) {}
        async fn mark_rejected_candidate(&self) {}
        async fn record_tool_call(&self, _t: &JournaledTool) {}
        async fn record_usage(&self, _i: u64, _o: u64) {}
    }

    struct CeilingBudget(usize);
    impl Budget for CeilingBudget {
        fn may_continue(&self, iteration: usize) -> bool {
            iteration < self.0
        }
    }

    #[tokio::test]
    async fn persistence_and_budget_are_object_safe() {
        let p: std::sync::Arc<dyn Persistence> = std::sync::Arc::new(NullPersistence);
        p.persist_message("user", "hi", None).await;
        p.record_tool_call(&JournaledTool::default()).await;
        let b: std::sync::Arc<dyn Budget> = std::sync::Arc::new(CeilingBudget(3));
        assert!(b.may_continue(2));
        assert!(!b.may_continue(3));
    }
}
