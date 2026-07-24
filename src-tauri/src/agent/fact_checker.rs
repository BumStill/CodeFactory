// SPDX-License-Identifier: Apache-2.0
//! Desktop fact checker (keystone slice 4.6b).
//!
//! Wraps the free `fact_check_reply` behind the [`FactChecker`] trait so the
//! shared loop can fact-check the model's reply mid-turn without knowing the
//! bin's `AgentMode` or the machine-probing (delivery / PATH) it does. Holds
//! only the run's `AgentMode`; owns no `AppHandle`. The `Interactive` early
//! return stays inside `fact_check_reply`, so this forwards verbatim.

use codefactory_agent_loop::services::FactChecker;

use super::{fact_check_reply, AgentMode};

pub(super) struct DesktopFactChecker {
    pub(super) mode: AgentMode,
}

impl FactChecker for DesktopFactChecker {
    fn fact_check(&self, reply: &str, instruction: &str) -> Option<String> {
        fact_check_reply(reply, instruction, self.mode)
    }
}
