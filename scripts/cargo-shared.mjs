// SPDX-License-Identifier: Apache-2.0
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  budgetBytesFromEnv,
  clearInUse,
  enforceBudget,
  formatGiB,
  markInUse,
} from "./build-cache-budget.mjs";

function git(args, cwd) {
  return execFileSync("git", args, { cwd, encoding: "utf8" }).trim();
}

// Every worktree shares one cache, so this deliberately resolves to the main
// checkout rather than the current worktree.
export function sharedTargetFor(cwd) {
  const commonDir = git(["rev-parse", "--path-format=absolute", "--git-common-dir"], cwd);
  return path.join(path.dirname(commonDir), ".codefactory-cache", "cargo-target");
}

// The manifest, unlike the cache, must come from the worktree the caller is
// standing in; using the shared-cache root would compile the main checkout.
export function worktreeRootFor(cwd) {
  return git(["rev-parse", "--show-toplevel"], cwd);
}

// pnpm forwards the `--` from `pnpm cargo:shared -- <cargo arguments>` to the
// script, so cargo would see it as a positional argument. Drop only the leading
// one: `cargo test <filter> -- --nocapture` still needs its own separator.
export function stripLeadingSeparator(args) {
  return args[0] === "--" ? args.slice(1) : args;
}

function hasExplicitManifestPath(args) {
  return args.some((arg) => arg === "--manifest-path" || arg.startsWith("--manifest-path="));
}

