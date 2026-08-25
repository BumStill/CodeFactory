#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate for preserving expanded project groups across session switches.

import { spawn, spawnSync } from "node:child_process";
import { access, mkdir, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const viteCli = path.join(root, "node_modules", "vite", "bin", "vite.js");
const port = Number(process.env.CODEFACTORY_SIDEBAR_EXPANSION_PORT ?? 1453);
const baseUrl = `http://127.0.0.1:${port}/sidebar-expansion-acceptance.html`;
const artifactDir = process.env.CODEFACTORY_SIDEBAR_EXPANSION_ARTIFACT_DIR ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-sidebar-expansion-headless");
function assert(condition, message) { if (!condition) throw new Error(message); }
async function firstBrowser() {
  const candidates = process.platform === "darwin"
    ? ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge", "/Applications/Chromium.app/Contents/MacOS/Chromium"]
    : process.platform === "win32"
      ? [path.join(process.env["PROGRAMFILES(X86)"] ?? "C:\\Program Files (x86)", "Microsoft/Edge/Application/msedge.exe"), path.join(process.env.PROGRAMFILES ?? "C:\\Program Files", "Google/Chrome/Application/chrome.exe")]
      : ["/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser"];
  for (const candidate of candidates) { try { await access(candidate); return candidate; } catch {} }
  throw new Error(`No system Chrome/Edge found. Tried: ${candidates.join(", ")}`);
}
async function waitForServer(child) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (child.exitCode != null || child.signalCode != null) throw new Error("Vite exited early");
    try { if ((await fetch(baseUrl)).ok) return; } catch {}
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Timed out waiting for ${baseUrl}`);
}
async function stopServer(child) {
  if (!child || child.exitCode != null || child.signalCode != null) return;
  if (process.platform === "win32") spawnSync("taskkill", ["/pid", String(child.pid), "/t", "/f"], { stdio: "ignore" });
  else { try { process.kill(-child.pid, "SIGTERM"); } catch { child.kill("SIGTERM"); } }
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
    browser = await chromium.launch({ executablePath: await firstBrowser(), headless: true, args: ["--disable-gpu", "--no-sandbox"] });
    const page = await browser.newPage({ viewport: { width: 420, height: 720 } });
    await page.goto(baseUrl, { waitUntil: "domcontentloaded" });
    await page.getByRole("main", { name: "Sidebar expansion acceptance" }).waitFor({ timeout: 10_000 });

    assert((await page.getByText("CodeFactory 主线", { exact: true }).count()) === 0, "project should start collapsed when quick session is active");
    await page.getByText("CodeFactory", { exact: true }).click();
    await page.getByText("CodeFactory 主线", { exact: true }).waitFor();
    await page.getByText("CodeFactory 主线", { exact: true }).click();
    await page.waitForFunction(() => document.querySelector('[aria-label="Sidebar expansion probe"]')?.getAttribute('data-current-session') === 'p1a');
    await page.getByText("改图脚本", { exact: true }).click();
    await page.waitForFunction(() => document.querySelector('[aria-label="Sidebar expansion probe"]')?.getAttribute('data-current-session') === 'q1');
    assert((await page.getByText("CodeFactory 主线", { exact: true }).count()) === 1, "expanded project should remain open after switching to quick session");
    assert((await page.getByText("CodeFactory 旧会话", { exact: true }).count()) === 1, "expanded project should keep all child sessions visible");
    await page.screenshot({ path: path.join(artifactDir, "preserved-after-switch.png"), fullPage: true });
    console.log(JSON.stringify({ status: "pass", artifactDir, checks: { expandedProjectSurvivesSessionSwitch: true } }, null, 2));
  } finally {
    if (browser) await browser.close();
    await stopServer(vite);
    await import("node:fs/promises").then(({ writeFile }) => writeFile(path.join(artifactDir, "vite.log"), viteLog));
    if (viteLog.trim()) console.error(viteLog.trim().split("\n").slice(-20).join("\n"));
  }
}
main().catch((error) => { console.error(`sidebar expansion headless acceptance failed: ${error.stack ?? error}`); process.exit(1); });
