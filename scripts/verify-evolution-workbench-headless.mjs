#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0

import { spawn, spawnSync } from "node:child_process";
import { access, mkdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const viteCli = path.join(root, "node_modules", "vite", "bin", "vite.js");
const port = Number(process.env.CODEFACTORY_EVOLUTION_HEADLESS_PORT ?? 1437);
const baseUrl = `http://127.0.0.1:${port}/evolution-acceptance.html`;
const artifactDir = process.env.CODEFACTORY_EVOLUTION_ARTIFACT_DIR
  ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-evolution-headless");
let activeBrowser;
let activeServer;
let signalShutdownStarted = false;

function browserCandidates() {
  if (process.env.CODEFACTORY_HEADLESS_BROWSER) return [process.env.CODEFACTORY_HEADLESS_BROWSER];
  if (process.platform === "darwin") {
    return [
      "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
      "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
      "/Applications/Chromium.app/Contents/MacOS/Chromium",
    ];
  }
  if (process.platform === "win32") {
    return [
      path.join(process.env["PROGRAMFILES(X86)"] ?? "C:\\Program Files (x86)", "Microsoft/Edge/Application/msedge.exe"),
      path.join(process.env.PROGRAMFILES ?? "C:\\Program Files", "Google/Chrome/Application/chrome.exe"),
    ];
  }
  return ["/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser"];
}

async function firstExecutable() {
  for (const candidate of browserCandidates()) {
    try {
      await access(candidate);
      return candidate;
    } catch {
      // Try the next system browser. The harness never downloads or runs an
      // unreviewed browser binary as a side effect of verification.
    }
  }
  throw new Error(`No system Chrome/Edge found. Tried: ${browserCandidates().join(", ")}`);
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function assertWithinViewport(locator, viewport, message) {
  const box = await locator.boundingBox();
  assert(
    box != null
      && box.x >= 0
      && box.y >= 0
      && box.x + box.width <= viewport.width
      && box.y + box.height <= viewport.height,
    `${message}: ${JSON.stringify(box)}`,
  );
}

async function waitForFocusedText(page, text, message) {
  try {
    await page.waitForFunction(
      (expected) => document.activeElement instanceof HTMLElement
        && document.activeElement !== document.body
        && document.activeElement.textContent?.includes(expected),
      text,
      { timeout: 5_000 },
    );
  } catch {
    throw new Error(`${message}; active=${await page.evaluate(() => document.activeElement?.textContent ?? "<none>")}`);
  }
}

async function waitForServer(child) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    if (child.spawnError) throw child.spawnError;
    if (child.exitCode != null || child.signalCode != null) {
      throw new Error(`Vite exited early with code=${child.exitCode} signal=${child.signalCode}`);
    }
    try {
      const response = await fetch(baseUrl);
      if (response.ok) return;
    } catch {
      // Startup race; retry within the bounded deadline.
    }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error(`Timed out waiting for ${baseUrl}`);
}

function waitForExit(child, timeoutMs) {
  if (child.exitCode != null || child.signalCode != null) return Promise.resolve(true);
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      child.off("exit", onExit);
      resolve(false);
    }, timeoutMs);
    const onExit = () => {
      clearTimeout(timer);
      resolve(true);
    };
    child.once("exit", onExit);
  });
}

async function stopServer(child) {
  if (!child || child.exitCode != null || child.signalCode != null) return;
  if (process.platform === "win32") {
    const result = spawnSync("taskkill", ["/pid", String(child.pid), "/t", "/f"], { stdio: "ignore" });
    if (result.status !== 0 && child.exitCode == null && child.signalCode == null) child.kill();
  } else {
    try {
      process.kill(-child.pid, "SIGTERM");
    } catch {
      child.kill("SIGTERM");
    }
  }
  if (await waitForExit(child, 5_000)) return;
  if (process.platform !== "win32") {
    try {
      process.kill(-child.pid, "SIGKILL");
    } catch {
      child.kill("SIGKILL");
    }
  }
  await waitForExit(child, 2_000);
}

async function shutdownFromSignal(signal) {
  if (signalShutdownStarted) return;
  signalShutdownStarted = true;
  await activeBrowser?.close().catch(() => {});
  await stopServer(activeServer);
  process.exitCode = signal === "SIGINT" ? 130 : 143;
}

process.once("SIGINT", () => { void shutdownFromSignal("SIGINT"); });
process.once("SIGTERM", () => { void shutdownFromSignal("SIGTERM"); });

