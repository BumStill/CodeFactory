// SPDX-License-Identifier: Apache-2.0
import { spawn } from "node:child_process";
import { mkdir, writeFile, access } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import assert from "node:assert/strict";
import { chromium } from "playwright-core";
import { gotoScenario } from "./headless-acceptance-support.mjs";
const artifactDir = path.join(os.tmpdir(), "codefactory-m135-headless");
await mkdir(artifactDir, { recursive: true });
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", "1459", "--strictPort"], { stdio: ["ignore", "pipe", "pipe"] });
let log = "";
server.stdout.on("data", (v) => { log += v; });
server.stderr.on("data", (v) => { log += v; });
console.log(JSON.stringify({ pid: server.pid, log: path.join(artifactDir, "vite.log") }));
let browser;
try {
  const url = "http://127.0.0.1:1459/attach-file-links-acceptance.html";
  let ready = false;
  for (let i = 0; i < 120; i++) {
    if (server.exitCode !== null) throw new Error(log);
    try { if ((await fetch(url)).ok) { ready = true; break; } } catch {}
    await new Promise((r) => setTimeout(r, 250));
  }
  assert(ready, "Vite ready within 30 seconds");
  const candidates = process.platform === "darwin" ? ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"] : process.platform === "win32" ? [path.join(process.env.PROGRAMFILES ?? "C:\\Program Files", "Google/Chrome/Application/chrome.exe")] : ["/usr/bin/chromium", "/usr/bin/google-chrome"];
  let executablePath;
  for (const candidate of candidates) { try { await access(candidate); executablePath = candidate; break; } catch {} }
  assert(executablePath, "system browser available");
  browser = await chromium.launch({ executablePath, headless: true });
  for (const theme of ["light", "dark"]) for (const width of [1366, 480]) {
    const page = await browser.newPage({ viewport: { width, height: 768 } });
    await gotoScenario(page, url, { landmark: page.getByRole("main", { name: "Attachment file links acceptance" }), label: "M135" });
    await page.evaluate((theme) => document.documentElement.classList.toggle("dark", theme === "dark"), theme);
    assert(await page.getByRole("button", { name: "附加文件" }).isEnabled());
    await page.locator('input[type="file"]').setInputFiles({ name: "synthetic.png", mimeType: "image/png", buffer: Buffer.from([137,80,78,71]) });
    await page.getByRole("img", { name: "synthetic.png" }).waitFor();
    await page.getByRole("button", { name: "发送", exact: true }).click();
    await page.getByRole("img", { name: "synthetic.png" }).waitFor();
    for (const name of ["本地报告", "相对报告", "网站"]) await page.getByRole("link", { name, exact: true }).click();
    await page.getByRole("button", { name: "系统打开 report.pdf" }).click();
    await page.getByRole("link", { name: "缺失文件" }).click();
    await page.getByRole("alert").filter({ hasText: "file not found" }).waitFor();
    const calls = await page.evaluate(() => window.__m135Calls);
    const saved = calls.find((v) => v.command === "save_chat_attachment");
    assert.equal(saved.args.cwd, null);
    assert.equal(saved.args.sessionId, "draft");
    assert.equal(saved.args.dataBase64, "iVBORw==");
    assert(calls.some((v) => v.command === "open_chat_link" && v.args.target === "file:///synthetic/report.pdf" && v.args.sessionId === "draft"));
    assert(calls.filter((v) => v.command === "open_chat_link" && v.args.target === "report.pdf").length >= 2);
    assert(calls.some((v) => v.command === "open_chat_link" && v.args.target === "https://example.com"));
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "no horizontal viewport overflow");
    await page.screenshot({ path: path.join(artifactDir, `${theme}-${width}.png`), fullPage: true });
    await page.close();
  }
  console.log(JSON.stringify({ result: "pass", artifactDir, checks: ["no-project upload and send", "local and relative links", "http link", "open button", "visible missing-file toast", "light/dark wide/narrow"] }));
} finally {
  if (browser) await browser.close();
  server.kill("SIGTERM");
  await writeFile(path.join(artifactDir, "vite.log"), log);
}
