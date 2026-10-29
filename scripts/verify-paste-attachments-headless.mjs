// SPDX-License-Identifier: Apache-2.0
import { spawn } from "node:child_process";
import { access, mkdir, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import assert from "node:assert/strict";
import { chromium } from "playwright-core";
import { gotoScenario } from "./headless-acceptance-support.mjs";
const dir = path.join(os.tmpdir(), "codefactory-m190-headless");
await mkdir(dir, { recursive: true });
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", "1490", "--strictPort"], { stdio: ["ignore", "pipe", "pipe"] });
let log = ""; server.stdout.on("data", v => log += v); server.stderr.on("data", v => log += v);
console.log(JSON.stringify({ pid: server.pid, log: path.join(dir, "vite.log") }));
let browser;
try {
  const url = "http://127.0.0.1:1490/paste-attachments-acceptance.html";
  let ready = false;
  for (let i = 0; i < 120; i++) {
    if (server.exitCode !== null) throw new Error(log);
    try { if ((await fetch(url)).ok) { ready = true; break; } } catch {}
    await new Promise(r => setTimeout(r, 250));
  }
  assert(ready);
  const candidates = process.platform === "darwin" ? ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"] : process.platform === "win32" ? [path.join(process.env.PROGRAMFILES ?? "C:\\Program Files", "Google/Chrome/Application/chrome.exe"), path.join(process.env["PROGRAMFILES(X86)"] ?? "C:\\Program Files (x86)", "Microsoft/Edge/Application/msedge.exe")] : ["/usr/bin/chromium", "/usr/bin/google-chrome"];
  let executablePath;
  for (const candidate of candidates) { try { await access(candidate); executablePath = candidate; break; } catch {} }
  assert(executablePath); browser = await chromium.launch({ executablePath, headless: true });
  for (const theme of ["light", "dark"]) for (const width of [1366, 480]) {
    const page = await browser.newPage({ viewport: { width, height: 768 } });
    await gotoScenario(page, url, { landmark: page.getByRole("main", { name: "M190 attachments" }), label: "M190" });
    await page.evaluate(theme => document.documentElement.dataset.theme = theme, theme);
    await page.evaluate(() => {
      const files = [new File(["a"], "shot.png", { type: "image/png" }), new File(["b"], "second.jpg", { type: "image/jpeg" }), new File(["c"], "report.pdf", { type: "application/pdf" })];
      const event = new ClipboardEvent("paste", { bubbles: true, cancelable: true });
      Object.defineProperty(event, "clipboardData", { value: { files, items: [], getData: () => "mixed text" } });
      assertDefault(document.querySelector("textarea").dispatchEvent(event));
      function assertDefault(value) { if (!value) throw new Error("mixed paste cancelled"); }
    });
    await page.getByRole("img", { name: "second.jpg" }).waitFor();
    await page.getByRole("button", { name: "系统打开 report.pdf" }).waitFor();
    await page.getByRole("textbox").fill("mixed text");
    await page.getByRole("button", { name: "发送", exact: true }).click();
    await page.getByRole("button", { name: "系统打开 report.pdf" }).waitFor();
    await page.reload();
    await page.getByRole("button", { name: "系统打开 report.pdf" }).waitFor();
    await page.getByRole("img", { name: "shot.png" }).waitFor();
    await page.getByRole("button", { name: "放大查看 shot.png" }).click();
    await page.getByRole("dialog", { name: "图片预览" }).waitFor();
    await page.keyboard.press("Escape");
    await page.getByRole("button", { name: "系统打开 report.pdf" }).click();
    assert(await page.evaluate(() => window.__m190Calls.some(c => c.command === "open_chat_link" && c.args.target.includes("report.pdf"))));
    assert(await page.getByText("2.0 MB", { exact: false }).count() > 0);
    await page.evaluate(theme => document.documentElement.dataset.theme = theme, theme);
    await page.getByRole("button", { name: "系统打开 report.pdf" }).scrollIntoViewIfNeeded();
    assert(await page.getByRole("button", { name: "系统打开 report.pdf" }).isVisible());
    assert(await page.getByRole("img", { name: "shot.png" }).isVisible());
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    await page.screenshot({ path: path.join(dir, `${theme}-${width}.png`), fullPage: true });
    await page.close();
  }
  console.log(JSON.stringify({ result: "pass", artifactDir: dir, checks: ["multi image/document paste", "mixed text default", "send and reload history", "lightbox", "controlled file open", "auto size", "light/dark wide/narrow"] }));
} finally {
  if (browser) await browser.close(); server.kill("SIGTERM"); await writeFile(path.join(dir, "vite.log"), log);
}
