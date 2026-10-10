// SPDX-License-Identifier: Apache-2.0
//! CF-BUILD: one budget for every build cache CodeFactory owns.
//!
//! Tasks used to compile their own multi-gigabyte cargo target directory and
//! never reclaim it (509 GB across 11 directories by 2026-10-10). This module
//! owns the behaviours the spec asks for:
//!
//! * **CF-BLD-R1 budget** — one total ceiling across every cache root, evicting
//!   least-recently-used first and never touching a cache still being built.
//! * **CF-BLD-R2 reclamation** — a cache owned by a task that reached a
//!   terminal state is removed within a bounded time.
//! * **CF-BLD-R3 disk guard** — free space is checked before a build; idle
//!   caches are reclaimed first and an honest sentence is shown if that is not
//!   enough, instead of a build dying half-way through.
//! * **CF-BLD-R7 build limiting** — a bounded number of heavy builds run at
//!   once; the rest queue in FIFO order behind a readable status.
//! * **CF-BLD-R10 audit** — every removal records what, how big, why, and who
//!   triggered it.
//!
//! Everything here is filesystem-only and deterministic, so the budget maths is
//! tested against synthetic directories (see the tests at the bottom).

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// ── Defaults ─────────────────────────────────────────────────────────────────

/// Total ceiling for every cache CodeFactory owns. One warm shared cache costs
/// about 6 GB (measured: a cold full-workspace test build writes 5.97 GB), so
/// 60 GiB holds a shared cache plus several live task caches — and is ~8x below
/// the 509 GB that accumulated when nothing was bounded.
pub const DEFAULT_BUDGET_BYTES: u64 = 60 * 1024 * 1024 * 1024;

/// Free-space floor checked before starting a task or a build. The measured
/// cold build writes 5.97 GB, so 20 GiB leaves room for a build plus the OS.
pub const DEFAULT_FREE_SPACE_FLOOR_BYTES: u64 = 20 * 1024 * 1024 * 1024;

/// Heavy builds allowed at once. Three concurrent builds writing at full speed
/// is the failure that filled the disk; two keeps disk and CPU saturated
/// without the third only slowing the other two down.
pub const DEFAULT_MAX_HEAVY_BUILDS: usize = 2;

/// An in-use marker older than this is treated as a dead builder, so a crashed
/// task cannot pin its cache forever.
pub const IN_USE_STALE_AFTER: Duration = Duration::from_secs(15 * 60);

/// Bounded wait for a *shared dependency* lock before proceeding independently
/// (CF-BLD-R5: parallel tasks must not block each other for long).
pub const SHARED_LOCK_WAIT_LIMIT: Duration = Duration::from_secs(120);

/// Bounded cleanup window after a task reaches a terminal state (CF-BLD-R2).
pub const RECLAIM_WITHIN: Duration = Duration::from_secs(10 * 60);

/// Marker file written inside a cache directory while its owner is building.
pub const IN_USE_MARKER: &str = ".codefactory-in-use";

// ── Types ────────────────────────────────────────────────────────────────────

/// Why a cache directory is being removed. Recorded in the audit log so a user
/// can answer "what cleaned this, and why" (CF-BLD-R10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvictionReason {
    /// Total size exceeded the configured ceiling (R1).
    OverBudget,
    /// The owning task finished, failed or was stopped (R2).
    TaskEnded,
    /// Free space dropped below the floor before a new build (R3).
    DiskPressure,
    /// Upgrade sweep over pre-existing caches (R8).
    Upgrade,
}

impl EvictionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            EvictionReason::OverBudget => "over_budget",
            EvictionReason::TaskEnded => "task_ended",
            EvictionReason::DiskPressure => "disk_pressure",
            EvictionReason::Upgrade => "upgrade",
        }
    }
}

/// One build-cache directory as it exists on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheEntry {
    pub path: PathBuf,
    pub bytes: u64,
    pub files: u64,
    /// Newest mtime seen in the tree — the "last used" signal for LRU.
    pub last_used_unix: u64,
    /// True while a live builder holds the in-use marker.
    pub in_use: bool,
    /// Task/objective the cache belongs to. `None` = shared cache.
    pub owner: Option<String>,
}

/// The eviction decision: what to delete and what that frees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvictionPlan {
    pub reason: EvictionReason,
    pub evict: Vec<CacheEntry>,
    pub freed_bytes: u64,
    pub kept_bytes: u64,
    /// Bytes still over the ceiling after evicting everything evictable. Zero
    /// when the plan brings the total back inside the budget.
    pub overflow_bytes: u64,
    /// Bytes held by in-use caches that were deliberately preserved.
    pub protected_bytes: u64,
}

pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Human-readable size used in user-facing text ("1.5 GB").
pub fn format_gib(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let value = bytes as f64 / GIB;
    if value >= 10.0 {
        format!("{value:.0} GB")
    } else {
        format!("{value:.1} GB")
    }
}

// ── Cache layout ─────────────────────────────────────────────────────────────

/// Per-task cache root inside a managed workspace.
pub fn task_cache_root(workspace: &Path) -> PathBuf {
    workspace.join(".codefactory-cache")
}

/// The repository's shared cargo cache (used by `pnpm cargo:shared` and by the
/// worktree post-checkout hook).
pub fn shared_cache_root(main_checkout: &Path) -> PathBuf {
    main_checkout.join(".codefactory-cache").join("cargo-target")
}

/// CF-BLD-R5: two tasks must never share one target directory for their *final*
/// artifacts — that is what produced "a symbol that does not exist in this
/// source". Only the dependency cache is shared; each task's own target path is
/// keyed by its workspace, so two tasks cannot collide.
pub fn task_target_dir(task_cache_root: &Path, toolchain: &str) -> PathBuf {
    task_cache_root.join(format!("cargo-target-{toolchain}"))
}

/// CF-BLD-R5: bounded wait for the shared dependency lock. Waiting longer than
/// this is worse than compiling the dependency locally, so callers proceed.
pub fn shared_lock_within_limit(waited: Duration) -> bool {
    waited <= SHARED_LOCK_WAIT_LIMIT
}

// ── Scanning ─────────────────────────────────────────────────────────────────

/// Is this cache directory currently being built into?
///
/// The marker holds the builder's heartbeat as unix seconds. A marker that is
/// missing, unreadable or stale means no live builder.
pub fn is_in_use(path: &Path, now: u64, stale_after: Duration) -> bool {
    let marker = path.join(IN_USE_MARKER);
    let mut text = String::new();
    if std::fs::File::open(&marker)
        .and_then(|mut f| f.read_to_string(&mut text))
        .is_err()
    {
        return false;
    }
    match text.trim().parse::<u64>() {
        Ok(beat) => beat.saturating_add(stale_after.as_secs()) >= now,
        Err(_) => false,
    }
}

