#!/usr/bin/env node
// M224 / R10 兼容矩阵：在**产品同一套 Seatbelt profile** 里跑真实工作流。
//
// profile 由 `cargo test --lib -- trusted_sandbox::tests::emits_repo_profile_for_the_r10_matrix`
// 生成到 target/m224/repo.sb；本脚本只消费它，不自己另造策略，避免"测的策略"和
// "产品的策略"分叉。
//
// 每个用例都真实启动进程；失败不会中断其余用例，最终结果写入
// target/m224/r10-matrix.json 并以非零退出码表达"有必过项失败"。

import { execFileSync, spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";

const repo = process.cwd();
const profile = path.join(repo, "target", "m224", "repo.sb");
const outDir = path.join(repo, "target", "m224");
const SANDBOX_EXEC = "/usr/bin/sandbox-exec";

if (!fs.existsSync(SANDBOX_EXEC)) {
  console.error(`R10 matrix: ${SANDBOX_EXEC} missing — cannot run on this host`);
  process.exit(2);
}
if (!fs.existsSync(profile)) {
  console.error(
    `R10 matrix: missing ${profile}\n` +
      "run: node scripts/cargo-shared.mjs test --manifest-path src-tauri/Cargo.toml --lib -- " +
      "trusted_sandbox::tests::emits_repo_profile_for_the_r10_matrix",
  );
  process.exit(2);
}

const results = [];
const started = Date.now();

function sandboxed(args, { cwd = repo, timeout = 900_000, env = {} } = {}) {
  const result = spawnSync(SANDBOX_EXEC, ["-f", profile, ...args], {
    cwd,
    timeout,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, ...env },
    maxBuffer: 64 * 1024 * 1024,
  });
  // 钩子/构建脚本常常写 stderr：两路都要留证据，否则"通过"可能是误判。
  const output = `${result.stdout ?? ""}${result.stderr ?? ""}`;
  return { ok: result.status === 0, output, status: result.status, signal: result.signal };
}

function record(id, title, runner) {
  const start = Date.now();
  let outcome;
  try {
    outcome = runner();
  } catch (error) {
    outcome = { ok: false, output: String(error && error.stack ? error.stack : error) };
  }
  const entry = {
    id,
    title,
    status: outcome.skipped ? "skipped" : outcome.ok ? "pass" : "fail",
    duration_ms: Date.now() - start,
    detail: outcome.detail ?? null,
    output: (outcome.output ?? "").slice(-1500),
  };
  results.push(entry);
  const tag = entry.status === "pass" ? "PASS" : entry.status === "skipped" ? "SKIP" : "FAIL";
  console.log(`[${tag}] ${id} — ${title} (${entry.duration_ms}ms)`);
  if (entry.status !== "pass" && entry.output) {
    console.log(
      entry.output
        .trim()
        .split("\n")
        .slice(-8)
        .map((line) => `       ${line}`)
        .join("\n"),
    );
  }
  return entry;
}

function mkTempRepo(tag) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), `m224-r10-${tag}-`));
  execFileSync("git", ["init", "-q", "-b", "main"], { cwd: dir });
  fs.writeFileSync(path.join(dir, "a.txt"), "a\n");
  execFileSync("git", ["add", "a.txt"], { cwd: dir });
  execFileSync(
    "git",
    ["-c", "user.email=t@example.com", "-c", "user.name=t", "commit", "-qm", "init"],
    { cwd: dir },
  );
  return dir;
}

// 1) Rust 项目测试（真实 cargo；target 目录放在工作区内，确保测的是沙箱白名单本身）
//
// 注意：这里不能筛 `trusted_sandbox`——那批用例自己要再调 sandbox-exec，而 macOS
// 禁止嵌套沙箱（sandbox_apply: Operation not permitted）。沙箱自身的用例在宿主侧
// 单独跑（`cargo test --lib -- trusted_sandbox`，8/8 通过），矩阵这里验证的是
// "项目测试能在这个 profile 里跑通"。
record("cargo-test", "cargo test（沙箱内）", () =>
  sandboxed(
    ["cargo", "test", "--manifest-path", "src-tauri/Cargo.toml", "--lib", "--", "config::settings"],
    { env: { CARGO_TARGET_DIR: path.join(repo, "target", "m224", "cargo") } },
  ),
);

// 2) 前端测试
record("pnpm-test", "pnpm exec vitest（沙箱内）", () =>
  sandboxed(["pnpm", "-s", "exec", "vitest", "run", "src/styles/radiusScaleAudit.test.ts"]),
);

// 3) 前端构建
record("pnpm-build", "pnpm build（沙箱内）", () => sandboxed(["pnpm", "-s", "build"]));

// 4) 版本化 pre-commit 钩子脚本本身
record("git-hook", "版本化 pre-commit 钩子脚本（沙箱内）", () => {
  const outcome = sandboxed(["python3", "tools/git/ensure_branch_current.py"]);
  // 钩子可能因"分支已落后于 origin/main"而拒绝提交：那是正常裁决，不是环境失败。
  const refused = /merge|落后|behind|out of date|fetch/i.test(outcome.output);
  return {
    ok: outcome.ok || refused,
    detail: outcome.ok ? "hook reported branch current" : "hook refused (expected when behind)",
    output: outcome.output,
  };
});

