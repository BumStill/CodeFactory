#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate: a new session always opens on the same start page.
//
// The defect this pins: creating a new session from a conversation that had
// ended, from one with a turn still running, or from one whose history was
// still loading produced different first impressions — sometimes a completely
// blank centre column, sometimes the start page scrolled into its middle with
// the usage card cut off above the fold. jsdom computes no layout and reuses no
// DOM across the branch swap, so only a real browser can see any of it.
//
// Each path is exercised the way the user does: view a long, scrolled
// conversation first, then start a new session. The gate asserts the start page
// content is identical across all three paths, that it opens at its own top,
// and that the composer is still visible and hit-testable.

import { spawn, spawnSync } from "node:child_process";
import { access, mkdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath, pathToFileURL } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const viteCli = path.join(root, "node_modules", "vite", "bin", "vite.js");
const port = Number(process.env.CODEFACTORY_NEW_SESSION_START_PORT ?? 1457);
const baseUrl = `http://127.0.0.1:${port}/new-session-start-acceptance.html`;
/**
 * Optional pre-built directory. Hosts that route the browser's traffic through
 * an HTTP proxy can block loopback navigation entirely; pointing the gate at a
 * statically built copy (`vite build --base ./`) lets the same real-browser
 * assertions run over `file://`, which uses no HTTP proxy at all.
 */
const staticDir = process.env.CODEFACTORY_NEW_SESSION_START_STATIC_DIR ?? null;
const pageUrl = staticDir
  ? pathToFileURL(path.join(staticDir, "new-session-start-acceptance.html")).href
  : baseUrl;
const artifactDir = process.env.CODEFACTORY_NEW_SESSION_START_ARTIFACT_DIR
  ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-new-session-start-headless");

const entryPaths = [
  { key: "ended", label: "从已结束的会话新建", streaming: false },
  { key: "running", label: "从有回合在跑的会话新建", streaming: true },
  { key: "loading", label: "从历史仍在加载的会话新建", streaming: false },
];

function assert(condition, message) { if (!condition) throw new Error(message); }

async function firstBrowser() {
  const candidates = process.platform === "darwin"
    ? ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge", "/Applications/Chromium.app/Contents/MacOS/Chromium"]
    : process.platform === "win32"
      ? [path.join(process.env["PROGRAMFILES(X86)"] ?? "C:\\Program Files (x86)", "Microsoft/Edge/Application/msedge.exe"), path.join(process.env.PROGRAMFILES ?? "C:\\Program Files", "Google/Chrome/Application/chrome.exe")]
      : ["/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser"];
  for (const candidate of candidates) { try { await access(candidate); return candidate; } catch {} }
  throw new Error(`No system Chrome/Edge found. Tried: ${candidates.join(", ")}`);
}

async function waitForServer(child) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (child.exitCode != null || child.signalCode != null) throw new Error("Vite exited early");
    try { if ((await fetch(baseUrl)).ok) return; } catch {}
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
    await new Promise((resolve) => setTimeout(resolve, 700));
    if (child.exitCode == null && child.signalCode == null) {
      try { process.kill(-child.pid, "SIGKILL"); } catch { child.kill("SIGKILL"); }
    }
  }
  // Release the captured pipes so a stubborn child can never keep this gate's
  // own process alive after the assertion result is already known.
  child.stdout?.destroy();
  child.stderr?.destroy();
}

