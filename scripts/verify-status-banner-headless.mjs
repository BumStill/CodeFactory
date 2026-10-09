#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate for the session status banner (CF-RSB-R1..R4). It renders
// the production MessageList in every banner state, asserts the visible copy and
// layout in a real browser (not jsdom), and captures light/dark/narrow shots.
//
// A host HTTP proxy can block browser navigation to loopback entirely, so this
// gate builds a static copy of the acceptance page (`base: "./"`) and drives it
// over `file://`, which uses no HTTP proxy at all.

import { access, mkdir, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "vite";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const artifactDir = process.env.CODEFACTORY_STATUS_BANNER_ARTIFACT_DIR
  ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-status-banner-headless");
const staticDir = process.env.CODEFACTORY_STATUS_BANNER_STATIC_DIR
  ?? path.join(os.tmpdir(), "codefactory-status-banner-static");
const pageUrl = pathToFileURL(path.join(staticDir, "status-banner-acceptance.html")).href;

// CF-RSB-R1: internal control-loop vocabulary that must never reach the banner.
const INTERNAL_VOCABULARY = /[a-z][a-z0-9]*(?:[._:-][a-z0-9]+)+|\b(?:objective|supervisor|remediation|generation|recovery|route|backoff)\b|恢复|补救|监督|目标|退避|观察|内部/i;

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
      // Try the next installed browser without downloading one.
    }
  }
  throw new Error(`No system Chrome/Edge found. Tried: ${candidates.join(", ")}`);
}

