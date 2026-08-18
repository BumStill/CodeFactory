#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate for turn handback: once the objective settles into
// waiting_core_input the progress banner must be gone from the layout, and no
// internal reason code may reach the screen. A turn that is genuinely running
// must keep its banner, so the fix cannot pass by suppressing everything.

import { spawn, spawnSync } from "node:child_process";
import { access, mkdir, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const viteCli = path.join(root, "node_modules", "vite", "bin", "vite.js");
const port = Number(process.env.CODEFACTORY_TURN_HANDBACK_PORT ?? 1452);
const baseUrl = `http://127.0.0.1:${port}/turn-handback-acceptance.html`;
const artifactDir = process.env.CODEFACTORY_TURN_HANDBACK_ARTIFACT_DIR
  ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-turn-handback-headless");

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
    try { process.kill(-child.pid, "SIGTERM"); } catch { child.kill("SIGTERM"); }
  }
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
  try {
    await waitForServer(vite);
    const executablePath = await firstBrowser();
    browser = await chromium.launch({ executablePath, headless: true, args: ["--disable-gpu", "--no-sandbox"] });
    const page = await browser.newPage({ viewport: { width: 1280, height: 720 } });
    await page.goto(baseUrl, { waitUntil: "networkidle" });
    await page.getByRole("main", { name: "Turn handback acceptance" }).waitFor({ timeout: 10_000 });

    const handedBack = page.getByRole("region", { name: "Handed back to the user" });
    const running = page.getByRole("region", { name: "Still running" });
    await handedBack.waitFor();
    await running.waitFor();

    assert(
      (await handedBack.getByTestId("turn-progress").count()) === 0
        && (await handedBack.getByTestId("turn-activity-progress").count()) === 0,
      "a turn handed back to the user must not keep a progress banner on screen",
    );
    assert(
      (await handedBack.getByText(/下一步 · /).count()) === 0,
      "a handed-back turn must not advertise a next step",
    );
    assert(
      (await handedBack.getByText(/预计还需/).count()) === 0,
      "a handed-back turn must not quote a remaining time",
    );
    assert(
      !(await handedBack.textContent() ?? "").includes("technical_recovery_exhausted"),
      "the internal reason code must never reach the screen",
    );
    assert(
      (await handedBack.getByRole("button", { name: "发送" }).count()) === 1
        && (await handedBack.getByRole("button", { name: "停止后续生成" }).count()) === 0,
      "handback must restore the real composer to Send instead of Stop",
    );
    assert(
      (await handedBack.getByLabel("已阻断").count()) === 1
        && !(await handedBack.textContent() ?? "").includes("等待远端"),
      "the waiting tool must remain visible as blocked without a live clock",
    );

    const runningBanner = running.getByTestId("turn-progress");
    await runningBanner.waitFor({ timeout: 10_000 });
    assert(
      await runningBanner.isVisible(),
      "a turn that is genuinely running must keep its progress banner",
    );
    assert(
      await runningBanner.getByText(/下一步 · /).isVisible(),
      "a running turn must keep showing its next step",
    );
    assert(
      (await runningBanner.textContent() ?? "").includes("命令已连续运行约 3 分钟"),
      "a running turn must keep its human waiting reason",
    );
    assert(
      await running.getByRole("button", { name: "停止后续生成" }).isVisible(),
      "the running control panel must keep the real Stop control",
    );
    const inlineStatus = running.getByTestId("inline-turn-status");
    assert(
      (await inlineStatus.count()) === 1
        && /^(Thinking|执行中|等待中|整理结果) · \d{2}:\d{2}$/.test((await inlineStatus.textContent() ?? "").trim()),
      "a running turn must render exactly one compact icon/status/clock receipt",
    );
    assert(
      (await handedBack.getByTestId("inline-turn-status").count()) === 0,
      "the inline running receipt must disappear after handback",
    );
    assert(
      !(await running.textContent() ?? "").includes("正在处理")
        && !(await running.textContent() ?? "").includes("运行中 ·"),
      "the body must not keep either legacy running indicator",
    );

    await page.screenshot({ path: path.join(artifactDir, "turn-handback-wide.png"), fullPage: true });
    await page.setViewportSize({ width: 430, height: 780 });
    await inlineStatus.scrollIntoViewIfNeeded();
    assert(
      await inlineStatus.isVisible(),
      "the compact running receipt must remain visible in a narrow viewport",
    );
    await page.screenshot({ path: path.join(artifactDir, "turn-handback-narrow.png"), fullPage: true });
    console.log(JSON.stringify({
      status: "pass",
      artifactDir,
      checks: {
        handedBackTurnRetiresTheBanner: true,
        handedBackTurnQuotesNoRemainingTime: true,
        internalReasonCodeNeverRendered: true,
        composerReturnsToSend: true,
        waitingToolBecomesBlocked: true,
        runningTurnKeepsItsBanner: true,
        runningTurnKeepsOneInlineReceipt: true,
        inlineReceiptFitsNarrowViewport: true,
      },
    }, null, 2));
  } finally {
    if (browser) await browser.close();
    await stopServer(vite);
    if (viteLog.trim()) {
      console.error(viteLog.trim().split("\n").slice(-20).join("\n"));
    }
  }
}

main().catch((error) => {
  console.error(`turn handback headless acceptance failed: ${error.stack ?? error}`);
  process.exit(1);
});