// Runs in the page. Returns everything the assertions below reason about.
function probeLayout() {
  const main = document.querySelector('main[aria-label="New session start acceptance"]');
  if (!main) return { missingMain: true };
  const shell = main.querySelector('[data-testid="workspace-composer-shell"]');
  const wrapper = main.querySelector(":scope > div.relative");
  const scroller = wrapper?.querySelector(":scope > div.absolute.inset-0") ?? null;
  const textarea = shell?.querySelector("textarea") ?? null;
  const usageCard = main.querySelector('section[role="region"][aria-label="今日用量与过去 4 周趋势"]');
  const hero = main.querySelector('section[aria-label="CodeFactory 欢迎"]');
  const shellRect = shell ? shell.getBoundingClientRect() : null;
  const usageRect = usageCard ? usageCard.getBoundingClientRect() : null;
  const textareaRect = textarea ? textarea.getBoundingClientRect() : null;
  const textareaHit = textareaRect
    ? document.elementFromPoint(
        Math.round(textareaRect.left + textareaRect.width / 2),
        Math.round(textareaRect.top + textareaRect.height / 2),
      )
    : null;
  return {
    viewportHeight: window.innerHeight,
    hasReadingColumn: !!main.querySelector('[data-testid="conversation-reading-column"]'),
    startPageRendered: !!hero && !main.querySelector('[data-testid="conversation-reading-column"]'),
    heroVisible: !!hero,
    usageCardPresent: !!usageCard,
    suggestionsPresent: (main.textContent ?? "").includes("可以试试"),
    scrollerScrollTop: scroller ? Math.round(scroller.scrollTop) : null,
    scrollerScrollHeight: scroller ? scroller.scrollHeight : null,
    usageCardTop: usageRect ? Math.round(usageRect.top) : null,
    composerBottomOffset: shellRect ? Math.round(shellRect.bottom) - window.innerHeight : null,
    textareaHittable: !!(textarea && textareaHit && (textareaHit === textarea || textarea.contains(textareaHit))),
    // Normalised to make the three entry paths comparable: the start page must
    // say exactly the same thing no matter where it was opened from.
    startPageText: (wrapper?.textContent ?? "").replace(/\s+/g, " ").trim(),
    surfaceBackground: getComputedStyle(main).backgroundColor,
  };
}

function scrollToBottom() {
  const main = document.querySelector('main[aria-label="New session start acceptance"]');
  const wrapper = main?.querySelector(":scope > div.relative");
  const scroller = wrapper?.querySelector(":scope > div.absolute.inset-0");
  if (!scroller) return { scrollTop: null, scrollHeight: null };
  scroller.scrollTop = scroller.scrollHeight;
  scroller.dispatchEvent(new Event("scroll"));
  return { scrollTop: Math.round(scroller.scrollTop), scrollHeight: scroller.scrollHeight };
}

async function runEntryPath(page, entry) {
  await page.evaluate(
    ({ streaming }) => window.__startPageAcceptance.openPreviousConversation(streaming),
    { streaming: entry.streaming },
  );
  await page.locator('[data-testid="conversation-reading-column"]').waitFor({ timeout: 10_000 });

  const scrolled = await page.evaluate(scrollToBottom);
  assert(
    scrolled.scrollTop != null && scrolled.scrollTop > 100,
    `${entry.key}: the previous conversation did not scroll (${JSON.stringify(scrolled)}), so this path proves nothing`,
  );

  // Give the browser a frame so the offset is committed before the switch.
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  await page.evaluate(({ key }) => window.__startPageAcceptance.startNewSession(key), { key: entry.key });
  await page.getByRole("region", { name: "CodeFactory 欢迎" }).waitFor({ timeout: 10_000 });

  const layout = await page.evaluate(probeLayout);
  assert(layout.startPageRendered, `${entry.key}: the start page did not render (${JSON.stringify(layout)})`);
  assert(layout.heroVisible, `${entry.key}: the start page hero is missing`);
  assert(layout.usageCardPresent, `${entry.key}: the usage card is missing`);
  assert(layout.suggestionsPresent, `${entry.key}: the suggested starting points are missing`);
  assert(
    layout.scrollerScrollTop === 0,
    `${entry.key}: the start page inherited scroll offset ${layout.scrollerScrollTop}px instead of opening at its top`,
  );
  assert(
    layout.usageCardTop != null && layout.usageCardTop >= 0,
    `${entry.key}: the usage card is scrolled above the fold (top=${layout.usageCardTop})`,
  );
  assert(
    layout.composerBottomOffset === 0,
    `${entry.key}: the composer is ${layout.composerBottomOffset}px off the viewport bottom`,
  );
  assert(layout.textareaHittable, `${entry.key}: the input is not visible and hit-testable`);
  return layout;
}

