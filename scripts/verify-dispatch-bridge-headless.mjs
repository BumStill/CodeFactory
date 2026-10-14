// SPDX-License-Identifier: Apache-2.0
import { spawn } from "node:child_process";
import { mkdir, writeFile, access } from "node:fs/promises";
import path from "node:path";
import { chromium } from "playwright-core";
import { allocateLoopbackPort, waitForAcceptanceDocument } from "./headless-acceptance-support.mjs";
const root = process.cwd();
const artifacts = path.join(root, "artifacts/m83/headless");
function assert(value, message) { if (!value) throw new Error(message); }
await mkdir(artifacts, { recursive: true });
const port = await allocateLoopbackPort(1453);
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--port", String(port), "--strictPort"], { cwd: root, stdio: ["ignore", "pipe", "pipe"] });
console.log(`Vite PID=${server.pid}; evidence=${artifacts}`);
server.stdout.on("data", (data) => process.stdout.write(data));
server.stderr.on("data", (data) => process.stderr.write(data));
let browser;
try {
  const url = `http://127.0.0.1:${port}/dispatch-bridge-acceptance.html`;
  await waitForAcceptanceDocument(server, url, "Dispatch Bridge Acceptance");
  let executablePath;
  for (const candidate of [process.env.CHROME_PATH, "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge", "/usr/bin/chromium", "/usr/bin/google-chrome"].filter(Boolean)) {
    try { await access(candidate); executablePath = candidate; break; } catch { /* next installed browser */ }
  }
  assert(executablePath, "installed Chromium browser required");
  browser = await chromium.launch({ executablePath, headless: true, args: ["--no-sandbox"] });
  const results = [];
  for (const theme of ["light", "dark"]) {
    const page = await browser.newPage({ viewport: { width: 1366, height: 768 } });
    await page.goto(`${url}?theme=${theme}`);
    await page.getByRole("button", { name: "设置", exact: true }).click();
    await page.getByRole("tab", { name: "关于", exact: true }).click();
    await page.evaluate(() => window.__dispatchProbe.dispatch("quota", "get_quota", {}));
    await page.waitForFunction(() => window.__dispatchProbe.replies.some((reply) => reply.requestId === "quota"));
    const quota = await page.evaluate(() => window.__dispatchProbe.replies.find((reply) => reply.requestId === "quota"));
    assert(quota.ok && quota.result[0].five_hour_cap_percent === 80, "settings quota fields mismatch");
    await page.evaluate(() => window.__dispatchProbe.dispatch("create", "create_and_send", { project: "/synthetic/project", message: "合成派单消息", delivery_authorized: false }));
    await page.waitForFunction(() => window.__dispatchProbe.replies.some((reply) => reply.requestId === "create"));
    const evidence = await page.evaluate(() => ({ reply: window.__dispatchProbe.replies.find((reply) => reply.requestId === "create"), sends: window.__dispatchProbe.sends }));
    assert(evidence.reply.ok, `create failed: ${JSON.stringify(evidence.reply)}`);
    assert(evidence.reply.result.delivery.status === "delivered", "false delivery receipt");
    assert(evidence.sends.length === 1 && evidence.sends[0].sessionId === evidence.reply.result.session_id && evidence.sends[0].content === "合成派单消息", "wrong session or duplicate send");
    assert(await page.getByRole("tab", { name: "关于", exact: true }).isVisible(), "background dispatch changed page");
    await page.screenshot({ path: path.join(artifacts, `settings-${theme}.png`), fullPage: true });
    await page.evaluate(() => window.__dispatchProbe.dispatch("switch", "switch_session", { session_id: "synthetic-existing" }));
    await page.getByRole("button", { name: "设置", exact: true }).waitFor();
    await page.waitForFunction(() => window.__dispatchProbe.replies.some((reply) => reply.requestId === "switch" && reply.ok));
    results.push({ theme, quota, ...evidence, switch_session: "pass" });
    await page.close();
  }
  const receipt = { status: "pass", environment: "real Chromium + production App/stores/handlers; synthetic IPC, not installed desktop", results };
  await writeFile(path.join(artifacts, "receipt.json"), JSON.stringify(receipt, null, 2));
  console.log(JSON.stringify(receipt, null, 2));
} finally {
  await browser?.close();
  server.kill("SIGTERM");
  await new Promise((resolve) => server.exitCode !== null ? resolve() : server.once("exit", resolve));
}
