// SPDX-License-Identifier: Apache-2.0
//! Event sink abstraction — the loop's output seam.
//!
//! Introduced in keystone slice 1 and moved here (slice 4.1) so the tauri-free
//! shared loop crate owns it. The agent loop's coupling to any concrete output
//! surface is one thing: `emit(StreamEvent)`. Emitting through this trait lets
//! the SAME loop drive the Tauri frontend (`TauriEventSink`, which stays in the
//! bin crate because it owns an `AppHandle`) or a headless surface (a collecting
//! or JSONL sink) with no code change. `crate::agent::events` re-exports
//! `EventSink` and `CollectingEventSink`, so existing paths keep compiling.

use crate::types::StreamEvent;

/// Where the agent loop sends its stream events. `Send + Sync` so it can live
/// behind an `Arc` shared across the loop's async tasks.
#[async_trait::async_trait]
pub trait EventSink: Send + Sync {
    fn emit(&self, event: StreamEvent);

    /// A usage row was just persisted (`Persistence::record_usage` returned a
    /// NEW row). The desktop `TauriEventSink` overrides this to refresh its cost
    /// UI (`model-usage-recorded` / `token-usage-recorded`); every other sink —
    /// headless, collecting — keeps the defaulted no-op. Keystone slice 4.6:
    /// the loop calls this instead of touching a raw `AppHandle`.
    fn usage_recorded(&self, _session_id: &str) {}

    /// One model round finished (after its tool batch, if any). Defaulted
    /// no-op; the eval sidecar overrides it to satisfy its bridge invariant —
    /// EVERY model round must emit at least one line carrying usage, either a
    /// `tool_request` or a `usage_snapshot`. A round whose tool calls were all
    /// denied emits no `tool_request`, so the sink fills the gap here
    /// (keystone slice 4.8 b14).
    async fn round_ended(&self) {}
}

/// Test / headless sink: records every event in order for assertion. Tauri-free,
/// so it lives in the shared crate alongside the trait.
#[derive(Default)]
pub struct CollectingEventSink {
    events: std::sync::Mutex<Vec<StreamEvent>>,
}

impl CollectingEventSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of the events emitted so far, in order.
    pub fn events(&self) -> Vec<StreamEvent> {
        self.events.lock().expect("event sink mutex").clone()
    }
}

#[async_trait::async_trait]
impl EventSink for CollectingEventSink {
    fn emit(&self, event: StreamEvent) {
        self.events.lock().expect("event sink mutex").push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collecting_sink_records_events_in_order() {
        let sink = CollectingEventSink::new();
        sink.emit(StreamEvent::TextDelta { content: "a".into() });
        sink.emit(StreamEvent::Done {
            input_tokens: 1,
            output_tokens: 2,
        });
        let events = sink.events();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], StreamEvent::TextDelta { .. }));
        assert!(matches!(
            events[1],
            StreamEvent::Done {
                input_tokens: 1,
                output_tokens: 2
            }
        ));
    }

    #[test]
    fn sink_is_object_safe_behind_a_trait_object() {
        // The loop stores `Arc<dyn EventSink>`; prove the trait is object-safe
        // and dispatches to the concrete impl.
        let sink: std::sync::Arc<dyn EventSink> = std::sync::Arc::new(CollectingEventSink::new());
        sink.emit(StreamEvent::Error {
            message: "x".into(),
        });
    }
}
