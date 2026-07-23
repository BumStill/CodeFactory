// SPDX-License-Identifier: Apache-2.0
//! Wire types now live in the tauri-free `codefactory-agent-loop` crate
//! (keystone slice 4.1). This module re-exports them so every existing
//! `crate::openrouter::types::*` path keeps compiling unchanged.
pub use codefactory_agent_loop::types::*;
