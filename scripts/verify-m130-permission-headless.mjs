// SPDX-License-Identifier: Apache-2.0
import { chromium } from "playwright-core";
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import assert from "node:assert/strict";
import { allocateLoopbackPort, waitForAcceptanceDocument } from "./headless-acceptance-support.mjs";
const artifacts = "/tmp/codefactory-m130-permission";
await mkdir(artifacts, { recursive: true });
const port = await allocateLoopbackPort(1458);
const url = `http://127.0.0.1:${port}/m130-permission-acceptance.html`;
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", String(port), "--strictPort"], { stdio: ["ignore", "pipe", "pipe"] });
let log = ""; server.stdout.on("data", d => log += d); server.stderr.on("data", d => log += d);
console.log(JSON.stringify({ service_pid: server.pid, log: `${artifacts}/vite.log` }));
let browser;
try {
  await waitForAcceptanceDocument(server, url, "m130-permission.tsx");
  browser = await chromium.launch({ executablePath: process.env.CHROME_PATH ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", headless: true });
  const page = await browser.newPage();
  for (const theme of ["light", "dark"]) for (const width of [1366, 390]) {
    await page.setViewportSize({ width, height: 768 });
    await page.goto(url);
    await page.evaluate(theme => document.documentElement.dataset.theme = theme, theme);
    const dialog = page.getByRole("dialog"); await dialog.waitFor();
    assert.match(await dialog.innerText(), /允许向远端推送并创建 PR \/ MR 吗/);
    assert.equal(await page.getByRole("button", { name: "信任本会话并允许" }).count(), 0);
    const allow = page.getByRole("button", { name: "允许交付", exact: true });
    const box = await allow.boundingBox();
    assert(box && box.x >= 0 && box.x + box.width <= width && box.y + box.height <= 768);
    await page.screenshot({ path: `${artifacts}/approval-${theme}-${width}.png` });
    await allow.click();
    await page.getByTestId("decision").filter({ hasText: "delivery_authorized" }).waitFor();
    assert.equal(await dialog.count(), 0);
    await page.reload(); await page.getByRole("dialog").waitFor();
    await page.getByRole("button", { name: "拒绝", exact: true }).click();
    await page.getByTestId("decision").filter({ hasText: "local_only" }).waitFor();
    const picker = page.getByRole("button", { name: /会话权限：信任/ });
    await picker.click();
    const toggle = page.getByRole("menuitemcheckbox", { name: /允许交付/ });
    assert.equal(await toggle.getAttribute("aria-checked"), "false");
    await page.screenshot({ path: `${artifacts}/switch-${theme}-${width}.png` });
    await toggle.click();
    await page.waitForFunction(() => window.__m130Authorized === true);
    await picker.click();
    assert.equal(await toggle.getAttribute("aria-checked"), "true");
    await toggle.click();
    await page.waitForFunction(() => window.__m130Authorized === false);
  }
  console.log(JSON.stringify({ status: "pass", artifacts, checks: ["session-delivery-question", "allow", "deny", "no-trust-bypass", "light-dark", "desktop-narrow"] }));
} finally { if (browser) await browser.close(); server.kill(); await writeFile(`${artifacts}/vite.log`, log); }