// 5) git commit（含自定义钩子真正被执行）
record("git-commit", "git commit + 钩子执行（沙箱内）", () => {
  const dir = mkTempRepo("commit");
  fs.writeFileSync(path.join(dir, "b.txt"), "b\n");
  execFileSync("git", ["add", "b.txt"], { cwd: dir });
  const hookDir = path.join(dir, ".githooks");
  fs.mkdirSync(hookDir, { recursive: true });
  const hook = path.join(hookDir, "pre-commit");
  fs.writeFileSync(hook, "#!/bin/sh\necho M224-HOOK-RAN\nexit 0\n");
  fs.chmodSync(hook, 0o755);
  const outcome = sandboxed(
    [
      "git",
      "-c",
      "core.hooksPath=.githooks",
      "-c",
      "user.email=t@example.com",
      "-c",
      "user.name=t",
      "commit",
      "-m",
      "sandboxed commit",
    ],
    { cwd: dir },
  );
  const hookRan = outcome.output.includes("M224-HOOK-RAN");
  return {
    ok: outcome.ok && hookRan,
    detail: hookRan ? "pre-commit hook executed" : "hook did not run",
    output: outcome.output,
  };
});

// 6) git merge（快进合并）
record("git-merge", "git merge 快进（沙箱内）", () => {
  const dir = mkTempRepo("merge");
  execFileSync("git", ["checkout", "-qb", "feature"], { cwd: dir });
  fs.writeFileSync(path.join(dir, "c.txt"), "c\n");
  execFileSync("git", ["add", "c.txt"], { cwd: dir });
  execFileSync(
    "git",
    ["-c", "user.email=t@example.com", "-c", "user.name=t", "commit", "-qm", "feature"],
    { cwd: dir },
  );
  const outcome = sandboxed(
    ["git", "-c", "user.email=t@example.com", "-c", "user.name=t", "merge", "--no-edit", "main"],
    { cwd: dir },
  );
  const merged = fs.existsSync(path.join(dir, "c.txt"));
  return { ok: outcome.ok && merged, detail: merged ? "fast-forward merge applied" : "", output: outcome.output };
});

// 7) git worktree
record("git-worktree", "git worktree add（沙箱内）", () => {
  const dir = mkTempRepo("worktree");
  const wt = path.join(os.tmpdir(), `m224-r10-wt-${process.pid}`);
  fs.rmSync(wt, { recursive: true, force: true });
  const outcome = sandboxed(["git", "worktree", "add", "--detach", wt, "main"], { cwd: dir });
  const created = fs.existsSync(path.join(wt, "a.txt"));
  return { ok: outcome.ok && created, detail: created ? `worktree at ${wt}` : "", output: outcome.output };
});

// 8) 后台任务启动路径
record("background-task", "后台任务（沙箱内 detached 子进程）", () => {
  const marker = path.join(outDir, "background-marker.txt");
  fs.rmSync(marker, { force: true });
  const errPath = path.join(outDir, "background-marker.err");
  fs.rmSync(errPath, { force: true });
  const fd = fs.openSync(errPath, "a");
  const child = spawn(
    SANDBOX_EXEC,
    [
      "-f",
      profile,
      "/bin/sh",
      "-c",
      // 工作区路径含空格：重定向目标必须加引号，否则会被 shell 拆词。
      `sleep 0.4; printf 'bg-ok' > '${marker}'`,
    ],
    { cwd: repo, detached: true, stdio: ["ignore", fd, fd] },
  );
  child.unref();
  const deadline = Date.now() + 25_000;
  while (Date.now() < deadline) {
    if (fs.existsSync(marker) && fs.readFileSync(marker, "utf8") === "bg-ok") {
      return { ok: true, detail: "detached sandboxed child wrote its marker" };
    }
    execFileSync("sleep", ["0.1"]);
  }
  const err = fs.existsSync(errPath) ? fs.readFileSync(errPath, "utf8") : "";
  return {
    ok: false,
    output: `background marker never appeared (exit=${child.exitCode} signal=${child.signalCode})\n${err}`,
  };
});

// 9) 浏览器验证（本机没有 Chrome 时明确记 skipped，不伪造通过）
record("browser", "Chrome 启动（沙箱内）", () => {
  const chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
  if (!fs.existsSync(chrome)) {
    return { skipped: true, output: "Chrome not installed on this host" };
  }
  const outcome = sandboxed([chrome, "--version"], { timeout: 60_000 });
  return { ok: outcome.ok, detail: "chrome --version ran inside the sandbox", output: outcome.output };
});

const summary = {
  requirement: "M224/R10 compatibility matrix",
  profile,
  started_at: new Date(started).toISOString(),
  duration_ms: Date.now() - started,
  totals: {
    pass: results.filter((r) => r.status === "pass").length,
    fail: results.filter((r) => r.status === "fail").length,
    skipped: results.filter((r) => r.status === "skipped").length,
  },
  results,
};
fs.mkdirSync(outDir, { recursive: true });
fs.writeFileSync(path.join(outDir, "r10-matrix.json"), `${JSON.stringify(summary, null, 2)}\n`);

console.log(
  `\nR10 matrix: ${summary.totals.pass} pass / ${summary.totals.fail} fail / ${summary.totals.skipped} skipped`,
);
console.log(`report: ${path.join(outDir, "r10-matrix.json")}`);
process.exit(summary.totals.fail === 0 ? 0 : 1);
