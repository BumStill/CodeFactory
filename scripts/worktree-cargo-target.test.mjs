// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { ensureSharedTargetLink } from "./worktree-cargo-target.mjs";

test("creates a shared target symlink when a worktree has no target", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codefactory-target-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const worktree = path.join(root, "worktree");
  const sharedTarget = path.join(root, "shared", "cargo-target");
  fs.mkdirSync(path.join(worktree, "src-tauri"), { recursive: true });

  assert.equal(ensureSharedTargetLink({ worktree, sharedTarget }), "linked");
  assert.equal(fs.lstatSync(path.join(worktree, "src-tauri", "target")).isSymbolicLink(), true);
  assert.equal(fs.realpathSync(path.join(worktree, "src-tauri", "target")), fs.realpathSync(sharedTarget));
});

test("does not replace an existing local target directory", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "codefactory-target-test-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const worktree = path.join(root, "worktree");
  const target = path.join(worktree, "src-tauri", "target");
  const sharedTarget = path.join(root, "shared", "cargo-target");
  fs.mkdirSync(target, { recursive: true });
  fs.writeFileSync(path.join(target, "keep"), "local build output");

  assert.equal(ensureSharedTargetLink({ worktree, sharedTarget }), "existing-directory");
  assert.equal(fs.lstatSync(target).isDirectory(), true);
  assert.equal(fs.readFileSync(path.join(target, "keep"), "utf8"), "local build output");
});
