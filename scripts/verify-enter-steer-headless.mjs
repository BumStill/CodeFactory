#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate for M2 (Enter really steers the running turn). jsdom does
// not lay out CSS, so this bundles the production MessageInput / MessageList
// acceptance entry with esbuild + tailwind, serves it from a plain node server
// and drives a real browser in light and dark mode. It asserts the promise the
// composer makes BEFORE Enter matches what Enter actually does, and that the
// receipt after Enter is the one the send path returned.

import { spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { access, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const entry = path.join(root, "src", "acceptance", "enter-steer.tsx");
const cssEntry = path.join(root, "src", "styles", "globals.css");
const tailwindConfig = path.join(root, "tailwind.config.js");
const port = Number(process.env.CODEFACTORY_ENTER_STEER_PORT ?? 1458);
const artifactDir = process.env.CODEFACTORY_ENTER_STEER_ARTIFACT_DIR
  ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-enter-steer-headless");
const bundleDir = path.join(artifactDir, "site");

const HINT_LIVE = "Enter 引导当前执行 · ⌘Enter 等这轮结束再发 · Shift+Enter 换行";
const HINT_PENDING = "Enter 继续引导当前执行 · 上一条等当前步骤结束后读取 · Shift+Enter 换行";
const HINT_STALLED = "Enter 当前步骤结束后读取 · Shift+Enter 换行";

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function exists(candidate) {
  try {
    await access(candidate);
    return true;
  } catch {
    return false;
  }
}

/** esbuild is a transitive dependency; pnpm does not link it into .bin. */
async function findEsbuildBinary() {
  const direct = path.join(root, "node_modules", ".bin", "esbuild");
  if (await exists(direct)) return direct;
  const { readdir } = await import("node:fs/promises");
  const pnpmDir = path.join(root, "node_modules", ".pnpm");
  for (const entryName of await readdir(pnpmDir)) {
    if (!entryName.startsWith("@esbuild+")) continue;
    const platformDir = path.join(pnpmDir, entryName, "node_modules", "@esbuild");
    if (!(await exists(platformDir))) continue;
    for (const platform of await readdir(platformDir)) {
      const binary = path.join(platformDir, platform, "bin", "esbuild");
      if (await exists(binary)) return binary;
    }
  }
  throw new Error("no esbuild binary found under node_modules/.pnpm/@esbuild+*");
}

function run(command, args, label) {
  const result = spawnSync(command, args, { cwd: root, encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`${label} failed (${result.status}): ${result.stderr || result.stdout}`);
  }
}

async function buildSite() {
  const esbuild = await findEsbuildBinary();
  run(
    esbuild,
    [
      entry,
      "--bundle",
      "--format=esm",
      "--platform=browser",
      "--jsx=automatic",
      "--loader:.ttf=empty",
      "--loader:.woff=empty",
      "--loader:.woff2=empty",
      "--loader:.css=empty",
      `--outfile=${path.join(bundleDir, "app.js")}`,
      "--log-level=warning",
      `--alias:@tauri-apps/api/core=${path.join(root, "src/acceptance/enter-steer-ipc.ts")}`,
      `--alias:@tauri-apps/api/event=${path.join(root, "src/acceptance/enter-steer-ipc.ts")}`,
    ],
    "esbuild bundle",
  );
  run(
    process.execPath,
    [
      path.join(root, "node_modules", "tailwindcss", "lib", "cli.js"),
      "-c",
      tailwindConfig,
      "-i",
      cssEntry,
      "-o",
      path.join(bundleDir, "styles.css"),
    ],
    "tailwind build",
  );
  await writeFile(
    path.join(bundleDir, "index.html"),
    `<!doctype html>
<html lang="zh-CN" data-theme="dark">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <link rel="stylesheet" href="./styles.css" />
    <title>CodeFactory Enter Steer Acceptance</title>
  </head>
  <body><div id="root"></div><script type="module" src="./app.js"></script></body>
</html>
`,
    "utf8",
  );
}

const CONTENT_TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
};

function serveSite() {
  const server = createServer(async (request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1");
    const relative = url.pathname === "/" ? "/index.html" : url.pathname;
    const file = path.join(bundleDir, path.normalize(relative).replace(/^(\.\.[/\\])+/, ""));
    try {
      const body = await readFile(file);
      response.writeHead(200, {
        "content-type": CONTENT_TYPES[path.extname(file)] ?? "application/octet-stream",
      });
      response.end(body);
    } catch {
      response.writeHead(404);
      response.end("not found");
    }
  });
  return new Promise((resolve) => {
    server.listen(port, "127.0.0.1", () => resolve(server));
  });
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
    if (await exists(candidate)) return candidate;
  }
  throw new Error(`No system Chrome/Edge found. Tried: ${candidates.join(", ")}`);
}

