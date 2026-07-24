// SPDX-License-Identifier: Apache-2.0
//! Loop capability seams (keystone slice 4.6): the desktop-only concerns the
//! shared loop reaches through trait objects instead of touching `Settings`,
//! the DB, or an `AppHandle` directly. Each has a desktop impl in the bin (under
//! `#[cfg(not(test))]`, #166) and a headless/no-op impl for the sidecar.

/// Per-round context decisions that read the live `Settings` (and, for ChatGPT,
/// the session DB). Re-queried EACH round by the loop so a mid-run model/window
/// change takes effect — a frozen snapshot would regress that. Headless returns
/// a fixed window / no vision / no reasoning effort.
#[async_trait::async_trait]
pub trait ContextPolicy: Send + Sync {
    /// `(select_limit(estimated), max_limit)` for the current model's window,
    /// in tokens (matches `context::ContextWindow`'s `u32` fields).
    async fn context_window(&self, estimated_tokens: u32) -> (u32, u32);
    /// Whether the active model accepts image input this round.
    async fn supports_vision(&self) -> bool;
    /// Pre-resolved ChatGPT reasoning effort for this round (empty for api
    /// styles that ignore it), so the transport stays DB-pure (slice 4.4d).
    async fn round_reasoning_effort(&self) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedContext;

    #[async_trait::async_trait]
    impl ContextPolicy for FixedContext {
        async fn context_window(&self, _estimated: u32) -> (u32, u32) {
            (100_000, 200_000)
        }
        async fn supports_vision(&self) -> bool {
            false
        }
        async fn round_reasoning_effort(&self) -> String {
            String::new()
        }
    }

    #[tokio::test]
    async fn context_policy_is_object_safe() {
        let p: std::sync::Arc<dyn ContextPolicy> = std::sync::Arc::new(FixedContext);
        assert_eq!(p.context_window(1_000).await, (100_000, 200_000));
        assert!(!p.supports_vision().await);
        assert!(p.round_reasoning_effort().await.is_empty());
    }
}
