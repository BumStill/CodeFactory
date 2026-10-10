// SPDX-License-Identifier: Apache-2.0
//! The app-side half of the local task entry point (U34 / CF-HDE-*).
//!
//! The transport and its security checks live in
//! `crate::headless_dispatch`; this module is only the Tauri command the
//! interface calls with its answer, so the waiting socket caller can be released
//! with a real result instead of timing out.

use std::sync::Arc;

use crate::headless_dispatch::{bridge::DispatchBridge, ErrorBody, Response};

/// The interface's answer to a forwarded request (CF-HDE-R1/R6).
///
/// A late answer — one that arrives after the caller already timed out — is
/// dropped, so a single request is never settled twice.
#[tauri::command]
pub fn dispatch_reply(
    bridge: tauri::State<'_, Arc<DispatchBridge>>,
    request_id: String,
    ok: bool,
    result: Option<serde_json::Value>,
    error: Option<ErrorBody>,
) -> Result<bool, String> {
    let response = if ok {
        Response::ok(result.unwrap_or(serde_json::Value::Null))
    } else {
        Response::failed(error.unwrap_or_else(|| ErrorBody::internal("the request failed")))
    };
    Ok(bridge.settle(&request_id, response))
}
