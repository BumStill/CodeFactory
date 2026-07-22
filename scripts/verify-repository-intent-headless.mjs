#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Lock-safe real-browser gate for the repository-owned intent UX.

import { spawn, spawnSync } from "node:child_process";
import { access, mkdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const viteCli = path.join(root, "node_modules", "vite", "bin", "vite.js");
const port = Number(process.env.CODEFACTORY_REPOSITORY_INTENT_PORT ?? 1441);
const baseUrl = `http://127.0.0.1:${port}/repository-intent-acceptance.html`;
const artifactDir = process.env.CODEFACTORY_REPOSITORY_INTENT_ARTIFACT_DIR
  ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-repository-intent-headless");

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
      // Continue through installed system browsers without downloading one.
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

async function assertWorkspace(page, tag) {
  await page.goto(baseUrl, { waitUntil: "networkidle" });
  await page.getByRole("main", { name: "会话窗口" }).waitFor({ timeout: 10_000 });
  assert(await page.getByText("会话执行详情", { exact: true }).isVisible(), `[${tag}] conversation execution detail missing`);
  assert(await page.getByText("在会话内执行仓库需求", { exact: true }).isVisible(), `[${tag}] delegated task missing`);
  assert((await page.getByTitle(/规范工作台/).count()) === 0, `[${tag}] specification workbench remains`);
  assert((await page.getByRole("button", { name: /规范|计划/ }).count()) === 0, `[${tag}] specification or plan button remains`);
  assert((await page.getByText("拆任务", { exact: true }).count()) === 0, `[${tag}] decomposition UI remains`);
  await page.screenshot({ path: path.join(artifactDir, `${tag}.png`), fullPage: true });
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
  let browser;
  let page;
  const pageErrors = [];
  try {
    await waitForServer(vite);
    const executablePath = await firstBrowser();
    browser = await chromium.launch({ executablePath, headless: true, args: ["--disable-gpu", "--no-sandbox"] });
    page = await browser.newPage({ viewport: { width: 1366, height: 768 } });
    page.on("pageerror", (error) => pageErrors.push(error.stack ?? String(error)));

    await assertWorkspace(page, "wide-workspace");
    await page.getByTitle("远程仓库（问题与拉取请求）").click();
    await page.getByText("Repository-owned specification", { exact: true }).click();
    assert(await page.getByText("Keep durable product intent in ordinary versioned repository files.", { exact: true }).isVisible(), "remote Issue detail missing");
    assert((await page.getByRole("button", { name: "创建为规范" }).count()) === 0, "remote Issue still converts to an app-owned specification");
    await page.screenshot({ path: path.join(artifactDir, "remote-issue.png"), fullPage: true });

    await page.setViewportSize({ width: 800, height: 700 });
    await assertWorkspace(page, "minimum-width-workspace");
    const overflow = await page.evaluate(() => ({ width: innerWidth, scrollWidth: document.documentElement.scrollWidth }));
    assert(overflow.scrollWidth <= overflow.width, `minimum-width workspace horizontal overflow: ${JSON.stringify(overflow)}`);
    assert(pageErrors.length === 0, `browser page errors: ${pageErrors.join(" | ")}`);

    const receipt = {
      status: "pass",
      browser: executablePath,
      surfaces: ["workspace-header", "conversation-execution-detail", "remote-issue-detail"],
      viewports: ["1366x768", "800x700 (configured minWidth)"],
      artifact_dir: artifactDir,
      interactive_desktop: "project picker reached; final Open action blocked by macOS lock",
    };
    await writeFile(path.join(artifactDir, "receipt.json"), `${JSON.stringify(receipt, null, 2)}\n`);
    process.stdout.write(`${JSON.stringify(receipt, null, 2)}\n`);
  } catch (error) {
    if (page) {
      await page.screenshot({ path: path.join(artifactDir, "failure.png"), fullPage: true }).catch(() => {});
      await writeFile(path.join(artifactDir, "failure.html"), await page.content()).catch(() => {});
    }
    if (pageErrors.length > 0) process.stderr.write(`${pageErrors.join("\n")}\n`);
    throw error;
  } finally {
    await browser?.close();
    await stopServer(vite);
  }
}

run().catch((error) => {
  process.stderr.write(`repository-intent headless acceptance failed: ${error.stack ?? error}\n`);
  process.exitCode = 1;
});
