#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
//
// CF-BUILD baseline / after-change measurement.
//
// Reports, for the two numbers the spec's CF-BLD-R5/R6 require:
//   1. how much a single full-workspace test build writes to disk
//      (target-dir growth in bytes + files, and third-party crates compiled);
//   2. how much of that is wasted by a *second* task starting from zero
//      (dependency recompile ratio = shared crates recompiled / crates compiled).
//
// Usage:
//   node scripts/build-cache-baseline.mjs                 # report existing caches only
//   node scripts/build-cache-baseline.mjs --build         # also run 2 cold builds
//   node scripts/build-cache-baseline.mjs --build --root /tmp/cf-baseline
//
// The measurement is deliberately filesystem-based: macOS has no per-process
// write accounting that survives a build, so we report the on-disk footprint a
// build leaves behind (du) plus the compile-unit count from cargo's own log.
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import { fileURLToPath } from "node:url";
import os from "node:os";
import path from "node:path";

const selfPath = fileURLToPath(import.meta.url);
const repoRoot = path.resolve(path.dirname(selfPath), "..");

function arg(name) {
  const i = process.argv.indexOf(name);
  return i >= 0 ? process.argv[i + 1] : undefined;
}
const wantBuild = process.argv.includes("--build");
const stage = arg("--stage"); // "a" | "b" | undefined (=both)
const logFile = arg("--log") || path.join(os.tmpdir(), "cf-build-baseline.log");
const root = arg("--root") || path.join(os.tmpdir(), "cf-build-baseline");
const outFile = arg("--out");

export function dirSizeBytes(dir) {
  // Portable, dependency-free: du -sk is the fastest measurement on macOS.
  if (!fs.existsSync(dir)) return { bytes: 0, files: 0 };
  const out = spawnSync("du", ["-sk", dir], { encoding: "utf8" });
  const bytes = Number.parseInt((out.stdout || "0").trim().split(/\s+/)[0], 10) * 1024;
  const files = Number.parseInt(
    (spawnSync("find", [dir, "-type", "f"], { encoding: "utf8" }).stdout || "")
      .split("\n")
      .filter(Boolean).length,
    10,
  );
  return { bytes: Number.isFinite(bytes) ? bytes : 0, files: Number.isFinite(files) ? files : 0 };
}

/** Cargo prints `Compiling <name> v<version>` to stderr; that is our compile-unit log. */
export function parseCompiledCrates(stderr) {
  const seen = new Set();
  for (const line of stderr.split("\n")) {
    const m = /^\s*Compiling (\S+) v(\S+)/.exec(line);
    if (m) seen.add(`${m[1]}@${m[2]}`);
  }
  return seen;
}

function cacheRoots() {
  const roots = [];
  const mainCache = path.join(repoRoot, ".codefactory-cache");
  if (fs.existsSync(mainCache)) {
    for (const entry of fs.readdirSync(mainCache)) {
      roots.push(path.join(mainCache, entry));
    }
  }
  const appData = path.join(os.homedir(), "Library", "Application Support", "com.codefactory.app");
  const workspaces = path.join(appData, "execution-workspaces");
  if (fs.existsSync(workspaces)) {
    for (const ws of fs.readdirSync(workspaces)) {
      const inner = path.join(workspaces, ws, ".codefactory-cache");
      if (fs.existsSync(inner)) {
        for (const entry of fs.readdirSync(inner)) roots.push(path.join(inner, entry));
      }
    }
  }
  return roots;
}

function measureExistingCaches() {
  const rows = cacheRoots().map((dir) => {
    const { bytes, files } = dirSizeBytes(dir);
    return { dir, bytes, files, gb: +(bytes / 1024 ** 3).toFixed(2) };
  });
  const totalBytes = rows.reduce((sum, r) => sum + r.bytes, 0);
  return {
    roots: rows.sort((a, b) => b.bytes - a.bytes),
    total_bytes: totalBytes,
    total_gb: +(totalBytes / 1024 ** 3).toFixed(2),
  };
}

function coldBuildSync(targetDir, label) {
  fs.rmSync(targetDir, { recursive: true, force: true });
  fs.mkdirSync(targetDir, { recursive: true });
  const before = dirSizeBytes(targetDir);
  const started = Date.now();
  // Stream cargo's stderr to a log so a long build is observable while it runs,
  // then parse the same text for the compile-unit count.
  const chunks = [];
  const log = fs.openSync(logFile, "a");
  const proc = spawn("cargo", ["test", "--no-run", "--manifest-path", "src-tauri/Cargo.toml", "--workspace"], {
    cwd: repoRoot,
    env: {
      ...process.env,
      CARGO_TARGET_DIR: targetDir,
      // CF-BLD-R6 after-change measurement: `pnpm cargo:shared` now disables
      // incremental artifacts in the shared cache (they are keyed by worktree
      // path and therefore write-only ballast there).
      ...(process.argv.includes("--no-incremental") ? { CARGO_INCREMENTAL: "0" } : {}),
    },
  });
  return new Promise((resolve, reject) => {
    proc.stdout.on("data", (d) => { chunks.push(d); fs.writeSync(log, d); });
    proc.stderr.on("data", (d) => { chunks.push(d); fs.writeSync(log, d); });
    proc.on("error", reject);
    proc.on("close", (status) => {
      fs.closeSync(log);
      const elapsedMs = Date.now() - started;
      const after = dirSizeBytes(targetDir);
      const compiled = parseCompiledCrates(Buffer.concat(chunks).toString("utf8"));
      resolve({
        label,
        target_dir: targetDir,
        exit_code: status,
        elapsed_ms: elapsedMs,
        bytes_written: after.bytes - before.bytes,
        gb_written: +((after.bytes - before.bytes) / 1024 ** 3).toFixed(2),
        files_created: after.files - before.files,
        crates_compiled: compiled.size,
        crates: [...compiled].sort(),
      });
    });
  });
}

function readJson(file, fallback) {
  try { return JSON.parse(fs.readFileSync(file, "utf8")); } catch { return fallback; }
}

async function main() {
  const report = { generated_at: new Date().toISOString(), existing_caches: measureExistingCaches() };
  const store = outFile || path.join(root, "report.json");
  if (wantBuild) {
    fs.mkdirSync(root, { recursive: true });
    const saved = readJson(store, {});
    const which = stage || "ab";
    if (which.includes("a")) {
      saved.first = await coldBuildSync(path.join(root, "task-a"), "task-a (first task)");
    }
    if (which.includes("b")) {
      saved.second = await coldBuildSync(path.join(root, "task-b"), "task-b (second task)");
    }
    if (saved.first && saved.second) {
      const shared = saved.first.crates.filter((c) => saved.second.crates.includes(c));
      saved.repeated_crates = shared.length;
      saved.dependency_recompile_ratio = saved.first.crates_compiled
        ? +(shared.length / saved.first.crates_compiled).toFixed(4)
        : null;
    }
    report.builds = saved;
  }
  const text = JSON.stringify(report, null, 2);
  if (outFile) fs.writeFileSync(outFile, text);
  process.stdout.write(text + "\n");
}

if (process.argv[1] && path.resolve(process.argv[1]) === selfPath) main();

// exported for tests
export { coldBuildSync };
