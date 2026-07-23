#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Lock-safe real-browser gate for the repository-owned intent UX.

import { spawn, spawnSync } from "node:child_process";
import { access, mkdir, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const viteCli = path.join(root, "node_modules", "vite", "bin", "vite.js");
const port = Number(process.env.CODEFACTORY_REPOSITORY_INTENT_PORT ?? 1441);
const baseUrl = `http://127.0.0.1:${port}/repository-intent-acceptance.html`;
const artifactDir = process.env.CODEFACTORY_REPOSITORY_INTENT_ARTIFACT_DIR
  ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-repository-intent-headless");

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
      // Continue through installed system browsers without downloading one.
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

async function assertWorkspace(page, tag) {
  await page.goto(baseUrl, { waitUntil: "networkidle" });
  const conversation = page.getByRole("main", { name: "会话窗口" });
  await conversation.waitFor({ timeout: 10_000 });
  const header = page.getByRole("banner", { name: "会话工具栏" });
  const sidebar = page.getByRole("complementary", { name: "会话列表" });
  await header.waitFor();
  await sidebar.waitFor();
  assert((await header.getByRole("button", { name: "新建空白会话" }).count()) === 0, `[${tag}] duplicate header new-session action remains`);
  assert((await page.getByRole("button", { name: "新建", exact: true }).count()) === 1, `[${tag}] workspace must expose exactly one new-session menu`);
  assert((await header.getByRole("button", { name: "收起会话侧栏" }).count()) === 1, `[${tag}] collapse control missing`);
  assert((await conversation.getByText("会话执行详情", { exact: true }).count()) === 0, `[${tag}] fixed execution detail remains in conversation`);
  assert((await conversation.getByText("在会话内执行仓库需求", { exact: true }).count()) === 0, `[${tag}] delegated task leaked into conversation`);
  assert((await page.getByText("执行流", { exact: true }).count()) === 0, `[${tag}] execution stream should stay hidden in project sessions`);
  const taskActivity = header.getByRole("button", { name: "打开任务活动" });
  await taskActivity.waitFor();
  const activityHeight = await taskActivity.evaluate((element) => element.getBoundingClientRect().height);
  assert(activityHeight <= 30, `[${tag}] task activity control too tall: ${activityHeight}`);
  await taskActivity.click();
  const taskDrawer = page.getByRole("dialog", { name: "任务活动" });
  await taskDrawer.waitFor();
  assert(await taskDrawer.getByText("在会话内执行仓库需求", { exact: true }).isVisible(), `[${tag}] delegated task missing from drawer`);
  await taskDrawer.getByRole("button", { name: "关闭任务活动" }).click();
  await taskDrawer.waitFor({ state: "detached" });

  const sessionRows = sidebar.locator("[data-session-row]");
  assert((await sessionRows.count()) >= 10, `[${tag}] sidebar does not expose at least 10 sessions in the fixture`);
  const rowHeights = await sessionRows.evaluateAll((rows) => rows.map((row) => row.getBoundingClientRect().height));
  assert(rowHeights.every((height) => height <= 46), `[${tag}] sidebar row exceeds 46px: ${JSON.stringify(rowHeights)}`);

  const completedGroup = conversation.getByRole("button", { name: "查看 3 个已完成操作" });
  await completedGroup.waitFor();
  const groupHeight = await completedGroup.evaluate((element) => element.getBoundingClientRect().height);
  assert(groupHeight <= 30, `[${tag}] completed tool group too tall: ${groupHeight}`);
  assert(await conversation.getByText("check failed", { exact: true }).isVisible(), `[${tag}] failed command reason is hidden`);
  await completedGroup.click();
  const compactCommand = conversation.getByRole("button", { name: /命令.*npm test/ });
  await compactCommand.waitFor();
  const commandHeight = await compactCommand.evaluate((element) => element.getBoundingClientRect().height);
  assert(commandHeight <= 30, `[${tag}] compact command exceeds 30px: ${commandHeight}`);
  assert((await page.getByTitle(/规范工作台/).count()) === 0, `[${tag}] specification workbench remains`);
  assert((await page.getByRole("button", { name: /规范|计划/ }).count()) === 0, `[${tag}] specification or plan button remains`);
  for (const label of ["我的画像", "进化审查", "能力评测", "资源中心", "AI Coding OS"]) {
    assert((await header.getByRole("button", { name: label }).count()) === 0, `[${tag}] ${label} remains in workspace toolbar`);
  }
  assert((await header.getByRole("button", { name: "设置" }).count()) === 1, `[${tag}] settings entry missing from workspace toolbar`);
  assert((await page.getByText("拆任务", { exact: true }).count()) === 0, `[${tag}] decomposition UI remains`);

  const expandedWidth = await conversation.evaluate((element) => element.getBoundingClientRect().width);
  await header.getByRole("button", { name: "收起会话侧栏" }).click();
  await sidebar.waitFor({ state: "detached" });
  assert((await header.getByRole("button", { name: "展开会话侧栏" }).count()) === 1, `[${tag}] restore control missing after collapse`);
  const collapsedWidth = await conversation.evaluate((element) => element.getBoundingClientRect().width);
  assert(collapsedWidth > expandedWidth, `[${tag}] conversation did not reclaim sidebar width: ${expandedWidth} -> ${collapsedWidth}`);
  const collapsedOverflow = await page.evaluate(() => ({ width: innerWidth, scrollWidth: document.documentElement.scrollWidth }));
  assert(collapsedOverflow.scrollWidth <= collapsedOverflow.width, `[${tag}] collapsed workspace horizontal overflow: ${JSON.stringify(collapsedOverflow)}`);
  await page.screenshot({ path: path.join(artifactDir, `${tag}-collapsed.png`), fullPage: true });

  await page.reload({ waitUntil: "networkidle" });
  await page.getByRole("main", { name: "会话窗口" }).waitFor({ timeout: 10_000 });
  assert((await page.getByRole("complementary", { name: "会话列表" }).count()) === 0, `[${tag}] collapsed state did not persist across reload`);
  await page.getByRole("banner", { name: "会话工具栏" }).getByRole("button", { name: "展开会话侧栏" }).click();
  await page.getByRole("complementary", { name: "会话列表" }).waitFor();
  assert((await page.getByRole("button", { name: "新建", exact: true }).count()) === 1, `[${tag}] restored sidebar changed the single new-session entry contract`);
  await page.reload({ waitUntil: "networkidle" });
  await page.getByRole("complementary", { name: "会话列表" }).waitFor({ timeout: 10_000 });
  assert((await page.getByRole("banner", { name: "会话工具栏" }).getByRole("button", { name: "收起会话侧栏" }).count()) === 1, `[${tag}] expanded state did not persist across reload`);
  await page.screenshot({ path: path.join(artifactDir, `${tag}.png`), fullPage: true });
}

async function run() {
  await rm(artifactDir, { recursive: true, force: true });
  await mkdir(artifactDir, { recursive: true });
  const vite = spawn(process.execPath, [viteCli, "--host", "127.0.0.1", "--port", String(port), "--strictPort"], {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, NO_COLOR: "1" },
    detached: process.platform !== "win32",
  });
  let browser;
  let page;
  const pageErrors = [];
  try {
    await waitForServer(vite);
    const executablePath = await firstBrowser();
    browser = await chromium.launch({ executablePath, headless: true, args: ["--disable-gpu", "--no-sandbox"] });
    page = await browser.newPage({ viewport: { width: 1366, height: 768 } });
    page.on("pageerror", (error) => pageErrors.push(error.stack ?? String(error)));

    await assertWorkspace(page, "wide-workspace");
    await page.getByTitle("远程仓库（问题与拉取请求）").click();
    await page.getByText("Repository-owned specification", { exact: true }).click();
    assert(await page.getByText("Keep durable product intent in ordinary versioned repository files.", { exact: true }).isVisible(), "remote Issue detail missing");
    assert((await page.getByRole("button", { name: "创建为规范" }).count()) === 0, "remote Issue still converts to an app-owned specification");
    await page.screenshot({ path: path.join(artifactDir, "remote-issue.png"), fullPage: true });

    await page.setViewportSize({ width: 800, height: 700 });
    await assertWorkspace(page, "minimum-width-workspace");
    const overflow = await page.evaluate(() => ({ width: innerWidth, scrollWidth: document.documentElement.scrollWidth }));
    assert(overflow.scrollWidth <= overflow.width, `minimum-width workspace horizontal overflow: ${JSON.stringify(overflow)}`);
    assert(pageErrors.length === 0, `browser page errors: ${pageErrors.join(" | ")}`);

    const receipt = {
      status: "pass",
      browser: executablePath,
      surfaces: ["workspace-header", "task-activity-drawer", "dense-session-sidebar", "compact-tool-activity", "remote-issue-detail"],
      viewports: ["1366x768", "800x700 (configured minWidth)"],
      artifact_dir: artifactDir,
      interactive_desktop: "project picker reached; final Open action blocked by macOS lock",
    };
    await writeFile(path.join(artifactDir, "receipt.json"), `${JSON.stringify(receipt, null, 2)}\n`);
    process.stdout.write(`${JSON.stringify(receipt, null, 2)}\n`);
  } catch (error) {
    if (page) {
      await page.screenshot({ path: path.join(artifactDir, "failure.png"), fullPage: true }).catch(() => {});
      await writeFile(path.join(artifactDir, "failure.html"), await page.content()).catch(() => {});
    }
    if (pageErrors.length > 0) process.stderr.write(`${pageErrors.join("\n")}\n`);
    throw error;
  } finally {
    await browser?.close();
    await stopServer(vite);
  }
}

run().catch((error) => {
  process.stderr.write(`repository-intent headless acceptance failed: ${error.stack ?? error}\n`);
  process.exitCode = 1;
});
