// SPDX-License-Identifier: Apache-2.0
//! Event sink wiring for the desktop app.
//!
//! The `EventSink` trait and the tauri-free `CollectingEventSink` now live in
//! the shared `codefactory-agent-loop` crate (keystone slice 4.1); they are
//! re-exported here so every existing `crate::agent::events::*` path keeps
//! compiling. Only `TauriEventSink` — which owns an `AppHandle` — stays in the
//! bin crate, keeping tauri out of the shared loop crate (#166).

pub use codefactory_agent_loop::events::{CollectingEventSink, EventSink};

use crate::openrouter::types::StreamEvent;

/// Production sink: forwards to the Tauri frontend on `stream:<session_id>`,
/// exactly as the loop did inline before. Best-effort (`.ok()`) — a closed
/// window must never fail a turn.
pub struct TauriEventSink {
    app: tauri::AppHandle,
    event_name: String,
    root_turn_id: std::sync::RwLock<Option<String>>,
}

impl TauriEventSink {
    pub fn new(app: tauri::AppHandle, session_id: &str) -> Self {
        Self {
            app,
            event_name: format!("stream:{session_id}"),
            root_turn_id: std::sync::RwLock::new(None),
        }
    }
}

fn scoped_payload(event: &StreamEvent, root_turn_id: Option<&str>) -> serde_json::Value {
    let mut payload = serde_json::to_value(event).expect("stream event serialization");
    if let Some(root) = root_turn_id {
        payload["source_root_turn_id"] = serde_json::Value::String(root.to_owned());
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_payload_retains_root_and_original_event_fields() {
        for root in ["old-root", "next-root"] {
            let payload = scoped_payload(
                &StreamEvent::TextDelta {
                    content: "synthetic".into(),
                },
                Some(root),
            );
            assert_eq!(payload["type"], "text_delta");
            assert_eq!(payload["content"], "synthetic");
            assert_eq!(payload["source_root_turn_id"], root);
        }
        let legacy = scoped_payload(
            &StreamEvent::TextDelta {
                content: "legacy".into(),
            },
            None,
        );
        assert!(legacy.get("source_root_turn_id").is_none());
    }
}

#[async_trait::async_trait]
impl EventSink for TauriEventSink {
    fn emit(&self, event: StreamEvent) {
        use tauri::Emitter;
        let root = self.root_turn_id.read().expect("stream root lock");
        let payload = scoped_payload(&event, root.as_deref());
        self.app.emit(&self.event_name, payload).ok();
    }

    fn bind_root_turn(&self, root_turn_id: Option<&str>) {
        *self.root_turn_id.write().expect("stream root lock") = root_turn_id.map(str::to_owned);
    }

    fn usage_recorded(&self, session_id: &str) {
        // Fire the cost-UI refresh events (moved here from the loop in slice 4.6
        // so the loop needs no `AppHandle`). Both fire for footer compatibility
        // during the migration. Best-effort — a closed window never fails a turn.
        use tauri::Emitter;
        self.app.emit("model-usage-recorded", session_id).ok();
        self.app.emit("token-usage-recorded", session_id).ok();
    }
}
