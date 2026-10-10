#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate for CF-QUOTA-R4: the subscription quota surface must render
// the used share, the cap and the reset time in both themes, must not overflow
// at a narrow viewport, and must label an estimated reading as an estimate.

import { spawn, spawnSync } from "node:child_process";
import { access, mkdir, rm } from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const viteCli = path.join(root, "node_modules", "vite", "bin", "vite.js");
const port = Number(process.env.CODEFACTORY_SUBSCRIPTION_QUOTA_PORT ?? 1447);
const baseUrl = `http://127.0.0.1:${port}/subscription-quota-acceptance.html`;
const artifactDir =
  process.env.CODEFACTORY_SUBSCRIPTION_QUOTA_ARTIFACT_DIR ??
  path.join(root, ".codefactory-cache", "quota-screenshots");

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function firstBrowser() {
  const candidates =
    process.platform === "darwin"
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
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch {
      child.kill("SIGTERM");
    }
  }
}

async function assertSurface(page, theme) {
  await page.getByRole("main", { name: "订阅用量上限验收" }).waitFor({ timeout: 10_000 });
  assert(
    await page.getByText("本窗口已用 82%（上限 80%）", { exact: false }).first().isVisible(),
    `${theme}: the over-cap endpoint must show its used share against its cap`,
  );
  assert(
    await page.getByText("已达上限，已切到其它端点，为你保留余量").isVisible(),
    `${theme}: an endpoint over its cap must say CodeFactory handed over`,
  );
  assert(
    await page.getByText("本地估算").isVisible(),
    `${theme}: a locally estimated reading must be labelled as an estimate`,
  );
  assert(
    await page.getByText(/约 \d{2}:\d{2} 恢复/).first().isVisible(),
    `${theme}: a known window reset must be shown as a clock hint`,
  );
  assert(
    await page.getByLabel("ChatGPT 五小时窗口上限百分比").isVisible(),
    `${theme}: the 5-hour cap must be editable`,
  );
  assert(
    await page.getByLabel("ChatGPT 每周上限百分比").isVisible(),
    `${theme}: the weekly cap must be editable`,
  );
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
  const screenshots = {};
  try {
    await waitForServer(vite);
    const executablePath = await firstBrowser();
    browser = await chromium.launch({ executablePath, headless: true });

    for (const theme of ["dark", "light"]) {
      const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
      await page.goto(`${baseUrl}?theme=${theme}`, { waitUntil: "domcontentloaded" });
      await assertSurface(page, theme);
      const themeAttr = await page.evaluate(() => document.documentElement.dataset.theme);
      assert(themeAttr === theme, `${theme}: the acceptance page must render in the ${theme} theme`);
      const file = path.join(artifactDir, `subscription-quota-${theme}.png`);
      await page.screenshot({ path: file, fullPage: true });
      screenshots[theme] = file;

      // Viewport Harness: the surface must not push a horizontal scrollbar at a
      // narrow (phone-width) viewport.
      await page.setViewportSize({ width: 375, height: 800 });
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      );
      assert(overflow <= 1, `${theme}: the quota surface overflows a 375px viewport by ${overflow}px`);
      await page.close();
    }

    console.log(JSON.stringify({ status: "pass", artifactDir, screenshots }, null, 2));
  } finally {
    if (browser) await browser.close();
    await stopServer(vite);
    if (viteLog.trim()) {
      console.error(viteLog.trim().split("\n").slice(-20).join("\n"));
    }
  }
}

main().catch((error) => {
  console.error(`subscription quota headless acceptance failed: ${error.stack ?? error}`);
  process.exit(1);
});
