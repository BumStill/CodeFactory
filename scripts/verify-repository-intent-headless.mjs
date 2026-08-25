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
  await page.goto(baseUrl, { waitUntil: "domcontentloaded" });
  const conversation = page.getByRole("main", { name: "会话窗口" });
  await conversation.waitFor({ timeout: 10_000 });
  const header = page.getByRole("banner", { name: "会话工具栏" });
  const sidebar = page.getByRole("complementary", { name: "会话列表" });
  await header.waitFor();
  await sidebar.waitFor();
  assert((await header.getByRole("button", { name: "新建空白会话" }).count()) === 0, `[${tag}] duplicate header new-session action remains`);
  assert((await page.getByRole("button", { name: "新建会话", exact: true }).count()) === 1, `[${tag}] workspace must expose exactly one new-session menu`);
  assert((await sidebar.getByRole("button", { name: "收起会话侧栏" }).count()) === 1, `[${tag}] collapse control missing`);
  assert((await conversation.getByText("会话执行详情", { exact: true }).count()) === 0, `[${tag}] fixed execution detail remains in conversation`);
  assert((await conversation.getByText("在会话内执行仓库需求", { exact: true }).count()) === 0, `[${tag}] delegated task leaked into conversation`);
  assert((await page.getByText("执行流", { exact: true }).count()) === 0, `[${tag}] execution stream should stay hidden in project sessions`);
  const localGit = header.getByRole("button", { name: /本地 Git；分支 codex\/repo-owned-specs；已同步；无本地变更/ });
  await localGit.waitFor();
  const delivery = header.getByRole("button", { name: /会话交付状态；PR #175；CI 通过.*已合并.*v1\.63\.0.*未验证上线/ });
  await delivery.waitFor();
  await delivery.click();
  const deliveryDrawer = page.locator('[data-testid="workspace-auxiliary-pane"][data-pane-kind="delivery"]');
  await deliveryDrawer.waitFor();
  assert(await deliveryDrawer.getByText("feat/workspace-ui → main", { exact: true }).isVisible(), `[${tag}] PR branch relation missing`);
  assert(await deliveryDrawer.getByText("3373a69", { exact: true }).isVisible(), `[${tag}] PR head SHA missing from CI step`);
  assert(await deliveryDrawer.getByText(/release artifact 可见/).isVisible(), `[${tag}] release must not be described as live`);
  assert(await deliveryDrawer.getByText(/真实上线还需要 deliver_changes 的部署观察或 live verifier 通过/).isVisible(), `[${tag}] live verifier explanation missing`);
  await deliveryDrawer.getByRole("button", { name: "关闭交付详情" }).click();
  await deliveryDrawer.waitFor({ state: "detached" });
  assert((await header.getByRole("button", { name: /检查点/ }).count()) === 0, `[${tag}] checkpoint counter must not remain in header`);
  const taskActivity = header.getByRole("button", { name: "打开任务活动" });
  await taskActivity.waitFor();
  const activityHeight = await taskActivity.evaluate((element) => element.getBoundingClientRect().height);
  assert(activityHeight <= 44, `[${tag}] task activity control exceeds the compact/touch target: ${activityHeight}`);
  await taskActivity.click();
  const taskDrawer = page.locator('[data-testid="workspace-auxiliary-pane"][data-pane-kind="tasks"]');
  await taskDrawer.waitFor();
  assert(await taskDrawer.getByText("在会话内执行仓库需求", { exact: true }).isVisible(), `[${tag}] delegated task missing from drawer`);
  assert(await taskDrawer.getByText("模型配置 6", { exact: true }).isVisible(), `[${tag}] provider blocker count is not actionable`);
  assert(await taskDrawer.getByText("系统正在处理失败项，并会自动续接剩余 2 项。", { exact: true }).isVisible(), `[${tag}] mixed failed/pending state has no system-owned recovery explanation`);
  assert((await taskDrawer.getByRole("button", { name: /继续执行/ }).count()) === 0, `[${tag}] generic continue action bypasses a failure blocker`);
  const settingsAction = taskDrawer.getByRole("button", { name: "打开模型设置" });
  await settingsAction.waitFor();
  assert((await taskDrawer.getByRole("button", { name: /已修复.*重试|继续执行/ }).count()) === 0, `[${tag}] technical recovery requires a manual continuation`);
  const settingsBox = await settingsAction.boundingBox();
  assert(settingsBox && settingsBox.x >= 0 && settingsBox.x + settingsBox.width <= page.viewportSize().width, `[${tag}] blocker action overflows drawer: ${JSON.stringify(settingsBox)}`);
  await settingsAction.click();
  assert(await page.evaluate(() => window.__settingsTab) === "endpoints", `[${tag}] provider action did not target endpoint/API-key settings`);
  await page.getByText("API 端点", { exact: true }).waitFor();
  assert(await page.getByText("API 端点", { exact: true }).isVisible(), `[${tag}] endpoint settings page did not render`);
  await page.locator("header button").first().click();
  await taskActivity.waitFor();
  await taskActivity.click();
  const reopenedTaskDrawer = page.locator('[data-testid="workspace-auxiliary-pane"][data-pane-kind="tasks"]');
  await reopenedTaskDrawer.waitFor();
  assert((await reopenedTaskDrawer.getByRole("button", { name: /已修复.*重试|继续执行/ }).count()) === 0, `[${tag}] reopened task activity exposes manual technical recovery`);
  await reopenedTaskDrawer.getByRole("button", { name: "关闭任务活动" }).click();
  await reopenedTaskDrawer.waitFor({ state: "detached" });

  const sessionRows = sidebar.locator("[data-session-row]");
  assert((await sessionRows.count()) >= 10, `[${tag}] sidebar does not expose at least 10 sessions in the fixture`);
  const rowHeights = await sessionRows.evaluateAll((rows) => rows.map((row) => row.getBoundingClientRect().height));
  assert(rowHeights.every((height) => height <= 50), `[${tag}] sidebar row exceeds the 50px density contract: ${JSON.stringify(rowHeights)}`);

  assert(await conversation.getByText("check failed", { exact: true }).isVisible(), `[${tag}] failed command reason is hidden`);
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
  await sidebar.getByRole("button", { name: "收起会话侧栏" }).click();
  await sidebar.waitFor({ state: "detached" });
  assert((await header.getByRole("button", { name: "展开会话侧栏" }).count()) === 1, `[${tag}] restore control missing after collapse`);
  const collapsedWidth = await conversation.evaluate((element) => element.getBoundingClientRect().width);
  assert(collapsedWidth > expandedWidth, `[${tag}] conversation did not reclaim sidebar width: ${expandedWidth} -> ${collapsedWidth}`);
  const collapsedOverflow = await page.evaluate(() => ({ width: innerWidth, scrollWidth: document.documentElement.scrollWidth }));
  assert(collapsedOverflow.scrollWidth <= collapsedOverflow.width, `[${tag}] collapsed workspace horizontal overflow: ${JSON.stringify(collapsedOverflow)}`);
  await page.screenshot({ path: path.join(artifactDir, `${tag}-collapsed.png`), fullPage: true });

  await page.reload({ waitUntil: "domcontentloaded" });
  await page.getByRole("main", { name: "会话窗口" }).waitFor({ timeout: 10_000 });
  assert((await page.getByRole("complementary", { name: "会话列表" }).count()) === 0, `[${tag}] collapsed state did not persist across reload`);
  await page.getByRole("banner", { name: "会话工具栏" }).getByRole("button", { name: "展开会话侧栏" }).click();
  await page.getByRole("complementary", { name: "会话列表" }).waitFor();
  assert((await page.getByRole("button", { name: "新建会话", exact: true }).count()) === 1, `[${tag}] restored sidebar changed the single new-session entry contract`);
  await page.reload({ waitUntil: "domcontentloaded" });
  await page.getByRole("complementary", { name: "会话列表" }).waitFor({ timeout: 10_000 });
  assert((await page.getByRole("complementary", { name: "会话列表" }).getByRole("button", { name: "收起会话侧栏" }).count()) === 1, `[${tag}] expanded state did not persist across reload`);
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
    await page.getByRole("banner", { name: "会话工具栏" }).getByRole("button", { name: /本地 Git；分支 codex\/repo-owned-specs/ }).click();
    const localGitDrawer = page.getByText("本地 Git", { exact: true });
    await localGitDrawer.waitFor();
    assert((await page.getByRole("button", { name: /恢复 \d+/ }).count()) === 0, "unchanged checkpoints should not create a recovery action");
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
      surfaces: ["workspace-header", "task-activity-drawer", "actionable-provider-blockers", "dense-session-sidebar", "compact-tool-activity", "local-git-drawer", "github-delivery-status", "remote-issue-detail"],
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
