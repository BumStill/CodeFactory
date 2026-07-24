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

    fn usage_recorded(&self, session_id: &str) {
        // Fire the cost-UI refresh events (moved here from the loop in slice 4.6
        // so the loop needs no `AppHandle`). Both fire for footer compatibility
        // during the migration. Best-effort — a closed window never fails a turn.
        use tauri::Emitter;
        self.app.emit("model-usage-recorded", session_id).ok();
        self.app.emit("token-usage-recorded", session_id).ok();
    }
}
