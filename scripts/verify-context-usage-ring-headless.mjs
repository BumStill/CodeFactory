#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// M295 real-browser gate: Windows-style occupied scrollbars, narrow viewport,
// and 125%/150% device scale must not move the trigger or hide the detail.

import { spawn, spawnSync } from "node:child_process";
import { access, mkdir, rm } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const viteCli = path.join(root, "node_modules", "vite", "bin", "vite.js");
const port = Number(process.env.CODEFACTORY_CONTEXT_USAGE_RING_PORT ?? 1459);
const baseUrl = `http://127.0.0.1:${port}/context-usage-ring-acceptance.html`;
const artifactDir = process.env.CODEFACTORY_CONTEXT_USAGE_RING_ARTIFACT_DIR
  ?? path.join(root, ".codefactory-cache", "m295-context-usage-ring");

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
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch {
      child.kill("SIGTERM");
    }
  }
}

function sameRect(before, after) {
  return ["x", "y", "width", "height"].every((key) => Math.abs(before[key] - after[key]) <= 0.5);
}

async function runCase(browser, { scale, width, theme }) {
  const context = await browser.newContext({
    viewport: { width, height: 720 },
    deviceScaleFactor: scale,
  });
  const page = await context.newPage();
  await page.goto(`${baseUrl}?theme=${theme}`, { waitUntil: "domcontentloaded" });
  const ring = page.getByTestId("context-usage-ring");
  await ring.waitFor();
  const before = await ring.boundingBox();
  assert(before, `${theme} ${scale}x: usage ring has no bounding box before click`);
  await ring.click();
  const detail = page.getByTestId("context-usage-detail-portal");
  await detail.waitFor();
  const after = await ring.boundingBox();
  const detailRect = await detail.boundingBox();
  assert(after, `${theme} ${scale}x: usage ring has no bounding box after click`);
  assert(detailRect, `${theme} ${scale}x: detail has no bounding box`);
  assert(sameRect(before, after), `${theme} ${scale}x: ring moved: ${JSON.stringify({ before, after })}`);
  assert(detailRect.x >= 0 && detailRect.y >= 0, `${theme} ${scale}x: detail begins outside viewport`);
  assert(detailRect.x + detailRect.width <= width + 0.5, `${theme} ${scale}x: detail exceeds viewport width`);
  assert(detailRect.y + detailRect.height <= 720.5, `${theme} ${scale}x: detail exceeds viewport height`);
  assert(Math.abs(detailRect.x + detailRect.width - (after.x + after.width)) <= 9,
    `${theme} ${scale}x: detail is not anchored near ring right edge`);

  await page.evaluate(() => window.scrollTo(0, 80));
  await page.waitForTimeout(50);
  const scrolledRing = await ring.boundingBox();
  const scrolledDetail = await detail.boundingBox();
  assert(scrolledRing && scrolledDetail, `${theme} ${scale}x: missing geometry after scroll`);
  assert(Math.abs(scrolledDetail.x + scrolledDetail.width - (scrolledRing.x + scrolledRing.width)) <= 9,
    `${theme} ${scale}x: detail did not follow ring after scroll`);

  await page.setViewportSize({ width: width - 32, height: 680 });
  const resizedRing = await ring.boundingBox();
  const resizedDetail = await detail.boundingBox();
  assert(resizedRing && resizedDetail, `${theme} ${scale}x: missing geometry after resize`);
  assert(resizedDetail.x >= 0 && resizedDetail.x + resizedDetail.width <= width - 31.5,
    `${theme} ${scale}x: detail is outside resized viewport`);
  assert(Math.abs(resizedDetail.x + resizedDetail.width - (resizedRing.x + resizedRing.width)) <= 9,
    `${theme} ${scale}x: detail did not follow ring after resize`);

  const screenshot = path.join(artifactDir, `usage-ring-${theme}-${String(scale).replace(".", "_")}x.png`);
  await page.screenshot({ path: screenshot, fullPage: false });
  await ring.click();
  assert(await detail.count() === 0, `${theme} ${scale}x: second trigger click did not close detail`);
  await ring.click();
  await page.getByRole("button", { name: "关闭上下文与用量详情" }).click();
  assert(await detail.count() === 0, `${theme} ${scale}x: close button did not close detail`);
  await context.close();
  return screenshot;
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
    browser = await chromium.launch({
      executablePath: await firstBrowser(),
      headless: true,
      args: ["--disable-breakpad", "--disable-crash-reporter"],
      env: {
        ...process.env,
        HOME: artifactDir,
        TMPDIR: artifactDir,
      },
    });
    const cases = [
      { scale: 1.25, width: 430, theme: "light" },
      { scale: 1.5, width: 390, theme: "dark" },
    ];
    const screenshots = [];
    for (const entry of cases) screenshots.push(await runCase(browser, entry));
    console.log(JSON.stringify({ status: "pass", cases, screenshots }, null, 2));
  } finally {
    if (browser) await browser.close();
    await stopServer(vite);
    if (viteLog.trim()) console.error(viteLog.trim().split("\n").slice(-20).join("\n"));
  }
}

main().catch((error) => {
  console.error(`M295 context usage ring acceptance failed: ${error.stack ?? error}`);
  process.exit(1);
});
