#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate: draft project picker menu must escape the clipped composer and remain selectable.

import { spawn, spawnSync } from "node:child_process";
import { access, mkdir, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const viteCli = path.join(root, "node_modules", "vite", "bin", "vite.js");
const port = Number(process.env.CODEFACTORY_DRAFT_PROJECT_PICKER_PORT ?? 1455);
const baseUrl = `http://127.0.0.1:${port}/draft-project-picker-acceptance.html`;
const artifactDir = process.env.CODEFACTORY_DRAFT_PROJECT_PICKER_ARTIFACT_DIR ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-draft-project-picker-headless");
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
    const page = await browser.newPage({ viewport: { width: 960, height: 720 } });
    await page.goto(baseUrl, { waitUntil: "networkidle" });
    await page.getByRole("main", { name: "Draft project picker acceptance" }).waitFor({ timeout: 10_000 });
    await page.getByRole("button", { name: "选择项目" }).click();
    const menu = page.getByRole("menu", { name: "项目选择" });
    await menu.waitFor({ timeout: 10_000 });
    const geometry = await page.evaluate(() => {
      const menu = document.querySelector('[role="menu"][aria-label="项目选择"]');
      const composer = document.querySelector('[data-testid="clipped-composer"]');
      if (!menu || !composer) throw new Error("missing menu or composer");
      const menuRect = menu.getBoundingClientRect();
      const composerRect = composer.getBoundingClientRect();
      return {
        menuParentIsBody: menu.parentElement === document.body,
        menuPosition: getComputedStyle(menu).position,
        menuZIndex: getComputedStyle(menu).zIndex,
        menuTop: menuRect.top,
        menuBottom: menuRect.bottom,
        composerTop: composerRect.top,
        composerBottom: composerRect.bottom,
      };
    });
    assert(geometry.menuParentIsBody, "project picker menu must be portaled to document.body");
    assert(geometry.menuPosition === "fixed", `project picker menu must be fixed, got ${geometry.menuPosition}`);
    assert(Number(geometry.menuZIndex) >= 100, `project picker menu z-index should sit above workspace overlays, got ${geometry.menuZIndex}`);
    assert(geometry.menuBottom <= geometry.composerTop + 1, `menu should render above clipped composer, geometry=${JSON.stringify(geometry)}`);
    await page.getByTitle("/Users/leo/Projects/CodeFactory").click();
    await page.waitForFunction(() => document.querySelector('[aria-label="Draft project picker probe"]')?.getAttribute('data-selected-cwd') === '/Users/leo/Projects/CodeFactory');
    await page.screenshot({ path: path.join(artifactDir, "draft-project-picker-visible.png"), fullPage: true });
    console.log(JSON.stringify({ status: "pass", artifactDir, checks: { menuEscapesComposerClip: true, projectPathSelectable: true } }, null, 2));
  } finally {
    if (browser) await browser.close();
    await stopServer(vite);
    await import("node:fs/promises").then(({ writeFile }) => writeFile(path.join(artifactDir, "vite.log"), viteLog));
    if (viteLog.trim()) console.error(viteLog.trim().split("\n").slice(-20).join("\n"));
  }
}
main().catch((error) => { console.error(`draft project picker headless acceptance failed: ${error.stack ?? error}`); process.exit(1); });
