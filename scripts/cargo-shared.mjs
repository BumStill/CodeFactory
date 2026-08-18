// SPDX-License-Identifier: Apache-2.0
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

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

function main() {
  const targetDir = sharedTargetFor(process.cwd());
  const args = stripLeadingSeparator(process.argv.slice(2));

  if (args.length === 1 && args[0] === "--print-target") {
    process.stdout.write(`${targetDir}\n`);
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

  const result = spawnSync("cargo", args, {
    cwd: resolved.cwd,
    env: { ...process.env, CARGO_TARGET_DIR: targetDir },
    stdio: "inherit",
  });

  if (result.error) {
    throw result.error;
  }
  process.exit(result.status ?? 1);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
