// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  resolveCargoCwd,
  selectTargetDir,
  stripLeadingSeparator,
  taskTargetFor,
} from "./cargo-shared.mjs";

const SCRIPT = fileURLToPath(new URL("./cargo-shared.mjs", import.meta.url));

function makeRepo(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codefactory-cargo-shared-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  fs.mkdirSync(path.join(root, "src-tauri", "crates", "agent-loop"), { recursive: true });
  fs.writeFileSync(path.join(root, "src-tauri", "Cargo.toml"), "[workspace]\n");
  fs.writeFileSync(path.join(root, "src-tauri", "crates", "agent-loop", "Cargo.toml"), "[package]\n");
  return root;
}

// pnpm >= 7 forwards the `--` separator to the script verbatim, so the
// documented `pnpm cargo:shared -- test -p foo` reached cargo as
// `cargo -- test -p foo` and died with "trailing arguments after built-in
// command `test` are unsupported".
test("strips the one leading separator that pnpm forwards", () => {
  assert.deepEqual(stripLeadingSeparator(["--", "test", "-p", "codefactory"]), ["test", "-p", "codefactory"]);
});

test("leaves args untouched when no separator leads", () => {
  assert.deepEqual(stripLeadingSeparator(["test", "-p", "codefactory"]), ["test", "-p", "codefactory"]);
});

// `cargo test <filter> -- --nocapture` needs its second `--`; stripping every
// separator instead of just the leading one would silently drop test-binary args.
test("preserves the separator that forwards args to the test binary", () => {
  assert.deepEqual(
    stripLeadingSeparator(["--", "test", "iteration_boundary", "--", "--nocapture"]),
    ["test", "iteration_boundary", "--", "--nocapture"],
  );
});

test("keeps a non-leading separator in place", () => {
  assert.deepEqual(stripLeadingSeparator(["test", "--", "--nocapture"]), ["test", "--", "--nocapture"]);
});

test("strips only a single leading separator", () => {
  assert.deepEqual(stripLeadingSeparator(["--", "--", "test"]), ["--", "test"]);
});

test("reduces a lone separator to no arguments", () => {
  assert.deepEqual(stripLeadingSeparator(["--"]), []);
});

// The workspace manifest lives in src-tauri/, so cargo discovers nothing from
// the repo root and exits with "could not find Cargo.toml".
test("falls back to the workspace manifest directory from the repo root", (t) => {
  const root = makeRepo(t);
  const resolved = resolveCargoCwd({ cwd: root, args: ["test", "-p", "codefactory"], worktreeRoot: root });

  assert.equal(resolved.cwd, path.join(root, "src-tauri"));
  assert.equal(resolved.relocated, true);
});

test("keeps the cwd when cargo can already discover a manifest", (t) => {
  const root = makeRepo(t);
  const cwd = path.join(root, "src-tauri");
  const resolved = resolveCargoCwd({ cwd, args: ["test", "--workspace"], worktreeRoot: root });

  assert.equal(resolved.cwd, cwd);
  assert.equal(resolved.relocated, false);
});

// Running inside a crate scopes cargo to that crate; relocating would silently
// widen the command to the whole workspace.
test("keeps a crate directory as the cwd", (t) => {
  const root = makeRepo(t);
  const cwd = path.join(root, "src-tauri", "crates", "agent-loop");
  const resolved = resolveCargoCwd({ cwd, args: ["test"], worktreeRoot: root });

  assert.equal(resolved.cwd, cwd);
  assert.equal(resolved.relocated, false);
});

// test:rust:fast passes `--manifest-path src-tauri/Cargo.toml` from the repo
// root; relocating into src-tauri/ would resolve it to src-tauri/src-tauri/.
test("keeps the cwd when an explicit manifest path is passed", (t) => {
  const root = makeRepo(t);
  const resolved = resolveCargoCwd({
    cwd: root,
    args: ["test", "--manifest-path", "src-tauri/Cargo.toml"],
    worktreeRoot: root,
  });

  assert.equal(resolved.cwd, root);
  assert.equal(resolved.relocated, false);
});

test("keeps the cwd for the --manifest-path=value form", (t) => {
  const root = makeRepo(t);
  const resolved = resolveCargoCwd({
    cwd: root,
    args: ["test", "--manifest-path=src-tauri/Cargo.toml"],
    worktreeRoot: root,
  });

  assert.equal(resolved.cwd, root);
  assert.equal(resolved.relocated, false);
});

