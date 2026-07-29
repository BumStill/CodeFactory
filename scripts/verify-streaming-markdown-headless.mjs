#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate for live timeline Markdown preservation.

import { spawn, spawnSync } from "node:child_process";
import { access, mkdir, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const viteCli = path.join(root, "node_modules", "vite", "bin", "vite.js");
const port = Number(process.env.CODEFACTORY_STREAMING_MARKDOWN_PORT ?? 1447);
const baseUrl = `http://127.0.0.1:${port}/streaming-markdown-acceptance.html`;
const artifactDir = process.env.CODEFACTORY_STREAMING_MARKDOWN_ARTIFACT_DIR
  ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-streaming-markdown-headless");

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
      // Try the next installed browser.
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

async function verifyViewport(page, viewport) {
  await page.setViewportSize(viewport);
  await page.goto(baseUrl, { waitUntil: "networkidle" });
  const main = page.getByRole("main", { name: "Streaming Markdown acceptance" });
  await main.waitFor({ timeout: 10_000 });
  await page.getByRole("button", { name: "模拟工具与后续文本" }).click();

  const firstStep = page.locator("[data-segment='step']").first();
  await firstStep.waitFor();
  assert(await firstStep.isVisible(), "transitioned Markdown step must be visible in the viewport");
  assert(await firstStep.locator("strong").innerText() === "当前验证状态", "bold title must survive tail-to-step transition");
  assert(await firstStep.locator("code").innerText() === "pnpm test", "inline code must survive tail-to-step transition");
  assert(await firstStep.locator("li").count() === 2, "list structure must survive tail-to-step transition");
  assert(await firstStep.locator("a").getAttribute("href") === "https://example.com/check", "link must survive tail-to-step transition");
  assert(!(await firstStep.innerText()).includes("**当前验证状态**"), "raw Markdown markers must not appear");
  assert(await firstStep.evaluate((node) => getComputedStyle(node).fontSize) === "15px", "intermediate narration must remain 15px");
  await page.screenshot({
    path: path.join(artifactDir, `streaming-markdown-transition-${viewport.width}x${viewport.height}.png`),
    fullPage: true,
  });

  await page.getByRole("button", { name: "加载长时间线边界" }).click();
  assert(await page.locator("[data-segment='step'] strong").count() === 12, "all completed long-timeline Markdown segments must remain structured");
  assert(await page.getByRole("button", { name: /展开较早的执行过程/ }).count() === 0, "active long turn must stay expanded");
  assert((await page.locator("body").evaluate((node) => node.scrollWidth <= node.clientWidth)), "page must not overflow horizontally");
  await page.screenshot({
    path: path.join(artifactDir, `streaming-markdown-${viewport.width}x${viewport.height}.png`),
    fullPage: true,
  });
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
    browser = await chromium.launch({ executablePath: await firstBrowser(), headless: true });
    const page = await browser.newPage();
    await verifyViewport(page, { width: 1366, height: 768 });
    await verifyViewport(page, { width: 800, height: 700 });
    console.log(JSON.stringify({
      status: "pass",
      artifactDir,
      checks: {
        markdownSurvivesTailToStep: true,
        bodySizePreserved: true,
        activeLongTimelineExpanded: true,
        viewports: ["1366x768", "800x700"],
      },
    }, null, 2));
  } finally {
    if (browser) await browser.close();
    await stopServer(vite);
    if (viteLog.trim()) console.error(viteLog.trim().split("\n").slice(-20).join("\n"));
  }
}

main().catch((error) => {
  console.error(`streaming markdown headless acceptance failed: ${error.stack ?? error}`);
  process.exit(1);
});
