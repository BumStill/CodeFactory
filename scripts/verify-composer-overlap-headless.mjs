#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate: the new-session composer must stay reachable in a short window.
//
// The regression this pins: MessageList's empty-state branch put WelcomeScreen
// straight into a `relative flex-1 min-h-0` wrapper as a plain block child. A
// block child sizes to its own content, so in a short window the welcome body
// grew past the wrapper and — the wrapper being positioned while the composer is
// not — painted on top of the composer, burying the input. jsdom computes no
// layout and cannot see any of this.

import { spawn, spawnSync } from "node:child_process";
import { access, mkdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const viteCli = path.join(root, "node_modules", "vite", "bin", "vite.js");
const port = Number(process.env.CODEFACTORY_COMPOSER_OVERLAP_PORT ?? 1456);
const baseUrl = `http://127.0.0.1:${port}/composer-overlap-acceptance.html`;
const artifactDir = process.env.CODEFACTORY_COMPOSER_OVERLAP_ARTIFACT_DIR
  ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-composer-overlap-headless");

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
  if (process.platform === "win32") spawnSync("taskkill", ["/pid", String(child.pid), "/t", "/f"], { stdio: "ignore" });
  else { try { process.kill(-child.pid, "SIGTERM"); } catch { child.kill("SIGTERM"); } }
}

// Runs in the page. Returns the geometry the assertions below reason about.
function probeLayout() {
  const main = document.querySelector('main[aria-label="Composer overlap acceptance"]');
  const shell = document.querySelector('[data-testid="workspace-composer-shell"]');
  const wrapper = main.querySelector(":scope > div.relative");
  const textarea = shell.querySelector("textarea");
  const shellRect = shell.getBoundingClientRect();
  const wrapperRect = wrapper.getBoundingClientRect();
  // The welcome body is whichever descendant of the wrapper actually scrolls.
  const scroller = Array.from(wrapper.querySelectorAll("*"))
    .find((el) => {
      const overflowY = getComputedStyle(el).overflowY;
      return (overflowY === "auto" || overflowY === "scroll") && el.scrollHeight > el.clientHeight;
    }) ?? null;
  // Sample inside the composer's own rectangle only, never the sidebar gutter.
  const xs = [shellRect.left + 40, (shellRect.left + shellRect.right) / 2, shellRect.right - 40];
  const ys = [shellRect.top + 5, shellRect.top + 25];
  const probes = [];
  for (const x of xs) {
    for (const y of ys) {
      const hit = document.elementFromPoint(Math.round(x), Math.round(y));
      probes.push({
        x: Math.round(x), y: Math.round(y),
        insideComposer: !!(hit && shell.contains(hit)),
        topmost: hit ? (hit.className || "").toString().slice(0, 60) : null,
      });
    }
  }
  const textareaRect = textarea.getBoundingClientRect();
  const textareaHit = document.elementFromPoint(
    Math.round(textareaRect.left + textareaRect.width / 2),
    Math.round(textareaRect.top + textareaRect.height / 2),
  );
  // Content-taller-than-column, measured the same way whether the content is
  // contained (the scroller overflows) or not (the wrapper overflows). Keeping
  // this fix-independent is what stops a broken build from tripping the
  // vacuity guard instead of the assertion that names the defect.
  const wrapperOverflowPx = wrapper.scrollHeight - wrapper.clientHeight;
  const scrollerOverflowPx = scroller ? scroller.scrollHeight - scroller.clientHeight : 0;
  return {
    viewportHeight: window.innerHeight,
    wrapperOverflowPx,
    welcomeContentOverflowsColumn: Math.max(wrapperOverflowPx, scrollerOverflowPx),
    composerBottomOffset: Math.round(shellRect.bottom) - window.innerHeight,
    textareaHittable: !!(textareaHit && (textareaHit === textarea || textarea.contains(textareaHit))),
    probes,
  };
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
  console.log(JSON.stringify({ service_pid: vite.pid, log: path.join(artifactDir, "vite.log"), url: baseUrl }));
  let browser;
  try {
    await waitForServer(vite);
    browser = await chromium.launch({ executablePath: await firstBrowser(), headless: true, args: ["--disable-gpu", "--no-sandbox"] });
    const checks = {};
    // Both viewports are deliberately short: the welcome content must not fit,
    // so the gate exercises the failure condition instead of passing vacuously.
    // Width matters too — a wide window lays the prompt cards out in two
    // columns, which shortens the content, so the wide case is made shorter to
    // compensate. The vacuity assertion below enforces both.
    for (const viewport of [{ width: 900, height: 420 }, { width: 1280, height: 380 }]) {
      const page = await browser.newPage({ viewport });
      await page.goto(baseUrl, { waitUntil: "networkidle" });
      await page.getByRole("main", { name: "Composer overlap acceptance" }).waitFor({ timeout: 10_000 });
      await page.getByRole("textbox", { name: "消息输入" }).waitFor({ timeout: 10_000 });
      const layout = await page.evaluate(probeLayout);
      const label = `${viewport.width}x${viewport.height}`;
      await page.screenshot({ path: path.join(artifactDir, `composer-${label}.png`), fullPage: false });

      assert(
        layout.welcomeContentOverflowsColumn > 0,
        `${label}: welcome content fits the column, so this gate proves nothing — shorten the viewport (${JSON.stringify(layout)})`,
      );
      assert(
        layout.wrapperOverflowPx <= 1,
        `${label}: welcome body overflows its wrapper by ${layout.wrapperOverflowPx}px and will paint over the composer`,
      );
      assert(
        layout.composerBottomOffset === 0,
        `${label}: composer is ${layout.composerBottomOffset}px off the viewport bottom`,
      );
      assert(
        layout.probes.every((probe) => probe.insideComposer),
        `${label}: composer is covered — ${JSON.stringify(layout.probes.filter((p) => !p.insideComposer))}`,
      );
      assert(layout.textareaHittable, `${label}: the input itself is not hit-testable`);
      checks[label] = layout;
      await page.close();
    }
    console.log(JSON.stringify({ status: "pass", artifactDir, checks }, null, 2));
  } finally {
    if (browser) await browser.close();
    await stopServer(vite);
    await writeFile(path.join(artifactDir, "vite.log"), viteLog);
    if (viteLog.trim()) console.error(viteLog.trim().split("\n").slice(-20).join("\n"));
  }
}

main().catch((error) => { console.error(`composer overlap headless acceptance failed: ${error.stack ?? error}`); process.exit(1); });