/// Write/refresh the in-use marker for a cache directory.
pub fn mark_in_use(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    let mut f = std::fs::File::create(path.join(IN_USE_MARKER))?;
    write!(f, "{}", now_unix())
}

/// Clear the in-use marker once a build has finished.
pub fn clear_in_use(path: &Path) {
    let _ = std::fs::remove_file(path.join(IN_USE_MARKER));
}

/// Measure one cache directory: size, file count and newest mtime.
pub fn scan_entry(
    path: &Path,
    owner: Option<String>,
    now: u64,
    stale_after: Duration,
) -> std::io::Result<CacheEntry> {
    let mut bytes = 0u64;
    let mut files = 0u64;
    let mut newest = 0u64;
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue, // unreadable subtree: measure what we can
        };
        if !entry.file_type().is_file() {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            bytes = bytes.saturating_add(meta.len());
            files = files.saturating_add(1);
            if let Ok(modified) = meta.modified() {
                if let Ok(since) = modified.duration_since(UNIX_EPOCH) {
                    newest = newest.max(since.as_secs());
                }
            }
        }
    }
    Ok(CacheEntry {
        path: path.to_path_buf(),
        bytes,
        files,
        last_used_unix: newest,
        in_use: is_in_use(path, now, stale_after),
        owner,
    })
}

/// CF-BLD-R8: only directories CodeFactory created are ever candidates — a
/// cache root, or a cargo target directory. Anything else in the user's
/// checkout is left alone.
pub fn looks_like_managed_cache(name: &str) -> bool {
    name == ".codefactory-cache" || name.starts_with("cargo-target")
}

/// Measure every direct child of `root` that CodeFactory created.
pub fn scan_root(
    root: &Path,
    owner_of: impl Fn(&Path) -> Option<String>,
    now: u64,
    stale_after: Duration,
) -> Vec<CacheEntry> {
    let mut entries = Vec::new();
    let read = match std::fs::read_dir(root) {
        Ok(read) => read,
        Err(_) => return entries,
    };
    for child in read.flatten() {
        let path = child.path();
        if !path.is_dir() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !looks_like_managed_cache(name) {
            continue;
        }
        if let Ok(entry) = scan_entry(&path, owner_of(&path), now, stale_after) {
            entries.push(entry);
        }
    }
    entries
}

/// Remove a cache directory. Returns the bytes freed.
pub fn remove_entry(entry: &CacheEntry) -> std::io::Result<u64> {
    if entry.path.exists() {
        std::fs::remove_dir_all(&entry.path)?;
    }
    Ok(entry.bytes)
}

// ── R1/R2/R8: eviction planning ──────────────────────────────────────────────

fn lru_order(mut candidates: Vec<CacheEntry>) -> Vec<CacheEntry> {
    candidates.sort_by(|a, b| {
        a.last_used_unix
            .cmp(&b.last_used_unix)
            .then_with(|| a.path.cmp(&b.path))
    });
    candidates
}

/// Plan an eviction that brings the total back under `limit`.
///
/// Least-recently-used first, and in-use caches are never candidates. When the
/// remaining total still exceeds the limit — because everything left is in use
/// — the plan reports the shortfall instead of silently giving up.
pub fn plan_eviction(entries: &[CacheEntry], limit: u64, reason: EvictionReason) -> EvictionPlan {
    let total: u64 = entries.iter().map(|e| e.bytes).sum();
    let protected_bytes: u64 = entries.iter().filter(|e| e.in_use).map(|e| e.bytes).sum();
    let mut evict = Vec::new();
    let mut freed_bytes = 0u64;
    if total > limit {
        for entry in lru_order(entries.iter().filter(|e| !e.in_use).cloned().collect()) {
            if total - freed_bytes <= limit {
                break;
            }
            freed_bytes = freed_bytes.saturating_add(entry.bytes);
            evict.push(entry);
        }
    }
    let kept_bytes = total.saturating_sub(freed_bytes);
    EvictionPlan {
        reason,
        evict,
        freed_bytes,
        kept_bytes,
        overflow_bytes: kept_bytes.saturating_sub(limit),
        protected_bytes,
    }
}

/// Plan reclamation for caches whose owning task reached a terminal state.
///
/// Runs regardless of the total budget: a finished task's cache is dead weight
/// the moment the task ends (R2), not something to wait for the ceiling to
/// catch — which is exactly how 160 GB accumulated.
pub fn plan_task_reclaim(entries: &[CacheEntry], ended_owners: &[String]) -> EvictionPlan {
    let evict: Vec<CacheEntry> = entries
        .iter()
        .filter(|e| !e.in_use)
        .filter(|e| {
            e.owner
                .as_ref()
                .map(|owner| ended_owners.iter().any(|ended| ended == owner))
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    let evict = lru_order(evict);
    let freed_bytes = evict.iter().map(|e| e.bytes).sum();
    let total: u64 = entries.iter().map(|e| e.bytes).sum();
    let protected_bytes: u64 = entries.iter().filter(|e| e.in_use).map(|e| e.bytes).sum();
    EvictionPlan {
        reason: EvictionReason::TaskEnded,
        evict,
        freed_bytes,
        kept_bytes: total.saturating_sub(freed_bytes),
        overflow_bytes: 0,
        protected_bytes,
    }
}

/// CF-BLD-R8: one sweep over pre-existing caches — drop caches owned by tasks
/// that are no longer running, then apply the ceiling. The budget pass only
/// sees survivors, so nothing is double-counted.
pub fn upgrade_sweep(
    entries: &[CacheEntry],
    limit: u64,
    ended_owners: &[String],
) -> Vec<EvictionPlan> {
    let mut plans = Vec::new();
    let reclaim = plan_task_reclaim(entries, ended_owners);
    let survivors: Vec<CacheEntry> = if reclaim.evict.is_empty() {
        entries.to_vec()
    } else {
        let gone: Vec<PathBuf> = reclaim.evict.iter().map(|e| e.path.clone()).collect();
        plans.push(reclaim);
        entries
            .iter()
            .filter(|e| !gone.contains(&e.path))
            .cloned()
            .collect()
    };
    let budget = plan_eviction(&survivors, limit, EvictionReason::Upgrade);
    if !budget.evict.is_empty() {
        plans.push(budget);
    }
    plans
}

/// Apply a plan, deleting every listed directory and recording each removal.
/// Returns the bytes actually freed.
pub fn apply_plan(plan: &EvictionPlan, trigger: &str, log: Option<&AuditLog>) -> u64 {
    let mut freed = 0u64;
    for entry in &plan.evict {
        match remove_entry(entry) {
            Ok(bytes) => {
                freed = freed.saturating_add(bytes);
                if let Some(log) = log {
                    let _ = log.record(&EvictionRecord {
                        at_unix: now_unix(),
                        path: entry.path.clone(),
                        bytes,
                        reason: plan.reason,
                        trigger: trigger.to_string(),
                        owner: entry.owner.clone(),
                    });
                }
            }
            Err(error) => {
                tracing::warn!(path = %entry.path.display(), %error, "build cache eviction failed; continuing");
            }
        }
    }
    freed
}

// ── R10: audit trail ─────────────────────────────────────────────────────────

/// One removal, with everything a user needs to explain it afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvictionRecord {
    pub at_unix: u64,
    pub path: PathBuf,
    pub bytes: u64,
    pub reason: EvictionReason,
    /// Who asked: `startup_sweep`, `task_end:<id>`, `disk_guard`, `manual_clean`.
    pub trigger: String,
    pub owner: Option<String>,
}

/// Append-only JSONL log. One line per removal keeps each write tiny, which is
/// the point of the task (R6) as much as the audit is (R10).
#[derive(Debug, Clone)]
pub struct AuditLog {
    file: PathBuf,
}

impl AuditLog {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        AuditLog { file: file.into() }
    }

    pub fn path(&self) -> &Path {
        &self.file
    }

    pub fn record(&self, record: &EvictionRecord) -> std::io::Result<()> {
        if let Some(parent) = self.file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let line = serde_json::to_string(record).unwrap_or_else(|_| "{}".to_string());
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)?;
        writeln!(file, "{line}")
    }

    pub fn read_all(&self) -> Vec<EvictionRecord> {
        let text = match std::fs::read_to_string(&self.file) {
            Ok(text) => text,
            Err(_) => return Vec::new(),
        };
        text.lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<EvictionRecord>(line).ok())
            .collect()
    }
}

