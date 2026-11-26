// SPDX-License-Identifier: Apache-2.0
import { spawn } from "node:child_process";
import { access, mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import assert from "node:assert/strict";
import { chromium } from "playwright-core";
import { gotoScenario } from "./headless-acceptance-support.mjs";

const dir = path.resolve(".codefactory-cache/m289-headless");
await mkdir(dir, { recursive: true });
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", "1492", "--strictPort"], { stdio: ["ignore", "pipe", "pipe"] });
let log = "";
server.stdout.on("data", (value) => { log += value; });
server.stderr.on("data", (value) => { log += value; });
console.log(JSON.stringify({ pid: server.pid, log: path.join(dir, "vite.log") }));
let browser;
try {
  const url = "http://127.0.0.1:1492/file-drop-attachments-acceptance.html";
  let ready = false;
  for (let attempt = 0; attempt < 120; attempt += 1) {
    if (server.exitCode !== null) throw new Error(log);
    try { if ((await fetch(url)).ok) { ready = true; break; } } catch {}
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  assert(ready);
  const candidates = process.platform === "darwin"
    ? ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"]
    : process.platform === "win32"
      ? [path.join(process.env.PROGRAMFILES ?? "C:\\Program Files", "Google/Chrome/Application/chrome.exe"), path.join(process.env["PROGRAMFILES(X86)"] ?? "C:\\Program Files (x86)", "Microsoft/Edge/Application/msedge.exe")]
      : ["/usr/bin/chromium", "/usr/bin/google-chrome"];
  let executablePath;
  for (const candidate of candidates) { try { await access(candidate); executablePath = candidate; break; } catch {} }
  assert(executablePath);
  browser = await chromium.launch({ executablePath, headless: true });
  for (const theme of ["light", "dark"]) for (const width of [1366, 480]) {
    const page = await browser.newPage({ viewport: { width, height: 768 } });
    await gotoScenario(page, url, { landmark: page.getByRole("main", { name: "M289 file drop" }), label: "M289" });
    await page.evaluate((selectedTheme) => { document.documentElement.dataset.theme = selectedTheme; }, theme);
    await page.evaluate(() => {
      const files = [new File(["png"], "drop.png", { type: "image/png" }), new File(["notes"], "notes.txt", { type: "text/plain" })];
      const dataTransfer = { types: ["Files"], files, items: files.map((file) => ({ kind: "file", getAsFile: () => file })) };
      const enter = new DragEvent("dragenter", { bubbles: true, cancelable: true });
      Object.defineProperty(enter, "dataTransfer", { value: dataTransfer });
      window.dispatchEvent(enter);
    });
    const target = page.getByRole("status");
    await target.waitFor();
    assert.equal(await target.textContent(), "松开以添加 2 个文件");
    assert(await target.isVisible());
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    await page.screenshot({ path: path.join(dir, `${theme}-${width}.png`), fullPage: true });
    await page.evaluate(() => {
      const files = [new File(["png"], "drop.png", { type: "image/png" }), new File(["notes"], "notes.txt", { type: "text/plain" })];
      const dataTransfer = { types: ["Files"], files, items: files.map((file) => ({ kind: "file", getAsFile: () => file })) };
      const drop = new DragEvent("drop", { bubbles: true, cancelable: true });
      Object.defineProperty(drop, "dataTransfer", { value: dataTransfer });
      window.dispatchEvent(drop);
    });
    await page.getByRole("img", { name: "drop.png" }).waitFor();
    await page.getByText("notes.txt").waitFor();
    assert.equal(await page.evaluate(() => window.__m289Calls.filter((call) => call.command === "save_session_attachment").length), 2);
    await page.close();
  }
  console.log(JSON.stringify({ result: "pass", artifactDir: dir, checks: ["whole-window overlay", "multi-file shared save route", "light/dark", "wide/narrow", "no horizontal overflow"] }));
} finally {
  if (browser) await browser.close();
  server.kill("SIGTERM");
  await writeFile(path.join(dir, "vite.log"), log);
}
