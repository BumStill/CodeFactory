// SPDX-License-Identifier: Apache-2.0
//
// Reclaims disk from finished worktrees.
//
// Parallel agents (Codex, Claude Code) each create a git worktree per task, and
// every worktree grows its own multi-GB `src-tauri/target`. Nothing removed them
// when the work merged, so they accumulated to ~300 GB across 22 worktrees before
// this script existed.
//
// WHY THIS ISN'T A THREE-LINE CRON: `git merge-base --is-ancestor` reports a
// squash-merged branch as NOT merged, because the squash commit is a different
// object than the branch tip. Every branch this repo merges is squash-merged, so
// an ancestor-only check calls everything unmerged and reclaims nothing. This
// asks GitHub whether a merged PR exists for the branch, and treats the ancestor
// check as the fallback for branches that never had one.
//
// Usage:
//   node scripts/worktree-janitor.mjs                 # report only (default)
//   node scripts/worktree-janitor.mjs --apply         # remove merged+clean worktrees
//   node scripts/worktree-janitor.mjs --apply --stale-days 7
//                                                     # also cargo-clean worktrees
//                                                     # untouched for 7+ days
//   node scripts/worktree-janitor.mjs --prefix claude/ # only branches under a prefix
//
// SAFETY — a worktree is removed ONLY when both hold:
//   1. its branch is provably merged (merged PR, or an ancestor of origin/main), and
//   2. it has no uncommitted changes at all.
// Anything unmerged, dirty, the current worktree, or the main checkout is reported
// and left alone. `--stale-days` only deletes build output, never source or branches.

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

const args = process.argv.slice(2);
const APPLY = args.includes("--apply");
const PREFIX = valueOf("--prefix") ?? "";
const STALE_DAYS = Number(valueOf("--stale-days") ?? 0);
// Cheap nag for `postinstall`: counts worktrees and exits. No `du`, no network,
// no `gh` — it must not slow down or fail an install. Worktrees are what carry
// the multi-GB target dirs, so the count alone is a good enough proxy.
const WARN_ONLY = args.includes("--warn-only");
const WARN_ABOVE = 8;

if (WARN_ONLY) {
  try {
    const count = execFileSync("git", ["worktree", "list"], {
      cwd: process.cwd(),
      encoding: "utf8",
    })
      .split("\n")
      .filter(Boolean).length;
    if (count > WARN_ABOVE) {
      console.warn(
        `\n! ${count} git worktrees exist for this repo. Each carries its own ` +
          `multi-GB Cargo target.\n  Run \`pnpm worktrees\` to see what is finished, ` +
          `\`pnpm worktrees:clean\` to reclaim it.\n`,
      );
    }
  } catch {
    // Never let a nag break an install.
  }
  process.exit(0);
}

function valueOf(flag) {
  const i = args.indexOf(flag);
  return i >= 0 ? args[i + 1] : undefined;
}

function git(cwd, ...a) {
  return execFileSync("git", a, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
}

function tryGit(cwd, ...a) {
  try {
    return git(cwd, ...a);
  } catch {
    return null;
  }
}

function bytes(n) {
  const u = ["B", "K", "M", "G", "T"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) {
    n /= 1024;
    i += 1;
  }
  return `${n.toFixed(n < 10 && i > 0 ? 1 : 0)}${u[i]}`;
}

function dirSize(dir) {
  try {
    const out = execFileSync("du", ["-sk", dir], { encoding: "utf8" });
    return Number(out.split("\t")[0]) * 1024;
  } catch {
    return 0;
  }
}

const repoRoot = path.dirname(git(process.cwd(), "rev-parse", "--path-format=absolute", "--git-common-dir"));

// Branches with a merged PR. One API call beats one per worktree, and this is the
// only signal that survives squash-merging.
const mergedPrBranches = new Set();
try {
  const json = execFileSync(
    "gh",
    ["pr", "list", "--state", "merged", "--limit", "200", "--json", "headRefName"],
    { cwd: repoRoot, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] },
  );
  for (const pr of JSON.parse(json)) mergedPrBranches.add(pr.headRefName);
} catch {
  console.warn("! gh unavailable — falling back to ancestor checks only.");
  console.warn("  Squash-merged branches will read as unmerged and be left alone.\n");
}

tryGit(repoRoot, "fetch", "origin", "main", "--quiet");
tryGit(repoRoot, "worktree", "prune");