// ── R3: disk guard ───────────────────────────────────────────────────────────

/// Free bytes on the volume holding `path`, when the platform can tell us.
#[cfg(unix)]
pub fn free_bytes(path: &Path) -> Option<u64> {
    use std::ffi::CString;
    let c_path = CString::new(path.to_string_lossy().as_bytes()).ok()?;
    // SAFETY: `c_path` is a valid NUL-terminated path and `statvfs` initialises
    // the whole struct on success.
    unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c_path.as_ptr(), &mut stat) != 0 {
            return None;
        }
        Some(stat.f_bavail as u64 * stat.f_frsize as u64)
    }
}

/// Windows fallback: ask the shell for the drive's free space. Deliberately
/// best-effort — an unmeasurable volume must not stop a build.
#[cfg(not(unix))]
pub fn free_bytes(path: &Path) -> Option<u64> {
    let script = format!(
        "(Get-Item -LiteralPath '{}' -ErrorAction Stop).PSDrive.Free",
        path.display()
    );
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// What the guard did before a build was allowed to start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskGuardOutcome {
    /// Free space after any reclamation.
    pub free_bytes: u64,
    pub floor_bytes: u64,
    pub reclaimed_bytes: u64,
    pub still_short_bytes: u64,
}

impl DiskGuardOutcome {
    pub fn admitted(&self) -> bool {
        self.still_short_bytes == 0
    }

    /// The sentence shown to the user. Plain words, no jargon, and it says what
    /// was already cleaned and what is still missing (CF-BLD-R3).
    pub fn user_message(&self) -> String {
        if self.admitted() {
            return format!("磁盘空间充足，可用 {}", format_gib(self.free_bytes));
        }
        format!(
            "空间不足，已清理 {}，还差 {}。请先释放磁盘空间，然后重试。",
            format_gib(self.reclaimed_bytes),
            format_gib(self.still_short_bytes)
        )
    }
}

/// CF-BLD-R3: check free space before a task/build starts, reclaim idle caches
/// when the floor is breached, and report the shortfall honestly.
///
/// `reclaim` performs the cleanup with the missing byte count as its target, so
/// this stays testable without touching a real filesystem.
pub fn guard_disk_space<F>(
    free_now: impl Fn() -> Option<u64>,
    floor: u64,
    mut reclaim: F,
) -> DiskGuardOutcome
where
    F: FnMut(u64) -> u64,
{
    let free = free_now().unwrap_or(u64::MAX);
    if free >= floor {
        return DiskGuardOutcome {
            free_bytes: free,
            floor_bytes: floor,
            reclaimed_bytes: 0,
            still_short_bytes: 0,
        };
    }
    let missing = floor - free;
    let reclaimed = reclaim(missing);
    let after = free.saturating_add(reclaimed);
    DiskGuardOutcome {
        free_bytes: after,
        floor_bytes: floor,
        reclaimed_bytes: reclaimed,
        still_short_bytes: floor.saturating_sub(after),
    }
}

// ── R7: heavy build limiter ──────────────────────────────────────────────────

#[derive(Debug, Default)]
struct LimiterState {
    running: usize,
    waiting: VecDeque<tokio::sync::oneshot::Sender<()>>,
}

/// Bounded concurrency for heavy builds with FIFO admission.
///
/// Queueing is *not* a failure: a queued build has consumed no retry budget and
/// cannot deadlock, because the slot is handed straight to the next waiter when
/// a permit is released (CF-BLD-R7).
#[derive(Debug)]
pub struct HeavyBuildLimiter {
    limit: usize,
    state: tokio::sync::Mutex<LimiterState>,
}

/// Held for the duration of one heavy build. Dropping it releases the slot.
#[derive(Debug)]
pub struct HeavyBuildPermit {
    limiter: Arc<HeavyBuildLimiter>,
}

