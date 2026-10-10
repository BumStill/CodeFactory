// SPDX-License-Identifier: Apache-2.0
//
// CF-BUILD for the repository's own build tooling (spec CF-BLD-R9).
//
// The Rust runtime manages the caches it creates inside managed workspaces.
// This module applies the *same* rules to the caches the repo's development
// tools create — `.codefactory-cache/cargo-target`, shared by `pnpm
// cargo:shared` and by every worktree's post-checkout symlink. Before this
// existed, that directory grew to 410 GB and was never trimmed.
//
// Rules (mirrors src-tauri/src/build_cache.rs):
//   R1  one total ceiling; evict least-recently-used first;
//       never evict a directory whose in-use marker is fresh;
//   R2  reclaim directories whose owner is gone (stale) regardless of the ceiling;
//   R10 record every removal (what, how big, why, who).
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

/** 60 GiB, matching `DEFAULT_BUDGET_BYTES` in src-tauri/src/build_cache.rs. */
export const DEFAULT_BUDGET_BYTES = 60 * 1024 * 1024 * 1024;

/** A cache idle for longer than this is reclaimable regardless of the ceiling. */
export const DEFAULT_STALE_DAYS = 14;

/** An in-use marker older than this belongs to a dead builder. */
export const IN_USE_STALE_MS = 15 * 60 * 1000;

export const IN_USE_MARKER = ".codefactory-in-use";

export function formatGiB(bytes) {
  const value = bytes / 1024 ** 3;
  return value >= 10 ? `${value.toFixed(0)} GB` : `${value.toFixed(1)} GB`;
}

export function budgetBytesFromEnv(env = process.env) {
  const raw = env.CODEFACTORY_BUILD_CACHE_BUDGET_GB;
  const gb = raw === undefined ? NaN : Number.parseFloat(raw);
  return Number.isFinite(gb) && gb > 0 ? Math.floor(gb * 1024 ** 3) : DEFAULT_BUDGET_BYTES;
}

/** Only directories our own tooling created are ever candidates (CF-BLD-R8). */
export function looksLikeManagedCache(name) {
  return name === ".codefactory-cache" || name.startsWith("cargo-target");
}

// `du -sk` is the fastest portable size probe; a missing dir is simply zero.
function dirStats(dir) {
  let bytes = 0;
  try {
    const out = execFileSync("du", ["-sk", dir]);
    bytes = Number.parseInt(String(out).trim().split(/\s+/)[0], 10) * 1024;
  } catch {
    bytes = 0;
  }
  let touched = 0;
  try {
    touched = fs.statSync(dir).mtimeMs;
  } catch {
    touched = 0;
  }
  return { bytes: Number.isFinite(bytes) ? bytes : 0, touched };
}

export function isInUse(dir, now = Date.now(), staleMs = IN_USE_STALE_MS) {
  try {
    const beat = Number.parseInt(
      fs.readFileSync(path.join(dir, IN_USE_MARKER), "utf8").trim(),
      10,
    );
    if (!Number.isFinite(beat)) return false;
    return beat + staleMs >= now;
  } catch {
    return false;
  }
}

export function markInUse(dir) {
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, IN_USE_MARKER), String(Date.now()));
}

export function clearInUse(dir) {
  try {
    fs.unlinkSync(path.join(dir, IN_USE_MARKER));
  } catch {
    /* already gone */
  }
}

/** Every managed cache directory directly under `roots`. */
export function scanCaches(roots, { now = Date.now(), staleMs = IN_USE_STALE_MS } = {}) {
  const entries = [];
  for (const root of roots) {
    if (!fs.existsSync(root)) continue;
    for (const name of fs.readdirSync(root)) {
      if (!looksLikeManagedCache(name)) continue;
      const dir = path.join(root, name);
      if (!fs.statSync(dir).isDirectory()) continue;
      const { bytes, touched } = dirStats(dir);
      entries.push({ path: dir, bytes, touched, in_use: isInUse(dir, now, staleMs) });
    }
  }
  return entries;
}

function lru(candidates) {
  return [...candidates].sort((a, b) => a.touched - b.touched || a.path.localeCompare(b.path));
}

/**
 * Pure planner: what to delete to get back under `limit`.
 * In-use entries are never candidates; when they alone exceed the limit the
 * plan reports the shortfall instead of pretending it succeeded.
 */
export function planEviction(entries, limit, reason = "over_budget") {
  const total = entries.reduce((sum, e) => sum + e.bytes, 0);
  const protectedBytes = entries.filter((e) => e.in_use).reduce((s, e) => s + e.bytes, 0);
  const evict = [];
  let freed = 0;
  if (total > limit) {
    for (const entry of lru(entries.filter((e) => !e.in_use))) {
      if (total - freed <= limit) break;
      freed += entry.bytes;
      evict.push({ ...entry, reason });
    }
  }
  const kept = total - freed;
  return { reason, evict, freed_bytes: freed, kept_bytes: kept, overflow_bytes: Math.max(0, kept - limit), protected_bytes: protectedBytes };
}

/** Entries nobody is using any more, whatever the ceiling says (R2/R8). */
export function planStaleReclaim(entries, staleDays = DEFAULT_STALE_DAYS, now = Date.now()) {
  const cutoff = now - staleDays * 24 * 60 * 60 * 1000;
  return {
    reason: "task_ended",
    evict: lru(entries.filter((e) => !e.in_use && e.touched < cutoff)).map((e) => ({ ...e, reason: "task_ended" })),
    ...summariseStale(entries, cutoff, staleDays),
  };
}

