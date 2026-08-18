// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { resolveCargoCwd, stripLeadingSeparator } from "./cargo-shared.mjs";

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