impl HeavyBuildLimiter {
    pub fn new(limit: usize) -> Arc<Self> {
        Arc::new(HeavyBuildLimiter {
            limit: limit.max(1),
            state: tokio::sync::Mutex::new(LimiterState::default()),
        })
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Would this command compile or run a full test suite? Used to decide
    /// whether a bash-tool invocation needs a heavy-build slot.
    pub fn is_heavy_build_command(command: &str) -> bool {
        let lower = command.to_ascii_lowercase();
        const MARKERS: [&str; 6] = [
            "cargo build",
            "cargo test",
            "cargo check",
            "cargo clippy",
            "pnpm build",
            "pnpm test",
        ];
        MARKERS.iter().any(|marker| lower.contains(marker))
    }

    /// Wait for a slot. Awaits until this build reaches the front of the queue.
    pub async fn acquire(self: &Arc<Self>) -> HeavyBuildPermit {
        let mut state = self.state.lock().await;
        if state.waiting.is_empty() && state.running < self.limit {
            state.running += 1;
            drop(state);
            return HeavyBuildPermit {
                limiter: Arc::clone(self),
            };
        }
        let (sender, receiver) = tokio::sync::oneshot::channel();
        state.waiting.push_back(sender);
        drop(state);
        // The releaser has already counted us as running; we only wait for the
        // hand-off. A dropped sender means the releaser skipped us.
        let _ = receiver.await;
        HeavyBuildPermit {
            limiter: Arc::clone(self),
        }
    }

    pub async fn running(&self) -> usize {
        self.state.lock().await.running
    }

    pub async fn waiting(&self) -> usize {
        self.state.lock().await.waiting.len()
    }

    /// User-facing status while a build waits in the queue.
    pub async fn status_label(&self) -> Option<String> {
        let waiting = self.waiting().await;
        if waiting == 0 {
            None
        } else {
            Some(format!("等待编译空位（前面还有 {waiting} 个构建）"))
        }
    }

    async fn release(&self) {
        let mut state = self.state.lock().await;
        // Hand the slot to the next live waiter; a cancelled waiter (dropped
        // receiver) must not leak the slot, so keep draining.
        while let Some(next) = state.waiting.pop_front() {
            if next.send(()).is_ok() {
                return;
            }
        }
        state.running = state.running.saturating_sub(1);
    }
}

impl Drop for HeavyBuildPermit {
    fn drop(&mut self) {
        let limiter = Arc::clone(&self.limiter);
        // Release without awaiting inside Drop: the hand-off is cheap, so run
        // it on the current runtime when there is one.
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move { limiter.release().await });
        }
    }
}

// ── Reporting ────────────────────────────────────────────────────────────────

/// Serialised overview for the occupancy panel and the background entry point.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildCacheReport {
    pub total_bytes: u64,
    pub budget_bytes: u64,
    pub entries: Vec<CacheEntry>,
    pub heavy_builds_running: usize,
    pub heavy_builds_waiting: usize,
    pub heavy_build_limit: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heavy_build_status: Option<String>,
}

impl BuildCacheReport {
    pub fn over_budget(&self) -> bool {
        self.total_bytes > self.budget_bytes
    }
}

/// Shared helper used by the runtime and by tests.
pub fn describe_entry(entry: &CacheEntry) -> String {
    format!(
        "{} — {} ({} 个文件{})",
        entry.path.display(),
        format_gib(entry.bytes),
        entry.files,
        if entry.in_use { "，构建中" } else { "" }
    )
}

/// Owner id of a managed-workspace cache path — the directory directly below
/// `execution-workspaces/`. Returns `None` for shared caches.
pub fn owner_from_workspace_path(path: &Path, container: &Path) -> Option<String> {
    let relative = path.strip_prefix(container).ok()?;
    relative
        .components()
        .next()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
}

/// CF-BLD-R10: size of one task's own build cache, so "how much did this task
/// cost me" is answerable.
pub fn task_cache_size(workspace: &Path) -> u64 {
    scan_entry(
        &task_cache_root(workspace),
        None,
        now_unix(),
        IN_USE_STALE_AFTER,
    )
    .map(|entry| entry.bytes)
    .unwrap_or(0)
}

// ── Runtime wiring: CF-BLD-R2/R3/R7/R8 in the live build path ────────────────
//
// Everything above is a pure planner. Everything below is what the running app
// calls, so the behaviours exist outside the unit tests: a real shell-tool
// invocation queues behind one machine-wide heavy-build bound, and the app's
// own supervisor reclaims a finished task's cache instead of waiting for the
// disk to fill up and a build to die half-way through.

/// One heavy-build limiter per process (CF-BLD-R7). Every shell-tool invocation
/// queues behind the same bound, so "at most N heavy builds at once" is a
/// property of this machine, not of whichever code path happened to start one.
static HEAVY_BUILDS: std::sync::OnceLock<Arc<HeavyBuildLimiter>> = std::sync::OnceLock::new();

pub fn heavy_build_limiter() -> Arc<HeavyBuildLimiter> {
    Arc::clone(HEAVY_BUILDS.get_or_init(|| HeavyBuildLimiter::new(configured_max_heavy_builds())))
}

fn env_gib(name: &str, default_gib: u64) -> u64 {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    std::env::var(name)
        .ok()
        .and_then(|raw| raw.trim().parse::<f64>().ok())
        .filter(|value| *value > 0.0)
        .map(|value| (value * GIB) as u64)
        .unwrap_or((default_gib as f64 * GIB) as u64)
}

/// Total ceiling for every cache CodeFactory owns (CF-BLD-R1).
pub fn configured_budget_bytes() -> u64 {
    env_gib(
        "CODEFACTORY_BUILD_CACHE_BUDGET_GB",
        DEFAULT_BUDGET_BYTES / (1024 * 1024 * 1024),
    )
}

/// Free-space floor checked before a task or a heavy build starts (CF-BLD-R3).
pub fn configured_free_space_floor_bytes() -> u64 {
    env_gib(
        "CODEFACTORY_BUILD_CACHE_FLOOR_GB",
        DEFAULT_FREE_SPACE_FLOOR_BYTES / (1024 * 1024 * 1024),
    )
}

/// Heavy builds allowed at once, machine-wide (CF-BLD-R7).
pub fn configured_max_heavy_builds() -> usize {
    std::env::var("CODEFACTORY_MAX_HEAVY_BUILDS")
        .ok()
        .and_then(|raw| raw.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_HEAVY_BUILDS)
}

/// How long a cache with no live owner row at all may sit before R8 drops it.
/// Long on purpose: an orphan is a crashed task, and a crashed task is still
/// resumable for far longer than a finished one.
pub const ORPHAN_IDLE_SECS: u64 = 30 * 24 * 60 * 60;

/// Where every removal is recorded (CF-BLD-R10). Kept beside the caches it
/// describes; `scan_root` ignores it because it is not a cache directory.
pub fn audit_log_path(container: &Path) -> PathBuf {
    container.join("build-cache-audit.jsonl")
}

/// Every cache under the managed-workspace container, with its task as owner.
///
/// The container holds one directory per task; each task's own cache lives in
/// `.codefactory-cache/` inside it. Only directories CodeFactory created are
/// ever measured (CF-BLD-R8), and the task directory itself is never touched —
/// removing it belongs to the workspace closeout, not to the build cache.
pub fn scan_workspace_container(container: &Path) -> Vec<CacheEntry> {
    let mut entries = Vec::new();
    let read = match std::fs::read_dir(container) {
        Ok(read) => read,
        Err(_) => return entries,
    };
    for workspace in read.flatten() {
        let path = workspace.path();
        if !path.is_dir() {
            continue;
        }
        let owner = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned);
        entries.extend(scan_root(
            &task_cache_root(&path),
            |_| owner.clone(),
            now_unix(),
            IN_USE_STALE_AFTER,
        ));
    }
    entries
}

