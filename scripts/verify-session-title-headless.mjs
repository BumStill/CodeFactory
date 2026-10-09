#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate for the sidebar session-title states (M40: CF-TTL-R1..R4).
//
// Asserts, in a real Chromium against the real `SessionSidebar`, that every
// title state renders inside the rail (no horizontal overflow) and that the
// over-long titles truncate with an ellipsis, in BOTH themes. Captures light and
// dark screenshots as evidence.
//
// Hosts that route the browser through an HTTP proxy can block loopback
// navigation entirely, so this gate builds the single acceptance page with the
// Vite API and drives it over `file://`, which uses no HTTP proxy at all.

import { spawnSync } from "node:child_process";
import { access, mkdir, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const artifactDir =
  process.env.CODEFACTORY_SESSION_TITLE_ARTIFACT_DIR ??
  path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-session-title-headless");
const staticDir = path.join(artifactDir, "static");
const pageUrl = pathToFileURL(path.join(staticDir, "session-title-acceptance.html")).href;

const TITLES = [
  "会话命名优化",
  "登录问题排查",
  "手工名称",
  "重构支付网关的超时重试与幂等键生成逻辑并补齐并发回归测试与可观测性埋点",
  "Investigate intermittent sidebar title regression across parallel session creation and provider timeouts",
  "新会话",
];
const TRUNCATING = new Set([TITLES[3], TITLES[4]]);

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
            path.join(
              process.env["PROGRAMFILES(X86)"] ?? "C:\\Program Files (x86)",
              "Microsoft/Edge/Application/msedge.exe",
            ),
            path.join(
              process.env.PROGRAMFILES ?? "C:\\Program Files",
              "Google/Chrome/Application/chrome.exe",
            ),
          ]
        : ["/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser"];
  for (const candidate of candidates) {
    try {
      await access(candidate);
      return candidate;
    } catch {}
  }
  throw new Error(`No system Chrome/Edge found. Tried: ${candidates.join(", ")}`);
}

/** Measure the rail once in the current theme. */
async function measure(page) {
  return page.evaluate(() => {
    const main = document.querySelector('[aria-label="Session title acceptance"]');
    const list = main.querySelector("ul");
    const scroller = list?.parentElement ?? null;
    const spans = [...main.querySelectorAll('span[title$="· 双击重命名"]')];
    const mainRect = main.getBoundingClientRect();
    return {
      mainOverflow: main.scrollWidth - main.clientWidth,
      scrollerOverflow: scroller ? scroller.scrollWidth - scroller.clientWidth : null,
      rows: spans.map((span) => {
        const style = getComputedStyle(span);
        const row = span.closest("button") ?? span.parentElement;
        return {
          text: span.textContent,
          overflowX: style.overflowX,
          textOverflow: style.textOverflow,
          whiteSpace: style.whiteSpace,
          scrollWidth: span.scrollWidth,
          clientWidth: span.clientWidth,
          rowRight: row.getBoundingClientRect().right,
          mainRight: mainRect.right,
        };
      }),
    };
  });
}

function assertMeasurements(m, theme) {
  assert(m.mainOverflow <= 1, `[${theme}] sidebar overflows horizontally by ${m.mainOverflow}px`);
  assert(
    m.scrollerOverflow === null || m.scrollerOverflow <= 1,
    `[${theme}] session list overflows horizontally by ${m.scrollerOverflow}px`,
  );
  assert(
    m.rows.length === TITLES.length,
    `[${theme}] expected ${TITLES.length} title rows, saw ${m.rows.length}`,
  );
  for (const title of TITLES) {
    assert(
      m.rows.some((row) => row.text === title),
      `[${theme}] missing title row: ${title}`,
    );
  }
  for (const row of m.rows) {
    assert(row.overflowX === "hidden", `[${theme}] "${row.text}" overflow-x is ${row.overflowX}`);
    assert(
      row.textOverflow === "ellipsis",
      `[${theme}] "${row.text}" text-overflow is ${row.textOverflow}`,
    );
    assert(row.whiteSpace === "nowrap", `[${theme}] "${row.text}" white-space is ${row.whiteSpace}`);
    assert(
      row.rowRight <= row.mainRight + 1,
      `[${theme}] "${row.text}" row overflows the rail (${row.rowRight} > ${row.mainRight})`,
    );
    if (TRUNCATING.has(row.text)) {
      assert(
        row.scrollWidth > row.clientWidth,
        `[${theme}] long title "${row.text}" is not truncating (scroll ${row.scrollWidth} <= client ${row.clientWidth})`,
      );
    } else {
      assert(
        row.scrollWidth <= row.clientWidth + 1,
        `[${theme}] short title "${row.text}" is clipped (scroll ${row.scrollWidth} > client ${row.clientWidth})`,
      );
    }
  }
}

async function main() {
  await rm(artifactDir, { recursive: true, force: true });
  await mkdir(artifactDir, { recursive: true });

  const { build } = await import("vite");
  await build({
    root,
    base: "./",
    logLevel: "warn",
    build: {
      outDir: staticDir,
      emptyOutDir: true,
      rollupOptions: { input: path.join(root, "session-title-acceptance.html") },
    },
  });

  let browser;
  try {
    browser = await chromium.launch({
      executablePath: await firstBrowser(),
      headless: true,
      // Drive the local static page directly: no HTTP proxy, and Chrome refuses
      // `file://` module scripts unless it is told to allow them.
      args: [
        "--disable-gpu",
        "--no-sandbox",
        "--no-proxy-server",
        "--allow-file-access-from-files",
        "--disable-web-security",
      ],
    });
    const page = await browser.newPage({ viewport: { width: 280, height: 640 } });
    const pageErrors = [];
    page.on("pageerror", (error) => pageErrors.push(String(error).split("\n")[0]));
    page.on("console", (message) => {
      if (message.type() === "error") pageErrors.push(`console: ${message.text().slice(0, 200)}`);
    });
    await page.goto(pageUrl, { waitUntil: "domcontentloaded", timeout: 20_000 });
    try {
      await page.getByLabel("Session title acceptance").waitFor({ timeout: 10_000 });
      await page.locator('span[title$="· 双击重命名"]').first().waitFor({ timeout: 10_000 });
    } catch {
      throw new Error(
        `the acceptance page did not render — ${pageErrors.join(" | ") || "no page error reported"}`,
      );
    }

    const screenshots = {};
    const evidence = {};
    for (const theme of ["dark", "light"]) {
      await page.evaluate((value) => {
        document.documentElement.setAttribute("data-theme", value);
      }, theme);
      await page.waitForTimeout(120);
      const measurements = await measure(page);
      assertMeasurements(measurements, theme);
      evidence[theme] = measurements;
      const shot = path.join(artifactDir, `session-title-${theme}.png`);
      await page.screenshot({ path: shot });
      const size = (await stat(shot)).size;
      assert(size > 3_000, `[${theme}] screenshot looks empty (${size} bytes)`);
      screenshots[theme] = { path: shot, bytes: size };
    }

    const report = { status: "pass", url: pageUrl, artifactDir, screenshots, checks: evidence };
    await writeFile(path.join(artifactDir, "evidence.json"), JSON.stringify(report, null, 2));
    console.log(JSON.stringify(report, null, 2));
  } finally {
    if (browser) await browser.close();
  }
}

main().catch((error) => {
  console.error(`session title headless acceptance failed: ${error.stack ?? error}`);
  process.exit(1);
});
