// SPDX-License-Identifier: Apache-2.0
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

export function sharedTargetFor(cwd) {
  const commonDir = execFileSync(
    "git",
    ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    { cwd, encoding: "utf8" },
  ).trim();
  return path.join(path.dirname(commonDir), ".codefactory-cache", "cargo-target");
}

export function ensureSharedTargetLink({ worktree, sharedTarget }) {
  const target = path.join(worktree, "src-tauri", "target");
  fs.mkdirSync(sharedTarget, { recursive: true });
  try {
    const stat = fs.lstatSync(target);
    if (!stat.isSymbolicLink()) return "existing-directory";
    return fs.realpathSync(target) === fs.realpathSync(sharedTarget) ? "already-linked" : "existing-link";
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  fs.symlinkSync(sharedTarget, target, "dir");
  return "linked";
}

function main() {
  const worktree = execFileSync("git", ["rev-parse", "--show-toplevel"], {
    cwd: process.cwd(), encoding: "utf8",
  }).trim();
  const result = ensureSharedTargetLink({ worktree, sharedTarget: sharedTargetFor(worktree) });
  if (result === "linked") console.log("cargo target: linked src-tauri/target to the shared cache");
  if (result === "existing-directory") console.warn("cargo target: kept existing local src-tauri/target; migrate it manually before using shared cache");
  if (result === "existing-link") console.warn("cargo target: kept src-tauri/target link because it points to another location");
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
