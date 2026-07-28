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

/// Tool lifecycle callbacks the loop fires around each tool call. Desktop wraps
/// the user's configured `HookRunner`; headless uses [`NoOpHooks`], which allows
/// every tool and records nothing. Both default to allow/no-op so a partial impl
/// degrades safely.
#[async_trait::async_trait]
pub trait LifecycleHooks: Send + Sync {
    /// Fired before a tool runs; returning `false` cancels the call.
    async fn pre_tool(&self, _tool_name: &str, _args: &serde_json::Value) -> bool {
        true
    }
    /// Fired after a tool completes (fire-and-forget; the result is already
    /// truncated by the caller).
    async fn post_tool(&self, _tool_name: &str, _result: &str, _duration_ms: u64) {}
}

/// Headless/no-op hooks: every tool is allowed, nothing is recorded. Owns no
/// `AppHandle`, so the sidecar and the unit-test EXE can construct it freely.
pub struct NoOpHooks;

impl LifecycleHooks for NoOpHooks {}

/// The loop's per-tool authorization outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PermissionOutcome {
    /// Run the tool.
    Allow,
    /// Skip the tool; feed this content back to the model as the tool result.
    Deny(String),
    /// The user cancelled while a prompt was pending: the loop finishes the
    /// remaining tool batch as cancelled and stops.
    Cancelled,
}

/// Decides whether a tool call may run, prompting the user when the policy
/// requires it. Desktop reads the live permission policy and, on `Ask`, emits a
/// prompt to the frontend and waits (or observes a cancellation); headless
/// auto-allows ([`AllowAllPermissions`]) since there is no user and the eval
/// sandbox is the boundary.
#[async_trait::async_trait]
pub trait PermissionGateway: Send + Sync {
    /// `bash_command` is the extracted shell command (for finer-grained
    /// matching), if the call is a `bash` invocation.
    async fn authorize(
        &self,
        tool_call: &crate::types::ToolCall,
        args: &serde_json::Value,
        bash_command: Option<&str>,
    ) -> PermissionOutcome;

    /// Word a completion-policy budget denial for this surface (keystone slice
    /// 4.8c b4). The default is the desktop's user-facing sentence; the eval
    /// sidecar overrides it with its own `policy denied command ({rule}):
    /// {reason}` contract, which its tests pin.
    fn format_budget_denial(&self, _rule: &str, reason: &str) -> String {
        format!(
            "Tool call denied by completion policy: {reason}. Resolve the current completion blocker or finalize."
        )
    }
}

/// Headless permission gateway: every tool is allowed. Owns no `AppHandle`.
pub struct AllowAllPermissions;

#[async_trait::async_trait]
impl PermissionGateway for AllowAllPermissions {
    async fn authorize(
        &self,
        _tool_call: &crate::types::ToolCall,
        _args: &serde_json::Value,
        _bash_command: Option<&str>,
    ) -> PermissionOutcome {
        PermissionOutcome::Allow
    }
}

/// Mid-loop fact-check of the model's reply against the turn's instruction,
/// returning a correction to inject as the next round's prompt (or `None`).
/// Sync on purpose — the desktop impl probes the machine (PATH / `gh`
/// availability) synchronously, exactly as the inline call did. Headless
/// ([`NoOpFactChecker`]) never corrects.
pub trait FactChecker: Send + Sync {
    fn fact_check(&self, reply: &str, instruction: &str) -> Option<String>;
}

/// Headless fact checker: never corrects.
pub struct NoOpFactChecker;

impl FactChecker for NoOpFactChecker {
    fn fact_check(&self, _reply: &str, _instruction: &str) -> Option<String> {
        None
    }
}

/// What one compaction pass did, so the loop can report it.
#[derive(Debug, Default)]
pub struct CompactionOutcome {
    pub messages: Vec<crate::types::ChatMessage>,
    /// True when anything was elided/dropped — the loop emits `ContextCompressed`.
    pub compacted: bool,
    pub elided_count: usize,
    pub tokens_freed: u32,
}

/// How a surface keeps the prompt inside its context budget (keystone slice
/// 4.8c). Called before EVERY model request, and it OWNS the history — the
/// desktop elides oversized messages by token estimate
/// ([`DefaultCompressor`]); the Terminal-Bench sidecar instead applies its
/// destructive char-budget digest. Making this a seam is what lets the sidecar
/// join the shared loop WITHOUT its eval scores moving: a token-based
/// compressor and a char-based compactor are not interchangeable.
pub trait ContextCompactor: Send + Sync {
    fn compact(
        &self,
        messages: Vec<crate::types::ChatMessage>,
        system_prompt: &str,
        context_limit: u32,
    ) -> CompactionOutcome;
}

/// The desktop compactor: today's `compress_if_needed` + the OpenAI tool-call
/// protocol repair, byte-identical to the pre-4.8c inline block.
pub struct DefaultCompressor;

impl ContextCompactor for DefaultCompressor {
    fn compact(
        &self,
        messages: Vec<crate::types::ChatMessage>,
        system_prompt: &str,
        context_limit: u32,
    ) -> CompactionOutcome {
        let compression = crate::context::compress_if_needed(messages, system_prompt, context_limit);
        CompactionOutcome {
            // Storage repair is not enough: compression can change the final
            // provider payload, so enforce the tool-call protocol at the last
            // boundary before the request.
            messages: crate::protocol::repair_openai_tool_protocol(compression.messages),
            compacted: compression.compressed,
            elided_count: compression.elided_count,
            tokens_freed: compression.tokens_freed,
        }
    }
}

