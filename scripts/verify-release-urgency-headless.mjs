// SPDX-License-Identifier: Apache-2.0
import { spawn } from "node:child_process";
import { mkdir, access, writeFile } from "node:fs/promises";
import { chromium } from "playwright-core";
import { allocateLoopbackPort, waitForAcceptanceDocument } from "./headless-acceptance-support.mjs";
const assert = (value, message) => { if (!value) throw new Error(message); };
const artifacts = "artifacts/m137/headless";
await mkdir(artifacts, { recursive: true });
const port = await allocateLoopbackPort(1453);
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--port", String(port), "--strictPort"], { stdio: ["ignore", "pipe", "pipe"] });
console.log(`Vite PID=${server.pid}; artifacts=${artifacts}`);
server.stdout.pipe(process.stdout); server.stderr.pipe(process.stderr);
let browser;
try {
  const url = `http://127.0.0.1:${port}/release-urgency-acceptance.html`;
  await waitForAcceptanceDocument(server, url, "Release Urgency Acceptance");
  let executablePath;
  for (const candidate of [process.env.CHROME_PATH, "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge", "/usr/bin/chromium"].filter(Boolean)) {
    try { await access(candidate); executablePath = candidate; break; } catch { /* next */ }
  }
  assert(executablePath, "Chromium required");
  browser = await chromium.launch({ executablePath, headless: true, args: ["--no-sandbox"] });
  const results = [];
  for (const theme of ["light", "dark"]) for (const width of [1366, 390]) {
    const page = await browser.newPage({ viewport: { width, height: 768 } });
    await page.goto(`${url}?theme=${theme}`);
    const picker = page.getByRole("combobox", { name: "发版紧急度" });
    await page.waitForFunction(() => document.querySelector("select")?.value === "immediate" && !document.querySelector("select")?.disabled);
    await picker.selectOption("hold");
    await page.waitForFunction(() => document.querySelector("select")?.value === "hold" && !document.querySelector("select")?.disabled);
    const box = await picker.boundingBox();
    assert(box && box.height >= 44 && box.x >= 0 && box.x + box.width <= width, "picker clipped or undersized");
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), "horizontal overflow");
    await page.screenshot({ path: `${artifacts}/${theme}-${width}.png`, fullPage: true });
    results.push({ theme, width, selected: await picker.inputValue(), box });
    await page.close();
  }
  await writeFile(`${artifacts}/receipt.json`, JSON.stringify({ status: "pass", environment: "real Chromium, production picker and synthetic IPC; installed app acceptance deferred to orchestrator", results }, null, 2));
  console.log("M137 real-browser acceptance PASS: light/dark, desktop/narrow, persisted selection and bounds");
} finally {
  await browser?.close(); server.kill("SIGTERM");
  await new Promise((resolve) => server.exitCode !== null ? resolve() : server.once("exit", resolve));
}
