#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Lock-safe acceptance gate for the resume-journal surface: drives a headless
// system Chrome/Edge against resume-journal-acceptance.html (real TaskDashboard
// + mocked Tauri IPC + a real resume_summary event replay). No app window, no
// unlocked desktop, no Rust backend — runs identically on a locked laptop and CI.

import { spawn, spawnSync } from "node:child_process";
import { access, mkdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const port = Number(process.env.CODEFACTORY_RESUME_HEADLESS_PORT ?? 1439);
const baseUrl = `http://127.0.0.1:${port}/resume-journal-acceptance.html`;
const artifactDir = process.env.CODEFACTORY_RESUME_ARTIFACT_DIR
  ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-resume-headless");
let activeBrowser;
let activeServer;
let signalShutdownStarted = false;

function browserCandidates() {
  if (process.env.CODEFACTORY_HEADLESS_BROWSER) return [process.env.CODEFACTORY_HEADLESS_BROWSER];
  if (process.platform === "darwin") {
    return [
      "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
      "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
      "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ];
  }
  if (process.platform === "win32") {
    return [
      path.join(process.env["PROGRAMFILES(X86)"] ?? "C:\\Program Files (x86)", "Microsoft/Edge/Application/msedge.exe"),
      path.join(process.env.PROGRAMFILES ?? "C:\\Program Files", "Google/Chrome/Application/chrome.exe"),
    ];
  }
  return ["/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser"];
}

async function firstExecutable() {
  for (const candidate of browserCandidates()) {
    try {
      await access(candidate);
      return candidate;
    } catch {
      // Try the next system browser; never download one as a side effect.
    }
  }
  throw new Error(`No system Chrome/Edge found. Tried: ${browserCandidates().join(", ")}`);
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function waitForServer(child) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (child.spawnError) throw child.spawnError;
    if (child.exitCode != null || child.signalCode != null) {
      throw new Error(`Vite exited early with code=${child.exitCode} signal=${child.signalCode}`);
    }
    try {
      const response = await fetch(baseUrl);
      if (response.ok) return;
    } catch {
      // Startup race; retry within the bounded deadline.
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Timed out waiting for ${baseUrl}`);
}

function waitForExit(child, timeoutMs) {
  if (child.exitCode != null || child.signalCode != null) return Promise.resolve(true);
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      child.off("exit", onExit);
      resolve(false);
    }, timeoutMs);
    const onExit = () => {
      clearTimeout(timer);
      resolve(true);
    };
    child.once("exit", onExit);
  });
}

async function stopServer(child) {
  if (!child || child.exitCode != null || child.signalCode != null) return;
  if (process.platform === "win32") {
    const result = spawnSync("taskkill", ["/pid", String(child.pid), "/t", "/f"], { stdio: "ignore" });
    if (result.status !== 0 && child.exitCode == null && child.signalCode == null) child.kill();
  } else {
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch {
      child.kill("SIGTERM");
    }
  }
  if (await waitForExit(child, 5_000)) return;
  if (process.platform !== "win32") {
    try {
      process.kill(-child.pid, "SIGKILL");
    } catch {
      child.kill("SIGKILL");
    }
  }
  await waitForExit(child, 2_000);
}

async function shutdownFromSignal(signal) {
  if (signalShutdownStarted) return;
  signalShutdownStarted = true;
  await activeBrowser?.close().catch(() => {});
  await stopServer(activeServer);
  process.exitCode = signal === "SIGINT" ? 130 : 143;
}

process.once("SIGINT", () => { void shutdownFromSignal("SIGINT"); });
process.once("SIGTERM", () => { void shutdownFromSignal("SIGTERM"); });

async function assertResumeSurface(page, viewport, tag) {
  await page.goto(baseUrl, { waitUntil: "networkidle" });
  const banner = page.getByTestId("resume-banner");
  await banner.waitFor({ timeout: 10_000 });

  const bannerText = (await banner.innerText()).replace(/\s+/g, "");
  assert(bannerText.includes("已从缓存恢复3个任务"), `[${tag}] restored count missing: ${bannerText}`);
  assert(bannerText.includes("重新执行2个"), `[${tag}] invalidated count missing: ${bannerText}`);
  assert(bannerText.includes("恢复中断任务1个"), `[${tag}] recovered count missing: ${bannerText}`);
  assert(bannerText.includes("输入变化"), `[${tag}] input_changed reason chip missing`);
  assert(bannerText.includes("检查点回滚"), `[${tag}] checkpoint_reverted reason chip missing`);
  assert(bannerText.includes("已恢复待执行"), `[${tag}] recovered-orphan chip missing`);

  // Three restored rows badge 已缓存, with the key_short tooltip.
  const badges = page.getByText("已缓存", { exact: true });
  assert((await badges.count()) === 3, `[${tag}] expected 3 已缓存 badges, got ${await badges.count()}`);
  const firstTitle = await badges.first().getAttribute("title");
  assert(firstTitle && firstTitle.includes("内容指纹"), `[${tag}] restored badge tooltip missing key_short`);

  // Invalidated tasks are back in the pending group (they will re-run).
  assert(await page.getByText("实现导出功能").first().isVisible(), `[${tag}] invalidated task row missing`);

  const overflow = await page.evaluate(() => ({
    width: window.innerWidth,
    scrollWidth: document.documentElement.scrollWidth,
  }));
  assert(overflow.scrollWidth <= overflow.width, `[${tag}] horizontal overflow: ${JSON.stringify(overflow)}`);

  await page.screenshot({ path: path.join(artifactDir, `${tag}.png`), fullPage: true });
}

async function run() {
  await rm(artifactDir, { recursive: true, force: true });
  await mkdir(artifactDir, { recursive: true });
  const vite = spawn("pnpm", ["exec", "vite", "--host", "127.0.0.1", "--port", String(port), "--strictPort"], {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, NO_COLOR: "1" },
    detached: process.platform !== "win32",
    shell: process.platform === "win32",
  });
  activeServer = vite;
  vite.spawnError = null;
  vite.on("error", (error) => { vite.spawnError = error; });
  let viteOutput = "";
  vite.stdout.on("data", (chunk) => { viteOutput += chunk.toString(); });
  vite.stderr.on("data", (chunk) => { viteOutput += chunk.toString(); });

  let browser;
  let page;
  const browserMessages = [];
  try {
    await waitForServer(vite);
    const executablePath = await firstExecutable();
    browser = await chromium.launch({
      executablePath,
      headless: true,
      args: ["--disable-gpu", "--no-sandbox"],
    });
    activeBrowser = browser;

    page = await browser.newPage({ viewport: { width: 1366, height: 768 } });
    page.on("console", (message) => browserMessages.push(`console:${message.type()}: ${message.text()}`));
    page.on("pageerror", (error) => browserMessages.push(`pageerror: ${error.stack ?? error}`));

    await assertResumeSurface(page, { width: 1366, height: 768 }, "wide-resume");

    await page.setViewportSize({ width: 390, height: 812 });
    await assertResumeSurface(page, { width: 390, height: 812 }, "narrow-resume");

    const pageErrors = browserMessages.filter((message) => message.startsWith("pageerror:"));
    assert(pageErrors.length === 0, `browser page errors: ${pageErrors.join(" | ")}`);

    const receipt = {
      status: "pass",
      browser: executablePath,
      surfaces: ["resume-banner-counts-reasons", "restored-badges", "invalidated-pending-rows"],
      viewports: ["1366x768", "390x812"],
      artifact_dir: artifactDir,
      interactive_desktop_required: false,
      os_lock_state_observed: "not_measured",
    };
    await writeFile(path.join(artifactDir, "receipt.json"), `${JSON.stringify(receipt, null, 2)}\n`);
    process.stdout.write(`${JSON.stringify(receipt, null, 2)}\n`);
  } catch (error) {
    process.stderr.write(`${viteOutput}\n`);
    if (browserMessages.length > 0) process.stderr.write(`${browserMessages.join("\n")}\n`);
    if (page) {
      await page.screenshot({ path: path.join(artifactDir, "failure.png"), fullPage: true }).catch(() => {});
      await writeFile(path.join(artifactDir, "failure.html"), await page.content()).catch(() => {});
    }
    throw error;
  } finally {
    await browser?.close();
    await stopServer(vite);
    activeBrowser = undefined;
    activeServer = undefined;
  }
}

run().catch((error) => {
  process.stderr.write(`resume-journal headless acceptance failed: ${error.stack ?? error}\n`);
  process.exitCode = 1;
});
