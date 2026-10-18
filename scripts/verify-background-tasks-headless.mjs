#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// CF-BG 阶段 3（R6）真实浏览器门禁：会话内「N 个后台任务进行中」小标记。
//
// jsdom 不计算布局，所以这里用生产组件 + 合成数据在真实 layout engine 里检查：
// 标记文案与数量正确、展开后能看到命令/来源/状态并可停止/取消、明暗两种主题下
// 面板不超出视口、窄屏不横向溢出。静态文件方式与其它 verify-*-headless 一致。

import { spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { access, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const entry = path.join(root, "src", "acceptance", "session-background-tasks.tsx");
const cssEntry = path.join(root, "src", "styles", "globals.css");
const tailwindConfig = path.join(root, "tailwind.config.js");
const port = Number(process.env.CODEFACTORY_BACKGROUND_TASKS_PORT ?? 1473);
const artifactDir = process.env.CODEFACTORY_BACKGROUND_TASKS_ARTIFACT_DIR
  ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-background-tasks-headless");
const bundleDir = path.join(artifactDir, "site");

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function exists(candidate) {
  try {
    await access(candidate);
    return true;
  } catch {
    return false;
  }
}

async function findEsbuildBinary() {
  const direct = path.join(root, "node_modules", ".bin", "esbuild");
  if (await exists(direct)) return direct;
  const { readdir } = await import("node:fs/promises");
  const pnpmDir = path.join(root, "node_modules", ".pnpm");
  for (const entryName of await readdir(pnpmDir)) {
    if (!entryName.startsWith("@esbuild+")) continue;
    const platformDir = path.join(pnpmDir, entryName, "node_modules", "@esbuild");
    if (!(await exists(platformDir))) continue;
    for (const platform of await readdir(platformDir)) {
      const binary = path.join(platformDir, platform, "bin", "esbuild");
      if (await exists(binary)) return binary;
    }
  }
  throw new Error("no esbuild binary found under node_modules/.pnpm/@esbuild+*");
}

function run(command, args, label) {
  const result = spawnSync(command, args, { cwd: root, encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`${label} failed (${result.status}): ${result.stderr || result.stdout}`);
  }
}

async function buildSite() {
  const esbuild = await findEsbuildBinary();
  run(esbuild, [entry, "--bundle", "--format=esm", "--platform=browser", "--jsx=automatic",
    "--loader:.css=empty", `--outfile=${path.join(bundleDir, "app.js")}`, "--log-level=warning"], "esbuild bundle");
  run(process.execPath, [path.join(root, "node_modules", "tailwindcss", "lib", "cli.js"),
    "-c", tailwindConfig, "-i", cssEntry, "-o", path.join(bundleDir, "styles.css")], "tailwind build");
  await writeFile(path.join(bundleDir, "index.html"),
    `<!doctype html><html lang="zh-CN" data-theme="dark"><head><meta charset="UTF-8" />
<meta name="viewport" content="width=device-width, initial-scale=1.0" />
<link rel="stylesheet" href="./styles.css" /><title>CodeFactory Background Tasks Acceptance</title></head>
<body><div id="root"></div><script type="module" src="./app.js"></script></body></html>`, "utf8");
}

function serveSite() {
  const types = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8" };
  const server = createServer(async (request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1");
    const relative = url.pathname === "/" ? "/index.html" : url.pathname;
    try {
      const body = await readFile(path.join(bundleDir, path.normalize(relative).replace(/^(\.\.[/\\])+/, "")));
      response.writeHead(200, { "content-type": types[path.extname(relative)] ?? "application/octet-stream" });
      response.end(body);
    } catch {
      response.writeHead(404);
      response.end("not found");
    }
  });
  return new Promise((resolve) => server.listen(port, "127.0.0.1", () => resolve(server)));
}

async function firstBrowser() {
  const candidates = process.platform === "darwin"
    ? ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
       "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
       "/Applications/Chromium.app/Contents/MacOS/Chromium"]
    : process.platform === "win32"
      ? [path.join(process.env["PROGRAMFILES(X86)"] ?? "C:\\Program Files (x86)", "Microsoft/Edge/Application/msedge.exe"),
         path.join(process.env.PROGRAMFILES ?? "C:\\Program Files", "Google/Chrome/Application/chrome.exe")]
      : ["/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser"];
  for (const candidate of candidates) if (await exists(candidate)) return candidate;
  throw new Error(`No system Chrome/Edge found. Tried: ${candidates.join(", ")}`);
}

async function main() {
  await rm(artifactDir, { recursive: true, force: true });
  await mkdir(bundleDir, { recursive: true });
  await buildSite();
  const server = await serveSite();
  let browser;
  try {
    browser = await chromium.launch({
      executablePath: await firstBrowser(), headless: true,
      proxy: { server: "direct://" },
      args: ["--disable-gpu", "--no-sandbox", "--no-proxy-server", "--proxy-bypass-list=*"],
    });
    const page = await browser.newPage({ viewport: { width: 1366, height: 768 } });
    await page.goto(`http://127.0.0.1:${port}/`, { waitUntil: "domcontentloaded" });
    await page.getByRole("main", { name: "Session background tasks acceptance" }).waitFor({ timeout: 15_000 });

    const screenshots = [];
    for (const theme of ["light", "dark"]) {
      const section = page.getByRole("region", { name: `${theme} 会话头部` });
      const marker = section.getByRole("button", { name: /后台活动：1 个后台任务进行中/ });
      await marker.waitFor({ timeout: 10_000 });
      assert((await marker.textContent()).includes("1 个后台任务进行中"), `${theme}: marker count`);
      await marker.click();
      const panel = section.getByRole("region", { name: "后台任务详情" });
      await panel.waitFor();
      for (const needle of ["gh pr checks 742 --watch", "由 agent 启动", "运行中", "pnpm test",
        "按你的要求", "退出码 0", "盯着 PR #742，合并了告诉我", "每 180 秒"]) {
        assert((await panel.textContent()).includes(needle), `${theme}: panel missing ${needle}`);
      }
      assert(await section.getByRole("button", { name: /停止后台任务 gh pr checks 742 --watch/ }).isVisible(),
        `${theme}: stop control missing`);
      assert(await section.getByRole("button", { name: /取消定时唤醒/ }).isVisible(),
        `${theme}: cancel control missing`);
      const box = await panel.boundingBox();
      assert(box && box.x >= 0 && box.y >= 0 && box.x + box.width <= 1366 && box.y + box.height <= 768,
        `${theme}: panel escapes the viewport: ${JSON.stringify(box)}`);
      const file = path.join(artifactDir, `background-tasks-${theme}-1366.png`);
      await page.screenshot({ path: file, fullPage: false });
      screenshots.push(file);
      await marker.click();
    }

    // 窄屏：面板必须收进视口，不能横向滚出可用宽度。
    await page.setViewportSize({ width: 390, height: 720 });
    const narrowSection = page.getByRole("region", { name: "light 会话头部" });
    await narrowSection.getByRole("button", { name: /后台活动/ }).click();
    const narrowPanel = narrowSection.getByRole("region", { name: "后台任务详情" });
    const narrowBox = await narrowPanel.boundingBox();
    assert(narrowBox && narrowBox.x >= 0 && narrowBox.x + narrowBox.width <= 390,
      `narrow: panel overflows horizontally: ${JSON.stringify(narrowBox)}`);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth);
    assert(!overflow, "narrow: the page scrolls horizontally");
    const narrowFile = path.join(artifactDir, "background-tasks-narrow-390.png");
    await page.screenshot({ path: narrowFile, fullPage: false });
    screenshots.push(narrowFile);

    console.log("background tasks headless acceptance: OK");
    for (const file of screenshots) console.log(`screenshot: ${file}`);
  } finally {
    if (browser) await browser.close();
    await new Promise((resolve) => server.close(resolve));
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
