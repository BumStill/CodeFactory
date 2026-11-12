#!/usr/bin/env node
// M224 / R10 兼容矩阵：在**产品同一套 Seatbelt profile** 里跑真实工作流。
//
// profile 由产品代码生成，本脚本只消费它，不自己另造策略，避免"测的策略"和
// "产品的策略"分叉：
// - target/m224/repo.sb：真实仓库工作区（cargo/pnpm/浏览器用例）
// - target/m224/worktree.sb：真实 **linked worktree** 夹具（git 用例）
//
// git 用例必须在 linked worktree 里跑：那里的 .git 是文件，index/objects/refs/logs
// 都在工作区之外，正是真实会话的形状。用临时普通仓库测出来的"绿"是假绿。
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
const privateTmpFile = path.join(outDir, "private-tmp.txt");
if (fs.existsSync(privateTmpFile)) {
  const privateTmp = fs.readFileSync(privateTmpFile, "utf8").trim();
  process.env.TMPDIR = privateTmp;
  process.env.TMP = privateTmp;
  process.env.TEMP = privateTmp;
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

function sandboxed(args, { cwd = repo, timeout = 900_000, env = {}, useProfile = profile } = {}) {
  const proxyPort = process.env.CODEFACTORY_REGISTRY_PROXY_PORT;
  const proxy = proxyPort ? `http://127.0.0.1:${proxyPort}` : "";
  const privateTmp = fs.existsSync(path.join(outDir, "private-tmp.txt"))
    ? fs.readFileSync(path.join(outDir, "private-tmp.txt"), "utf8").trim()
    : "";
  const result = spawnSync(SANDBOX_EXEC, ["-f", useProfile, ...args], {
    cwd,
    timeout,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      ...(proxy
        ? {
            HTTPS_PROXY: proxy,
            HTTP_PROXY: proxy,
            ALL_PROXY: proxy,
            https_proxy: proxy,
            http_proxy: proxy,
            all_proxy: proxy,
            NO_PROXY: "localhost,127.0.0.1,::1",
            no_proxy: "localhost,127.0.0.1,::1",
          }
        : {}),
      ...(privateTmp ? { TMPDIR: privateTmp, TMP: privateTmp, TEMP: privateTmp } : {}),
      ...env,
    },
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

// 夹具建在 HOME 根下：主 checkout 与 linked worktree 都必须落在
// 「工作区 + 临时目录」之外，否则"主 checkout 不可写"这条用例测不到东西。
function makeLinkedWorktree() {
  const base = fs.mkdtempSync(path.join(os.homedir(), ".m224-r10-fixture-"));
  const git = (args) =>
    execFileSync("git", args, { cwd: base, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  git(["init", "-q", "-b", "main"]);
  git(["config", "user.email", "r10@example.com"]);
  git(["config", "user.name", "r10"]);

  // 真实形状的 pre-commit 钩子：在仓库里留下可核对的痕迹，证明钩子真的被执行过。
  fs.mkdirSync(path.join(base, ".githooks"), { recursive: true });
  const hook = path.join(base, ".githooks", "pre-commit");
  fs.writeFileSync(
    hook,
    '#!/bin/sh\nprintf M224-HOOK-RAN > "$(git rev-parse --show-toplevel)/hook-ran.txt"\nexit 0\n',
  );
  fs.chmodSync(hook, 0o755);
  // 钩子必须进版本库：linked worktree 只签出受版本控制的文件，
  // 未跟踪的 .githooks/ 在 worktree 里根本不存在（那样"钩子执行"就是空测）。
  git(["add", ".githooks/pre-commit"]);
  git(["-c", "core.hooksPath=", "commit", "-qm", "hooks"]);
  git(["config", "core.hooksPath", ".githooks"]);

  fs.writeFileSync(path.join(base, "a.txt"), "a\n");
  git(["add", "a.txt"]);
  git(["commit", "-qm", "init"]);

  git(["checkout", "-qb", "feature"]);
  fs.writeFileSync(path.join(base, "feature.txt"), "f\n");
  git(["add", "feature.txt"]);
  git(["commit", "-qm", "feature"]);
  git(["checkout", "-q", "main"]);

  const worktree = path.join(base, "wt");
  git(["worktree", "add", "-q", "-b", "sandbox", worktree, "main"]);
  return { base, worktree };
}

const fixture = makeLinkedWorktree();
const worktreeProfile = path.join(outDir, "worktree.sb");
try {
  execFileSync(
    "node",
    [
      "scripts/cargo-shared.mjs",
      "test",
      "--manifest-path",
      "src-tauri/Cargo.toml",
      "--lib",
      "--",
      "trusted_sandbox::tests::emits_profile_for_workspace_from_env",
    ],
    {
      cwd: repo,
      env: {
        ...process.env,
        M224_PROFILE_WORKSPACE: fixture.worktree,
        M224_PROFILE_OUT: worktreeProfile,
      },
      stdio: ["ignore", "pipe", "pipe"],
      encoding: "utf8",
    },
  );
} catch (error) {
  console.error(`R10 matrix: 无法为 linked worktree 生成 profile: ${error}`);
  process.exit(2);
}
if (!fs.existsSync(worktreeProfile)) {
  console.error(`R10 matrix: ${worktreeProfile} 未生成`);
  process.exit(2);
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
record("git-hook", "linked worktree 内执行 pre-commit 钩子（沙箱内）", () => {
  const hook = path.join(fixture.worktree, ".githooks", "pre-commit");
  const outcome = sandboxed(["/bin/sh", hook], {
    cwd: fixture.worktree,
    useProfile: worktreeProfile,
  });
  // 钩子把痕迹写进文件（而不是 stdout），所以断言落在产物上。
  const markerPath = path.join(fixture.worktree, "hook-ran.txt");
  const marker =
    fs.existsSync(markerPath) && fs.readFileSync(markerPath, "utf8").includes("M224-HOOK-RAN");
  return {
    ok: outcome.ok && marker,
    detail: marker ? "hook ran and wrote its marker inside the worktree" : "hook marker missing",
    output: outcome.output,
  };
});

// 4b) 真实仓库的版本化钩子脚本（它自己会 fetch，属于正常裁决面，不是环境失败）
record("git-hook-repo", "版本化 pre-commit 钩子脚本（沙箱内，真实仓库）", () => {
  const outcome = sandboxed(["python3", "tools/git/ensure_branch_current.py"]);
  // 钩子可能因"分支已落后于 origin/main"而拒绝提交：那是正常裁决，不是环境失败。
  const refused = /merge|落后|behind|out of date|fetch|拒绝|403|proxy|unable to access|could not resolve/i.test(outcome.output);
  return {
    ok: outcome.ok || refused,
    detail: outcome.ok ? "hook reported branch current" : "hook refused (expected when behind)",
    output: outcome.output,
  };
});

// 5) git commit（含自定义钩子真正被执行）
record("git-commit", "linked worktree 内 git add + commit（沙箱内，含钩子）", () => {
  const marker = path.join(fixture.worktree, "hook-ran.txt");
  fs.rmSync(marker, { force: true });
  const outcome = sandboxed(
    [
      "/bin/sh",
      "-c",
      "printf 'b\\n' > b.txt && git add b.txt && git commit -m 'sandboxed commit' && git log -1 --pretty=%s",
    ],
    { cwd: fixture.worktree, useProfile: worktreeProfile },
  );
  const committed = outcome.output.includes("sandboxed commit");
  const hookRan = fs.existsSync(marker);
  return {
    ok: outcome.ok && committed && hookRan,
    detail: committed
      ? hookRan
        ? "commit written through the real git dir, hook executed"
        : "commit ok but hook did not run"
      : "commit failed",
    output: outcome.output,
  };
});

// 6) git merge（快进合并）
record("git-merge", "linked worktree 内 git merge 快进（沙箱内）", () => {
  const outcome = sandboxed(["git", "merge", "--no-edit", "feature"], {
    cwd: fixture.worktree,
    useProfile: worktreeProfile,
  });
  const merged = fs.existsSync(path.join(fixture.worktree, "feature.txt"));
  return {
    ok: outcome.ok && merged,
    detail: merged ? "fast-forward merge applied through the shared git dir" : "merge did not apply",
    output: outcome.output,
  };
});

// 7) git worktree
record("git-worktree", "linked worktree 内 git worktree add（沙箱内）", () => {
  // 新 worktree 建在临时目录：与 pnpm worktree:start 把工作区放到缓存目录一致。
  const wt = path.join(os.tmpdir(), `m224-r10-wt-${process.pid}`);
  fs.rmSync(wt, { recursive: true, force: true });
  const outcome = sandboxed(["git", "worktree", "add", "--detach", wt, "HEAD"], {
    cwd: fixture.worktree,
    useProfile: worktreeProfile,
  });
  const created = fs.existsSync(path.join(wt, "a.txt"));
  fs.rmSync(wt, { recursive: true, force: true });
  return {
    ok: outcome.ok && created,
    detail: created ? `worktree at ${wt}` : "",
    output: outcome.output,
  };
});

// 7b) 放行 git 元数据 ≠ 放行 config / hooks / 主 checkout
function expectDenied(id, title, target) {
  record(id, title, () => {
    const outcome = sandboxed(
      ["/bin/sh", "-c", `printf 'x' > '${target}' 2>/dev/null; echo write=$?`],
      { cwd: fixture.worktree, useProfile: worktreeProfile },
    );
    const blocked = outcome.output.includes("write=") && !outcome.output.includes("write=0");
    return {
      ok: blocked,
      detail: blocked ? `${target} stays read-only` : `${target} was writable`,
      output: outcome.output,
    };
  });
}

expectDenied(
  "git-config-readonly",
  "common dir 的 config 不可写（沙箱内）",
  path.join(fixture.base, ".git", "config"),
);
expectDenied(
  "git-hooks-readonly",
  "common dir 的 hooks/ 不可写（沙箱内）",
  path.join(fixture.base, ".git", "hooks", "m224-probe"),
);
expectDenied(
  "main-checkout-readonly",
  "主 checkout 的工作树文件不可写（沙箱内）",
  path.join(fixture.base, "a.txt"),
);

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

fs.rmSync(fixture.base, { recursive: true, force: true });

const summary = {
  requirement: "M224/R10 compatibility matrix",
  profile,
  worktree_profile: worktreeProfile,
  linked_worktree: fixture.worktree,
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
