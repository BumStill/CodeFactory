#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate for per-session permission modes.

import { spawn, spawnSync } from "node:child_process";
import { access, mkdir, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const viteCli = path.join(root, "node_modules", "vite", "bin", "vite.js");
const port = Number(process.env.CODEFACTORY_PERMISSION_MODE_PORT ?? 1449);
const baseUrl = `http://127.0.0.1:${port}/permission-mode-acceptance.html`;
const artifactDir = process.env.CODEFACTORY_PERMISSION_MODE_ARTIFACT_DIR
  ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-permission-mode-headless");

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
    try { await access(candidate); return candidate; } catch { /* next */ }
  }
  throw new Error(`No system Chrome/Edge found. Tried: ${candidates.join(", ")}`);
}

async function waitForServer(child) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (child.exitCode != null || child.signalCode != null) throw new Error("Vite exited early");
    try { if ((await fetch(baseUrl)).ok) return; } catch { /* startup race */ }
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
  console.log(JSON.stringify({ service_pid: vite.pid, log: path.join(artifactDir, "vite.log"), url: baseUrl }));

  let browser;
  try {
    await waitForServer(vite);
    browser = await chromium.launch({ executablePath: await firstBrowser(), headless: true });
    const page = await browser.newPage({ viewport: { width: 1280, height: 720 } });
    await page.goto(baseUrl, { waitUntil: "networkidle" });
    await page.getByRole("main", { name: "Permission mode acceptance" }).waitFor({ timeout: 10_000 });

    const modeSelect = page.getByLabel("会话权限");
    await modeSelect.waitFor();
    assert(await modeSelect.inputValue() === "standard", "default session mode should be standard");
    await modeSelect.selectOption("trusted");
    await page.getByTestId("current-permission-mode").waitFor({ state: "visible" });
    assert((await page.getByTestId("current-permission-mode").innerText()).includes("mode:trusted"), "mode switch should update only the session");

    await page.getByRole("button", { name: "打开设置" }).click();
    await page.getByRole("button", { name: "端点", exact: true }).waitFor();
    assert((await page.getByRole("button", { name: "权限" }).count()) === 0, "global permissions tab should be hidden");
    assert((await page.getByText("工具权限").count()) === 0, "global tool-list editor should be hidden");

    await page.screenshot({ path: path.join(artifactDir, "permission-mode.png"), fullPage: true });
    console.log(JSON.stringify({
      status: "pass",
      artifactDir,
      checks: {
        sessionPermissionModeVisible: true,
        sessionPermissionModeSwitches: true,
        settingsPermissionTabHidden: true,
      },
    }, null, 2));
  } finally {
    if (browser) await browser.close();
    await stopServer(vite);
    await import("node:fs/promises").then(({ writeFile }) => writeFile(path.join(artifactDir, "vite.log"), viteLog));
    if (viteLog.trim()) console.error(viteLog.trim().split("\n").slice(-20).join("\n"));
  }
}

main().catch((error) => {
  console.error(`permission mode headless acceptance failed: ${error.stack ?? error}`);
  process.exit(1);
});