/** Type text into a panel's composer and press Enter, exactly like a user. */
async function steerFrom(page, panel, text) {
  const region = page.getByRole("region", { name: panel });
  const textarea = region.getByRole("textbox").first();
  await textarea.click();
  await textarea.fill(text);
  await textarea.press("Enter");
}

async function main() {
  await rm(artifactDir, { recursive: true, force: true });
  await mkdir(bundleDir, { recursive: true });
  await buildSite();

  const server = await serveSite();
  const baseUrl = `http://127.0.0.1:${port}/`;
  let browser;
  const screenshots = [];
  try {
    const executablePath = await firstBrowser();
    browser = await chromium.launch({
      executablePath,
      headless: true,
      proxy: { server: "direct://" },
      args: ["--disable-gpu", "--no-sandbox", "--no-proxy-server", "--proxy-bypass-list=*"],
    });

    for (const theme of ["dark", "light"]) {
      const page = await browser.newPage({ viewport: { width: 1366, height: 768 } });
      await page.goto(baseUrl, { waitUntil: "networkidle" });
      await page.getByRole("main", { name: "Enter steering acceptance" }).waitFor({ timeout: 10_000 });
      await page.getByLabel("切换主题").click();
      const applied = await page.evaluate(() => document.documentElement.dataset.theme);
      assert(applied === "light", `theme toggle must apply before assertion; got ${applied}`);
      if (theme === "dark") await page.getByLabel("切换主题").click();
      assert(
        (await page.evaluate(() => document.documentElement.dataset.theme)) === theme,
        `acceptance page must render in ${theme}`,
      );

      const live = page.getByRole("region", { name: "Live run" });
      const blocked = page.getByRole("region", { name: "Tool blocked" });
      const stalled = page.getByRole("region", { name: "Stalled run" });

      // BEFORE Enter: the promise each state makes.
      assert(
        (await live.getByTestId("composer-shortcut-hint").textContent())?.includes(HINT_LIVE),
        "a live run must promise that Enter steers the current execution",
      );
      assert(
        (await blocked.getByTestId("composer-shortcut-hint").textContent())?.includes(HINT_PENDING),
        "a second steer while one is already pending must say it also waits",
      );
      assert(
        (await stalled.getByTestId("composer-shortcut-hint").textContent())?.includes(HINT_STALLED),
        "a stalled run must not promise steering",
      );
      assert(
        (await stalled.getByRole("button", { name: "当前步骤结束后读取" }).count()) === 1,
        "a stalled run's primary control must not be labelled 引导当前执行",
      );
      assert(
        (await page.getByText("等待当前步骤结束…").count()) === 1,
        "the conversation must show the undelivered steer as waiting",
      );

      // AFTER Enter: the receipt must be the one the send path returned.
      await steerFrom(page, "Live run", "改用 chrome channel");
      await live.getByText("已插入当前执行").waitFor({ timeout: 5_000 });
      await steerFrom(page, "Tool blocked", "这条等工具返回");
      await blocked.getByText("当前步骤结束后读取").waitFor({ timeout: 5_000 });
      await steerFrom(page, "Stalled run", "直接开新一轮");
      await stalled.getByText("已开始新一轮").waitFor({ timeout: 5_000 });

      const screenshot = path.join(artifactDir, `enter-steer-${theme}-1366x768.png`);
      await page.screenshot({ path: screenshot, fullPage: true });
      screenshots.push(screenshot);

      await page.setViewportSize({ width: 420, height: 820 });
      await page.waitForTimeout(150);
      const narrow = path.join(artifactDir, `enter-steer-${theme}-420x820.png`);
      await page.screenshot({ path: narrow, fullPage: true });
      screenshots.push(narrow);
      await page.close();
    }

    console.log(`enter-steer headless acceptance OK\n${screenshots.join("\n")}`);
  } finally {
    if (browser) await browser.close();
    await new Promise((resolve) => server.close(resolve));
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
