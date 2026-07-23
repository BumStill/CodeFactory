// SPDX-License-Identifier: Apache-2.0
//! Event sink abstraction — keystone slice 1.
//!
//! The agent loop's coupling to the Tauri `AppHandle` is overwhelmingly one
//! thing: `app.emit(event_name, StreamEvent)` — the UI progress stream. This
//! trait lets the loop emit through an interface instead of a concrete
//! `AppHandle`, so the SAME loop can later run headless (a collecting/streaming
//! sink) with the full tool surface. Slice 1 changes nothing the user sees:
//! the desktop app wires a `TauriEventSink` that emits byte-identically.

use crate::openrouter::types::StreamEvent;

/// Where the agent loop sends its stream events. `Send + Sync` so it can live
/// behind an `Arc` shared across the loop's async tasks.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: StreamEvent);
}

/// Production sink: forwards to the Tauri frontend on `stream:<session_id>`,
/// exactly as the loop did inline before. Best-effort (`.ok()`) — a closed
/// window must never fail a turn.
pub struct TauriEventSink {
    app: tauri::AppHandle,
    event_name: String,
}

impl TauriEventSink {
    pub fn new(app: tauri::AppHandle, session_id: &str) -> Self {
        Self {
            app,
            event_name: format!("stream:{session_id}"),
        }
    }
}

impl EventSink for TauriEventSink {
    fn emit(&self, event: StreamEvent) {
        use tauri::Emitter;
        self.app.emit(&self.event_name, event).ok();
    }
}

/// Test / headless sink: records every event in order for assertion.
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
        // Downcast-free check: a second Arc clone sees the same shared state is
        // not needed here; object-safety + no panic is the contract.
    }
}
