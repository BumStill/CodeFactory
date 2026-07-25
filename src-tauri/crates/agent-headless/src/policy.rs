// SPDX-License-Identifier: Apache-2.0
//! Runtime policy profiles and wall-clock budget helpers.
//!
//! Extracted verbatim from `main.rs` (keystone slice 4.8a) — a pure module
//! split with ZERO behaviour change, so the later seam adoption (4.8b) shows up
//! as a small readable diff instead of being buried in a 2775-line file.


use codefactory_agent_core::*;
use serde::Deserialize;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RuntimePolicyProfile {
    Product,
    #[default]
    Benchmark,
}

pub(crate) enum RuntimePolicy {
    Product(ProductPolicy),
    Benchmark(BenchmarkPolicy),
}

impl RuntimePolicy {
    pub(crate) fn new(profile: RuntimePolicyProfile, allow_network: bool) -> Self {
        match profile {
            RuntimePolicyProfile::Product => Self::Product(ProductPolicy::new(allow_network)),
            RuntimePolicyProfile::Benchmark => Self::Benchmark(BenchmarkPolicy::new(allow_network)),
        }
    }

    pub(crate) fn evaluate_command(&self, command: &str) -> PolicyDecision {
        match self {
            Self::Product(policy) => policy.evaluate_command(command),
            Self::Benchmark(policy) => policy.evaluate_command(command),
        }
    }
}

pub(crate) fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(crate) fn remaining_wall_time(started: Instant, wall_time_budget_sec: Option<u64>) -> Option<(u64, u64)> {
    let total = wall_time_budget_sec?.max(1);
    let remaining = total.saturating_sub(started.elapsed().as_secs());
    Some((remaining, total))
}

pub(crate) fn clamp_timeout_to_wall_reserve(requested: u64, remaining: u64, reserve: u64) -> u64 {
    requested.min(remaining.saturating_sub(reserve).max(1))
}

pub(crate) fn budget_exhaustion_message(stopped_for_wall_budget: bool) -> &'static str {
    if stopped_for_wall_budget {
        "Stopped because the wall-clock budget entered its final reserve before completion."
    } else {
        "Stopped because the model step budget was exhausted before completion."
    }
}

pub(crate) fn should_finish_after_model_error(wall_time: Option<(u64, u64)>, outcome_count: usize) -> bool {
    let Some((remaining, total)) = wall_time else {
        return false;
    };
    outcome_count > 0 && remaining <= (total / 15).max(60)
}

pub(crate) fn completion_recovery_attempts_after_tool_batch(
    attempts: u32,
    _material_evidence_progress: bool,
) -> u32 {
    attempts
}