// No manifest to fall back to: leave cargo alone so it reports its own error
// instead of us inventing a directory.
test("keeps the cwd when no workspace manifest exists", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codefactory-cargo-shared-empty-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const resolved = resolveCargoCwd({ cwd: root, args: ["test"], worktreeRoot: root });

  assert.equal(resolved.cwd, root);
  assert.equal(resolved.relocated, false);
});

// Worktrees live under the main checkout at .claude/worktrees/<name>, and the
// shared cargo cache deliberately resolves to the main checkout. Reusing that
// root for the manifest made `pnpm cargo:shared` compile the main checkout's
// sources while the caller stood in a worktree.
test("falls back to the manifest inside the current worktree", (t) => {
  const mainCheckout = makeRepo(t);
  const worktree = path.join(mainCheckout, ".claude", "worktrees", "feature");
  fs.mkdirSync(path.join(worktree, "src-tauri"), { recursive: true });
  fs.writeFileSync(path.join(worktree, "src-tauri", "Cargo.toml"), "[workspace]\n");

  const resolved = resolveCargoCwd({ cwd: worktree, args: ["test", "-p", "codefactory"], worktreeRoot: worktree });

  assert.equal(resolved.cwd, path.join(worktree, "src-tauri"));
  assert.equal(resolved.relocated, true);
  assert.notEqual(resolved.cwd, path.join(mainCheckout, "src-tauri"));
});

// ---------------------------------------------------------------------------
// M270: building under the trusted-mode sandbox
//
// The shared cache lives in the main checkout, which the sandbox does not let
// us write. markInUse() therefore threw EPERM and the script died before cargo
// ever ran — while the reported exit code read 0. These tests pin the two
// halves of the fix: a documented task-local fallback, and never reporting
// success for work that did not happen.
// ---------------------------------------------------------------------------

function chmodWritable(dir) {
  let entries = [];
  try {
    entries = fs.readdirSync(dir, { withFileTypes: true });
  } catch {
    entries = [];
  }
  for (const entry of entries) {
    if (entry.isDirectory()) chmodWritable(path.join(dir, entry.name));
  }
  try {
    fs.chmodSync(dir, 0o755);
  } catch {
    /* already writable or gone */
  }
}

// A throwaway git repo: sharedTargetFor() resolves the shared cache from
// `git rev-parse --git-common-dir`, so a fake profile has to be a real repo.
function makeGitRepo(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codefactory-cargo-shared-sandbox-"));
  t.after(() => {
    chmodWritable(root);
    fs.rmSync(root, { recursive: true, force: true });
  });
  fs.mkdirSync(path.join(root, "src-tauri"), { recursive: true });
  fs.writeFileSync(path.join(root, "src-tauri", "Cargo.toml"), "[workspace]\n");
  execFileSync("git", ["init", "-q", "."], { cwd: root });
  fs.mkdirSync(path.join(root, ".codefactory-cache", "cargo-target"), { recursive: true });
  return root;
}

// Stands in for cargo and reports the target directory it was handed, so the
// test can prove which cache the build actually used.
function fakeCargo(t, exitCode) {
  const bin = fs.mkdtempSync(path.join(os.tmpdir(), "codefactory-cargo-bin-"));
  t.after(() => fs.rmSync(bin, { recursive: true, force: true }));
  const file = path.join(bin, "cargo");
  fs.writeFileSync(file, `#!/bin/sh\nprintf '%s\\n' "$CARGO_TARGET_DIR"\nexit ${exitCode}\n`);
  fs.chmodSync(file, 0o755);
  return bin;
}

function runShared(root, binDir) {
  return spawnSync(process.execPath, [SCRIPT, "check"], {
    cwd: root,
    encoding: "utf8",
    env: { ...process.env, PATH: `${binDir}${path.delimiter}${process.env.PATH}` },
  });
}

test("keeps the shared target when its in-use marker can be written", () => {
  const sharedDir = path.join("/shared", ".codefactory-cache", "cargo-target");
  const marked = [];
  const selection = selectTargetDir({
    sharedDir,
    worktreeRoot: "/worktree",
    mark: (dir) => marked.push(dir),
  });

  assert.equal(selection.targetDir, sharedDir);
  assert.equal(selection.degraded, null);
  assert.deepEqual(marked, [sharedDir]);
});

