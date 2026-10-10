#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate for the build-cache occupancy panel (CF-BLD-R4).
//
// jsdom proves the component's text; only a real engine resolves the theme
// tokens, so only a real engine can show that the occupancy panel actually
// renders — and that its light and dark renderings are different rather than
// the same colours under two class names. This gate mounts the production
// component, asserts the numbers it shows, clicks the one action, and captures
// light and dark screenshots.
//
// A host HTTP proxy can block navigation to loopback, so this builds a static
// copy of the acceptance page (`base: "./"`) and drives it over `file://`.

import { mkdir, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "vite";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const artifactDir =
  process.env.CODEFACTORY_BUILD_CACHE_ARTIFACT_DIR ??
  path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-build-cache-headless");
const staticDir =
  process.env.CODEFACTORY_BUILD_CACHE_STATIC_DIR ??
  path.join(os.tmpdir(), "codefactory-build-cache-static");
const pageUrl = pathToFileURL(path.join(staticDir, "build-cache-acceptance.html")).href;

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
            "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
            "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
          ]
        : ["/usr/bin/google-chrome", "/usr/bin/chromium-browser", "/usr/bin/chromium"];
  const { access } = await import("node:fs/promises");
  for (const candidate of candidates) {
    try {
      await access(candidate);
      return candidate;
    } catch {
      /* keep looking */
    }
  }
  throw new Error("no chromium-based browser found for the build-cache acceptance gate");
}

async function main() {
  await rm(artifactDir, { recursive: true, force: true });
  await mkdir(artifactDir, { recursive: true });
  await mkdir(staticDir, { recursive: true });
  await build({
    root,
    base: "./",
    logLevel: "warn",
    build: {
      outDir: staticDir,
      emptyOutDir: true,
      rollupOptions: {
        input: { "build-cache-acceptance": path.join(root, "build-cache-acceptance.html") },
      },
    },
  });

  const browser = await chromium.launch({
    executablePath: await firstBrowser(),
    headless: true,
    proxy: { server: "direct://" },
    args: [
      "--disable-gpu",
      "--no-sandbox",
      "--no-proxy-server",
      "--proxy-bypass-list=<-loopback>",
      "--allow-file-access-from-files",
      "--disable-web-security",
    ],
  });

  const pageErrors = [];
  try {
    const page = await browser.newPage({ viewport: { width: 1000, height: 720 } });
    page.on("pageerror", (error) => pageErrors.push(String(error).split("\n")[0]));
    page.on("console", (message) => {
      if (message.type() === "error") pageErrors.push(`console: ${message.text().slice(0, 300)}`);
    });

    await page.goto(pageUrl, { waitUntil: "domcontentloaded" });
    const panel = page.locator('[data-testid="build-cache-panel"]');
    try {
      await panel.waitFor({ timeout: 15_000 });
    } catch {
      throw new Error(
        `the acceptance page did not render — ${pageErrors.join(" | ") || "no page error reported"}`,
      );
    }
    await page.getByText("编译缓存占用 12 GB，上限 60 GB").waitFor({ timeout: 10_000 });

    // CF-BLD-R4: the occupancy a user is shown is the runtime's own numbers.
    assert(await panel.isVisible(), "the build-cache panel must be visible");
    const text = await panel.innerText();
    assert(
      text.includes("等待编译空位（前面还有 2 个构建）"),
      `an in-flight build must be visible instead of looking idle: ${text}`,
    );
    assert(text.includes("5.0 GB"), `the first cache size must be shown: ${text}`);
    assert(text.includes("7.0 GB（构建中）"), `an in-use cache must say so: ${text}`);
    const entries = page.locator('[data-testid="build-cache-entries"] > li');
    assert((await entries.count()) === 2, "both caches must be listed");

    const box = await panel.boundingBox();
    assert(box && box.width > 0 && box.height > 0, "the panel must occupy a real layout box");

    // The app's theme is `data-theme` on the root element, and `:root` carries
    // the dark palette as the pre-hydration fallback — so light must be asked
    // for explicitly rather than assumed.
    await page.evaluate(() => {
      document.documentElement.dataset.theme = "light";
    });
    await page.waitForTimeout(150);
    const lightBg = await panel.evaluate((node) => getComputedStyle(node).backgroundColor);
    await page.screenshot({ path: path.join(artifactDir, "build-cache-light.png") });

    await page.evaluate(() => {
      document.documentElement.dataset.theme = "dark";
    });
    await page.waitForTimeout(150);
    const darkBg = await panel.evaluate((node) => getComputedStyle(node).backgroundColor);
    await page.screenshot({ path: path.join(artifactDir, "build-cache-dark.png") });
    assert(
      lightBg !== darkBg,
      `light and dark must render differently (both were ${lightBg}) — the toggle or the tokens are not wired`,
    );

    // CF-BLD-R4: one action cleans up, and it reports what really happened.
    await page.getByTestId("build-cache-cleanup").click();
    await page
      .getByText("已清理 7.0 GB，现在占用 5.0 GB。")
      .waitFor({ timeout: 10_000 });
    await page.screenshot({ path: path.join(artifactDir, "build-cache-dark-after-cleanup.png") });

    assert(pageErrors.length === 0, `the page reported errors: ${pageErrors.join(" | ")}`);

    process.stdout.write(
      JSON.stringify(
        {
          status: "ok",
          scenario: "CF-BLD-R4",
          light_screenshot: path.join(artifactDir, "build-cache-light.png"),
          dark_screenshot: path.join(artifactDir, "build-cache-dark.png"),
          dark_after_cleanup_screenshot: path.join(
            artifactDir,
            "build-cache-dark-after-cleanup.png",
          ),
          light_panel_background: lightBg,
          dark_panel_background: darkBg,
        },
        null,
        2,
      ) + "\n",
    );
  } finally {
    await browser.close();
  }
}

main().catch((error) => {
  process.stderr.write(`build-cache headless verification failed: ${error.message}\n`);
  process.exit(1);
});
