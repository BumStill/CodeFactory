#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate for reconnect-banner attribution in MessageList.

import { spawn, spawnSync } from "node:child_process";
import { access, mkdir, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const viteCli = path.join(root, "node_modules", "vite", "bin", "vite.js");
const port = Number(process.env.CODEFACTORY_RECONNECT_BANNER_PORT ?? 1446);
const baseUrl = `http://127.0.0.1:${port}/reconnect-banner-acceptance.html`;
const artifactDir = process.env.CODEFACTORY_RECONNECT_BANNER_ARTIFACT_DIR
  ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-reconnect-banner-headless");

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function firstBrowser() {
  const candidates = process.platform === "darwin"
    ? [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
      ]
    : process.platform === "win32"
      ? [
          path.join(process.env["PROGRAMFILES(X86)"] ?? "C:\\Program Files (x86)", "Microsoft/Edge/Application/msedge.exe"),
          path.join(process.env.PROGRAMFILES ?? "C:\\Program Files", "Google/Chrome/Application/chrome.exe"),
        ]
      : ["/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser"];
  for (const candidate of candidates) {
    try {
      await access(candidate);
      return candidate;
    } catch {
      // Try the next installed browser without downloading one.
    }
  }
  throw new Error(`No system Chrome/Edge found. Tried: ${candidates.join(", ")}`);
}

async function waitForServer(child) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (child.exitCode != null || child.signalCode != null) throw new Error("Vite exited early");
    try {
      if ((await fetch(baseUrl)).ok) return;
    } catch {
      // Startup race.
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Timed out waiting for ${baseUrl}`);
}

async function stopServer(child) {
  if (!child || child.exitCode != null || child.signalCode != null) return;
  if (process.platform === "win32") {
    spawnSync("taskkill", ["/pid", String(child.pid), "/t", "/f"], { stdio: "ignore" });
  } else {
    try { process.kill(-child.pid, "SIGTERM"); } catch { child.kill("SIGTERM"); }
  }
}

async function main() {
  await rm(artifactDir, { recursive: true, force: true });
  await mkdir(artifactDir, { recursive: true });
  const vite = spawn(process.execPath, [viteCli, "--host", "127.0.0.1", "--port", String(port), "--strictPort"], {
    cwd: root,
    detached: process.platform !== "win32",
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, BROWSER: "none" },
  });
  let viteLog = "";
  vite.stdout.on("data", (chunk) => { viteLog += chunk.toString(); });
  vite.stderr.on("data", (chunk) => { viteLog += chunk.toString(); });

  let browser;
  try {
    await waitForServer(vite);
    const executablePath = await firstBrowser();
    browser = await chromium.launch({ executablePath, headless: true });
    const page = await browser.newPage({ viewport: { width: 1280, height: 720 } });
    await page.goto(baseUrl, { waitUntil: "networkidle" });
    await page.getByRole("main", { name: "Reconnect banner acceptance" }).waitFor({ timeout: 10_000 });

    const toolPanel = page.locator("section", { hasText: "Tool command is still running" });
    const modelPanel = page.locator("section", { hasText: "Actually waiting on model transport" });
    await toolPanel.waitFor();
    await modelPanel.waitFor();

    assert(
      await toolPanel.getByText("模型连接曾短暂不稳定，已完成重连", { exact: true }).isVisible(),
      "tool-running panel should show completed reconnect evidence, not an active model reconnect warning",
    );
    assert(
      (await toolPanel.getByText("模型连接不稳定，正在重新连接…", { exact: true }).count()) === 0,
      "tool-running panel must not blame model instability while a command is still running",
    );
    assert(
      await toolPanel.getByText(/运行中/).isVisible(),
      "tool-running panel should visibly contain a running tool state",
    );
    assert(
      await modelPanel.getByText("模型连接不稳定，正在重新连接…", { exact: true }).isVisible(),
      "model-waiting panel should still show the active reconnect warning",
    );
    const retryDisclosure = toolPanel
      .getByText("模型连接曾短暂不稳定，已完成重连", { exact: true })
      .locator("xpath=ancestor::details[1]");
    await retryDisclosure.locator("summary").click();
    assert(
      await retryDisclosure.evaluate((node) => node.open),
      "retry disclosure should open in the tool-running fixture",
    );
    assert(
      await retryDisclosure.getByText(/HTTP 503 Service Unavailable/).isVisible(),
      "retry evidence should remain expandable in the tool-running fixture",
    );
    await page.screenshot({ path: path.join(artifactDir, "reconnect-banner.png"), fullPage: true });
    console.log(JSON.stringify({
      status: "pass",
      artifactDir,
      checks: {
        toolRunningDoesNotShowActiveReconnect: true,
        modelWaitingStillShowsActiveReconnect: true,
      },
    }, null, 2));
  } finally {
    if (browser) await browser.close();
    await stopServer(vite);
    if (viteLog.trim()) {
      console.error(viteLog.trim().split("\n").slice(-20).join("\n"));
    }
  }
}

main().catch((error) => {
  console.error(`reconnect banner headless acceptance failed: ${error.stack ?? error}`);
  process.exit(1);
});
