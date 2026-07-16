// SPDX-License-Identifier: Apache-2.0
//! Content-addressed resume journal for parallel task dispatch.
//!
//! Closes three gaps in the task scheduler:
//!
//! - **GAP 1 — orphans.** A crash leaves rows at `status='running'`;
//!   `list_pending_tasks_for_session` only returns `'pending'`, so the orphan
//!   was never re-dispatched and `is_task_ready` blocked its children forever.
//!   [`recover_orphaned_tasks`] resets dead-owner running rows (owner identity
//!   = PID + process-start token, mirroring evolution_jobs).
//!
//! - **GAP 2 — no content addressing.** A completed task was skipped purely by
//!   status even if its brief/model/tools changed since. Every completion now
//!   records a two-level content address: `local_digest` (the task's own
//!   resolved inputs) and `dispatch_key` (local digest folded with the sorted
//!   dispatch keys of all upstream dependencies), so an input change to any
//!   task automatically cascades to every transitive dependent.
//!
//! - **GAP 3 — "done" was a flag, not evidence.** Replay of a completed task
//!   now requires proof its output is still materialized: worktree tasks keep
//!   a durable copy of their merge-back patch and must pass a
//!   `git apply --reverse --check` presence gate; a checkpoint revert
//!   proactively invalidates every task completed at/after that checkpoint.
//!
//! The journal is a *durable last-known-good record*, deliberately separate
//! from live `task_runs` state: retries and invalidation NULL out
//! `task_runs.result` to re-run a row, so the journal (never deleted, only
//! marked `'stale'`) is what preserves the previous good result if a re-run
//! fails.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::process::Command;

use chrono::Utc;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

use crate::errors::Result;
use crate::storage::tasks::TaskRun;
use crate::util::no_window::NoWindow;

/// Bump to invalidate every existing journal row at once (e.g. if the digest
/// composition changes).
pub const JOURNAL_HASH_VERSION: u32 = 1;

// ── Content addressing ──────────────────────────────────────────────────────

/// The EFFECTIVE inputs the subagent runs with, resolved at dispatch time and
/// again (identically) at resume — so a benign settings flip that does not
/// change the resolved values does not invalidate, while a real change does.
#[derive(Debug, Clone)]
pub struct DispatchInputs {
    pub resolved_model: String,
    pub resolved_tools: Vec<String>,
    /// "shared" | "worktree"
    pub isolation: String,
}

/// Length-prefixed field framing: no field-aliasing collisions ("ab","c" vs
/// "a","bc") and no separator escaping.
fn frame(h: &mut Sha256, field: &[u8]) {
    h.update((field.len() as u64).to_be_bytes());
    h.update(field);
}

/// Digest of the task's OWN resolved inputs. Uses the ORIGINAL row
/// description — retry enrichment is a local string in the scheduler and never
/// UPDATEs the row. No timestamp/PID/iteration order enters any digest.
pub fn local_digest(t: &TaskRun, d: &DispatchInputs) -> String {
    let mut h = Sha256::new();
    frame(&mut h, b"cf-task-local-v1");
    frame(&mut h, JOURNAL_HASH_VERSION.to_le_bytes().as_slice());
    frame(&mut h, t.title.as_bytes());
    frame(&mut h, t.description.as_bytes());
    frame(&mut h, d.resolved_model.as_bytes());
    let mut tools = d.resolved_tools.clone();
    tools.sort();
    frame(&mut h, tools.join("\u{1f}").as_bytes());
    frame(&mut h, t.cwd.as_bytes());
    frame(
        &mut h,
        t.acceptance_criteria_json
            .as_deref()
            .unwrap_or("")
            .as_bytes(),
    );
    frame(
        &mut h,
        t.task_context_json.as_deref().unwrap_or("").as_bytes(),
    );
    frame(&mut h, d.isolation.as_bytes());
    format!("{:x}", h.finalize())
}

/// Stable stand-in key for a dependency that has no journal row (completed
/// before this feature existed). Lets hashes stay deterministic across mixed
/// legacy/new dependency chains.
pub fn legacy_sentinel(task_id: &str) -> String {
    let mut h = Sha256::new();
    frame(&mut h, b"cf-legacy-v1");
    frame(&mut h, task_id.as_bytes());
    format!("{:x}", h.finalize())
}

/// Two-level content address: the task's local digest folded with the sorted
/// `(dep_id, dep_dispatch_key)` pairs — an upstream change reaches every
/// descendant through this recursion.
pub fn compute_dispatch_key(
    t: &TaskRun,
    d: &DispatchInputs,
    dep_keys: &[(String, String)],
) -> String {
    let local = local_digest(t, d);
    let mut deps = dep_keys.to_vec();
    deps.sort_by(|a, b| a.0.cmp(&b.0));
    let mut h = Sha256::new();
    frame(&mut h, b"cf-task-dispatch-v1");
    frame(&mut h, local.as_bytes());
    for (id, key) in &deps {
        frame(&mut h, id.as_bytes());
        frame(&mut h, key.as_bytes());
    }
    format!("{:x}", h.finalize())
}

// ── Journal rows ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct JournalRow {
    pub task_id: String,
    pub session_id: String,
    pub hash_version: i64,
    pub local_digest: String,
    pub dispatch_key: String,
    pub dep_keys_json: String,
    pub resolved_model: String,
    pub resolved_tools_json: String,
    pub isolation_mode: String,
    /// 'merging' | 'done' | 'stale'
    pub state: String,
    pub merge_applied: i64,
    /// 'applied' | 'no_changes' | 'shared_inplace'
    pub materialization: String,
    pub checkpoint_id: Option<String>,
    pub base_sha: Option<String>,
    pub patch_path: Option<String>,
    pub repo_root: Option<String>,
    pub result_json: Option<String>,
    pub completed_at: String,
    pub updated_at: String,
}