function findManifestDir(startDir) {
  let dir = path.resolve(startDir);
  for (;;) {
    if (fs.existsSync(path.join(dir, "Cargo.toml"))) return dir;
    const parent = path.dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

// The workspace manifest lives in src-tauri/, so cargo discovers nothing from
// the repo root. Fall back to it only when cargo could not have resolved a
// manifest itself, so running inside src-tauri/ or a single crate keeps its
// narrower scope and an explicit --manifest-path keeps resolving against the
// caller's directory.
export function resolveCargoCwd({ cwd, args, worktreeRoot }) {
  if (hasExplicitManifestPath(args)) return { cwd, relocated: false };
  if (findManifestDir(cwd)) return { cwd, relocated: false };

  const workspaceDir = path.join(worktreeRoot, "src-tauri");
  if (!fs.existsSync(path.join(workspaceDir, "Cargo.toml"))) return { cwd, relocated: false };
  return { cwd: workspaceDir, relocated: true };
}

// CF-BLD-R9: the shared cache is governed by the same ceiling as the caches the
// runtime manages, and every build marks the directory it is using so an idle
// sweep can never delete a cache out from under a running build.
export function sharedBuildEnvironment({ targetDir, baseEnv = process.env }) {
  return {
    ...baseEnv,
    CARGO_TARGET_DIR: targetDir,
    // Incremental artifacts are keyed by the *worktree path*, so in a cache
    // shared by many worktrees they are write-only ballast — 201 GB of the
    // 410 GB that had to be deleted by hand. Sharing only pays off without them.
    CARGO_INCREMENTAL: "0",
  };
}

function describeFailure(error) {
  return error?.message ?? String(error);
}

// M270-R3: the shared cache lives in the *main checkout*, but a trusted-mode
// sandbox only lets a command write inside the workspace it was launched from
// (plus `<workspace>/.codefactory-cache`, which the sandbox profiles already
// whitelist). Writing the in-use marker into the main checkout is therefore
// refused under the sandbox, so a build there needs a target directory it can
// actually mark. The name keeps the `cargo-target-*` shape the cache budget
// recognises, and being workspace-scoped keeps two tasks off one final path.
export function taskTargetFor(worktreeRoot, env = process.env) {
  const raw = env.CODEFACTORY_TASK_ID || path.basename(worktreeRoot) || `pid-${process.pid}`;
  const tag = raw.replace(/[^A-Za-z0-9._-]/g, "-").slice(0, 48) || `pid-${process.pid}`;
  return path.join(worktreeRoot, ".codefactory-cache", `cargo-target-${tag}`);
}

// Pick the target directory by *proving* it can be marked, because the marker is
// what keeps the LRU sweep from deleting a cache mid-build. Degrading silently
// would look like the shared cache was used while the artifacts landed in a
// directory the sweep never sees, so the reason travels back to the caller.
export function selectTargetDir({ sharedDir, worktreeRoot, mark = markInUse, env = process.env }) {
  try {
    mark(sharedDir);
    return { targetDir: sharedDir, degraded: null };
  } catch (error) {
    const reason = describeFailure(error);
    const fallback = taskTargetFor(worktreeRoot, env);
    try {
      mark(fallback);
    } catch (fallbackError) {
      throw new Error(
        `shared build cache ${sharedDir} is not writable (${reason}), and the task-local directory ` +
          `${fallback} is not writable either (${describeFailure(fallbackError)}). Nothing was compiled. ` +
          `Fix: make ${path.join(worktreeRoot, ".codefactory-cache")} writable inside the workspace, ` +
          `or run this command outside the sandbox so the shared cache can be marked.`,
      );
    }
    return { targetDir: fallback, degraded: { sharedDir, fallback, reason } };
  }
}

export function enforceSharedBudget({ repoRoot, targetDir, env = process.env, logFile }) {
  const cacheRoot = path.join(repoRoot, ".codefactory-cache");
  return enforceBudget({
    roots: [cacheRoot],
    limitBytes: budgetBytesFromEnv(env),
    keep: [targetDir],
    trigger: "cargo_shared",
    logFile: logFile ?? path.join(cacheRoot, "build-cache-audit.jsonl"),
  });
}

function main() {
  const sharedDir = sharedTargetFor(process.cwd());
  const args = stripLeadingSeparator(process.argv.slice(2));

  if (args.length === 1 && args[0] === "--print-target") {
    process.stdout.write(`${sharedDir}\n`);
    process.exit(0);
  }

  if (args.length === 0) {
    process.stderr.write("Usage: pnpm cargo:shared -- <cargo arguments>\n");
    process.exit(2);
  }

  const worktreeRoot = worktreeRootFor(process.cwd());
  const resolved = resolveCargoCwd({ cwd: process.cwd(), args, worktreeRoot });
  if (resolved.relocated) {
    process.stderr.write(`cargo target: running in ${path.relative(worktreeRoot, resolved.cwd)} (workspace manifest)\n`);
  }

  // Mark before sweeping so this build's own directory can never be a
  // candidate, then bring the cache back inside the ceiling.
  let selection;
  try {
    selection = selectTargetDir({ sharedDir, worktreeRoot });
  } catch (error) {
    // M270-R1/R3: nothing was compiled, so this must not look like success.
    process.stderr.write(`cargo target: ${describeFailure(error)}\n`);
    process.exit(1);
  }
  const targetDir = selection.targetDir;
  if (selection.degraded) {
    process.stderr.write(
      `cargo target: ${selection.degraded.sharedDir} is not writable (${selection.degraded.reason}); ` +
        `building in task-local ${selection.degraded.fallback} and leaving the shared cache alone.\n`,
    );
  }

  let budgetSummary = null;
  let budgetFailure = null;
  try {
    budgetSummary = enforceSharedBudget({ repoRoot: path.resolve(targetDir, "..", ".."), targetDir });
  } catch (error) {
    // The build is the user's work, so it still runs — but a sweep that could
    // not run must not be reported as a clean build (M270-R5).
    budgetFailure = describeFailure(error);
    process.stderr.write(`cargo target: build-cache budget pass failed: ${budgetFailure}\n`);
  }
  if (budgetSummary && budgetSummary.reclaimed_bytes + budgetSummary.evicted_bytes > 0) {
    process.stderr.write(
      `cargo target: reclaimed ${formatGiB(budgetSummary.reclaimed_bytes + budgetSummary.evicted_bytes)} of shared build cache\n`,
    );
  }

  let result;
  let clearFailure = null;
  try {
    result = spawnSync("cargo", args, {
      cwd: resolved.cwd,
      env: sharedBuildEnvironment({ targetDir }),
      stdio: "inherit",
    });
  } finally {
    try {
      clearInUse(targetDir);
    } catch (error) {
      // A marker we cannot clear keeps the sweep away from this directory for
      // good, which is a real failure — report it instead of masking cargo's code.
      clearFailure = describeFailure(error);
      process.stderr.write(`cargo target: could not clear the in-use marker in ${targetDir}: ${clearFailure}\n`);
    }
  }

  if (!result || result.error) {
    process.stderr.write(`cargo target: could not run cargo: ${describeFailure(result?.error)}\n`);
    process.exit(1);
  }
  const status = result.status ?? 1;
  if (status !== 0) {
    process.exit(status);
  }
  process.exit(budgetFailure || clearFailure ? 1 : 0);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
