#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0

import { spawn, spawnSync } from "node:child_process";
import { access, mkdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const viteCli = path.join(root, "node_modules", "vite", "bin", "vite.js");
const port = Number(process.env.CODEFACTORY_USAGE_HEADLESS_PORT ?? 1439);
const baseUrl = `http://127.0.0.1:${port}/usage-acceptance.html`;
const artifactDir = process.env.CODEFACTORY_USAGE_ARTIFACT_DIR
  ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-token-usage-headless");
let activeBrowser;
let activeServer;

function browserCandidates() {
  if (process.env.CODEFACTORY_HEADLESS_BROWSER) return [process.env.CODEFACTORY_HEADLESS_BROWSER];
  if (process.platform === "darwin") return [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
  ];
  if (process.platform === "win32") return [
    path.join(process.env["PROGRAMFILES(X86)"] ?? "C:\\Program Files (x86)", "Microsoft/Edge/Application/msedge.exe"),
    path.join(process.env.PROGRAMFILES ?? "C:\\Program Files", "Google/Chrome/Application/chrome.exe"),
  ];
  return ["/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser"];
}

async function firstExecutable() {
  for (const candidate of browserCandidates()) {
    try { await access(candidate); return candidate; } catch { /* try next */ }
  }
  throw new Error(`No system Chrome/Edge found. Tried: ${browserCandidates().join(", ")}`);
}

function assert(condition, message) { if (!condition) throw new Error(message); }

async function waitForServer(child) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (child.spawnError) throw child.spawnError;
    if (child.exitCode != null || child.signalCode != null) throw new Error(`Vite exited early: ${child.exitCode ?? child.signalCode}`);
    try { if ((await fetch(baseUrl)).ok) return; } catch { /* startup race */ }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Timed out waiting for ${baseUrl}`);
}

function waitForExit(child, timeoutMs) {
  if (child.exitCode != null || child.signalCode != null) return Promise.resolve(true);
  return new Promise((resolve) => {
    const timer = setTimeout(() => { child.off("exit", onExit); resolve(false); }, timeoutMs);
    const onExit = () => { clearTimeout(timer); resolve(true); };
    child.once("exit", onExit);
  });
}

async function stopServer(child) {
  if (!child || child.exitCode != null || child.signalCode != null) return;
  if (process.platform === "win32") {
    const result = spawnSync("taskkill", ["/pid", String(child.pid), "/t", "/f"], { stdio: "ignore" });
    if (result.status !== 0) child.kill();
  } else {
    try { process.kill(-child.pid, "SIGTERM"); } catch { child.kill("SIGTERM"); }
  }
  if (await waitForExit(child, 5_000)) return;
  try { process.kill(-child.pid, "SIGKILL"); } catch { child.kill("SIGKILL"); }
  await waitForExit(child, 2_000);
}

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.once(signal, () => { void activeBrowser?.close(); void stopServer(activeServer); });
}

async function run() {
  await rm(artifactDir, { recursive: true, force: true });
  await mkdir(artifactDir, { recursive: true });
  const vite = spawn(process.execPath, [viteCli, "--host", "127.0.0.1", "--port", String(port), "--strictPort"], {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, NO_COLOR: "1" },
    detached: process.platform !== "win32",
  });
  activeServer = vite;
  vite.spawnError = null;
  vite.on("error", (error) => { vite.spawnError = error; });
  let viteOutput = "";
  vite.stdout.on("data", (chunk) => { viteOutput += chunk.toString(); });
  vite.stderr.on("data", (chunk) => { viteOutput += chunk.toString(); });
  const browserMessages = [];
  let browser;
  let page;
  try {
    await waitForServer(vite);
    const executablePath = await firstExecutable();
    browser = await chromium.launch({ executablePath, headless: true, args: ["--disable-gpu", "--no-sandbox"] });
    activeBrowser = browser;
    page = await browser.newPage({ viewport: { width: 1366, height: 768 } });
    page.on("console", (message) => browserMessages.push(`console:${message.type()}: ${message.text()}`));
    page.on("pageerror", (error) => browserMessages.push(`pageerror:${error.stack ?? error}`));
    await page.goto(baseUrl, { waitUntil: "networkidle" });

    await page.getByRole("region", { name: "最近 28 天 Token 用量" }).waitFor();
    assert(await page.getByText("80K").isVisible(), "welcome today's total missing");
    assert(await page.getByText("订阅流量").isVisible(), "subscription semantics missing");
    assert(!await page.getByText(/实际费用 \$/).isVisible(), "subscription traffic exposed fake actual dollars");
    await page.screenshot({ path: path.join(artifactDir, "wide-welcome.png"), fullPage: true });

    await page.getByRole("button", { name: "查看用量详情" }).click();
    await page.getByRole("region", { name: "用量与预算" }).waitFor();
    assert(await page.getByRole("grid", { name: "Token 消耗地图，近 365 天" }).isVisible(), "365-day map missing");
    assert(await page.getByLabel("2026-07-19，数据缺失").isVisible(), "missing state inaccessible");
    assert(await page.getByLabel(/2026-07-20，24K Tokens，历史回填/).isVisible(), "partial state inaccessible");
    assert(await page.getByLabel(/2026-07-22，80K Tokens.*今天/).isVisible(), "today state inaccessible");
    assert(await page.getByText("今日 Token 已达到预算的 80%").isVisible(), "80 percent budget warning missing");
    await page.getByRole("button", { name: "预算占比" }).click();
    await page.getByRole("button", { name: "请求次数" }).click();
    await page.getByRole("button", { name: "Tokens" }).click();
    await page.screenshot({ path: path.join(artifactDir, "wide-settings.png"), fullPage: true });

    const day = page.getByLabel(/2026-07-21，46K Tokens/);
    await day.focus();
    await page.keyboard.press("Enter");
    await page.getByRole("region", { name: "2026-07-21 用量明细" }).waitFor();
    assert(await page.getByText("交互会话").isVisible(), "surface breakdown missing");
    assert(await page.getByText("修复图片识别与上传链路").isVisible(), "top session missing");
    await page.getByRole("button", { name: "查看作业日志" }).first().click();
    assert(await page.getByRole("status").getByText("已打开作业日志：project-session/task-1").isVisible(), "job log handoff missing");
    await page.getByRole("button", { name: "近 90 天" }).click();
    await page.getByRole("grid", { name: "Token 消耗地图，近 90 天" }).waitFor();
    await page.getByRole("button", { name: "近 180 天" }).click();
    await page.getByRole("grid", { name: "Token 消耗地图，近 180 天" }).waitFor();
    await page.screenshot({ path: path.join(artifactDir, "wide-day-detail.png"), fullPage: true });

    await page.setViewportSize({ width: 375, height: 812 });
    await page.reload({ waitUntil: "networkidle" });
    await page.getByRole("button", { name: "设置 / 用量与预算" }).click();
    await page.getByRole("region", { name: "用量与预算" }).waitFor();
    const overflow = await page.evaluate(() => ({ width: window.innerWidth, scrollWidth: document.documentElement.scrollWidth }));
    assert(overflow.scrollWidth <= overflow.width, `horizontal overflow: ${JSON.stringify(overflow)}`);
    await page.getByRole("button", { name: "近 90 天" }).focus();
    await page.keyboard.press("Enter");
    await page.getByRole("grid", { name: "Token 消耗地图，近 90 天" }).waitFor();
    assert(await page.getByLabel("按月用量列表").isVisible(), "narrow monthly list alternative missing");
    await page.screenshot({ path: path.join(artifactDir, "narrow-settings.png"), fullPage: true });

    const pageErrors = browserMessages.filter((message) => message.startsWith("pageerror:"));
    assert(pageErrors.length === 0, `browser page errors: ${pageErrors.join(" | ")}`);
    const receipt = {
      status: "pass",
      browser: executablePath,
      surfaces: ["new-session-28-day-summary", "settings-365-180-90-heatmap", "day-detail-job-log", "budget-threshold"],
      viewports: ["1366x768", "375x812"],
      billing_semantics: "subscription-no-fake-dollar",
      keyboard_paths: ["day-grid-enter", "range-button-enter"],
      artifact_dir: artifactDir,
      interactive_desktop_required: false,
      os_lock_state_observed: "not_measured",
    };
    await writeFile(path.join(artifactDir, "receipt.json"), `${JSON.stringify(receipt, null, 2)}\n`);
    process.stdout.write(`${JSON.stringify(receipt, null, 2)}\n`);
  } catch (error) {
    process.stderr.write(`${viteOutput}\n${browserMessages.join("\n")}\n`);
    if (page) await page.screenshot({ path: path.join(artifactDir, "failure.png"), fullPage: true }).catch(() => {});
    throw error;
  } finally {
    await browser?.close();
    await stopServer(vite);
    activeBrowser = undefined;
    activeServer = undefined;
  }
}

run().catch((error) => {
  process.stderr.write(`token usage headless acceptance failed: ${error.stack ?? error}\n`);
  process.exitCode = 1;
});