/// A compactor that never touches the history — for surfaces whose budget is
/// managed elsewhere (or not at all).
pub struct NoOpCompactor;

impl ContextCompactor for NoOpCompactor {
    fn compact(
        &self,
        messages: Vec<crate::types::ChatMessage>,
        _system_prompt: &str,
        _context_limit: u32,
    ) -> CompactionOutcome {
        CompactionOutcome {
            messages,
            ..Default::default()
        }
    }
}

/// Mid-run user input. The loop drains this at a round boundary — the same
/// place it polls the cancel flag, and for the same reason: an in-flight tool
/// call is never interrupted, but the user should not have to wait out a
/// 150-step turn to correct its course.
///
/// Drained messages are real user input. Unlike the completion gate's injected
/// prompts they are persisted to the transcript and replayed to the model.
///
/// Desktop reads the session's queue from the shared `InterjectionQueue`, which
/// the task scheduler drains at its own boundary — one inbox, one command, two
/// consumers. Headless uses [`NoSteering`].
#[async_trait::async_trait]
pub trait SteerInbox: Send + Sync {
    /// Take everything pending, leaving the inbox empty. Oldest first.
    async fn drain(&self) -> Vec<String>;
}

/// A steer inbox that is always empty — for surfaces with no interactive user
/// (the eval sidecar, unattended runs).
pub struct NoSteering;

#[async_trait::async_trait]
impl SteerInbox for NoSteering {
    async fn drain(&self) -> Vec<String> {
        Vec::new()
    }
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

    #[tokio::test]
    async fn noop_hooks_allow_all_and_are_object_safe() {
        let h: std::sync::Arc<dyn LifecycleHooks> = std::sync::Arc::new(NoOpHooks);
        assert!(h.pre_tool("bash", &serde_json::json!({"cmd": "ls"})).await);
        // post_tool is fire-and-forget: it must simply not panic.
        h.post_tool("bash", "output", 12).await;
    }

    #[test]
    fn a_custom_compactor_displaces_the_default_entirely() {
        // The point of the seam (slice 4.8c): a surface with a DIFFERENT budget
        // discipline — e.g. the eval sidecar's destructive char-budget digest —
        // must be able to replace token-based elision wholesale, so joining the
        // shared loop cannot silently move its scores.
        struct DropAllButLast;
        impl ContextCompactor for DropAllButLast {
            fn compact(
                &self,
                messages: Vec<crate::types::ChatMessage>,
                _system_prompt: &str,
                _context_limit: u32,
            ) -> CompactionOutcome {
                let elided = messages.len().saturating_sub(1);
                CompactionOutcome {
                    messages: messages.into_iter().last().into_iter().collect(),
                    compacted: elided > 0,
                    elided_count: elided,
                    tokens_freed: 0,
                }
            }
        }
        fn msg(text: &str) -> crate::types::ChatMessage {
            crate::types::ChatMessage {
                role: "user".into(),
                content: crate::types::MessageContent::Text(text.into()),
                tool_calls: None,
                tool_call_id: None,
                name: None,
                reasoning_content: None,
            }
        }
        let c: std::sync::Arc<dyn ContextCompactor> = std::sync::Arc::new(DropAllButLast);
        let out = c.compact(vec![msg("a"), msg("b"), msg("c")], "sys", 1_000);
        assert_eq!(out.messages.len(), 1, "custom rule fully replaced the default");
        assert!(out.compacted);
        assert_eq!(out.elided_count, 2);

        // The desktop default leaves a small history untouched (well under the
        // 75% trigger) — i.e. it is genuinely a different discipline.
        let d: std::sync::Arc<dyn ContextCompactor> = std::sync::Arc::new(DefaultCompressor);
        let out = d.compact(vec![msg("a"), msg("b"), msg("c")], "sys", 1_000_000);
        assert_eq!(out.messages.len(), 3);
        assert!(!out.compacted);

        // NoOpCompactor never touches anything.
        let n: std::sync::Arc<dyn ContextCompactor> = std::sync::Arc::new(NoOpCompactor);
        let out = n.compact(vec![msg("a"), msg("b")], "sys", 1);
        assert_eq!(out.messages.len(), 2);
        assert!(!out.compacted);
    }

    #[test]
    fn noop_fact_checker_never_corrects_and_is_object_safe() {
        let f: std::sync::Arc<dyn FactChecker> = std::sync::Arc::new(NoOpFactChecker);
        assert_eq!(f.fact_check("the sky is green", "verify claims"), None);
    }

    #[tokio::test]
    async fn allow_all_permissions_allow_and_are_object_safe() {
        let g: std::sync::Arc<dyn PermissionGateway> = std::sync::Arc::new(AllowAllPermissions);
        let tc = crate::types::ToolCall {
            id: "t".into(),
            r#type: "function".into(),
            function: crate::types::FunctionCall {
                name: "bash".into(),
                arguments: "{}".into(),
            },
        };
        assert_eq!(
            g.authorize(&tc, &serde_json::json!({"command": "rm -rf /"}), Some("rm -rf /"))
                .await,
            PermissionOutcome::Allow
        );
    }
}
