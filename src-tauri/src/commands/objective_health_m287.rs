// SPDX-License-Identifier: Apache-2.0

//! Compatibility wrapper for the existing Objective health command.
//! M287's read-only liveness aggregate is exposed by the sibling
//! `session_liveness::query_session_liveness_metrics` operation.

#[path = "objective_health.rs"]
mod base;

pub use base::*;
