#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate for M36 ("编辑文件行最后写 123b-169b … 肯定是+多少行-多少行").
// jsdom does not lay out CSS and never computes a colour, so this renders the
// production ToolCallCard rows and the production TurnResultSnapshot in a real
// browser, in light and dark mode, and asserts: the collapsed edit row shows
// "+X −Y" and no "…b" character count, the write row shows a created file's
// line count without inventing deletions, the turn total aggregates only the
// edits that landed, the "+" and "−" colours stay visibly distinct and legible
// against the surface in both themes, and nothing is clipped.
//
// Same static-file approach as the M31/M32 gates: esbuild bundles the
// acceptance entry, the tailwind CLI generates the real stylesheet, and a
// plain node http server serves the result, so the gate never depends on the
// vite dev-server transform pipeline (which locked-down sandboxes block).

import { spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { access, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const entry = path.join(root, "src", "acceptance", "edit-line-stats.tsx");
const cssEntry = path.join(root, "src", "styles", "globals.css");
const tailwindConfig = path.join(root, "tailwind.config.js");
const port = Number(process.env.CODEFACTORY_EDIT_LINE_STATS_PORT ?? 1459);
const artifactDir = process.env.CODEFACTORY_EDIT_LINE_STATS_ARTIFACT_DIR
  ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-edit-line-stats-headless");
const bundleDir = path.join(artifactDir, "site");

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
    <title>CodeFactory Edit Line Stats Acceptance</title>
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

/** WCAG relative-luminance contrast ratio between two "rgb(...)" colours. */
const CONTRAST_HELPER = `(foreground, background) => {
  const parse = (value) => {
    const match = value.match(/rgba?\\(([^)]+)\\)/);
    if (!match) throw new Error("unparsable colour: " + value);
    const parts = match[1].split(",").map((part) => Number.parseFloat(part));
    return parts.slice(0, 3);
  };
  const luminance = (rgb) => {
    const [r, g, b] = rgb.map((channel) => {
      const c = channel / 255;
      return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const a = luminance(parse(foreground));
  const b = luminance(parse(background));
  const [light, dark] = a > b ? [a, b] : [b, a];
  return (light + 0.05) / (dark + 0.05);
}`;

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
    await page.getByRole("main", { name: "Edit line stats acceptance" }).waitFor({ timeout: 15_000 });

    const rows = page.locator("section[aria-label='Edit rows'] button").first();
    const editRow = page.getByRole("button", { name: /编辑 · src\/lib\/session-title\.ts · \+1 −1/ });
    const card = page.getByTestId("turn-result-snapshot");
    const total = page.getByTestId("turn-line-summary");

    const checkStats = async (theme) => {
      assert(await editRow.isVisible(), `${theme}: the collapsed edit row must be visible`);

      // The character-count summary ("985b → 4133b") must be gone.
      const editRowText = (await editRow.textContent()) ?? "";
      assert(
        !/→/.test(editRowText) && !/\d+b/.test(editRowText),
        `${theme}: the edit row must not print a character count; rendered=${JSON.stringify(editRowText)}`,
      );
      assert(
        editRowText.includes("+1") && editRowText.includes("−1"),
        `${theme}: the edit row must show +1 −1; rendered=${JSON.stringify(editRowText)}`,
      );

      const addRow = await page
        .getByRole("button", { name: /编辑 · src\/lib\/chat-plan\.ts · \+3 −0/ })
        .isVisible();
      const removeRow = await page
        .getByRole("button", { name: /编辑 · src\/lib\/dead-code\.ts · \+0 −2/ })
        .isVisible();
      assert(addRow, `${theme}: an edit that only adds lines must read +3 −0`);
      assert(removeRow, `${theme}: an edit that only removes lines must read +0 −2`);

      const writeRow = await page.getByTestId("line-change-stats").nth(3).textContent();
      assert(
        (writeRow ?? "").trim() === "+4",
        `${theme}: a created file must show its new line count only; rendered=${JSON.stringify(writeRow)}`,
      );
      const deniedRow = page.getByRole("button", { name: /编辑 · src\/lib\/denied\.ts/ });
      assert(
        !((await deniedRow.textContent()) ?? "").includes("+"),
        `${theme}: a denied edit must not claim added lines`,
      );

      // The turn total: four files (the denied edit is excluded), +8 −3.
      const totalText = (await total.textContent()) ?? "";
      assert(
        totalText.includes("本次改了 4 个文件"),
        `${theme}: the turn total must name the changed file count; rendered=${JSON.stringify(totalText)}`,
      );
      assert(
        totalText.includes("+8") && totalText.includes("−3"),
        `${theme}: the turn total must sum the landed edits (+8 −3); rendered=${JSON.stringify(totalText)}`,
      );

      // Colour convention: "+" green, "−" red, both legible on the surface.
      const colours = await card.evaluate(
        (element, helperSource) => {
          const contrast = eval(helperSource);
          const stats = element.querySelector('[data-testid="line-change-stats"]');
          if (!stats) throw new Error("no line stats inside the card");
          const [added, removed] = stats.querySelectorAll("span");
          const surface = element.querySelector("button[data-testid='turn-line-summary']");
          const background = getComputedStyle(surface).backgroundColor;
          return {
            added: getComputedStyle(added).color,
            removed: getComputedStyle(removed).color,
            addedContrast: contrast(getComputedStyle(added).color, background),
            removedContrast: contrast(getComputedStyle(removed).color, background),
          };
        },
        CONTRAST_HELPER,
      );
      assert(
        colours.added !== colours.removed,
        `${theme}: additions and deletions must not share one colour (${JSON.stringify(colours)})`,
      );
      const [addedRed, addedGreen] = colours.added.match(/\d+/g).map(Number);
      const [removedRed, removedGreen] = colours.removed.match(/\d+/g).map(Number);
      assert(
        addedGreen > addedRed && removedRed > removedGreen,
        `${theme}: additions must be green-dominant and deletions red-dominant; got ${JSON.stringify(colours)}`,
      );
      assert(
        colours.addedContrast >= 3 && colours.removedContrast >= 3,
        `${theme}: line stats must stay legible on the card surface; ${JSON.stringify(colours)}`,
      );

      // Nothing overflows the card or the row.
      const overflow = await card.evaluate((element) => ({
        scrollWidth: element.scrollWidth,
        clientWidth: element.clientWidth,
      }));
      assert(
        overflow.scrollWidth <= overflow.clientWidth + 1,
        `${theme}: the card overflows horizontally (${JSON.stringify(overflow)})`,
      );
      const rowOverflow = await rows.evaluate((element) => ({
        scrollWidth: element.scrollWidth,
        clientWidth: element.clientWidth,
      }));
      assert(
        rowOverflow.scrollWidth <= rowOverflow.clientWidth + 1,
        `${theme}: an edit row overflows its container (${JSON.stringify(rowOverflow)})`,
      );
    };

    await total.waitFor();
    await checkStats("dark");
    await page.screenshot({ path: path.join(artifactDir, "edit-line-stats-dark.png"), fullPage: true });

    await page.getByRole("button", { name: /切换主题/ }).click();
    await page.waitForFunction(() => document.documentElement.getAttribute("data-theme") === "light");
    await checkStats("light");
    await page.screenshot({ path: path.join(artifactDir, "edit-line-stats-light.png"), fullPage: true });

    // Clicking the total opens the existing changes view (the file list).
    const changesVisibleBefore = await page.getByText("改动的文件").isVisible();
    assert(!changesVisibleBefore, "the changes view must start closed");
    await total.click();
    await page.getByText("改动的文件").waitFor({ timeout: 5_000 });
    await page.screenshot({ path: path.join(artifactDir, "edit-line-stats-open-changes-light.png"), fullPage: true });

    console.log(JSON.stringify({
      status: "pass",
      artifactDir,
      screenshots: [
        "edit-line-stats-dark.png",
        "edit-line-stats-light.png",
        "edit-line-stats-open-changes-light.png",
      ],
      checks: {
        editRowShowsAddedAndRemovedLines: true,
        editRowDropsCharacterCount: true,
        createdFileShowsAddedOnly: true,
        deniedEditNotCounted: true,
        turnTotalAggregatesLandedEdits: true,
        greenAdditionsRedDeletionsBothThemes: true,
        legibleOnSurfaceBothThemes: true,
        noClippingBothThemes: true,
        totalOpensExistingChangesView: true,
      },
    }, null, 2));
  } finally {
    if (browser) await browser.close();
    await new Promise((resolve) => server.close(resolve));
  }
}

main().catch((error) => {
  console.error(`edit line stats headless acceptance failed: ${error.stack ?? error}`);
  process.exit(1);
});