/// What one maintenance pass did. Also what the occupancy panel reports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenanceOutcome {
    pub scanned: usize,
    pub before_bytes: u64,
    pub after_bytes: u64,
    pub reclaimed_bytes: u64,
    pub evicted_bytes: u64,
    pub protected_bytes: u64,
    pub overflow_bytes: u64,
    pub removed: Vec<PathBuf>,
}

/// Everything a maintenance pass needs to decide. The caller owns the database
/// question ("which tasks ended?"); this stays filesystem-only and testable.
pub struct MaintenanceRequest<'a> {
    pub container: &'a Path,
    /// Workspace ids whose task reached a terminal state (CF-BLD-R2).
    pub ended_owners: &'a [String],
    /// Workspace ids with no live row at all; dropped only once idle past
    /// `idle_before_unix` (CF-BLD-R8 backlog), never while they look resumable.
    pub orphan_owners: &'a [String],
    /// A zero here disables the orphan idle rule.
    pub idle_before_unix: u64,
    pub limit_bytes: u64,
    pub trigger: &'a str,
}

/// Run R2, then the R8 backlog rule, then R1 — in that order, so the ceiling
/// only counts what actually survived.
pub fn maintain(request: &MaintenanceRequest<'_>, log: Option<&AuditLog>) -> MaintenanceOutcome {
    let entries = scan_workspace_container(request.container);
    maintain_entries(&entries, request, log)
}

/// The same pass over an already-scanned set, so tests can drive synthetic
/// directories without touching a real workspace container.
pub fn maintain_entries(
    entries: &[CacheEntry],
    request: &MaintenanceRequest<'_>,
    log: Option<&AuditLog>,
) -> MaintenanceOutcome {
    let before_bytes: u64 = entries.iter().map(|entry| entry.bytes).sum();
    let protected_bytes: u64 = entries.iter().filter(|e| e.in_use).map(|e| e.bytes).sum();
    let mut removed: Vec<PathBuf> = Vec::new();
    let mut reclaimed_bytes = 0u64;
    let mut evicted_bytes = 0u64;

    // R2 — a finished task's cache is dead weight now, not a budget question.
    let ended = plan_task_reclaim(entries, request.ended_owners);
    reclaimed_bytes = reclaimed_bytes.saturating_add(apply_plan(&ended, request.trigger, log));
    removed.extend(ended.evict.iter().map(|entry| entry.path.clone()));

    // R8 backlog — an orphan nobody can resume and nobody touched for a long
    // time. A live task's cache is never a candidate, however idle it looks.
    if request.idle_before_unix > 0 {
        let orphans: Vec<CacheEntry> = entries
            .iter()
            .filter(|entry| !removed.contains(&entry.path))
            .filter(|entry| !entry.in_use)
            .filter(|entry| {
                entry.owner.as_ref().is_some_and(|owner| {
                    request.orphan_owners.iter().any(|candidate| candidate == owner)
                })
            })
            .filter(|entry| entry.last_used_unix < request.idle_before_unix)
            .cloned()
            .collect();
        if !orphans.is_empty() {
            let freed = orphans.iter().map(|entry| entry.bytes).sum();
            let plan = EvictionPlan {
                reason: EvictionReason::Upgrade,
                evict: lru_order(orphans),
                freed_bytes: freed,
                kept_bytes: before_bytes.saturating_sub(freed),
                overflow_bytes: 0,
                protected_bytes,
            };
            reclaimed_bytes =
                reclaimed_bytes.saturating_add(apply_plan(&plan, request.trigger, log));
            removed.extend(plan.evict.iter().map(|entry| entry.path.clone()));
        }
    }

    // R1 — the survivors answer to the ceiling.
    let survivors: Vec<CacheEntry> = entries
        .iter()
        .filter(|entry| !removed.contains(&entry.path))
        .cloned()
        .collect();
    let budget = plan_eviction(&survivors, request.limit_bytes, EvictionReason::OverBudget);
    evicted_bytes = evicted_bytes.saturating_add(apply_plan(&budget, request.trigger, log));
    removed.extend(budget.evict.iter().map(|entry| entry.path.clone()));

    let after_bytes =
        before_bytes.saturating_sub(reclaimed_bytes.saturating_add(evicted_bytes));
    MaintenanceOutcome {
        scanned: entries.len(),
        before_bytes,
        after_bytes,
        reclaimed_bytes,
        evicted_bytes,
        protected_bytes,
        overflow_bytes: after_bytes.saturating_sub(request.limit_bytes),
        removed,
    }
}

/// CF-BLD-R8: the pass that runs once as the app comes up — ended tasks first,
/// then the long-orphaned backlog, then the ceiling.
pub fn startup_sweep(
    container: &Path,
    ended_owners: &[String],
    orphan_owners: &[String],
    log: Option<&AuditLog>,
) -> MaintenanceOutcome {
    let request = MaintenanceRequest {
        container,
        ended_owners,
        orphan_owners,
        idle_before_unix: now_unix().saturating_sub(ORPHAN_IDLE_SECS),
        limit_bytes: configured_budget_bytes(),
        trigger: "startup",
    };
    maintain(&request, log)
}

/// CF-BLD-R3 in the live path: free idle caches when the disk is short, and say
/// plainly how much was cleaned and how much is still missing.
///
/// `keep` is the cache the caller is about to build into. Reclaiming is never
/// allowed to delete the very target directory this build is starting with.
pub fn guard_before_build(
    container: &Path,
    keep: &[PathBuf],
    trigger: &str,
    log: Option<&AuditLog>,
) -> DiskGuardOutcome {
    let floor = configured_free_space_floor_bytes();
    let probe = container.to_path_buf();
    guard_disk_space(
        move || free_bytes(&probe),
        floor,
        |missing| reclaim_until(container, keep, missing, trigger, log),
    )
}

/// Drop least-recently-used caches until at least `target` bytes are free.
/// In-use caches, and anything under `keep`, are never candidates: a build in
/// flight must never be the thing deleted to make room for another one.
pub fn reclaim_until(
    container: &Path,
    keep: &[PathBuf],
    target: u64,
    trigger: &str,
    log: Option<&AuditLog>,
) -> u64 {
    let entries: Vec<CacheEntry> = scan_workspace_container(container)
        .into_iter()
        .filter(|entry| !keep.iter().any(|prefix| entry.path.starts_with(prefix)))
        .collect();
    let total: u64 = entries.iter().map(|entry| entry.bytes).sum();
    let plan = plan_eviction(
        &entries,
        total.saturating_sub(target),
        EvictionReason::DiskPressure,
    );
    apply_plan(&plan, trigger, log)
}

