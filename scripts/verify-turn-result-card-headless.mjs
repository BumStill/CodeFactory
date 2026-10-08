#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate for the M31 result card. jsdom does not lay out CSS, so
// this renders the production TurnResultSnapshot in a real browser in light
// and dark mode and asserts the contract the task names: what happened, where
// the work is, what comes next, no internal vocabulary, no misleading progress
// number, and no clipped text.
//
// It bundles the acceptance entry with the esbuild CLI and generates Tailwind
// CSS with the tailwindcss CLI, then serves the temp directory from a plain
// node http server. That keeps the gate independent of the vite dev-server
// transform pipeline (whose esbuild service can be unavailable in locked-down
// sandboxes), while still exercising real layout.

import { spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { access, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const entry = path.join(root, "src", "acceptance", "turn-result-card.tsx");
const cssEntry = path.join(root, "src", "styles", "globals.css");
const tailwindConfig = path.join(root, "tailwind.config.js");
const port = Number(process.env.CODEFACTORY_TURN_RESULT_CARD_PORT ?? 1454);
const artifactDir = process.env.CODEFACTORY_TURN_RESULT_CARD_ARTIFACT_DIR
  ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-turn-result-card-headless");
const bundleDir = path.join(artifactDir, "site");

/** Words that must never reach the user through the card. */
const BANNED_WORDS = [
  "证据",
  "复核",
  "当前边界",
  "失败证据",
  "中断证据",
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
    <title>CodeFactory Turn Result Card Acceptance</title>
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
      // The host may export HTTP(S)_PROXY for outbound traffic; keep the
      // loopback dev server off it.
      proxy: { server: "direct://" },
      args: ["--disable-gpu", "--no-sandbox", "--no-proxy-server", "--proxy-bypass-list=*"],
    });
    const page = await browser.newPage({ viewport: { width: 1100, height: 900 } });
    await page.goto(baseUrl, { waitUntil: "domcontentloaded" });
    await page.getByRole("main", { name: "Turn result card acceptance" }).waitFor({ timeout: 15_000 });

    const failed = page.getByRole("region", { name: "Failed without a PR" });
    const completed = page.getByRole("region", { name: "Completed with a PR" });
    const waiting = page.getByRole("region", { name: "Waiting on CI" });
    await failed.waitFor();
    await completed.waitFor();
    await waiting.waitFor();

    const cardText = async (region) =>
      (await region.getByTestId("turn-result-snapshot").textContent()) ?? "";

    const assertClean = async (label, region) => {
      const lowered = (await cardText(region)).toLowerCase();
      for (const word of BANNED_WORDS) {
        assert(
          !lowered.includes(word.toLowerCase()),
          `${label}: internal word "${word}" reached the card; rendered=${JSON.stringify(await cardText(region))}`,
        );
      }
    };

    const assertNoClipping = async (label, region) => {
      const card = region.getByTestId("turn-result-snapshot");
      const overflow = await card.evaluate((element) => ({
        scrollWidth: element.scrollWidth,
        clientWidth: element.clientWidth,
        scrollHeight: element.scrollHeight,
        clientHeight: element.clientHeight,
      }));
      assert(
        overflow.scrollWidth <= overflow.clientWidth + 1,
        `${label}: the card overflows horizontally (${JSON.stringify(overflow)})`,
      );
      assert(await card.isVisible(), `${label}: the card must be visible`);
    };

    // State (a): failed, no PR, six changed files, plan never tracked.
    const failedCard = failed.getByTestId("turn-result-snapshot");
    const failedText = await cardText(failed);
    assert(
      (await failedCard.getAttribute("data-verdict")) === "failed",
      "the failed objective must render the failed verdict",
    );
    assert(failedText.includes("没做成"), `failed state must say it did not get done; rendered=${JSON.stringify(failedText)}`);
    assert(/已经改了 6 个文件/.test(failedText), `failed state must say where the changes are; rendered=${JSON.stringify(failedText)}`);
    assert(failedText.includes("保存在本次会话"), "failed state must say the changes are saved in this session's workspace");
    assert(failedText.includes("继续"), "failed state must say how to continue");
    assert(!failedText.includes("0/5"), "the card must not show a progress count the agent never tracked");
    assert(await failedCard.isVisible(), "the failed card must be visible");
    await assertClean("failed", failed);
    await assertNoClipping("failed", failed);

    // State (b): completed with a delivered PR.
    const completedText = await cardText(completed);
    assert(
      (await completed.getByTestId("turn-result-snapshot").getAttribute("data-status-tone")) === "success",
      "the completed objective must render the success tone",
    );
    assert(completedText.includes("已完成"), "completed state must say it is done");
    assert(completedText.includes("PR #568"), `completed state must name the PR; rendered=${JSON.stringify(completedText)}`);
    const prLink = completed.getByRole("link", { name: /PR #568/ });
    assert(
      (await prLink.getAttribute("href")) === "https://github.com/BumStill/CodeFactory/pull/568",
      "the PR link must point at the delivered pull request",
    );
    assert(await prLink.isVisible(), "the PR link must be visible");
    await assertClean("completed", completed);
    await assertNoClipping("completed", completed);

    // State (c): waiting on CI stays neutral and clean.
    const waitingText = await cardText(waiting);
    assert(waitingText.includes("外部等待"), `waiting state must keep its owner label; rendered=${JSON.stringify(waitingText)}`);
    await assertClean("waiting", waiting);
    await assertNoClipping("waiting", waiting);

    await page.screenshot({ path: path.join(artifactDir, "turn-result-card-dark.png"), fullPage: true });

    // Light mode: same states, recomputed layout and colors.
    await page.getByRole("button", { name: /切换主题/ }).click();
    await page.waitForFunction(() => document.documentElement.getAttribute("data-theme") === "light");
    const failedLightText = await cardText(failed);
    assert(failedLightText.includes("没做成"), "light mode must keep the failed verdict");
    await assertClean("failed-light", failed);
    await assertNoClipping("failed-light", failed);
    await assertClean("completed-light", completed);
    await assertClean("waiting-light", waiting);
    await page.screenshot({ path: path.join(artifactDir, "turn-result-card-light.png"), fullPage: true });

    // Narrow viewport: the card must stay inside the layout.
    await page.setViewportSize({ width: 430, height: 900 });
    await assertNoClipping("failed-narrow", failed);
    await page.screenshot({ path: path.join(artifactDir, "turn-result-card-narrow.png") });

    console.log(JSON.stringify({
      status: "pass",
      artifactDir,
      screenshots: ["turn-result-card-dark.png", "turn-result-card-light.png", "turn-result-card-narrow.png"],
      checks: {
        failedSaysItDidNotGetDone: true,
        failedSaysWhereTheChangesAre: true,
        failedSaysHowToContinue: true,
        untrackedPlanCountHidden: true,
        completedSaysDone: true,
        completedLinksThePullRequest: true,
        waitingStaysNeutral: true,
        noInternalVocabularyInAnyState: true,
        noClippingInLightOrDark: true,
      },
    }, null, 2));
  } finally {
    if (browser) await browser.close();
    await new Promise((resolve) => server.close(resolve));
  }
}

main().catch((error) => {
  console.error(`turn result card headless acceptance failed: ${error.stack ?? error}`);
  process.exit(1);
});
