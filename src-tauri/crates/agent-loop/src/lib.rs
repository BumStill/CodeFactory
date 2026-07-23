// SPDX-License-Identifier: Apache-2.0
//! Tauri-free shared agent-loop crate (keystone slice 4).
//!
//! This crate is the single home for the pieces the desktop app and the
//! Terminal-Bench headless sidecar must share: the wire [`types`], the
//! [`events`] output seam, and — in later slices — the pluggable `ToolBackend`
//! / `Persistence` / `ModelTransport` traits and the one loop body both surfaces
//! drive. It MUST NOT depend on `tauri` or on the `codefactory` bin crate. Every
//! `AppHandle`/`SqlitePool` owner (`TauriEventSink`, `DesktopToolBackend`,
//! `SqlitePersistence`) stays in the bin crate under `#[cfg(not(test))]` so the
//! unit-test EXE never links Tauri runtime entrypoints
//! (`STATUS_ENTRYPOINT_NOT_FOUND`, hotfix #166).
//!
//! Slice 4.1: the wire types and `EventSink` moved here; `crate::openrouter`
//! and `crate::agent::events` re-export them, so no call site changes.
//!
//! Slice 4.2 (this): the pluggable seams — [`tool::ToolBackend`],
//! [`transport::ModelTransport`], [`journal::Persistence`]/[`journal::Budget`] —
//! plus the loop-level [`run::RunConfig`]/[`run::RunOutcome`]. Provisional and
//! unconsumed: they lock the seam shapes and prove object-safety before the
//! desktop impls (4.3-4.6) and the sidecar (4.8) wire in.

pub mod events;
pub mod journal;
pub mod run;
pub mod tool;
pub mod transport;
pub mod types;