function summariseStale(entries, cutoff, staleDays) {
  const planned = lru(entries.filter((e) => !e.in_use && e.touched < cutoff));
  return {
    freed_bytes: planned.reduce((s, e) => s + e.bytes, 0),
    kept_bytes: entries.reduce((s, e) => s + e.bytes, 0) - planned.reduce((s, e) => s + e.bytes, 0),
    overflow_bytes: 0,
    protected_bytes: entries.filter((e) => e.in_use).reduce((s, e) => s + e.bytes, 0),
    stale_days: staleDays,
    cutoff,
  };
}

export function auditLogPath(root) {
  return path.join(root, "..", "build-cache-audit.jsonl");
}

export function recordEviction(logFile, record) {
  fs.mkdirSync(path.dirname(logFile), { recursive: true });
  fs.appendFileSync(logFile, JSON.stringify(record) + "\n");
}

function removePlanned(plan, trigger, logFile) {
  let freed = 0;
  for (const entry of plan.evict) {
    try {
      const stat = dirStats(entry.path);
      fs.rmSync(entry.path, { recursive: true, force: true });
      freed += stat.bytes;
      if (logFile) {
        recordEviction(logFile, {
          at_unix: Math.floor(Date.now() / 1000),
          path: entry.path,
          bytes: stat.bytes,
          reason: plan.reason,
          trigger,
          owner: path.basename(path.dirname(entry.path)),
        });
      }
    } catch {
      /* never let one stubborn directory stop the sweep */
    }
  }
  return freed;
}

/**
 * Apply R1/R2 to the given roots and report what happened. `keep` lists paths
 * that must survive this pass (typically the directory this process is about to
 * build into).
 */
export function enforceBudget({
  roots,
  limitBytes = DEFAULT_BUDGET_BYTES,
  logFile,
  trigger = "manual",
  staleDays = DEFAULT_STALE_DAYS,
  keep = [],
  now = Date.now(),
}) {
  const keepSet = new Set(keep.map((p) => path.resolve(p)));
  const all = scanCaches(roots, { now }).map((entry) =>
    keepSet.has(path.resolve(entry.path)) ? { ...entry, in_use: true } : entry,
  );
  const before = all.reduce((s, e) => s + e.bytes, 0);
  const reclaim = planStaleReclaim(all, staleDays, now);
  const reclaimed = removePlanned(reclaim, trigger, logFile);
  const gone = new Set(reclaim.evict.map((e) => path.resolve(e.path)));
  const survivors = all.filter((e) => !gone.has(path.resolve(e.path)));
  const budget = planEviction(survivors, limitBytes, "over_budget");
  const evicted = removePlanned(budget, trigger, logFile);
  const after = throughout(survivors) - evicted;
  return {
    before_bytes: before,
    after_bytes: Math.max(0, after),
    limit_bytes: limitBytes,
    reclaimed_bytes: reclaimed,
    evicted_bytes: evicted,
    overflow_bytes: Math.max(0, Math.max(0, after) - limitBytes),
    protected_bytes: survivors.filter((e) => e.in_use).reduce((s, e) => s + e.bytes, 0),
    entries: survivors,
  };
}

function throughout(entries) {
  return entries.reduce((s, e) => s + e.bytes, 0);
}

/** The shared cargo cache used by `pnpm cargo:shared` and worktree symlinks. */
export function sharedCacheRootFor(repoRoot) {
  return path.join(repoRoot, ".codefactory-cache", "cargo-target");
}

function parseArgs(argv) {
  const roots = [];
  let limitGb;
  let enforce = false;
  let staleDays = DEFAULT_STALE_DAYS;
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--enforce") enforce = true;
    else if (arg === "--status") enforce = false;
    else if (arg === "--root") roots.push(argv[++i]);
    else if (arg === "--limit-gb") limitGb = Number.parseFloat(argv[++i]);
    else if (arg === "--stale-days") staleDays = Number.parseFloat(argv[++i]);
  }
  return { roots, limitGb, enforce, staleDays };
}

function main() {
  const { roots, limitGb, enforce, staleDays } = parseArgs(process.argv.slice(2));
  const repoRoot = path.resolve(path.dirname(new URL(import.meta.url).pathname), "..");
  const searchRoots = roots.length ? roots : [path.join(repoRoot, ".codefactory-cache")];
  const limitBytes = limitGb ? Math.floor(limitGb * 1024 ** 3) : budgetBytesFromEnv();
  if (!enforce) {
    const entries = scanCaches(searchRoots);
    const total = entries.reduce((s, e) => s + e.bytes, 0);
    process.stdout.write(
      `build cache: ${formatGiB(total)} of ${formatGiB(limitBytes)} budget across ${entries.length} director` +
        `${entries.length === 1 ? "y" : "ies"}\n`,
    );
    for (const entry of entries.sort((a, b) => b.bytes - a.bytes)) {
      process.stdout.write(`  ${formatGiB(entry.bytes)}\t${entry.path}${entry.in_use ? " (in use)" : ""}\n`);
    }
    return;
  }
  const result = enforceBudget({
    roots: searchRoots,
    limitBytes,
    staleDays,
    logFile: path.join(repoRoot, ".codefactory-cache", "build-cache-audit.jsonl"),
    trigger: "cli_enforce",
  });
  process.stdout.write(
    `build cache: ${formatGiB(result.before_bytes)} -> ${formatGiB(result.after_bytes)} ` +
      `(reclaimed ${formatGiB(result.reclaimed_bytes)}, over-budget ${formatGiB(result.evicted_bytes)})\n`,
  );
  if (result.overflow_bytes > 0) {
    process.stdout.write(
      `space is short: still ${formatGiB(result.overflow_bytes)} over budget because ${formatGiB(result.protected_bytes)} is in use\n`,
    );
  }
}

const selfPath = new URL(import.meta.url).pathname;
if (process.argv[1] && path.resolve(process.argv[1]) === path.resolve(decodeURIComponent(selfPath))) main();