async function main() {
  await rm(artifactDir, { recursive: true, force: true });
  await mkdir(staticDir, { recursive: true });
  await build({
    root,
    base: "./",
    logLevel: "warn",
    build: {
      outDir: staticDir,
      emptyOutDir: true,
      rollupOptions: {
        input: { "status-banner-acceptance": path.join(root, "status-banner-acceptance.html") },
      },
    },
  });

  let browser;
  try {
    browser = await chromium.launch({
      executablePath: await firstBrowser(),
      headless: true,
      proxy: { server: "direct://" },
      args: [
        "--disable-gpu",
        "--no-sandbox",
        "--no-proxy-server",
        "--proxy-bypass-list=<-loopback>",
        // Chrome refuses file:// module scripts by default; this static path
        // exists exactly for hosts where loopback navigation is blocked.
        "--allow-file-access-from-files",
        "--disable-web-security",
      ],
    });
    const page = await browser.newPage({ viewport: { width: 1360, height: 940 } });
    const pageErrors = [];
    page.on("pageerror", (error) => pageErrors.push(String(error).split("\n")[0]));
    page.on("console", (message) => {
      if (message.type() === "error") pageErrors.push(`console: ${message.text().slice(0, 300)}`);
    });
    await page.goto(pageUrl, { waitUntil: "domcontentloaded" });
    try {
      await page.getByRole("main", { name: "Status banner acceptance" }).waitFor({ timeout: 15_000 });
    } catch {
      throw new Error(`the acceptance page did not render — ${pageErrors.join(" | ") || "no page error reported"}`);
    }

    const fixture = (id) => page.locator(`[data-fixture="${id}"]`);
    const banner = (id) => fixture(id).locator('[data-testid="turn-activity-progress"]');

    // CF-RSB-R3: running shows plain execution text with a real layout box.
    assert(await banner("running").isVisible(), "running fixture should show the banner");
    assert(
      (await banner("running").innerText()).includes("正在执行命令"),
      "running fixture should show plain running text",
    );
    const runningBox = await banner("running").boundingBox();
    assert(runningBox && runningBox.width > 0 && runningBox.height > 0, "banner must occupy a real layout box");

    // CF-RSB-R2: waiting shows a truthful estimate in human units.
    assert(
      (await banner("waiting").innerText()).includes("约 45 秒后重试"),
      "waiting fixture should show a truthful estimate",
    );

    // CF-RSB-R1/R2: the evidence banner — no owner, no raw label, no 0ms.
    const overdueText = await banner("overdue").innerText();
    assert(overdueText.includes("马上重试"), "overdue fixture should say 马上重试");
    for (const forbidden of ["0ms", "objective-supervisor", "下次观察", "route", "退避"]) {
      assert(!overdueText.includes(forbidden), `overdue banner must not contain "${forbidden}" (got: ${overdueText})`);
    }

    // CF-RSB-R2: unknown estimate shows no time at all.
    const unknownText = await banner("unknown").innerText();
    assert(unknownText.includes("等待"), "unknown fixture should still say it is waiting");
    assert(!unknownText.includes("后重试"), "unknown fixture must not invent a time");
    assert(!/ms/i.test(unknownText), "unknown fixture must not show a raw millisecond value");

    // CF-RSB-R4: the user is told plainly what to do.
    assert(
      (await banner("authorization").innerText()).includes("需要你先授权才能继续"),
      "authorization fixture should tell the user what to do",
    );

    // CF-RSB-R3: a finished task shows no banner at all.
    assert(
      (await banner("completed").count()) === 0,
      "completed fixture must not show a still-processing banner",
    );

    // CF-RSB-R1: no fixture leaks internal vocabulary anywhere in its text.
    for (const id of ["running", "waiting", "overdue", "unknown", "authorization", "completed"]) {
      const text = await fixture(id).innerText();
      assert(!INTERNAL_VOCABULARY.test(text), `fixture "${id}" leaks internal vocabulary: ${text}`);
    }

    // CF-RSB-R5: the "before" panel keeps the recorded evidence verbatim, so the
    // screenshots show a real before/after rather than a relabelled after.
    const beforeText = await fixture("before").innerText();
    assert(beforeText.includes("objective-supervisor:chat"), "before panel must keep the recorded internal owner");
    assert(beforeText.includes("下次观察 0ms 后"), "before panel must keep the recorded 0ms hint");

    // CF-RSB-R5: before/after screenshots in light and dark, plus a narrow window.
    const shots = [
      { name: "status-banner-light.png", theme: "light", viewport: { width: 1360, height: 940 } },
      { name: "status-banner-dark.png", theme: "dark", viewport: { width: 1360, height: 940 } },
      { name: "status-banner-light-narrow.png", theme: "light", viewport: { width: 420, height: 900 } },
      { name: "status-banner-dark-narrow.png", theme: "dark", viewport: { width: 420, height: 900 } },
    ];
    let lightSurface = null;
    for (const shot of shots) {
      await page.setViewportSize(shot.viewport);
      await page.evaluate((theme) => {
        document.documentElement.dataset.theme = theme;
      }, shot.theme);
      await page.waitForTimeout(150);
      // A theme that never applied would make light and dark screenshots
      // identical; assert the real surface colour actually changed.
      const surface = await page.evaluate(() =>
        getComputedStyle(document.documentElement).getPropertyValue("--surface-0").trim(),
      );
      if (shot.theme === "light") {
        lightSurface = surface;
      } else if (lightSurface) {
        assert(
          surface !== lightSurface,
          `light and dark themes rendered the same surface colour (${surface})`,
        );
      }
      // Re-assert in narrow layout that the banner still fits on screen.
      const narrowBox = await banner("running").boundingBox();
      assert(narrowBox, `banner must stay laid out in ${shot.name}`);
      assert(
        narrowBox.x >= 0 && narrowBox.x + narrowBox.width <= shot.viewport.width + 1,
        `banner overflows the ${shot.viewport.width}px viewport in ${shot.name}`,
      );
      await page.screenshot({ path: path.join(artifactDir, shot.name), fullPage: true });
    }

    console.log(JSON.stringify({
      status: "pass",
      artifactDir,
      url: pageUrl,
      checks: {
        runningShowsPlainText: true,
        waitingShowsTruthfulEstimate: true,
        overdueSaysImmediateNotZeroMs: true,
        unknownShowsNoTime: true,
        authorizationTellsUserWhatToDo: true,
        completedShowsNoBanner: true,
        noInternalVocabularyInAnyState: true,
      },
    }, null, 2));
  } catch (error) {
    if (browser) await browser.close();
    console.error(`status banner headless acceptance failed: ${error.stack ?? error}`);
    process.exit(1);
  }
  if (browser) await browser.close();
}

main().catch((error) => {
  console.error(`status banner headless acceptance failed: ${error.stack ?? error}`);
  process.exit(1);
});
