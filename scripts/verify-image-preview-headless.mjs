#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate for image markdown previews and click-to-enlarge in both transcript and input tray.

import { spawn, spawnSync } from "node:child_process";
import { access, mkdir, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const viteCli = path.join(root, "node_modules", "vite", "bin", "vite.js");
const port = Number(process.env.CODEFACTORY_IMAGE_PREVIEW_PORT ?? 1454);
const baseUrl = `http://127.0.0.1:${port}/image-preview-acceptance.html`;
const artifactDir = process.env.CODEFACTORY_IMAGE_PREVIEW_ARTIFACT_DIR ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-image-preview-headless");
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
async function pasteImage(page) {
  await page.evaluate(async () => {
    const input = document.querySelector("textarea");
    if (!input) throw new Error("missing message input textarea");
    const bytes = new Uint8Array([137,80,78,71]);
    const file = new File([bytes], "chat box.png", { type: "image/png" });
    const event = new ClipboardEvent("paste", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "clipboardData", { value: { items: [{ kind: "file", type: "image/png", getAsFile: () => file }] } });
    input.dispatchEvent(event);
  });
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
    const page = await browser.newPage({ viewport: { width: 960, height: 720 } });
    await page.goto(baseUrl, { waitUntil: "networkidle" });
    await page.getByRole("main", { name: "Image preview acceptance" }).waitFor({ timeout: 10_000 });

    const transcriptImage = page.getByRole("img", { name: "IMG_6190.png" }).first();
    await transcriptImage.waitFor({ timeout: 10_000 });
    assert(await page.getByText(/!\[IMG_6190\.png\]/).count() === 0, "raw markdown should not be visible in transcript");
    await page.getByRole("button", { name: "放大查看 IMG_6190.png" }).click();
    await page.getByRole("dialog", { name: "图片预览" }).waitFor();
    assert(await page.getByRole("dialog", { name: "图片预览" }).getByRole("img", { name: "IMG_6190.png" }).count() === 1, "transcript lightbox should show full image");
    await page.keyboard.press("Escape");
    await page.getByRole("dialog", { name: "图片预览" }).waitFor({ state: "hidden" });

    await pasteImage(page);
    await page.getByRole("img", { name: "chat box.png" }).waitFor({ timeout: 10_000 });
    await page.getByRole("button", { name: "放大查看 chat box.png" }).click();
    await page.getByRole("dialog", { name: "图片预览" }).waitFor();
    assert(await page.getByRole("dialog", { name: "图片预览" }).getByRole("img", { name: "chat box.png" }).count() === 1, "input tray lightbox should show full image");
    await page.screenshot({ path: path.join(artifactDir, "image-preview-lightbox.png"), fullPage: true });
    console.log(JSON.stringify({ status: "pass", artifactDir, checks: { transcriptPreviewClickable: true, inputPreviewClickable: true, spacedFileUrlPreviewVisible: true } }, null, 2));
  } finally {
    if (browser) await browser.close();
    await stopServer(vite);
    await import("node:fs/promises").then(({ writeFile }) => writeFile(path.join(artifactDir, "vite.log"), viteLog));
    if (viteLog.trim()) console.error(viteLog.trim().split("\n").slice(-20).join("\n"));
  }
}
main().catch((error) => { console.error(`image preview headless acceptance failed: ${error.stack ?? error}`); process.exit(1); });