/// Live occupancy for the panel and the background entry point (CF-BLD-R4).
pub async fn current_report(container: &Path, limiter: &HeavyBuildLimiter) -> BuildCacheReport {
    let entries = scan_workspace_container(container);
    BuildCacheReport {
        total_bytes: entries.iter().map(|entry| entry.bytes).sum(),
        budget_bytes: configured_budget_bytes(),
        entries,
        heavy_builds_running: limiter.running().await,
        heavy_builds_waiting: limiter.waiting().await,
        heavy_build_limit: limiter.limit(),
        heavy_build_status: limiter.status_label().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "cf-build-cache-test-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Build a synthetic cache directory. Size is set exactly so budget maths is
    /// deterministic; `scan_measures_the_tree` is what proves real measurement.
    fn cache(root: &Path, name: &str, bytes: u64, touched: u64, in_use: bool) -> CacheEntry {
        let path = root.join(name);
        fs::create_dir_all(path.join("debug/deps")).unwrap();
        fs::write(path.join("debug/deps/blob"), vec![7u8; bytes as usize]).unwrap();
        if in_use {
            mark_in_use(&path).unwrap();
        }
        let mut entry = scan_entry(&path, None, now_unix(), IN_USE_STALE_AFTER).unwrap();
        entry.bytes = bytes;
        entry.files = 1;
        entry.last_used_unix = touched;
        entry
    }

    fn owned(
        root: &Path,
        name: &str,
        bytes: u64,
        touched: u64,
        in_use: bool,
        owner: &str,
    ) -> CacheEntry {
        let mut entry = cache(root, name, bytes, touched, in_use);
        entry.owner = Some(owner.to_string());
        entry
    }

    fn names(plan: &EvictionPlan) -> Vec<String> {
        plan.evict
            .iter()
            .map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn scan_measures_the_tree_and_ignores_unowned_directories() {
        let root = temp_dir("scan");
        let entry = cache(&root, "cargo-target-abc", 4096, 1_700_000_000, false);
        assert_eq!(entry.bytes, 4096);
        assert_eq!(entry.files, 1);
        assert!(!entry.in_use);

        // The user's own source must never look like a cache (CF-BLD-R8).
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), b"fn main() {}").unwrap();
        let scanned = scan_root(&root, |_| None, now_unix(), IN_USE_STALE_AFTER);
        assert_eq!(scanned.len(), 1, "scanned = {scanned:?}");
        assert!(looks_like_managed_cache(".codefactory-cache"));
        assert!(looks_like_managed_cache("cargo-target-1a2b"));
        assert!(!looks_like_managed_cache("src"));
    }

    #[test]
    fn stale_in_use_marker_does_not_protect_a_dead_builder() {
        let root = temp_dir("stale");
        let path = root.join("task-a");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join(IN_USE_MARKER), "1").unwrap();
        assert!(!is_in_use(&path, now_unix(), IN_USE_STALE_AFTER));
        mark_in_use(&path).unwrap();
        assert!(is_in_use(&path, now_unix(), IN_USE_STALE_AFTER));
        clear_in_use(&path);
        assert!(!is_in_use(&path, now_unix(), IN_USE_STALE_AFTER));
    }

    #[test]
    fn r1_evicts_least_recently_used_first_and_never_an_in_use_cache() {
        let root = temp_dir("r1");
        let entries = vec![
            cache(&root, "oldest", 100, 1_000, false),
            cache(&root, "newest", 100, 3_000, false),
            cache(&root, "building", 100, 500, true),
        ];
        // 300 total against a 150 ceiling: both idle caches must go, oldest
        // first, while the least-recently-used *in-use* cache survives.
        let plan = plan_eviction(&entries, 150, EvictionReason::OverBudget);
        assert_eq!(plan.reason, EvictionReason::OverBudget);
        assert_eq!(names(&plan), vec!["oldest", "newest"]);
        assert_eq!(plan.freed_bytes, 200);
        assert_eq!(plan.kept_bytes, 100);
        assert_eq!(plan.overflow_bytes, 0);
        assert_eq!(plan.protected_bytes, 100);
    }

    #[test]
    fn r1_reports_the_shortfall_when_everything_left_is_in_use() {
        let root = temp_dir("r1-over");
        let entries = vec![
            cache(&root, "idle", 100, 1_000, false),
            cache(&root, "busy-a", 100, 2_000, true),
            cache(&root, "busy-b", 100, 3_000, true),
        ];
        let plan = plan_eviction(&entries, 250, EvictionReason::OverBudget);
        assert_eq!(names(&plan), vec!["idle"]);
        assert_eq!(plan.overflow_bytes, 0);

        let tight = plan_eviction(&entries, 150, EvictionReason::OverBudget);
        assert_eq!(tight.freed_bytes, 100);
        assert_eq!(tight.overflow_bytes, 50);
        assert_eq!(tight.protected_bytes, 200);
    }

    #[test]
    fn r2_reclaims_every_terminal_state_and_leaves_running_tasks_alone() {
        let root = temp_dir("r2");
        let entries = vec![
            owned(&root, "done", 100, 1_000, false, "obj-completed"),
            owned(&root, "failed", 100, 1_100, false, "obj-failed"),
            owned(&root, "stopped", 100, 1_200, false, "obj-stopped"),
            owned(&root, "running", 100, 1_300, false, "obj-running"),
            owned(&root, "still-building", 100, 1_400, true, "obj-failed"),
        ];
        let ended = vec![
            "obj-completed".to_string(),
            "obj-failed".to_string(),
            "obj-stopped".to_string(),
        ];
        let plan = plan_task_reclaim(&entries, &ended);
        assert_eq!(plan.reason, EvictionReason::TaskEnded);
        // All three terminal states are reclaimed; the running task is not, and
        // nor is the terminal task's cache that a live builder still holds.
        assert_eq!(names(&plan), vec!["done", "failed", "stopped"]);
        assert_eq!(plan.freed_bytes, 300);
        assert_eq!(plan.protected_bytes, 100);
    }

    #[test]
    fn r8_mixed_legacy_state_reclaims_ended_then_applies_the_ceiling() {
        let root = temp_dir("r8");
        let entries = vec![
            owned(&root, "legacy-idle", 400, 1_000, false, "obj-old"),
            owned(&root, "legacy-live", 400, 1_100, false, "obj-live"),
            owned(&root, "legacy-building", 400, 1_200, true, "obj-live"),
        ];
        let ended = vec!["obj-old".to_string()];
        // Ceiling 500: reclaiming 400 still leaves 800, so the budget pass also
        // has to evict the idle live-task cache. The in-use one survives.
        let plans = upgrade_sweep(&entries, 500, &ended);
        assert_eq!(plans.len(), 2, "plans = {plans:?}");
        assert_eq!(plans[0].reason, EvictionReason::TaskEnded);
        assert_eq!(names(&plans[0]), vec!["legacy-idle"]);
        assert_eq!(plans[1].reason, EvictionReason::Upgrade);
        assert_eq!(names(&plans[1]), vec!["legacy-live"]);
        assert_eq!(plans[1].protected_bytes, 400);
    }

    #[test]
    fn r10_every_eviction_records_what_how_big_why_and_who() {
        let root = temp_dir("r10");
        let log = AuditLog::new(root.join("audit/build-cache.jsonl"));
        let entries = vec![owned(&root, "gone", 1234, 1_000, false, "obj-7")];
        let plan = plan_task_reclaim(&entries, &["obj-7".to_string()]);
        let freed = apply_plan(&plan, "task_end:obj-7", Some(&log));
        assert_eq!(freed, 1234);

        let records = log.read_all();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert!(record.path.ends_with("gone"));
        assert_eq!(record.bytes, 1234);
        assert_eq!(record.reason, EvictionReason::TaskEnded);
        assert_eq!(record.trigger, "task_end:obj-7");
        assert_eq!(record.owner.as_deref(), Some("obj-7"));
        assert!(record.at_unix > 0);
        // The directory really is gone.
        assert!(!root.join("gone").exists());
    }

    #[test]
    fn r3_disk_guard_reclaims_first_and_then_says_what_is_still_missing() {
        let floor = 20 * 1024 * 1024 * 1024;
        // Plenty of room: admitted, nothing reclaimed.
        let ok = guard_disk_space(|| Some(floor + 1), floor, |_| 0);
        assert!(ok.admitted());
        assert_eq!(ok.reclaimed_bytes, 0);
        assert!(ok.user_message().contains("充足"));

        // Below the floor, but cleanup frees enough.
        let recovered = guard_disk_space(|| Some(floor - 3_000_000_000), floor, |missing| missing);
        assert!(recovered.admitted());
        assert_eq!(recovered.reclaimed_bytes, 3_000_000_000);

        // Cleanup cannot free enough: the sentence says so in plain words and
        // names both the reclaimed amount and the remaining shortfall.
        let short = guard_disk_space(|| Some(floor - 5_000_000_000), floor, |_| 1_000_000_000);
        assert!(!short.admitted());
        assert_eq!(short.still_short_bytes, 4_000_000_000);
        let message = short.user_message();
        assert!(message.contains("空间不足"), "{message}");
        assert!(message.contains("已清理"), "{message}");
        assert!(message.contains("还差"), "{message}");
    }

    #[test]
    fn r5_shared_layout_keeps_final_artifacts_per_task_and_bounds_the_lock_wait() {
        let container = PathBuf::from("/tmp/execution-workspaces");
        let a = container.join("obj-a");
        let b = container.join("obj-b");
        let a_target = task_target_dir(&task_cache_root(&a), "stable");
        let b_target = task_target_dir(&task_cache_root(&b), "stable");
        // Separate final-artifact directories: no cross-task contamination.
        assert_ne!(a_target, b_target);
        assert!(!a_target.starts_with(&b));
        assert!(!b_target.starts_with(&a));
        assert_eq!(
            owner_from_workspace_path(&task_cache_root(&a), &container).as_deref(),
            Some("obj-a")
        );
        // Only the dependency cache is shared, and it is not inside a task.
        let shared = shared_cache_root(&PathBuf::from("/tmp/repo"));
        assert_eq!(shared, PathBuf::from("/tmp/repo/.codefactory-cache/cargo-target"));
        // The shared lock wait is bounded: past the limit a task proceeds.
        assert!(shared_lock_within_limit(Duration::from_secs(30)));
        assert!(!shared_lock_within_limit(SHARED_LOCK_WAIT_LIMIT + Duration::from_secs(1)));
    }

    #[test]
    fn heavy_build_detection_matches_compile_and_full_test_commands_only() {
        assert!(HeavyBuildLimiter::is_heavy_build_command("cargo test --workspace"));
        assert!(HeavyBuildLimiter::is_heavy_build_command("CARGO_TARGET_DIR=/tmp/x cargo build"));
        assert!(HeavyBuildLimiter::is_heavy_build_command("pnpm test"));
        assert!(!HeavyBuildLimiter::is_heavy_build_command("git status"));
        assert!(!HeavyBuildLimiter::is_heavy_build_command("ls src-tauri"));
    }

    #[tokio::test]
    async fn r7_the_overflow_queues_instead_of_failing_and_is_admitted_in_order() {
        let limiter = HeavyBuildLimiter::new(2);
        assert_eq!(limiter.limit(), 2);
        assert!(limiter.status_label().await.is_none());

        let first = limiter.acquire().await;
        let second = limiter.acquire().await;
        assert_eq!(limiter.running().await, 2);
        assert_eq!(limiter.waiting().await, 0);

        // Builds three and four queue. Queueing is not a failure.
        let l3 = Arc::clone(&limiter);
        let third = tokio::spawn(async move { l3.acquire().await });
        let l4 = Arc::clone(&limiter);
        let fourth = tokio::spawn(async move { l4.acquire().await });
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
        assert_eq!(limiter.waiting().await, 2);
        assert_eq!(limiter.running().await, 2);
        let label = limiter.status_label().await.unwrap();
        assert!(label.contains("等待编译空位"), "{label}");

        // Releasing a slot admits the *first* waiter without inflating the
        // running count, and without deadlocking.
        drop(first);
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
        assert_eq!(limiter.waiting().await, 1);
        assert_eq!(limiter.running().await, 2);
        let third_permit = tokio::time::timeout(Duration::from_secs(5), third)
            .await
            .expect("queued build must be admitted")
            .unwrap();

        drop(second);
        let fourth_permit = tokio::time::timeout(Duration::from_secs(5), fourth)
            .await
            .expect("second queued build must be admitted")
            .unwrap();
        assert_eq!(limiter.waiting().await, 0);

        drop(third_permit);
        drop(fourth_permit);
        for _ in 0..32 {
            tokio::task::yield_now().await;
        }
        assert_eq!(limiter.running().await, 0);
    }

    // ── CF-BLD-R2/R3/R8: the runtime maintenance pass ────────────────────────

    fn request<'a>(
        container: &'a Path,
        ended: &'a [String],
        orphans: &'a [String],
        idle_before: u64,
        limit: u64,
    ) -> MaintenanceRequest<'a> {
        MaintenanceRequest {
            container,
            ended_owners: ended,
            orphan_owners: orphans,
            idle_before_unix: idle_before,
            limit_bytes: limit,
            trigger: "test",
        }
    }

    #[test]
    fn r2_runtime_pass_reclaims_ended_tasks_and_leaves_live_and_in_use_caches_alone() {
        let root = temp_dir("r2-runtime");
        let live = owned(&root, "cargo-target-live", 4096, 1_700_000_000, false, "task-live");
        let busy = owned(&root, "cargo-target-busy", 4096, 1_700_000_000, true, "task-done");
        let done = owned(&root, "cargo-target-done", 8192, 1_700_000_000, false, "task-done");
        let entries = vec![live.clone(), busy.clone(), done.clone()];
        let ended = vec!["task-done".to_string()];
        let none: Vec<String> = Vec::new();

        let outcome = maintain_entries(&entries, &request(&root, &ended, &none, 0, u64::MAX), None);

        assert_eq!(
            outcome.removed,
            vec![done.path.clone()],
            "only the ended task's idle cache is reclaimed"
        );
        assert_eq!(outcome.reclaimed_bytes, 8192);
        assert!(!done.path.exists());
        assert!(
            busy.path.exists(),
            "an ended task's cache survives while a live builder still holds it"
        );
        assert!(
            live.path.exists(),
            "a task that can still continue keeps its cache"
        );
    }

    #[test]
    fn r8_orphan_backlog_drops_only_caches_idle_past_the_window() {
        let root = temp_dir("r8-orphan");
        let old = owned(&root, "cargo-target-old", 4096, 1_000, false, "task-gone");
        let fresh = owned(&root, "cargo-target-fresh", 4096, 9_000, false, "task-gone");
        let live_idle = owned(&root, "cargo-target-live", 4096, 1_000, false, "task-live");
        let entries = vec![old.clone(), fresh.clone(), live_idle.clone()];
        let ended: Vec<String> = Vec::new();
        let orphans = vec!["task-gone".to_string()];

        let outcome = maintain_entries(&entries, &request(&root, &ended, &orphans, 5_000, u64::MAX), None);

        assert_eq!(outcome.removed, vec![old.path.clone()]);
        assert!(
            fresh.path.exists(),
            "an orphan touched inside the idle window is still resumable"
        );
        assert!(
            live_idle.path.exists(),
            "a live task's cache is never an orphan candidate, however idle"
        );
    }

    #[test]
    fn r1_still_applies_to_what_survives_the_runtime_pass() {
        let root = temp_dir("r1-after-runtime");
        let a = owned(&root, "cargo-target-a", 4096, 1_000, false, "task-live");
        let b = owned(&root, "cargo-target-b", 4096, 2_000, false, "task-live");
        let ended: Vec<String> = Vec::new();
        let none: Vec<String> = Vec::new();

        let outcome =
            maintain_entries(&[a.clone(), b.clone()], &request(&root, &ended, &none, 0, 4096), None);

        assert_eq!(outcome.evicted_bytes, 4096);
        assert_eq!(outcome.after_bytes, 4096);
        assert_eq!(outcome.overflow_bytes, 0);
        assert!(
            a.path.exists() ^ b.path.exists(),
            "the ceiling drops caches until the survivors fit"
        );
    }

    #[test]
    fn scan_and_reclaim_touch_only_a_tasks_own_cache_directory() {
        let container = temp_dir("container");
        for owner in ["task-one", "task-two"] {
            let cache_root = task_cache_root(&container.join(owner));
            fs::create_dir_all(&cache_root).unwrap();
            let target = cache_root.join("cargo-target-test");
            fs::create_dir_all(target.join("debug/deps")).unwrap();
            fs::write(target.join("debug/deps/blob"), vec![5u8; 4096]).unwrap();
        }
        // A sibling the app did not create must never be measured or removed.
        let source = container.join("task-one").join("src");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("main.rs"), b"fn main(){}").unwrap();

        let scanned = scan_workspace_container(&container);
        assert_eq!(scanned.len(), 2);
        assert_eq!(scanned.iter().map(|entry| entry.bytes).sum::<u64>(), 8192);
        assert!(scanned.iter().all(|entry| entry.owner.is_some()));

        let freed = reclaim_until(&container, &[], 4096, "test", None);
        assert_eq!(freed, 4096);
        assert_eq!(scan_workspace_container(&container).len(), 1);
        assert!(
            source.join("main.rs").exists(),
            "the task's own source is never a build-cache candidate"
        );
    }

    #[test]
    fn reclaim_never_deletes_the_cache_the_current_build_is_starting_into() {
        let container = temp_dir("keep");
        for owner in ["task-one", "task-two"] {
            let cache_root = task_cache_root(&container.join(owner));
            fs::create_dir_all(&cache_root).unwrap();
            let target = cache_root.join("cargo-target-test");
            fs::create_dir_all(target.join("debug/deps")).unwrap();
            fs::write(target.join("debug/deps/blob"), vec![5u8; 4096]).unwrap();
        }
        let mine = task_cache_root(&container.join("task-one"));

        // Ask for more room than exists: the guard may clear every other task
        // but must never touch the directory this build is about to write into.
        let freed = reclaim_until(&container, std::slice::from_ref(&mine), 64 * 1024, "test", None);

        assert_eq!(freed, 4096, "only the other task's cache is reclaimable");
        assert!(mine.join("cargo-target-test").exists(), "the current build's cache is kept");
        assert!(!task_cache_root(&container.join("task-two")).join("cargo-target-test").exists());
    }

    #[test]
    fn runtime_defaults_match_the_documented_budget() {
        assert_eq!(DEFAULT_BUDGET_BYTES, 60 * 1024 * 1024 * 1024);
        if std::env::var("CODEFACTORY_BUILD_CACHE_BUDGET_GB").is_err() {
            assert_eq!(configured_budget_bytes(), DEFAULT_BUDGET_BYTES);
        }
        if std::env::var("CODEFACTORY_BUILD_CACHE_FLOOR_GB").is_err() {
            assert_eq!(configured_free_space_floor_bytes(), DEFAULT_FREE_SPACE_FLOOR_BYTES);
        }
        if std::env::var("CODEFACTORY_MAX_HEAVY_BUILDS").is_err() {
            assert_eq!(configured_max_heavy_builds(), DEFAULT_MAX_HEAVY_BUILDS);
        }
    }

    #[tokio::test]
    async fn report_serialises_occupancy_and_heavy_build_state() {
        let container = temp_dir("report");
        let limiter = HeavyBuildLimiter::new(1);

        let report = current_report(&container, &limiter).await;

        assert_eq!(report.total_bytes, 0);
        assert_eq!(report.budget_bytes, configured_budget_bytes());
        assert_eq!(report.heavy_build_limit, 1);
        assert!(report.heavy_build_status.is_none());
        let rendered = serde_json::to_value(&report).unwrap();
        for field in ["total_bytes", "budget_bytes", "entries", "heavy_builds_running"] {
            assert!(rendered.get(field).is_some(), "report must carry {field}");
        }
    }
}
