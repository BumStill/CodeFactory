// SPDX-License-Identifier: Apache-2.0
//
// CF-BLD-R9: the repo's own build tooling obeys the same budget as the runtime.
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import {
  DEFAULT_STALE_DAYS,
  enforceBudget,
  markInUse,
  planEviction,
  planStaleReclaim,
  scanCaches,
} from "./build-cache-budget.mjs";
import {
  enforceSharedBudget,
  sharedBuildEnvironment,
} from "./cargo-shared.mjs";

function tempRoot(label) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), `cf-budget-${label}-`));
  return dir;
}

function makeCache(root, name, bytes, { inUse = false, ageMs = 0 } = {}) {
  const dir = path.join(root, name);
  fs.mkdirSync(dir, { recursive: true });
  fs.mkdirSync(path.join(dir, "debug"), { recursive: true });
  fs.writeFileSync(path.join(dir, "debug", "blob"), Buffer.alloc(bytes, 3));
  fs.writeFileSync(path.join(dir, "blob"), Buffer.alloc(bytes, 3));
  if (ageMs > 0) {
    const when = new Date(Date.now() - ageMs);
    fs.utimesSync(path.join(dir, "blob"), when, when);
    fs.utimesSync(dir, when, when);
  }
  if (inUse) markInUse(dir);
  return dir;
}

const DAY = 24 * 60 * 60 * 1000;

test("R1: eviction is least-recently-used first and never touches an in-use cache", () => {
  const entries = [
    { path: "/c/old", bytes: 100, touched: 1_000, in_use: false },
    { path: "/c/new", bytes: 100, touched: 3_000, in_use: false },
    { path: "/c/busy", bytes: 100, touched: 500, in_use: true },
  ];
  const plan = planEviction(entries, 150);
  assert.deepEqual(
    plan.evict.map((e) => e.path),
    ["/c/old", "/c/new"],
  );
  assert.equal(plan.freed_bytes, 200);
  assert.equal(plan.kept_bytes, 100);
  assert.equal(plan.overflow_bytes, 0);
  assert.equal(plan.protected_bytes, 100);

  // Even the least-recently-used cache survives when it is the one building.
  const protectedPlan = planEviction(entries, 50);
  assert.deepEqual(
    protectedPlan.evict.map((e) => e.path),
    ["/c/old", "/c/new"],
  );
  assert.equal(protectedPlan.overflow_bytes, 50);
});

test("R2: anything idle past the stale window is reclaimable regardless of the ceiling", () => {
  const now = Date.now();
  const entries = [
    { path: "/c/legacy", bytes: 400, touched: now - 30 * DAY, in_use: false },
    { path: "/c/fresh", bytes: 400, touched: now - DAY, in_use: false },
    { path: "/c/busy", bytes: 400, touched: now - 30 * DAY, in_use: true },
  ];
  const plan = planStaleReclaim(entries, DEFAULT_STALE_DAYS, now);
  assert.deepEqual(
    plan.evict.map((e) => e.path),
    ["/c/legacy"],
  );
});

test("R8/R1/R2: a real sweep reclaims stale caches, honours the ceiling, protects in-use ones", () => {
  const root = tempRoot("sweep");
  const stale = makeCache(root, "cargo-target-legacy", 64 * 1024, { ageMs: 40 * DAY });
  const recent = makeCache(root, "cargo-target-recent", 64 * 1024, { ageMs: DAY });
  const busy = makeCache(root, "cargo-target-busy", 64 * 1024, { inUse: true, ageMs: 40 * DAY });
  const untouched = path.join(root, "src");
  fs.mkdirSync(untouched, { recursive: true });
  fs.writeFileSync(path.join(untouched, "main.rs"), "fn main() {}");

  const logFile = path.join(root, "audit", "build-cache.jsonl");
  const result = enforceBudget({
    roots: [root],
    limitBytes: 10 * 1024 ** 3,
    staleDays: 14,
    logFile,
    trigger: "test_sweep",
  });

  assert.equal(fs.existsSync(stale), false, "stale cache must be reclaimed");
  assert.equal(fs.existsSync(busy), true, "in-use cache must never be reclaimed");
  assert.equal(fs.existsSync(recent), true, "recent cache inside budget must survive");
  assert.equal(fs.existsSync(untouched), true, "the user's own source must be untouched");
  assert.ok(result.before_bytes >= 3 * 64 * 1024);

  const records = fs
    .readFileSync(logFile, "utf8")
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line));
  const record = records.find((r) => r.path === stale);
  assert.ok(record, `expected an audit record for ${stale}; got ${JSON.stringify(records)}`);
  assert.equal(record.reason, "task_ended");
  assert.equal(record.trigger, "test_sweep");
  assert.ok(record.bytes > 0);
  assert.ok(record.at_unix > 0);
  assert.equal(record.owner, path.basename(root));
});

test("R1: when only in-use caches remain the sweep reports the shortfall instead of deleting them", () => {
  const root = tempRoot("short");
  const busy = makeCache(root, "cargo-target-busy-a", 32 * 1024, { inUse: true });
  const alsoBusy = makeCache(root, "cargo-target-busy-b", 32 * 1024, { inUse: true });
  const result = enforceBudget({ roots: [root], limitBytes: 1024, staleDays: DEFAULT_STALE_DAYS, trigger: "test" });
  assert.equal(fs.existsSync(busy), true);
  assert.equal(fs.existsSync(alsoBusy), true);
  assert.ok(result.overflow_bytes > 0, "a real shortfall must be reported, not hidden");
  assert.ok(result.protected_bytes > 0);
});

test("R9: the shared cargo command enforces the budget and stops writing incremental ballast", () => {
  const env = sharedBuildEnvironment({ targetDir: "/tmp/shared-target", baseEnv: { PATH: "/bin" } });
  assert.equal(env.CARGO_TARGET_DIR, "/tmp/shared-target");
  // Incremental artifacts are keyed by worktree path, so in a cache shared by
  // many worktrees they are write-only ballast (201 GB of the 410 GB).
  assert.equal(env.CARGO_INCREMENTAL, "0");
  assert.equal(env.PATH, "/bin");

  const repoRoot = tempRoot("repo");
  const cacheRoot = path.join(repoRoot, ".codefactory-cache");
  const legacy = makeCache(cacheRoot, "cargo-target-legacy", 32 * 1024, { ageMs: 40 * DAY });
  const active = path.join(cacheRoot, "cargo-target");
  makeCache(cacheRoot, "cargo-target", 32 * 1024, { ageMs: 40 * DAY });
  const result = enforceSharedBudget({
    repoRoot,
    targetDir: active,
    env: {},
    logFile: path.join(cacheRoot, "build-cache-audit.jsonl"),
  });
  assert.equal(fs.existsSync(legacy), false, "the idle legacy cache must be reclaimed");
  assert.equal(
    fs.existsSync(active),
    true,
    "the cache this build is about to use must be kept even though it looks idle",
  );
  assert.ok(result.before_bytes > 0);
});

test("scanCaches only ever considers directories our own tooling created", () => {
  const root = tempRoot("scan");
  makeCache(root, "cargo-target-one", 4096);
  makeCache(root, ".codefactory-cache", 4096);
  fs.mkdirSync(path.join(root, "target"), { recursive: true });
  fs.mkdirSync(path.join(root, "node_modules"), { recursive: true });
  const scanned = scanCaches([root]).map((e) => path.basename(e.path)).sort();
  assert.deepEqual(scanned, [".codefactory-cache", "cargo-target-one"]);
});