// R3: no silent downgrade — the caller gets the reason, not just a new path.
test("degrades to a task-local target and reports why when the shared cache is not writable", () => {
  const sharedDir = path.join("/shared", ".codefactory-cache", "cargo-target");
  const marked = [];
  const selection = selectTargetDir({
    sharedDir,
    worktreeRoot: "/worktree",
    env: { CODEFACTORY_TASK_ID: "m270" },
    mark: (dir) => {
      if (dir === sharedDir) throw Object.assign(new Error("EPERM: operation not permitted, open '" + dir + "'"), { code: "EPERM" });
      marked.push(dir);
    },
  });

  assert.equal(selection.targetDir, path.join("/worktree", ".codefactory-cache", "cargo-target-m270"));
  assert.deepEqual(marked, [selection.targetDir]);
  assert.equal(selection.degraded.sharedDir, sharedDir);
  assert.match(selection.degraded.reason, /EPERM/);
});

// Both unwritable is a hard stop: the message must name both failures and the
// way out, otherwise the operator is back to guessing.
test("fails with both reasons and a remedy when neither cache can be marked", () => {
  const sharedDir = path.join("/shared", ".codefactory-cache", "cargo-target");
  assert.throws(
    () =>
      selectTargetDir({
        sharedDir,
        worktreeRoot: "/worktree",
        env: { CODEFACTORY_TASK_ID: "m270" },
        mark: (dir) => {
          throw new Error(`EPERM: operation not permitted, ${dir}`);
        },
      }),
    (error) => {
      assert.match(error.message, /\/shared/);
      assert.match(error.message, /cargo-target-m270/);
      assert.match(error.message, /Fix:/);
      return true;
    },
  );
});

test("names the task-local target after the task and keeps it under .codefactory-cache", () => {
  const dir = taskTargetFor("/worktree", { CODEFACTORY_TASK_ID: "m270" });
  assert.equal(dir, path.join("/worktree", ".codefactory-cache", "cargo-target-m270"));
});

// R3 end to end: under the sandbox the build must still run, in a task-local
// directory, with the reason on stderr.
test("builds in the task-local cache when the shared cache is read-only", (t) => {
  const root = makeGitRepo(t);
  const shared = path.join(root, ".codefactory-cache", "cargo-target");
  fs.chmodSync(shared, 0o555);
  const bin = fakeCargo(t, 0);

  const run = runShared(root, bin);

  assert.equal(run.status, 0, run.stderr);
  // mkdtemp hands back /var/... while the script resolves the git path through
  // /private/var/..., so compare the real path.
  const realRoot = fs.realpathSync(root);
  const usedTarget = run.stdout.trim().split("\n").pop();
  assert.ok(
    usedTarget.startsWith(path.join(realRoot, ".codefactory-cache", "cargo-target-")),
    `cargo was handed ${usedTarget}`,
  );
  assert.match(run.stderr, /not writable/);
  assert.ok(run.stderr.includes(shared), run.stderr);
});

// R1/R3: when neither directory works, nothing is compiled and the exit code
// says so — the incident reported success for a build that never happened.
test("exits nonzero and compiles nothing when no cache directory can be marked", (t) => {
  const root = makeGitRepo(t);
  // Both halves read-only: the shared directory itself refuses the marker, and
  // the container refuses to create the task-local fallback.
  fs.chmodSync(path.join(root, ".codefactory-cache", "cargo-target"), 0o555);
  fs.chmodSync(path.join(root, ".codefactory-cache"), 0o555);
  const bin = fakeCargo(t, 0);

  const run = runShared(root, bin);

  assert.notEqual(run.status, 0, run.stdout);
  assert.equal(run.stdout.trim(), "");
  assert.match(run.stderr, /Nothing was compiled/);
  assert.match(run.stderr, /writable/);
});

// R1: cargo's own failure code is passed through untouched.
test("propagates cargo's exit code", (t) => {
  const root = makeGitRepo(t);
  const bin = fakeCargo(t, 7);

  const run = runShared(root, bin);

  assert.equal(run.status, 7, run.stderr);
});

// R5: a budget pass that could not run must not look like a clean build. A
// dangling managed-cache symlink is a real way scanCaches() fails.
test("exits nonzero when the budget pass fails even though cargo succeeded", (t) => {
  const root = makeGitRepo(t);
  fs.symlinkSync(path.join(root, ".codefactory-cache", "missing"), path.join(root, ".codefactory-cache", "cargo-target-ghost"));
  const bin = fakeCargo(t, 0);

  const run = runShared(root, bin);

  assert.notEqual(run.status, 0, `stdout=${run.stdout} stderr=${run.stderr}`);
  assert.match(run.stderr, /budget pass failed/);
});