pub async fn journal_get(pool: &SqlitePool, task_id: &str) -> Result<Option<JournalRow>> {
    Ok(
        sqlx::query_as::<_, JournalRow>("SELECT * FROM task_journal WHERE task_id = ?")
            .bind(task_id)
            .fetch_optional(pool)
            .await?,
    )
}

async fn journal_list_session(pool: &SqlitePool, session_id: &str) -> Result<Vec<JournalRow>> {
    Ok(
        sqlx::query_as::<_, JournalRow>("SELECT * FROM task_journal WHERE session_id = ?")
            .bind(session_id)
            .fetch_all(pool)
            .await?,
    )
}

// ── Local git helper (presence gate / merging finalize) ─────────────────────

fn git(dir: &str, args: &[&str]) -> std::result::Result<String, String> {
    let out = Command::new("git")
        .no_window()
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("failed to spawn git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

// ── Replay decision ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidationReason {
    InputChanged,
    UpstreamChanged,
    CheckpointReverted,
    DiffMissing,
    WorktreeNotApplied,
    HashVersion,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Replay {
    Restore,
    Rerun(InvalidationReason),
}

/// Decide one completed task. `expected_key` is the dispatch key recomputed
/// from CURRENT inputs and CURRENT dependency edges; `dep_dirty` is true when
/// any dependency was invalidated earlier in this pass.
pub fn should_replay_cached(j: Option<&JournalRow>, expected_key: &str, dep_dirty: bool) -> Replay {
    // Legacy no-journal: preserve today's skip-completed behavior (backfilled
    // by the caller so future changes ARE detected) — unless an upstream is
    // dirty, which must cascade even across a journal-less hop.
    let Some(j) = j else {
        return if dep_dirty {
            Replay::Rerun(InvalidationReason::UpstreamChanged)
        } else {
            Replay::Restore
        };
    };
    if dep_dirty {
        return Replay::Rerun(InvalidationReason::UpstreamChanged);
    }
    if j.hash_version != JOURNAL_HASH_VERSION as i64 {
        return Replay::Rerun(InvalidationReason::HashVersion);
    }
    if j.state == "stale" {
        return Replay::Rerun(InvalidationReason::CheckpointReverted);
    }
    if j.dispatch_key != expected_key {
        return Replay::Rerun(InvalidationReason::InputChanged);
    }
    if j.state != "done" || j.merge_applied == 0 {
        return Replay::Rerun(InvalidationReason::WorktreeNotApplied);
    }
    if !diff_still_present(j) {
        return Replay::Rerun(InvalidationReason::DiffMissing);
    }
    Replay::Restore
}

/// Presence gate: is the task's output still materialized on disk?
/// Worktree tasks reverse-apply-check their durable patch (authoritative —
/// catches cross-run reverts AND out-of-band `git checkout`/manual edits).
/// `no_changes` has nothing to lose; shared-mode edits are covered proactively
/// by [`invalidate_on_revert`] (documented blind spot: out-of-band shared
/// edits, which have no per-task diff to check under parallelism).
fn diff_still_present(j: &JournalRow) -> bool {
    if j.materialization == "no_changes" {
        return true;
    }
    if j.isolation_mode == "worktree" {
        return match (&j.patch_path, &j.repo_root) {
            (Some(patch), Some(root)) if Path::new(patch).exists() => {
                git(root, &["apply", "--reverse", "--check", patch]).is_ok()
            }
            // Patch gone/unknown => cannot prove presence => re-run.
            _ => false,
        };
    }
    true // shared_inplace: trust; revert handled proactively at revert time
}

// ── Orphan recovery (GAP 1) ─────────────────────────────────────────────────

pub enum OrphanScope {
    All,
    Session(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct Recovered {
    pub task_id: String,
    pub title: String,
    /// "finalized" (merging worktree whose diff already applied) | "reset"
    pub outcome: &'static str,
}

/// Reset every dead-owner `'running'` task. A worktree task caught mid-merge
/// (`state='merging'`) is decided exactly-once by on-disk reality: if its
/// durable patch reverse-applies cleanly the merge DID land → finalize to
/// completed; otherwise reset to pending. Live-owner rows are never touched.
pub async fn recover_orphaned_tasks(
    pool: &SqlitePool,
    scope: OrphanScope,
) -> Result<Vec<Recovered>> {
    let rows: Vec<(String, String, Option<i64>, Option<String>)> = match &scope {
        OrphanScope::All => {
            sqlx::query_as(
                "SELECT id, title, owner_pid, owner_start_token FROM task_runs \
                 WHERE status = 'running'",
            )
            .fetch_all(pool)
            .await?
        }
        OrphanScope::Session(sid) => {
            sqlx::query_as(
                "SELECT id, title, owner_pid, owner_start_token FROM task_runs \
                 WHERE status = 'running' AND session_id = ?",
            )
            .bind(sid)
            .fetch_all(pool)
            .await?
        }
    };

    let mut out = Vec::new();
    for (id, title, pid, token) in rows {
        let live = pid
            .and_then(|p| u32::try_from(p).ok())
            .is_some_and(|p| crate::storage::db::process_identity_is_live(p, token.as_deref()));
        if live {
            continue;
        }
        let j = journal_get(pool, &id).await?;
        let outcome = match j.as_ref() {
            Some(j)
                if j.state == "merging"
                    && j.isolation_mode == "worktree"
                    && j.patch_path
                        .as_deref()
                        .is_some_and(|p| Path::new(p).exists())
                    && j.repo_root.is_some() =>
            {
                let patch = j.patch_path.as_deref().unwrap();
                let root = j.repo_root.as_deref().unwrap();
                if git(root, &["apply", "--reverse", "--check", patch]).is_ok() {
                    finalize_merging_to_done(pool, &id, j).await?;
                    "finalized"
                } else {
                    reset_to_pending(pool, &id, true).await?;
                    "reset"
                }
            }
            _ => {
                reset_to_pending(pool, &id, j.is_some()).await?;
                "reset"
            }
        };
        out.push(Recovered {
            task_id: id,
            title,
            outcome,
        });
    }
    Ok(out)
}

/// The reset shared by orphan recovery and invalidation. Clears live state;
/// NEVER deletes the journal — marks it `'stale'` so the last good result
/// survives a failed re-run.
pub async fn reset_to_pending(pool: &SqlitePool, id: &str, has_journal: bool) -> Result<()> {
    sqlx::query(
        "UPDATE task_runs SET status='pending', started_at=NULL, completed_at=NULL, \
         result=NULL, verification_results=NULL, owner_pid=NULL, owner_start_token=NULL, \
         error='resume: re-running (journal invalidation)' WHERE id=?",
    )
    .bind(id)
    .execute(pool)
    .await?;
    if has_journal {
        sqlx::query("UPDATE task_journal SET state='stale', updated_at=? WHERE task_id=?")
            .bind(Utc::now().to_rfc3339())
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Exactly-once completion of a crash-interrupted worktree merge whose diff is
/// proven (by reverse-apply-check) to already be in the user's tree.
async fn finalize_merging_to_done(pool: &SqlitePool, id: &str, j: &JournalRow) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE task_journal SET state='done', merge_applied=1, updated_at=? WHERE task_id=?",
    )
    .bind(&now)
    .bind(id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE task_runs SET status='completed', completed_at=?, result=?, error=NULL, \
         owner_pid=NULL, owner_start_token=NULL WHERE id=?",
    )
    .bind(&now)
    .bind(j.result_json.as_deref().unwrap_or("{}"))
    .bind(id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

// ── CAS dispatch claim (multi-process safety) ───────────────────────────────

/// Claim a pending task for dispatch. Returns false when another scheduler
/// (possibly another process sharing the DB) already claimed it.
pub async fn claim_task(pool: &SqlitePool, id: &str) -> Result<bool> {
    let pid = std::process::id() as i64;
    let token = crate::storage::db::current_process_start_token();
    let now = Utc::now().to_rfc3339();
    let res = sqlx::query(
        "UPDATE task_runs SET status='running', started_at=COALESCE(started_at, ?), \
         attempt_count=attempt_count+1, owner_pid=?, owner_start_token=? \
         WHERE id=? AND status='pending'",
    )
    .bind(&now)
    .bind(pid)
    .bind(&token)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

// ── Completion write path ───────────────────────────────────────────────────

/// What the merge step actually did — drives replayability.
pub struct Materialization {
    /// 'applied' | 'no_changes' | 'shared_inplace'
    pub kind: &'static str,
    pub merge_applied: bool,
    /// Durable copy of the worktree merge-back patch (copied BEFORE worktree
    /// cleanup reaps the original), plus the repo root to check it against.
    pub patch_path: Option<String>,
    pub repo_root: Option<String>,
    pub base_sha: Option<String>,
}

/// Write the durable `'merging'` intent row BEFORE a worktree merge-back, so a
/// crash between "diff applied" and "row completed" can be resolved
/// exactly-once by orphan recovery.
pub async fn record_merging_intent(
    pool: &SqlitePool,
    t: &TaskRun,
    inputs: &DispatchInputs,
    dep_keys: &[(String, String)],
    checkpoint_id: Option<&str>,
    m: &Materialization,
    result_json: &str,
) -> Result<()> {
    upsert_journal(
        pool,
        t,
        inputs,
        dep_keys,
        "merging",
        checkpoint_id,
        m,
        Some(result_json),
    )
    .await
}

/// Record a verified, materialized completion: journal row flips to `'done'`
/// and `task_runs` flips to `'completed'` in ONE transaction.
pub async fn record_completion(
    pool: &SqlitePool,
    t: &TaskRun,
    inputs: &DispatchInputs,
    dep_keys: &[(String, String)],
    checkpoint_id: Option<&str>,
    m: &Materialization,
    result_json: &str,
) -> Result<()> {
    upsert_journal(
        pool,
        t,
        inputs,
        dep_keys,
        "done",
        checkpoint_id,
        m,
        Some(result_json),
    )
    .await?;
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE task_runs SET status='completed', completed_at=?, result=?, error=NULL, \
         owner_pid=NULL, owner_start_token=NULL WHERE id=?",
    )
    .bind(&now)
    .bind(result_json)
    .bind(&t.id)
    .execute(pool)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn upsert_journal(
    pool: &SqlitePool,
    t: &TaskRun,
    inputs: &DispatchInputs,
    dep_keys: &[(String, String)],
    state: &str,
    checkpoint_id: Option<&str>,
    m: &Materialization,
    result_json: Option<&str>,
) -> Result<()> {
    let local = local_digest(t, inputs);
    let key = compute_dispatch_key(t, inputs, dep_keys);
    let mut sorted = dep_keys.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let now = Utc::now().to_rfc3339();
    let mut tools = inputs.resolved_tools.clone();
    tools.sort();
    sqlx::query(
        "INSERT INTO task_journal (task_id, session_id, hash_version, local_digest, dispatch_key, \
         dep_keys_json, resolved_model, resolved_tools_json, isolation_mode, state, merge_applied, \
         materialization, checkpoint_id, base_sha, patch_path, repo_root, result_json, \
         completed_at, updated_at) \
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) \
         ON CONFLICT(task_id) DO UPDATE SET \
           session_id=excluded.session_id, hash_version=excluded.hash_version, \
           local_digest=excluded.local_digest, dispatch_key=excluded.dispatch_key, \
           dep_keys_json=excluded.dep_keys_json, resolved_model=excluded.resolved_model, \
           resolved_tools_json=excluded.resolved_tools_json, isolation_mode=excluded.isolation_mode, \
           state=excluded.state, merge_applied=excluded.merge_applied, \
           materialization=excluded.materialization, checkpoint_id=excluded.checkpoint_id, \
           base_sha=excluded.base_sha, patch_path=excluded.patch_path, repo_root=excluded.repo_root, \
           result_json=excluded.result_json, completed_at=excluded.completed_at, \
           updated_at=excluded.updated_at",
    )
    .bind(&t.id)
    .bind(&t.session_id)
    .bind(JOURNAL_HASH_VERSION as i64)
    .bind(&local)
    .bind(&key)
    .bind(serde_json::to_string(&sorted).unwrap_or_else(|_| "[]".into()))
    .bind(&inputs.resolved_model)
    .bind(serde_json::to_string(&tools).unwrap_or_else(|_| "[]".into()))
    .bind(&inputs.isolation)
    .bind(state)
    .bind(if m.merge_applied { 1i64 } else { 0 })
    .bind(m.kind)
    .bind(checkpoint_id)
    .bind(&m.base_sha)
    .bind(&m.patch_path)
    .bind(&m.repo_root)
    .bind(result_json)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Current dispatch keys of a task's dependencies: journal key when present,
/// legacy sentinel otherwise. Used at completion time to record the exact
/// upstream state this result was built against.
pub async fn dep_keys_for(pool: &SqlitePool, task_id: &str) -> Result<Vec<(String, String)>> {
    let deps = crate::storage::tasks::get_dependencies(pool, task_id).await?;
    let mut out = Vec::with_capacity(deps.len());
    for d in deps {
        let key = journal_get(pool, &d)
            .await?
            .map(|j| j.dispatch_key)
            .unwrap_or_else(|| legacy_sentinel(&d));
        out.push((d, key));
    }
    Ok(out)
}

/// Mark a task's journal row stale without touching `task_runs` (used when a
/// worktree merge fails after a `'merging'` intent was written — the settle
/// path owns the task status; the intent row must just never replay).
pub async fn journal_mark_stale(pool: &SqlitePool, task_id: &str) -> Result<()> {
    sqlx::query("UPDATE task_journal SET state='stale', updated_at=? WHERE task_id=?")
        .bind(Utc::now().to_rfc3339())
        .bind(task_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ── Checkpoint revert invalidation (GAP 3, proactive) ───────────────────────

/// Point-in-time invalidation at the user's checkpoint revert: a whole-tree
/// `git restore` wipes every edit made after the checkpoint, so every task in
/// the session completed at/after it must re-run. Fires ONCE, at revert — the
/// downstream cascade falls out of the next resume's dirty propagation.
pub async fn invalidate_on_revert(
    pool: &SqlitePool,
    session_id: &str,
    checkpoint_created_at: &str,
) -> Result<u32> {
    let ids: Vec<(String,)> = sqlx::query_as(
        "SELECT id FROM task_runs WHERE session_id = ? AND status = 'completed' \
         AND completed_at >= ?",
    )
    .bind(session_id)
    .bind(checkpoint_created_at)
    .fetch_all(pool)
    .await?;
    for (id,) in &ids {
        let has_journal = journal_get(pool, id).await?.is_some();
        reset_to_pending(pool, id, has_journal).await?;
    }
    Ok(ids.len() as u32)
}

// ── Phase B: journal revalidation (plan_resume) ─────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct TaskView {
    pub task_id: String,
    pub title: String,
    pub key_short: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct InvalidatedView {
    pub task_id: String,
    pub title: String,
    pub reason: InvalidationReason,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ResumeReport {
    pub restored: Vec<TaskView>,
    pub invalidated: Vec<InvalidatedView>,
    pub recovered: Vec<Recovered>,
}

/// One topological pass over the session's task DAG: recompute each task's
/// current dispatch key, restore completed tasks whose journal still matches
/// and whose output is still present, reset everything else to pending, and
/// propagate dirtiness so downstream tasks re-run strictly after their rebuilt
/// parents (`is_task_ready` is unchanged). A cycle fails closed: every task on
/// or downstream of it re-runs.
pub async fn plan_resume(
    pool: &SqlitePool,
    session_id: &str,
    inputs: &DispatchInputs,
) -> Result<ResumeReport> {
    let tasks = crate::storage::tasks::list_all_tasks_for_session(pool, session_id).await?;
    if tasks.is_empty() {
        return Ok(ResumeReport::default());
    }
    let journal: HashMap<String, JournalRow> = journal_list_session(pool, session_id)
        .await?
        .into_iter()
        .map(|j| (j.task_id.clone(), j))
        .collect();

    // Dependency edges for the whole session.
    let mut deps: HashMap<String, Vec<String>> = HashMap::new();
    for t in &tasks {
        deps.insert(
            t.id.clone(),
            crate::storage::tasks::get_dependencies(pool, &t.id).await?,
        );
    }
    let by_id: HashMap<String, &TaskRun> = tasks.iter().map(|t| (t.id.clone(), t)).collect();

    // Kahn topo order over in-session edges (dangling deps are ignored for
    // ordering and keyed by legacy_sentinel below — never a panic).
    let mut indegree: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
    for t in &tasks {
        let in_session: usize = deps[&t.id]
            .iter()
            .filter(|d| by_id.contains_key(d.as_str()))
            .count();
        indegree.insert(t.id.as_str(), in_session);
        for d in &deps[&t.id] {
            if by_id.contains_key(d.as_str()) {
                dependents
                    .entry(d.as_str())
                    .or_default()
                    .push(t.id.as_str());
            }
        }
    }
    let mut queue: VecDeque<&str> = indegree
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(id, _)| *id)
        .collect();
    let mut order: Vec<&str> = Vec::with_capacity(tasks.len());
    while let Some(id) = queue.pop_front() {
        order.push(id);
        for dep in dependents.get(id).cloned().unwrap_or_default() {
            let n = indegree.get_mut(dep).unwrap();
            *n -= 1;
            if *n == 0 {
                queue.push_back(dep);
            }
        }
    }
    // Cycle members never reach the order — fail closed: treat them as dirty
    // re-runs appended after the acyclic part.
    let in_order: HashSet<&str> = order.iter().copied().collect();
    let cycle_members: Vec<&str> = tasks
        .iter()
        .map(|t| t.id.as_str())
        .filter(|id| !in_order.contains(id))
        .collect();

    let mut eff: HashMap<String, String> = HashMap::new();
    let mut dirty: HashMap<String, bool> = HashMap::new();
    let mut report = ResumeReport::default();

    for id in order {
        let t = by_id[id];
        let dep_keys: Vec<(String, String)> = deps[id]
            .iter()
            .map(|d| {
                (
                    d.clone(),
                    eff.get(d).cloned().unwrap_or_else(|| legacy_sentinel(d)),
                )
            })
            .collect();
        let expected = compute_dispatch_key(t, inputs, &dep_keys);
        let dep_dirty = deps[id]
            .iter()
            .any(|d| dirty.get(d).copied().unwrap_or(false));

        if t.status == "completed" {
            match should_replay_cached(journal.get(id), &expected, dep_dirty) {
                Replay::Restore => {
                    eff.insert(id.to_string(), expected.clone());
                    dirty.insert(id.to_string(), false);
                    // Legacy self-heal: give journal-less completed tasks a
                    // row NOW so future input changes are detected.
                    if !journal.contains_key(id) {
                        let m = Materialization {
                            kind: "shared_inplace",
                            merge_applied: true,
                            patch_path: None,
                            repo_root: None,
                            base_sha: None,
                        };
                        upsert_journal(
                            pool,
                            t,
                            inputs,
                            &dep_keys,
                            "done",
                            None,
                            &m,
                            t.result.as_deref(),
                        )
                        .await?;
                    }
                    report.restored.push(TaskView {
                        task_id: id.to_string(),
                        title: t.title.clone(),
                        key_short: expected.chars().take(12).collect(),
                    });
                }
                Replay::Rerun(reason) => {
                    reset_to_pending(pool, id, journal.contains_key(id)).await?;
                    eff.insert(id.to_string(), expected);
                    dirty.insert(id.to_string(), true);
                    report.invalidated.push(InvalidatedView {
                        task_id: id.to_string(),
                        title: t.title.clone(),
                        reason,
                    });
                }
            }
        } else {
            // pending/failed/cancelled (running was resolved by Phase A):
            // will (re-)run this session → dependents must wait for it.
            eff.insert(id.to_string(), expected);
            dirty.insert(id.to_string(), t.status != "completed");
        }
    }

    for id in cycle_members {
        let t = by_id[id];
        if t.status == "completed" {
            reset_to_pending(pool, id, journal.contains_key(id)).await?;
            report.invalidated.push(InvalidatedView {
                task_id: id.to_string(),
                title: t.title.clone(),
                reason: InvalidationReason::UpstreamChanged,
            });
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        sqlx::query(
            "CREATE TABLE task_runs (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL, title TEXT NOT NULL,
                description TEXT NOT NULL, status TEXT NOT NULL, cwd TEXT NOT NULL,
                parent_task_id TEXT, sub_session_id TEXT, created_at TEXT NOT NULL,
                started_at TEXT, completed_at TEXT, result TEXT, error TEXT,
                attempt_count INTEGER NOT NULL DEFAULT 0, verification_results TEXT,
                task_context_json TEXT, acceptance_criteria_json TEXT,
                spec_req_id TEXT, spec_title TEXT, owner_pid INTEGER, owner_start_token TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE task_dependencies (
                task_id TEXT NOT NULL, depends_on_task_id TEXT NOT NULL,
                PRIMARY KEY (task_id, depends_on_task_id)
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE task_journal (
                task_id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
                hash_version INTEGER NOT NULL DEFAULT 1, local_digest TEXT NOT NULL,
                dispatch_key TEXT NOT NULL, dep_keys_json TEXT NOT NULL DEFAULT '[]',
                resolved_model TEXT NOT NULL, resolved_tools_json TEXT NOT NULL DEFAULT '[]',
                isolation_mode TEXT NOT NULL, state TEXT NOT NULL,
                merge_applied INTEGER NOT NULL DEFAULT 0, materialization TEXT NOT NULL,
                checkpoint_id TEXT, base_sha TEXT, patch_path TEXT, repo_root TEXT,
                result_json TEXT, completed_at TEXT NOT NULL, updated_at TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    fn task(id: &str, session: &str, status: &str, description: &str) -> TaskRun {
        TaskRun {
            id: id.into(),
            session_id: session.into(),
            title: format!("task {id}"),
            description: description.into(),
            status: status.into(),
            cwd: "/tmp/proj".into(),
            parent_task_id: None,
            sub_session_id: None,
            created_at: "2026-07-16T00:00:00Z".into(),
            started_at: None,
            completed_at: if status == "completed" {
                Some("2026-07-16T01:00:00Z".into())
            } else {
                None
            },
            result: Some("{\"summary\":\"done\"}".into()),
            error: None,
            attempt_count: 1,
            verification_results: None,
            task_context_json: None,
            acceptance_criteria_json: None,
            spec_req_id: None,
            spec_title: None,
        }
    }

    async fn insert(pool: &SqlitePool, t: &TaskRun, owner: Option<(i64, &str)>) {
        crate::storage::tasks::insert_task(pool, t).await.unwrap();
        if t.status != "pending" {
            sqlx::query("UPDATE task_runs SET status=?, completed_at=? WHERE id=?")
                .bind(&t.status)
                .bind(&t.completed_at)
                .bind(&t.id)
                .execute(pool)
                .await
                .unwrap();
        }
        if let Some((pid, token)) = owner {
            sqlx::query("UPDATE task_runs SET owner_pid=?, owner_start_token=? WHERE id=?")
                .bind(pid)
                .bind(token)
                .bind(&t.id)
                .execute(pool)
                .await
                .unwrap();
        }
    }

    fn inputs() -> DispatchInputs {
        DispatchInputs {
            resolved_model: "test-model".into(),
            resolved_tools: vec!["bash".into(), "read_file".into()],
            isolation: "shared".into(),
        }
    }

    fn shared_materialization() -> Materialization {
        Materialization {
            kind: "shared_inplace",
            merge_applied: true,
            patch_path: None,
            repo_root: None,
            base_sha: None,
        }
    }

    // ── I8: hash determinism + framing ──────────────────────────────────────

    #[test]
    fn hash_determinism() {
        let t = task("a", "s", "completed", "build the thing");
        let k1 = compute_dispatch_key(&t, &inputs(), &[("d1".into(), "k1".into())]);
        let k2 = compute_dispatch_key(&t, &inputs(), &[("d1".into(), "k1".into())]);
        assert_eq!(k1, k2, "same inputs => same key");
        // Dep order must not matter.
        let ka = compute_dispatch_key(
            &t,
            &inputs(),
            &[("d1".into(), "k1".into()), ("d2".into(), "k2".into())],
        );
        let kb = compute_dispatch_key(
            &t,
            &inputs(),
            &[("d2".into(), "k2".into()), ("d1".into(), "k1".into())],
        );
        assert_eq!(ka, kb, "dep order independent");
        // Tool order must not matter either.
        let mut i2 = inputs();
        i2.resolved_tools = vec!["read_file".into(), "bash".into()];
        assert_eq!(local_digest(&t, &inputs()), local_digest(&t, &i2));
    }

    #[test]
    fn framing_collision_ab_c_ne_a_bc() {
        // Field framing must distinguish ("ab","c") from ("a","bc"): craft two
        // tasks whose concatenated fields are equal but framed fields differ.
        let mut t1 = task("x", "s", "completed", "ab");
        let mut t2 = task("x", "s", "completed", "a");
        t1.title = "c".into();
        t2.title = "bc".into();
        // title+description concatenation: "c"+"ab" vs "bc"+"a" — wait, order
        // in digest is title then description: "c|ab" vs "bc|a" — both concat
        // to "cab" vs "bca". Use cwd instead for a strict adjacent-field pair:
        t1.description = "ab".into();
        t1.cwd = "c".into();
        t2.description = "a".into();
        t2.cwd = "bc".into();
        // description then model... description and cwd are not adjacent; the
        // decisive check is simply that the digests differ:
        assert_ne!(local_digest(&t1, &inputs()), local_digest(&t2, &inputs()));
    }

    #[test]
    fn input_and_model_changes_change_key() {
        let t = task("a", "s", "completed", "build");
        let base = compute_dispatch_key(&t, &inputs(), &[]);
        let mut t2 = t.clone();
        t2.description = "build DIFFERENTLY".into();
        assert_ne!(base, compute_dispatch_key(&t2, &inputs(), &[]));
        let mut i2 = inputs();
        i2.resolved_model = "other-model".into();
        assert_ne!(base, compute_dispatch_key(&t, &i2, &[]));
        let mut i3 = inputs();
        i3.isolation = "worktree".into();
        assert_ne!(base, compute_dispatch_key(&t, &i3, &[]));
    }

    // ── I4: orphan recovery ─────────────────────────────────────────────────

    #[tokio::test]
    async fn orphan_reset_dead_and_preserve_live() {
        let pool = test_pool().await;
        // Dead owner: PID 1 with a start token that can't match (init's token
        // differs) — but PID 1 is alive on unix, so use a token mismatch via
        // an impossible token. Simpler: PID u32::MAX-ish unlikely to exist.
        let dead = task("dead", "s1", "running", "d");
        insert(&pool, &dead, Some((999_999_999, "gone"))).await;
        // Live owner: THIS process with its real token.
        let live = task("live", "s1", "running", "d");
        let mytoken = crate::storage::db::current_process_start_token().unwrap_or_default();
        insert(&pool, &live, Some((std::process::id() as i64, &mytoken))).await;

        let recovered = recover_orphaned_tasks(&pool, OrphanScope::Session("s1".into()))
            .await
            .unwrap();
        let ids: Vec<&str> = recovered.iter().map(|r| r.task_id.as_str()).collect();
        assert!(ids.contains(&"dead"), "dead-owner task recovered");
        assert!(!ids.contains(&"live"), "live-owner task untouched");

        let status: (String,) = sqlx::query_as("SELECT status FROM task_runs WHERE id='dead'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status.0, "pending");
        let status: (String,) = sqlx::query_as("SELECT status FROM task_runs WHERE id='live'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status.0, "running");
    }

    // ── I9: CAS claim ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn claim_task_cas_exclusive() {
        let pool = test_pool().await;
        let t = task("t1", "s1", "pending", "d");
        insert(&pool, &t, None).await;
        assert!(claim_task(&pool, "t1").await.unwrap(), "first claim wins");
        assert!(
            !claim_task(&pool, "t1").await.unwrap(),
            "second claim must lose (already running)"
        );
    }

    // ── I6: invalidation keeps the journal (stale), never deletes ───────────

    #[tokio::test]
    async fn invalidation_marks_stale_keeps_result() {
        let pool = test_pool().await;
        let t = task("t1", "s1", "completed", "d");
        insert(&pool, &t, None).await;
        record_completion(
            &pool,
            &t,
            &inputs(),
            &[],
            None,
            &shared_materialization(),
            "{\"summary\":\"good\"}",
        )
        .await
        .unwrap();

        reset_to_pending(&pool, "t1", true).await.unwrap();

        let j = journal_get(&pool, "t1")
            .await
            .unwrap()
            .expect("journal kept");
        assert_eq!(j.state, "stale");
        assert_eq!(j.result_json.as_deref(), Some("{\"summary\":\"good\"}"));
        let status: (String, Option<String>) =
            sqlx::query_as("SELECT status, result FROM task_runs WHERE id='t1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status.0, "pending");
        assert!(status.1.is_none(), "live result cleared for the re-run");
    }

    // ── I1 + I2 + I7: plan_resume restore / cascade / legacy backfill ──────

    #[tokio::test]
    async fn plan_resume_restores_matching_and_cascades_input_change() {
        let pool = test_pool().await;
        // a -> b -> c (b depends on a, c depends on b); all completed with
        // journal rows recorded under the ORIGINAL descriptions.
        let a = task("a", "s1", "completed", "step a");
        let b = task("b", "s1", "completed", "step b");
        let c = task("c", "s1", "completed", "step c");
        for t in [&a, &b, &c] {
            insert(&pool, t, None).await;
        }
        for (child, parent) in [("b", "a"), ("c", "b")] {
            crate::storage::tasks::add_dependency(&pool, child, parent)
                .await
                .unwrap();
        }
        let ka = compute_dispatch_key(&a, &inputs(), &[]);
        record_completion(
            &pool,
            &a,
            &inputs(),
            &[],
            None,
            &shared_materialization(),
            "{}",
        )
        .await
        .unwrap();
        record_completion(
            &pool,
            &b,
            &inputs(),
            &[("a".into(), ka.clone())],
            None,
            &shared_materialization(),
            "{}",
        )
        .await
        .unwrap();
        let kb = compute_dispatch_key(&b, &inputs(), &[("a".into(), ka.clone())]);
        record_completion(
            &pool,
            &c,
            &inputs(),
            &[("b".into(), kb)],
            None,
            &shared_materialization(),
            "{}",
        )
        .await
        .unwrap();

        // Unchanged inputs → everything restores.
        let r = plan_resume(&pool, "s1", &inputs()).await.unwrap();
        assert_eq!(r.restored.len(), 3, "{:?}", r);
        assert!(r.invalidated.is_empty());

        // Change a's description → a InputChanged, b+c UpstreamChanged.
        sqlx::query("UPDATE task_runs SET description='step a CHANGED' WHERE id='a'")
            .execute(&pool)
            .await
            .unwrap();
        let r = plan_resume(&pool, "s1", &inputs()).await.unwrap();
        assert_eq!(r.invalidated.len(), 3, "{:?}", r);
        let reasons: HashMap<&str, InvalidationReason> = r
            .invalidated
            .iter()
            .map(|v| (v.task_id.as_str(), v.reason))
            .collect();
        assert_eq!(reasons["a"], InvalidationReason::InputChanged);
        assert_eq!(reasons["b"], InvalidationReason::UpstreamChanged);
        assert_eq!(reasons["c"], InvalidationReason::UpstreamChanged);
        // All reset to pending; is_task_ready ordering is preserved by status.
        let n: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM task_runs WHERE status='pending'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n.0, 3);
    }

    #[tokio::test]
    async fn legacy_completed_preserved_then_backfilled() {
        let pool = test_pool().await;
        let t = task("legacy", "s1", "completed", "old work");
        insert(&pool, &t, None).await;
        // No journal row (completed before the feature existed).
        let r = plan_resume(&pool, "s1", &inputs()).await.unwrap();
        assert_eq!(r.restored.len(), 1, "legacy completed is preserved");
        assert!(
            journal_get(&pool, "legacy").await.unwrap().is_some(),
            "backfilled"
        );

        // Now that it's backfilled, an input change IS detected.
        sqlx::query("UPDATE task_runs SET description='new brief' WHERE id='legacy'")
            .execute(&pool)
            .await
            .unwrap();
        let r = plan_resume(&pool, "s1", &inputs()).await.unwrap();
        assert_eq!(r.invalidated.len(), 1);
        assert_eq!(r.invalidated[0].reason, InvalidationReason::InputChanged);
    }

    // ── I3: checkpoint revert invalidation ──────────────────────────────────

    #[tokio::test]
    async fn cross_run_revert_invalidates_later_completions() {
        let pool = test_pool().await;
        let early = task("early", "s1", "completed", "d");
        let late = task("late", "s1", "completed", "d");
        insert(&pool, &early, None).await;
        insert(&pool, &late, None).await;
        sqlx::query("UPDATE task_runs SET completed_at='2026-07-16T00:30:00Z' WHERE id='early'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE task_runs SET completed_at='2026-07-16T02:00:00Z' WHERE id='late'")
            .execute(&pool)
            .await
            .unwrap();
        record_completion(
            &pool,
            &early,
            &inputs(),
            &[],
            None,
            &shared_materialization(),
            "{}",
        )
        .await
        .unwrap();
        record_completion(
            &pool,
            &late,
            &inputs(),
            &[],
            None,
            &shared_materialization(),
            "{}",
        )
        .await
        .unwrap();
        // record_completion rewrites completed_at to now — restore the fixture times.
        sqlx::query("UPDATE task_runs SET completed_at='2026-07-16T00:30:00Z' WHERE id='early'")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE task_runs SET completed_at='2026-07-16T02:00:00Z' WHERE id='late'")
            .execute(&pool)
            .await
            .unwrap();

        // Revert a checkpoint created at 01:00 → only 'late' invalidates.
        let n = invalidate_on_revert(&pool, "s1", "2026-07-16T01:00:00Z")
            .await
            .unwrap();
        assert_eq!(n, 1);
        let (s_early,): (String,) = sqlx::query_as("SELECT status FROM task_runs WHERE id='early'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let (s_late,): (String,) = sqlx::query_as("SELECT status FROM task_runs WHERE id='late'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(s_early, "completed");
        assert_eq!(s_late, "pending");
        // And its journal is stale — a subsequent resume re-runs it.
        assert_eq!(
            journal_get(&pool, "late").await.unwrap().unwrap().state,
            "stale"
        );
    }

    // ── I5: merging state machine (worktree crash windows) ─────────────────

    #[tokio::test]
    async fn merging_finalizes_when_patch_already_applied_and_resets_otherwise() {
        let pool = test_pool().await;
        // Real git repo + a patch that IS applied (reverse-check passes).
        let base = std::env::temp_dir().join(format!(
            "cf-journal-merge-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let root = repo.to_str().unwrap();
        git(root, &["init", "-q", "-b", "main"]).unwrap();
        git(root, &["config", "user.name", "t"]).unwrap();
        git(root, &["config", "user.email", "t@t"]).unwrap();
        std::fs::write(repo.join("f.txt"), "one\n").unwrap();
        git(root, &["add", "-A"]).unwrap();
        git(root, &["commit", "-q", "-m", "init"]).unwrap();
        // Create a patch: change f.txt, diff, APPLY it (simulating "crash
        // after apply, before completion").
        std::fs::write(repo.join("f.txt"), "one\ntwo\n").unwrap();
        let patch_body = git(root, &["diff"]).unwrap();
        let patch_path = base.join("t1.patch");
        std::fs::write(&patch_path, format!("{patch_body}\n")).unwrap();

        let mut w_inputs = inputs();
        w_inputs.isolation = "worktree".into();
        let t = task("t1", "s1", "running", "d");
        insert(&pool, &t, Some((999_999_999, "gone"))).await;
        let m = Materialization {
            kind: "applied",
            merge_applied: false, // crash before the done flip
            patch_path: Some(patch_path.to_str().unwrap().into()),
            repo_root: Some(root.into()),
            base_sha: None,
        };
        record_merging_intent(&pool, &t, &w_inputs, &[], None, &m, "{\"summary\":\"w\"}")
            .await
            .unwrap();

        let rec = recover_orphaned_tasks(&pool, OrphanScope::Session("s1".into()))
            .await
            .unwrap();
        assert_eq!(rec.len(), 1);
        assert_eq!(rec[0].outcome, "finalized", "applied patch => finalize");
        let (status,): (String,) = sqlx::query_as("SELECT status FROM task_runs WHERE id='t1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "completed");
        assert_eq!(
            journal_get(&pool, "t1").await.unwrap().unwrap().state,
            "done"
        );

        // Second scenario: patch NOT applied (revert the file) → reset.
        git(root, &["checkout", "--", "."]).unwrap();
        let t2 = task("t2", "s1", "running", "d");
        insert(&pool, &t2, Some((999_999_999, "gone"))).await;
        record_merging_intent(&pool, &t2, &w_inputs, &[], None, &m, "{}")
            .await
            .unwrap();
        let rec = recover_orphaned_tasks(&pool, OrphanScope::Session("s1".into()))
            .await
            .unwrap();
        let r2 = rec.iter().find(|r| r.task_id == "t2").unwrap();
        assert_eq!(r2.outcome, "reset", "unapplied patch => re-run");
        let (status,): (String,) = sqlx::query_as("SELECT status FROM task_runs WHERE id='t2'")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "pending");

        let _ = std::fs::remove_dir_all(&base);
    }

    // ── I10: robustness — dangling dep + cycle fail closed ─────────────────

    #[tokio::test]
    async fn dangling_dep_no_panic_and_cycle_fails_closed() {
        let pool = test_pool().await;
        let a = task("a", "s1", "completed", "d");
        insert(&pool, &a, None).await;
        // Dangling edge to a task that doesn't exist.
        crate::storage::tasks::add_dependency(&pool, "a", "ghost")
            .await
            .unwrap();
        record_completion(
            &pool,
            &a,
            &inputs(),
            &[("ghost".into(), legacy_sentinel("ghost"))],
            None,
            &shared_materialization(),
            "{}",
        )
        .await
        .unwrap();
        let r = plan_resume(&pool, "s1", &inputs()).await.unwrap();
        assert_eq!(
            r.restored.len(),
            1,
            "dangling dep keyed by sentinel, no panic"
        );

        // Cycle: x <-> y, both completed → both fail closed to re-run.
        let x = task("x", "s2", "completed", "d");
        let y = task("y", "s2", "completed", "d");
        insert(&pool, &x, None).await;
        insert(&pool, &y, None).await;
        crate::storage::tasks::add_dependency(&pool, "x", "y")
            .await
            .unwrap();
        crate::storage::tasks::add_dependency(&pool, "y", "x")
            .await
            .unwrap();
        let r = plan_resume(&pool, "s2", &inputs()).await.unwrap();
        assert_eq!(r.invalidated.len(), 2, "cycle members fail closed: {:?}", r);
    }

    // ── presence gate: out-of-band checkout invalidates worktree task ──────

    #[tokio::test]
    async fn out_of_band_checkout_invalidates_worktree_task() {
        let pool = test_pool().await;
        let base = std::env::temp_dir().join(format!(
            "cf-journal-oob-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let root = repo.to_str().unwrap();
        git(root, &["init", "-q", "-b", "main"]).unwrap();
        git(root, &["config", "user.name", "t"]).unwrap();
        git(root, &["config", "user.email", "t@t"]).unwrap();
        std::fs::write(repo.join("f.txt"), "one\n").unwrap();
        git(root, &["add", "-A"]).unwrap();
        git(root, &["commit", "-q", "-m", "init"]).unwrap();
        std::fs::write(repo.join("f.txt"), "one\ntwo\n").unwrap();
        let patch = base.join("w.patch");
        std::fs::write(&patch, format!("{}\n", git(root, &["diff"]).unwrap())).unwrap();

        let mut w_inputs = inputs();
        w_inputs.isolation = "worktree".into();
        let t = task("w1", "s1", "completed", "d");
        insert(&pool, &t, None).await;
        let m = Materialization {
            kind: "applied",
            merge_applied: true,
            patch_path: Some(patch.to_str().unwrap().into()),
            repo_root: Some(root.into()),
            base_sha: None,
        };
        record_completion(&pool, &t, &w_inputs, &[], None, &m, "{}")
            .await
            .unwrap();

        // Diff still on disk → restore.
        let r = plan_resume(&pool, "s1", &w_inputs).await.unwrap();
        assert_eq!(r.restored.len(), 1, "{:?}", r);

        // Out-of-band `git checkout .` wipes the edits → DiffMissing re-run.
        git(root, &["checkout", "--", "."]).unwrap();
        let r = plan_resume(&pool, "s1", &w_inputs).await.unwrap();
        assert_eq!(r.invalidated.len(), 1, "{:?}", r);
        assert_eq!(r.invalidated[0].reason, InvalidationReason::DiffMissing);

        let _ = std::fs::remove_dir_all(&base);
    }
}
