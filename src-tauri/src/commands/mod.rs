// SPDX-License-Identifier: Apache-2.0
pub mod backup;
pub mod benchmark;
pub mod chat;
pub mod checkpoints;
pub mod control_plane;
pub mod costs;
pub mod evidence;
pub mod evolution;
pub mod files;
pub mod git;
pub mod git_remote;
pub mod hooks;
pub mod interjections;
pub mod knowledge;
pub mod learning;
pub mod mcp;
pub mod memory;
pub mod models;
pub mod preferences;
pub mod session;
#[cfg(test)]
mod session_quick_tests;
pub mod settings;
pub mod skills;
pub mod specs;
pub mod tasks;
pub mod terminal;
#[cfg(test)]
mod usage_acceptance_tests;