async function run() {
  await rm(artifactDir, { recursive: true, force: true });
  await mkdir(artifactDir, { recursive: true });
  const vite = spawn(process.execPath, [viteCli, "--host", "127.0.0.1", "--port", String(port), "--strictPort"], {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, NO_COLOR: "1" },
    detached: process.platform !== "win32",
  });
  activeServer = vite;
  vite.spawnError = null;
  vite.on("error", (error) => { vite.spawnError = error; });
  let viteOutput = "";
  vite.stdout.on("data", (chunk) => { viteOutput += chunk.toString(); });
  vite.stderr.on("data", (chunk) => { viteOutput += chunk.toString(); });

  let browser;
  let page;
  const browserMessages = [];
  try {
    await waitForServer(vite);
    const executablePath = await firstExecutable();
    browser = await chromium.launch({
      executablePath,
      headless: true,
      args: ["--disable-gpu", "--no-sandbox"],
    });
    activeBrowser = browser;

    page = await browser.newPage({ viewport: { width: 1366, height: 768 } });
    page.on("console", (message) => browserMessages.push(`console:${message.type()}: ${message.text()}`));
    page.on("pageerror", (error) => browserMessages.push(`pageerror: ${error.stack ?? error}`));
    await page.goto(baseUrl, { waitUntil: "networkidle" });
    await page.getByRole("heading", { name: "进化审查" }).waitFor();
    assert(await page.getByText("全局偏好 response_language = zh-CN（当前项目未覆盖）").isVisible(), "global preference fallback is not visible");
    assert(await page.getByRole("button", { name: /多次人工决定/ }).isVisible(), "wide preference candidate missing");
    assert(await page.getByRole("button", { name: /工具读取文件/ }).isVisible(), "wide memory candidate missing");
    await page.screenshot({ path: path.join(artifactDir, "wide-review.png"), fullPage: true });

    await page.getByRole("button", { name: "拒绝" }).click();
    await page.getByText("确认拒绝这个候选").waitFor();
    await page.keyboard.press("Escape");
    await waitForFocusedText(page, "拒绝", "cancel did not restore reject focus");

    await page.getByRole("button", { name: "拒绝" }).click();
    await page.getByRole("button", { name: "确认拒绝" }).click();
    const memoryQueueButton = page.getByRole("button", { name: /工具读取文件/ });
    await memoryQueueButton.waitFor();
    await waitForFocusedText(page, "工具读取文件", "next candidate was not focused after reject");

    await page.getByRole("button", { name: "批准并运行 Evals" }).click();
    const wideAuto = page.getByRole("checkbox", { name: /通过后自动激活/ });
    assert(!await wideAuto.isChecked(), "auto activation must default off");
    await wideAuto.check();
    await page.getByRole("button", { name: "确认批准并运行 Evals" }).click();
    const historyAction = page.getByRole("button", { name: "查看决定历史" });
    await historyAction.waitFor();
    await waitForFocusedText(page, "查看决定历史", "history action was not focused after final decision");
    await historyAction.click();
    assert(await page.getByText("已拒绝", { exact: false }).first().isVisible(), "rejected decision missing from history");
    await page.screenshot({ path: path.join(artifactDir, "wide-history.png"), fullPage: true });

    await page.getByRole("tab", { name: /评测与激活 1/ }).click();
    assert(await page.getByText("7/7 required cases 通过").isVisible(), "Eval case summary missing");
    assert(await page.getByText("回滚准备度").isVisible(), "required rollback Eval case missing");
    assert(await page.getByText(/receipt headless-activation/).isVisible(), "activation receipt missing");
    await page.screenshot({ path: path.join(artifactDir, "wide-eval-active.png"), fullPage: true });
    await page.getByRole("button", { name: "回滚" }).click();
    await page.getByRole("button", { name: "确认回滚" }).click();
    await page.getByText(/已按 exact receipt 回滚/).waitFor();
    await page.screenshot({ path: path.join(artifactDir, "wide-rollback.png"), fullPage: true });

    await page.getByRole("tab", { name: "作业与日志" }).click();
    await page.getByRole("button", { name: /人工批准与评测 已完成/ }).click();
    await page.getByText("headless-approve").waitFor();
    assert(await page.getByText("激活安全 Evals 全部通过").isVisible(), "Eval job log missing");
    assert(await page.getByText("Eval 通过后已激活，下一次 Agent 调用生效").isVisible(), "activation job log missing");
    await page.screenshot({ path: path.join(artifactDir, "wide-eval-activation-log.png"), fullPage: true });
    await page.getByRole("button", { name: /跨会话分析 已完成/ }).click();
    for (const stage of ["分析范围已确定", "轨迹读取完成", "隐私处理完成", "候选提取完成", "候选去重完成", "分析完成"]) {
      assert(await page.getByText(stage).first().isVisible(), `analysis stage missing: ${stage}`);
    }
    await page.screenshot({ path: path.join(artifactDir, "wide-job-log.png"), fullPage: true });

    await page.setViewportSize({ width: 390, height: 812 });
    await page.reload({ waitUntil: "networkidle" });
    await page.getByRole("heading", { name: "进化审查" }).waitFor();
    const narrowCandidate = page.getByRole("button", { name: /多次人工决定/ });
    assert(await narrowCandidate.isVisible(), "narrow candidate list missing");
    assert(!await page.getByRole("button", { name: "批准并运行 Evals" }).isVisible(), "narrow detail should be hidden before selection");
    await narrowCandidate.focus();
    await page.keyboard.press("Enter");
    const backButton = page.getByRole("button", { name: "返回候选队列" });
    await backButton.waitFor();
    const narrowAccept = page.getByRole("button", { name: "批准并运行 Evals" });
    const narrowReject = page.getByRole("button", { name: "拒绝" });
    assert(await narrowAccept.isVisible(), "narrow detail action missing");
    await assertWithinViewport(narrowAccept, { width: 390, height: 812 }, "narrow accept action is outside the viewport");
    await assertWithinViewport(narrowReject, { width: 390, height: 812 }, "narrow reject action is outside the viewport");
    await page.screenshot({ path: path.join(artifactDir, "narrow-detail.png"), fullPage: true });
    await backButton.focus();
    await page.keyboard.press("Enter");
    assert(await narrowCandidate.isVisible(), "narrow list did not return");
    await waitForFocusedText(page, "多次人工决定", "narrow back did not restore candidate focus");
    await page.screenshot({ path: path.join(artifactDir, "narrow-list.png"), fullPage: true });

    await page.keyboard.press("Enter");
    await narrowAccept.click({ trial: true });
    await narrowAccept.focus();
    await page.keyboard.press("Enter");
    const narrowConfirmAccept = page.getByRole("button", { name: "确认批准并运行 Evals" });
    await narrowConfirmAccept.waitFor();
    await assertWithinViewport(narrowConfirmAccept, { width: 390, height: 812 }, "narrow accept confirmation is outside the viewport");
    await page.keyboard.press("Escape");
    await waitForFocusedText(page, "批准并运行 Evals", "narrow cancel did not restore accept focus");

    await narrowReject.focus();
    await page.keyboard.press("Enter");
    const narrowConfirmReject = page.getByRole("button", { name: "确认拒绝" });
    await narrowConfirmReject.waitFor();
    await assertWithinViewport(narrowConfirmReject, { width: 390, height: 812 }, "narrow reject confirmation is outside the viewport");
    await waitForFocusedText(page, "确认拒绝", "narrow reject confirmation did not receive focus");
    await page.keyboard.press("Enter");

    const narrowMemoryCandidate = page.getByRole("button", { name: /工具读取文件/ });
    await narrowMemoryCandidate.waitFor();
    await waitForFocusedText(page, "工具读取文件", "narrow reject did not focus the next candidate");
    await page.keyboard.press("Enter");
    const narrowMemoryAccept = page.getByRole("button", { name: "批准并运行 Evals" });
    await narrowMemoryAccept.click({ trial: true });
    await assertWithinViewport(narrowMemoryAccept, { width: 390, height: 812 }, "narrow memory accept action is outside the viewport");
    await narrowMemoryAccept.focus();
    await page.keyboard.press("Enter");
    const narrowAuto = page.getByRole("checkbox", { name: /通过后自动激活/ });
    assert(!await narrowAuto.isChecked(), "narrow auto activation must default off");
    await narrowAuto.focus();
    await page.keyboard.press("Space");
    const narrowConfirmMemory = page.getByRole("button", { name: "确认批准并运行 Evals" });
    await narrowConfirmMemory.waitFor();
    await assertWithinViewport(narrowConfirmMemory, { width: 390, height: 812 }, "narrow memory confirmation is outside the viewport");
    await narrowConfirmMemory.focus();
    await waitForFocusedText(page, "确认批准并运行 Evals", "narrow memory confirmation did not receive focus");
    await page.keyboard.press("Enter");

    const narrowHistoryAction = page.getByRole("button", { name: "查看决定历史" });
    await narrowHistoryAction.waitFor();
    await waitForFocusedText(page, "查看决定历史", "narrow final decision did not focus history");
    await page.keyboard.press("Enter");
    assert(await page.getByText("已拒绝", { exact: false }).first().isVisible(), "narrow rejected decision missing from history");
    await page.screenshot({ path: path.join(artifactDir, "narrow-history.png"), fullPage: true });

    const narrowEvalTab = page.getByRole("tab", { name: /评测与激活 1/ });
    await narrowEvalTab.focus();
    await page.keyboard.press("Enter");
    assert(await page.getByText("7/7 required cases 通过").isVisible(), "narrow Eval summary missing");
    const narrowRollback = page.getByRole("button", { name: "回滚" });
    await assertWithinViewport(narrowRollback, { width: 390, height: 812 }, "narrow rollback is outside the viewport");
    await narrowRollback.focus();
    await page.keyboard.press("Enter");
    const narrowConfirmRollback = page.getByRole("button", { name: "确认回滚" });
    await assertWithinViewport(narrowConfirmRollback, { width: 390, height: 812 }, "narrow rollback confirmation is outside the viewport");
    await page.keyboard.press("Enter");
    await page.getByText(/已按 exact receipt 回滚/).waitFor();
    await page.screenshot({ path: path.join(artifactDir, "narrow-eval-rollback.png"), fullPage: true });

    const narrowJobsTab = page.getByRole("tab", { name: "作业与日志" });
    await narrowJobsTab.focus();
    await page.keyboard.press("Enter");
    const narrowEvalJob = page.getByRole("button", { name: /人工批准与评测 已完成/ });
    await narrowEvalJob.focus();
    await page.keyboard.press("Enter");
    assert(await page.getByText("激活安全 Evals 全部通过").isVisible(), "narrow Eval job log missing");
    const narrowAnalysisJob = page.getByRole("button", { name: /跨会话分析 已完成/ });
    await narrowAnalysisJob.focus();
    await page.keyboard.press("Enter");
    for (const stage of ["分析范围已确定", "轨迹读取完成", "隐私处理完成", "候选提取完成", "候选去重完成", "分析完成"]) {
      assert(await page.getByText(stage).first().isVisible(), `narrow analysis stage missing: ${stage}`);
    }
    await page.screenshot({ path: path.join(artifactDir, "narrow-job-log.png"), fullPage: true });
    const overflow = await page.evaluate(() => ({ width: window.innerWidth, scrollWidth: document.documentElement.scrollWidth }));
    assert(overflow.scrollWidth <= overflow.width, `horizontal overflow: ${JSON.stringify(overflow)}`);
    const pageErrors = browserMessages.filter((message) => message.startsWith("pageerror:"));
    assert(pageErrors.length === 0, `browser page errors: ${pageErrors.join(" | ")}`);

    const receipt = {
      status: "pass",
      browser: executablePath,
      surfaces: [
        "wide-review-eval-activation-rollback-log",
        "narrow-list-detail-back",
        "narrow-keyboard-eval-activation-rollback-log",
      ],
      viewports: ["1366x768", "390x812"],
      candidate_revision: "headless-memory:1",
      eval_run_id: "headless-eval-run",
      activation_receipt_id: "headless-activation",
      rollback_status: "rolled_back",
      artifact_dir: artifactDir,
      interactive_desktop_required: false,
      os_lock_state_observed: "not_measured",
    };
    await writeFile(path.join(artifactDir, "receipt.json"), `${JSON.stringify(receipt, null, 2)}\n`);
    process.stdout.write(`${JSON.stringify(receipt, null, 2)}\n`);
  } catch (error) {
    process.stderr.write(`${viteOutput}\n`);
    if (browserMessages.length > 0) process.stderr.write(`${browserMessages.join("\n")}\n`);
    if (page) {
      await page.screenshot({ path: path.join(artifactDir, "failure.png"), fullPage: true }).catch(() => {});
      await writeFile(path.join(artifactDir, "failure.html"), await page.content()).catch(() => {});
    }
    throw error;
  } finally {
    await browser?.close();
    await stopServer(vite);
    activeBrowser = undefined;
    activeServer = undefined;
  }
}

run().catch((error) => {
  process.stderr.write(`evolution headless acceptance failed: ${error.stack ?? error}\n`);
  process.exitCode = 1;
});