// `git worktree list --porcelain` emits stanzas separated by blank lines.
const worktrees = git(repoRoot, "worktree", "list", "--porcelain")
  .split("\n\n")
  .map((stanza) => {
    const dir = stanza.match(/^worktree (.+)$/m)?.[1];
    const branch = stanza.match(/^branch refs\/heads\/(.+)$/m)?.[1] ?? "(detached)";
    return dir ? { dir, branch } : null;
  })
  .filter(Boolean);

const here = git(process.cwd(), "rev-parse", "--show-toplevel");
const rows = [];

for (const { dir, branch } of worktrees) {
  if (!fs.existsSync(dir)) continue;
  if (PREFIX && !branch.startsWith(PREFIX)) continue;

  const isCurrent = dir === here;
  const isMain = dir === repoRoot;
  const head = tryGit(dir, "rev-parse", "HEAD");
  const dirty = (tryGit(dir, "status", "--porcelain") ?? "").split("\n").filter(Boolean).length;

  const byAncestor = head
    ? tryGit(repoRoot, "merge-base", "--is-ancestor", head, "origin/main") !== null
    : false;
  const byPr = mergedPrBranches.has(branch);
  const merged = byAncestor || byPr;

  const lastTouch = fs.statSync(dir).mtimeMs;
  const ageDays = Math.floor((Date.now() - lastTouch) / 86_400_000);

  rows.push({ dir, branch, isCurrent, isMain, dirty, merged, byPr, ageDays, size: dirSize(dir) });
}

rows.sort((a, b) => b.size - a.size);

let removable = 0;
let cleanable = 0;
console.log(`${rows.length} worktrees under ${repoRoot}\n`);

for (const r of rows) {
  const name = path.basename(r.dir);
  let verdict;
  if (r.isCurrent) verdict = "SKIP  current worktree";
  else if (r.isMain) verdict = "SKIP  main checkout";
  else if (!r.merged) verdict = `KEEP  unmerged${r.dirty ? ` + ${r.dirty} uncommitted` : ""}`;
  else if (r.dirty) verdict = `KEEP  merged but ${r.dirty} uncommitted — resolve by hand`;
  else {
    verdict = `REMOVE merged${r.byPr ? " (squash PR)" : ""}`;
    removable += r.size;
  }

  const target = path.join(r.dir, "src-tauri", "target");
  const staleTarget =
    STALE_DAYS > 0 && !r.isCurrent && r.ageDays >= STALE_DAYS && fs.existsSync(target);
  if (staleTarget && !verdict.startsWith("REMOVE")) cleanable += dirSize(target);

  console.log(
    `  ${bytes(r.size).padStart(6)}  ${String(r.ageDays).padStart(3)}d  ${verdict.padEnd(46)} ${name}`,
  );
  if (staleTarget && !verdict.startsWith("REMOVE")) {
    console.log(`          ${" ".repeat(52)}└ stale target: ${bytes(dirSize(target))}`);
  }
}

console.log(`\nReclaimable: ${bytes(removable)} from removals` +
  (STALE_DAYS > 0 ? ` + ${bytes(cleanable)} from stale build output` : ""));

if (!APPLY) {
  console.log("\nReport only. Re-run with --apply to act.");
  process.exit(0);
}

for (const r of rows) {
  if (r.isCurrent || r.isMain || !r.merged || r.dirty) continue;
  process.stdout.write(`removing ${path.basename(r.dir)} … `);
  try {
    git(repoRoot, "worktree", "remove", "--force", r.dir);
    // The branch is merged, so its ref carries nothing the remote lacks.
    tryGit(repoRoot, "branch", "-D", r.branch);
    console.log("ok");
  } catch (error) {
    console.log(`failed: ${error.message.split("\n")[0]}`);
  }
}

if (STALE_DAYS > 0) {
  for (const r of rows) {
    if (r.isCurrent || r.ageDays < STALE_DAYS) continue;
    const target = path.join(r.dir, "src-tauri", "target");
    if (!fs.existsSync(target)) continue;
    process.stdout.write(`cleaning build output in ${path.basename(r.dir)} … `);
    fs.rmSync(target, { recursive: true, force: true });
    console.log("ok");
  }
}

tryGit(repoRoot, "worktree", "prune");
console.log("\nDone.");
