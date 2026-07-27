// SPDX-License-Identifier: Apache-2.0
// Removes one finished, clean worktree after GitHub confirms its PR was merged.
// This intentionally does not use merge-base: squash merges have a different tip.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const args = process.argv.slice(2);
const APPLY = args.includes("--apply");
const pathIndex = args.indexOf("--path");
const requestedPath = pathIndex >= 0 ? args[pathIndex + 1] : undefined;

function fail(message) {
  console.error(`worktree closeout: ${message}`);
  process.exit(2);
}
function git(cwd, ...command) {
  return execFileSync("git", command, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
}

if (!requestedPath) fail("usage: pnpm worktrees:closeout -- --path <worktree> --apply");
const here = git(process.cwd(), "rev-parse", "--show-toplevel");
const commonDir = git(process.cwd(), "rev-parse", "--path-format=absolute", "--git-common-dir");
const repoRoot = path.dirname(commonDir);
const target = path.resolve(requestedPath);
const worktree = git(repoRoot, "worktree", "list", "--porcelain").split("\n\n")
  .map((stanza) => ({ dir: stanza.match(/^worktree (.+)$/m)?.[1], branch: stanza.match(/^branch refs\/heads\/(.+)$/m)?.[1] ?? "(detached)" }))
  .find((row) => row.dir === target);

if (!worktree?.dir || !fs.existsSync(target)) fail("target is not a registered worktree");
if (target === here) fail("refusing to remove the current worktree");
if (target === repoRoot) fail("refusing to remove the main checkout");
if (worktree.branch === "(detached)") fail("detached worktrees require an explicit manual decision");
const dirty = git(target, "status", "--porcelain").split("\n").filter(Boolean);
if (dirty.length) fail(`refusing dirty worktree (${dirty.length} uncommitted); save or discard it first`);

let mergedPrs;
try {
  mergedPrs = JSON.parse(execFileSync("gh", ["pr", "list", "--state", "merged", "--head", worktree.branch, "--json", "url,mergedAt"], {
    cwd: repoRoot, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"],
  }));
} catch {
  fail("GitHub CLI query failed; refusing to guess squash-merge status");
}
if (!mergedPrs.length) fail(`no merged GitHub PR found for ${worktree.branch}`);
const pr = mergedPrs[0];
if (!APPLY) {
  console.log(`READY ${target} (${worktree.branch}, merged ${pr.mergedAt})`);
  console.log("Report only. Re-run with --apply to remove the worktree and local branch.");
  process.exit(0);
}
git(repoRoot, "worktree", "remove", "--force", target);
git(repoRoot, "branch", "-D", worktree.branch);
git(repoRoot, "worktree", "prune");
console.log(`removed ${target} and local branch ${worktree.branch} (${pr.url})`);
