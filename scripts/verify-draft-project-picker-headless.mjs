#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate: draft project picker menu must escape the clipped composer and remain selectable.

import { spawn, spawnSync } from "node:child_process";
import { access, mkdir, rm, writeFile } from "node:fs/promises";
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
    const page = await browser.newPage({ viewport: { width: 375, height: 812 } });
    await page.goto(baseUrl, { waitUntil: "networkidle" });
    await page.getByRole("main", { name: "Draft project picker acceptance" }).waitFor({ timeout: 10_000 });
    const compactGeometry = await page.evaluate(() => {
      const control = document.querySelector('[data-testid="message-input-control-row"]');
      const toolbar = document.querySelector('[data-testid="composer-utility-toolbar"]');
      const shortcut = document.querySelector('[data-testid="composer-shortcut-hint"]');
      const scope = document.querySelector('[aria-label^="选择项目："]');
      const model = document.querySelector('[aria-label^="选择下一回合模型："]');
      const more = document.querySelector('[aria-label="更多选项"]');
      if (!control || !toolbar || !shortcut || !scope || !model || !more) {
        throw new Error("missing compact composer controls");
      }
      const controlRect = control.getBoundingClientRect();
      const toolbarRect = toolbar.getBoundingClientRect();
      const targetRects = [scope, model, more].map((node) => node.getBoundingClientRect());
      return {
        viewportWidth: document.documentElement.clientWidth,
        pageScrollWidth: document.documentElement.scrollWidth,
        controlScrollWidth: control.scrollWidth,
        controlClientWidth: control.clientWidth,
        toolbarScrollWidth: toolbar.scrollWidth,
        toolbarClientWidth: toolbar.clientWidth,
        controlLeft: controlRect.left,
        controlRight: controlRect.right,
        controlHeight: controlRect.height,
        toolbarHeight: toolbarRect.height,
        shortcutDisplay: getComputedStyle(shortcut).display,
        targetSizes: targetRects.map((rect) => ({ width: rect.width, height: rect.height })),
      };
    });
    assert(compactGeometry.pageScrollWidth <= compactGeometry.viewportWidth, `375px page overflow: ${JSON.stringify(compactGeometry)}`);
    assert(compactGeometry.controlScrollWidth <= compactGeometry.controlClientWidth + 1, `composer overflow: ${JSON.stringify(compactGeometry)}`);
    assert(compactGeometry.toolbarScrollWidth <= compactGeometry.toolbarClientWidth + 1, `toolbar overflow: ${JSON.stringify(compactGeometry)}`);
    assert(compactGeometry.controlLeft >= 0 && compactGeometry.controlRight <= compactGeometry.viewportWidth, `composer must stay inside viewport: ${JSON.stringify(compactGeometry)}`);
    assert(compactGeometry.controlHeight <= 140, `resting composer must remain compact: ${JSON.stringify(compactGeometry)}`);
    assert(compactGeometry.toolbarHeight <= 56, `toolbar must not wrap into a third layer: ${JSON.stringify(compactGeometry)}`);
    assert(compactGeometry.shortcutDisplay === "none", `shortcut must stay hidden at 375px: ${JSON.stringify(compactGeometry)}`);
    assert(compactGeometry.targetSizes.every(({ width, height }) => width >= 44 && height >= 44), `narrow targets must be at least 44px: ${JSON.stringify(compactGeometry)}`);
    await page.screenshot({ path: path.join(artifactDir, "composer-375.png"), fullPage: true });

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
    assert(await page.getByRole("menuitemradio", { name: /独立任务/ }).getAttribute("aria-checked") === "true", "current draft scope must be exposed to assistive technology");
    assert(await page.getByRole("menuitemradio", { name: /独立任务/ }).evaluate((node) => node === document.activeElement), "project menu must focus its current item");
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("Enter");
    await page.waitForFunction(() => document.querySelector('[aria-label="Draft project picker probe"]')?.getAttribute('data-selected-cwd') === '/Users/leo/Projects/CodeFactory');
    await page.screenshot({ path: path.join(artifactDir, "draft-project-picker-visible.png"), fullPage: true });

    await page.setViewportSize({ width: 1366, height: 768 });
    await page.getByPlaceholder("描述任务或继续对话…").focus();
    const wideGeometry = await page.evaluate(() => {
      const control = document.querySelector('[data-testid="message-input-control-row"]');
      const shortcut = document.querySelector('[data-testid="composer-shortcut-hint"]');
      if (!control || !shortcut) throw new Error("missing wide composer controls");
      return {
        pageScrollWidth: document.documentElement.scrollWidth,
        viewportWidth: document.documentElement.clientWidth,
        controlHeight: control.getBoundingClientRect().height,
        shortcutDisplay: getComputedStyle(shortcut).display,
      };
    });
    assert(wideGeometry.pageScrollWidth <= wideGeometry.viewportWidth, `wide page overflow: ${JSON.stringify(wideGeometry)}`);
    assert(wideGeometry.controlHeight <= 140, `wide composer must remain compact: ${JSON.stringify(wideGeometry)}`);
    assert(wideGeometry.shortcutDisplay !== "none", `focused wide composer should progressively disclose shortcuts: ${JSON.stringify(wideGeometry)}`);
    await page.screenshot({ path: path.join(artifactDir, "composer-1366-focused.png"), fullPage: true });

    const receipt = {
      status: "pass",
      artifactDir,
      checks: {
        compact375: compactGeometry,
        menuEscapesComposerClip: true,
        projectKeyboardSelectable: true,
        wideFocusedComposer: wideGeometry,
      },
    };
    await writeFile(path.join(artifactDir, "receipt.json"), `${JSON.stringify(receipt, null, 2)}\n`);
    console.log(JSON.stringify(receipt, null, 2));
  } finally {
    if (browser) await browser.close();
    await stopServer(vite);
    await writeFile(path.join(artifactDir, "vite.log"), viteLog);
    if (viteLog.trim()) console.error(viteLog.trim().split("\n").slice(-20).join("\n"));
  }
}
main().catch((error) => { console.error(`draft project picker headless acceptance failed: ${error.stack ?? error}`); process.exit(1); });
