#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate for the M37 progress bar. jsdom does not lay out CSS, so
// this renders the production TurnProgress in a real browser in light and dark
// mode and asserts the contract the task names: a plan the agent never tracked
// shows no fake count or percentage, a tracked plan shows the real one, the
// completion gate rerunning its checks stays neutral and keeps its wording off
// screen, and a wait the user must clear still warns. It also fails on clipped
// or overflowing text.
//
// It bundles the acceptance entry with the esbuild CLI and generates Tailwind
// CSS with the tailwindcss CLI, then serves the temp directory from a plain
// node http server, so the gate keeps working when the vite dev server or a
// host proxy is unavailable.

import { spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { access, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const entry = path.join(root, "src", "acceptance", "turn-progress.tsx");
const cssEntry = path.join(root, "src", "styles", "globals.css");
const tailwindConfig = path.join(root, "tailwind.config.js");
const port = Number(process.env.CODEFACTORY_TURN_PROGRESS_PORT ?? 1456);
const artifactDir = process.env.CODEFACTORY_TURN_PROGRESS_ARTIFACT_DIR
  ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-turn-progress-headless");
const bundleDir = path.join(artifactDir, "site");

/** Words that must never reach the user through the bar. */
const BANNED_WORDS = [
  "证据",
  "复核",
  "当前边界",
  "恢复耗尽",
  "安全上限",
  "系统故障",
  "已登记",
  "能力更新",
  "incident",
  "objective",
  "remediation",
  "generation",
  "recovery",
];

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
      "--loader:.css=empty",
      `--outfile=${path.join(bundleDir, "app.js")}`,
      "--log-level=warning",
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
    <title>CodeFactory Turn Progress Acceptance</title>
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

async function main() {
  await rm(artifactDir, { recursive: true, force: true });
  await mkdir(bundleDir, { recursive: true });
  await buildSite();

  const server = await serveSite();
  const baseUrl = `http://127.0.0.1:${port}/`;
  let browser;
  try {
    const executablePath = await firstBrowser();
    browser = await chromium.launch({
      executablePath,
      headless: true,
      proxy: { server: "direct://" },
      args: ["--disable-gpu", "--no-sandbox", "--no-proxy-server", "--proxy-bypass-list=*"],
    });
    const page = await browser.newPage({ viewport: { width: 1100, height: 900 } });
    await page.goto(baseUrl, { waitUntil: "domcontentloaded" });
    await page.getByRole("main", { name: "Turn progress acceptance" }).waitFor({ timeout: 15_000 });

    const untracked = page.getByRole("region", { name: "Plan not tracked" });
    const tracked = page.getByRole("region", { name: "Plan tracked" });
    const gate = page.getByRole("region", { name: "Completion gate rerunning checks" });
    const authorizing = page.getByRole("region", { name: "Needs authorization" });
    for (const region of [untracked, tracked, gate, authorizing]) await region.waitFor();

    const barText = async (region) =>
      (await region.getByTestId("turn-progress").textContent()) ?? "";

    const assertClean = async (label, region) => {
      const lowered = (await barText(region)).toLowerCase();
      for (const word of BANNED_WORDS) {
        assert(
          !lowered.includes(word.toLowerCase()),
          `${label}: internal word "${word}" reached the bar; rendered=${JSON.stringify(await barText(region))}`,
        );
      }
    };

    const assertFits = async (label, region) => {
      const bar = region.getByTestId("turn-progress");
      const overflow = await bar.evaluate((element) => ({
        scrollWidth: element.scrollWidth,
        clientWidth: element.clientWidth,
        scrollHeight: element.scrollHeight,
        clientHeight: element.clientHeight,
      }));
      assert(
        overflow.scrollWidth <= overflow.clientWidth + 1,
        `${label}: the bar overflows horizontally (${JSON.stringify(overflow)})`,
      );
      const viewportOverflow = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      );
      assert(viewportOverflow <= 1, `${label}: the page overflows horizontally by ${viewportOverflow}px`);
      assert(await bar.isVisible(), `${label}: the bar must be visible`);
    };

    // State (a): the agent never checked off a step. 2026-10-09 10:21 shape.
    const untrackedText = await barText(untracked);
    assert(
      !/\d+\/\d+/.test(untrackedText),
      `untracked plan must not show a step count; rendered=${JSON.stringify(untrackedText)}`,
    );
    assert(
      !untrackedText.includes("%"),
      `untracked plan must not show a percentage; rendered=${JSON.stringify(untrackedText)}`,
    );
    assert(
      !untrackedText.includes("个计划步骤"),
      `untracked plan must not claim a plan of steps; rendered=${JSON.stringify(untrackedText)}`,
    );
    assert(
      (await untracked.getByTestId("turn-progress").getAttribute("data-status-tone")) === "progress",
      "untracked plan must keep the neutral tone",
    );
    assert(
      (await untracked.locator('[role="progressbar"]').count()) === 0,
      "untracked plan must not render a progress bar that reads 0%",
    );
    assert(
      /7m\s?29s/.test(untrackedText) || untrackedText.includes("7m29s"),
      `untracked plan must still show the real elapsed time; rendered=${JSON.stringify(untrackedText)}`,
    );
    assert(
      untrackedText.includes("当前 · "),
      `untracked plan must still say what it is doing now; rendered=${JSON.stringify(untrackedText)}`,
    );
    await assertClean("untracked", untracked);
    await assertFits("untracked", untracked);

    // State (b): tracked steps — the real count and percentage are the point.
    const trackedText = await barText(tracked);
    assert(trackedText.includes("已完成 2/4"), `tracked plan must show 2/4; rendered=${JSON.stringify(trackedText)}`);
    assert(trackedText.includes("50%"), `tracked plan must show 50%; rendered=${JSON.stringify(trackedText)}`);
    assert(
      (await tracked.locator('[role="progressbar"]').getAttribute("aria-valuenow")) === "50",
      "tracked plan must expose the real progressbar value",
    );
    await assertClean("tracked", tracked);
    await assertFits("tracked", tracked);

    // State (c): the completion gate rerunning its checks is normal work.
    const gateBar = gate.getByTestId("turn-progress");
    const gateText = await barText(gate);
    assert(
      (await gateBar.getAttribute("data-status-tone")) === "progress",
      "the completion gate must not paint the warning tone",
    );
    assert(
      !gateText.includes("验证证据不足"),
      `the completion gate wording must stay off screen; rendered=${JSON.stringify(gateText)}`,
    );
    assert(
      !(await gateBar.getAttribute("class") ?? "").includes("border-status-warning"),
      "the completion gate must not use the warning border",
    );
    assert(
      gateText.includes("正在补跑检查"),
      `the completion gate must say in plain words what it is doing; rendered=${JSON.stringify(gateText)}`,
    );
    await assertClean("gate", gate);
    await assertFits("gate", gate);

    // State (d): a wait the user has to clear keeps its warning (requirement 3).
    const authorizingBar = authorizing.getByTestId("turn-progress");
    const authorizingText = await barText(authorizing);
    assert(
      (await authorizingBar.getAttribute("data-status-tone")) === "warning",
      "a required authorization must keep the warning tone",
    );
    assert(
      authorizingText.includes("需要你先授权才能继续"),
      `the authorization wait must keep plain wording; rendered=${JSON.stringify(authorizingText)}`,
    );
    await assertClean("authorizing", authorizing);
    await assertFits("authorizing", authorizing);

    await page.screenshot({ path: path.join(artifactDir, "turn-progress-dark.png"), fullPage: true });

    // Light mode: same states, recomputed layout and colors.
    await page.getByRole("button", { name: /切换主题/ }).click();
    await page.waitForFunction(() => document.documentElement.getAttribute("data-theme") === "light");
    assert(
      !(await barText(untracked)).includes("%"),
      "light mode must not reintroduce a percentage for an untracked plan",
    );
    await assertClean("untracked-light", untracked);
    await assertFits("untracked-light", untracked);
    await assertClean("tracked-light", tracked);
    await assertFits("tracked-light", tracked);
    await assertClean("gate-light", gate);
    await assertFits("gate-light", gate);
    await assertClean("authorizing-light", authorizing);
    await page.screenshot({ path: path.join(artifactDir, "turn-progress-light.png"), fullPage: true });

    // Narrow viewport: the conversation is often not full width.
    await page.setViewportSize({ width: 430, height: 900 });
    for (const [label, region] of [
      ["untracked", untracked],
      ["tracked", tracked],
      ["gate", gate],
      ["authorizing", authorizing],
    ]) {
      await assertFits(`${label}-narrow`, region);
    }
    await page.screenshot({ path: path.join(artifactDir, "turn-progress-narrow.png"), fullPage: true });

    console.log(JSON.stringify({
      status: "pass",
      artifactDir,
      screenshots: ["turn-progress-dark.png", "turn-progress-light.png", "turn-progress-narrow.png"],
      checks: {
        untracked: await barText(untracked),
        tracked: await barText(tracked),
        completionGate: await barText(gate),
        needsAuthorization: await barText(authorizing),
      },
    }, null, 2));
  } finally {
    if (browser) await browser.close();
    await new Promise((resolve) => server.close(resolve));
  }
}

main().catch((error) => {
  console.error(`turn progress acceptance failed: ${error.message}`);
  process.exit(1);
});