async function main() {
  // A gate that hangs is worse than a gate that fails: bound the whole run.
  const watchdog = setTimeout(() => {
    console.error("new session start headless acceptance failed: watchdog timeout after 150s");
    process.exit(1);
  }, 150_000);
  await rm(artifactDir, { recursive: true, force: true });
  await mkdir(artifactDir, { recursive: true });
  const vite = staticDir
    ? null
    : spawn(process.execPath, [viteCli, "--host", "127.0.0.1", "--port", String(port), "--strictPort"], {
        cwd: root,
        detached: process.platform !== "win32",
        stdio: ["ignore", "pipe", "pipe"],
        env: { ...process.env, BROWSER: "none" },
      });
  let viteLog = "";
  if (vite) {
    vite.stdout.on("data", (chunk) => { viteLog += chunk.toString(); });
    vite.stderr.on("data", (chunk) => { viteLog += chunk.toString(); });
  }
  console.log(JSON.stringify({ service_pid: vite?.pid ?? null, log: path.join(artifactDir, "vite.log"), url: pageUrl }));
  let browser;
  try {
    if (vite) await waitForServer(vite);
    browser = await chromium.launch({
      executablePath: await firstBrowser(),
      headless: true,
      // The host may route the browser through a system HTTP proxy; this gate
      // drives a local Vite server, so the browser must reach loopback directly.
      proxy: { server: "direct://" },
      args: [
        "--disable-gpu",
        "--no-sandbox",
        "--no-proxy-server",
        "--proxy-bypass-list=<-loopback>",
        // Chrome refuses `file://` module scripts by default; the static
        // fallback path exists exactly for hosts where loopback navigation is
        // blocked, so it relaxes that for this local acceptance page only.
        ...(staticDir ? ["--allow-file-access-from-files", "--disable-web-security"] : []),
      ],
    });
    const checks = {};
    const startPageTexts = new Map();
    for (const entry of entryPaths) {
      // A short viewport is deliberate: the start page is taller than the
      // column, so an inherited offset is visible rather than clamped away.
      const page = await browser.newPage({ viewport: { width: 900, height: 560 } });
      const pageErrors = [];
      page.on("pageerror", (error) => pageErrors.push(String(error).split("\n")[0]));
      page.on("console", (message) => {
        if (message.type() === "error") pageErrors.push(`console: ${message.text().slice(0, 300)}`);
      });
      page.on("requestfailed", (request) => {
        pageErrors.push(`requestfailed: ${request.url().slice(0, 160)} (${request.failure()?.errorText ?? "unknown"})`);
      });
      await page.goto(pageUrl, { waitUntil: "domcontentloaded" });
      try {
        await page.getByRole("main", { name: "New session start acceptance" }).waitFor({ timeout: 10_000 });
      } catch {
        throw new Error(
          `${entry.key}: the acceptance page did not render — ${pageErrors.join(" | ") || "no page error reported"}`,
        );
      }
      await page.getByRole("textbox", { name: "消息输入" }).waitFor({ timeout: 10_000 });

      const layout = await runEntryPath(page, entry);
      startPageTexts.set(entry.key, layout.startPageText);
      checks[entry.key] = layout;

      await page.screenshot({ path: path.join(artifactDir, `start-page-${entry.key}-dark.png`) });
      await page.evaluate(() => document.documentElement.setAttribute("data-theme", "light"));
      await page.waitForFunction(
        () => getComputedStyle(document.querySelector("main")).backgroundColor !== "rgb(30, 30, 30)",
        null,
        { timeout: 5_000 },
      ).catch(() => {});
      const light = await page.evaluate(probeLayout);
      assert(
        light.surfaceBackground !== layout.surfaceBackground,
        `${entry.key}: the light theme did not apply (${layout.surfaceBackground} → ${light.surfaceBackground})`,
      );
      await page.screenshot({ path: path.join(artifactDir, `start-page-${entry.key}-light.png`) });
      await page.evaluate(() => document.documentElement.setAttribute("data-theme", "dark"));
      await page.close();
    }

    const [firstKey, ...otherKeys] = [...startPageTexts.keys()];
    const firstText = startPageTexts.get(firstKey);
    assert(firstText && firstText.length > 0, "the start page rendered no text at all");
    for (const key of otherKeys) {
      assert(
        startPageTexts.get(key) === firstText,
        `start page differs between entry paths: ${firstKey} vs ${key}`,
      );
    }
    console.log(JSON.stringify({ status: "pass", artifactDir, checks }, null, 2));
  } finally {
    clearTimeout(watchdog);
    if (browser) await browser.close();
    if (vite) {
      await stopServer(vite);
      await writeFile(path.join(artifactDir, "vite.log"), viteLog);
      if (viteLog.trim()) console.error(viteLog.trim().split("\n").slice(-20).join("\n"));
    }
  }
}

main().catch((error) => { console.error(`new session start headless acceptance failed: ${error.stack ?? error}`); process.exit(1); });
