// SPDX-License-Identifier: Apache-2.0
//
// pnpm worktree:start <branch-name>
//
// Starts the worktree-default development lifecycle: fetches the latest
// origin/main, creates an isolated worktree at .claude/worktrees/<slug>
// checked out on a new branch, and prints the exact closeout command to run
// after the PR merges. The main checkout stays clean and releasable.
//
// See docs/principles/worktree-default-development.md.

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const BRANCH_RE = /^[a-z0-9]+(\/[a-z0-9][a-z0-9._-]*)*$/;
const RESERVED = new Set(["main", "origin", "HEAD", "master"]);

export function slugifyBranch(branchName) {
  return branchName.replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
}

export function validateBranchName(branchName) {
  if (!branchName) return "missing branch name (usage: pnpm worktree:start <branch-name>)";
  if (RESERVED.has(branchName)) return `reserved branch name: ${branchName}`;
  if (!BRANCH_RE.test(branchName)) {
    return `invalid branch name "${branchName}" (expected e.g. fix/my-change, feat/foo-bar)`;
  }
  return null;
}

export function worktreeDirFor(repoRoot, branchName) {
  return path.join(repoRoot, ".claude", "worktrees", slugifyBranch(branchName));
}

function git(cwd, ...args) {
  return execFileSync("git", args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
}

function main() {
  const branchName = process.argv[2];
  const invalid = validateBranchName(branchName);
  if (invalid) {
    console.error(`worktree:start: ${invalid}`);
    process.exit(2);
  }

  const repoRoot = git(process.cwd(), "rev-parse", "--show-toplevel");
  const dir = worktreeDirFor(repoRoot, branchName);

  if (fs.existsSync(dir)) {
    console.error(`worktree:start: target already exists: ${dir}`);
    process.exit(2);
  }
  git(repoRoot, "branch", "--list", branchName).split("\n").filter(Boolean).forEach((existing) => {
    if (existing.trim() === branchName) {
      console.error(`worktree:start: branch already exists: ${branchName}`);
      process.exit(2);
    }
  });

  console.log(`worktree:start: fetching latest origin/main …`);
  git(repoRoot, "fetch", "--prune", "origin", "main");
  const originHead = git(repoRoot, "rev-parse", "--short", "origin/main");

  git(repoRoot, "worktree", "add", dir, "-b", branchName, "origin/main");
  console.log(`worktree:start: created ${dir}`);
  console.log(`worktree:start: branch ${branchName} @ origin/main (${originHead})`);
  console.log(`worktree:start: cargo target auto-linked to the shared cache by post-checkout hook`);
  console.log("");
  console.log("Next steps:");
  console.log(`  cd ${dir}`);
  console.log("  … develop, verify, commit …");
  console.log("  … deliver_changes (PR -> CI -> squash merge) …");
  console.log(`After the PR merges, clean up automatically:`);
  console.log(`  pnpm worktrees:closeout -- --path ${dir} --apply`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
