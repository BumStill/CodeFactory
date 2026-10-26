// SPDX-License-Identifier: Apache-2.0
//! M180 regression at the production sensitive/proof entry used by the gateway.
#[path = "../src/util/no_window.rs"]
pub mod no_window;
mod util { pub use crate::no_window; }
#[path = "../src/agent/trusted_git.rs"]
mod trusted_git;

const INCIDENT: &str = "sed -n '175,244p' src-tauri/src/agent/usage_windows.rs; sed -n '223,257p' src-tauri/src/vision/mod.rs; sed -n '35,100p' src-tauri/src/agent/internal_text.rs; grep -n 'attach_usage_settings' src-tauri/src/agent/mod.rs src-tauri/src/agent/model_transport.rs; grep -n 'settings' src-tauri/src/agent/internal_text.rs | head -15; git worktree list; git log -1 --oneline; git log -1 --oneline origin/main";

#[test]
fn m180_incident_segments_do_not_enter_snapshot_approval() {
    for segment in INCIDENT.split(';') {
        println!("segment={segment:?} sensitive={}", trusted_git::sensitive(segment));
    }
    assert!(!trusted_git::sensitive(INCIDENT), "read-only worktree listing must not enter the rewrite snapshot gate");
}

#[test]
fn m180_worktree_queries_and_mutations_remain_distinct() {
    for query in ["git worktree list", "git worktree list --porcelain", "git worktree list -v", "git -C repo worktree list"] {
        assert!(!trusted_git::sensitive(query), "{query}");
    }
    for mutation in ["git worktree remove --force elsewhere", "git worktree prune", "git worktree add ../other", "git worktree move old new", "git worktree lock other", "git worktree list; git reset --hard HEAD", "git worktree list; git push --force origin main"] {
        assert!(trusted_git::sensitive(mutation), "{mutation}");
    }
}
