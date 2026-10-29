// SPDX-License-Identifier: Apache-2.0
import { chromium } from 'playwright-core';
import { spawn } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
import { allocateLoopbackPort, waitForAcceptanceDocument } from './headless-acceptance-support.mjs';
const artifacts = '/tmp/codefactory-m198-permission';
await mkdir(artifacts, { recursive: true });
const port = await allocateLoopbackPort(1458);
const url = `http://127.0.0.1:${port}/m198-permission-acceptance.html`;
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', String(port), '--strictPort'], { stdio: ['ignore', 'pipe', 'pipe'] });
let log = ''; server.stdout.on('data', d => log += d); server.stderr.on('data', d => log += d);
console.log(JSON.stringify({ service_pid: server.pid, log: `${artifacts}/vite.log` }));
let browser;
try {
  await waitForAcceptanceDocument(server, url, 'm198-permission.tsx');
  browser = await chromium.launch({ executablePath: process.env.CHROME_PATH ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', headless: true });
  const page = await browser.newPage();
  for (const theme of ['light', 'dark']) for (const width of [1366, 390]) {
    await page.setViewportSize({ width, height: 768 });
    await page.goto(url);
    await page.evaluate(theme => document.documentElement.dataset.theme = theme, theme);
    const reason = page.getByTestId('permission-reason');
    await reason.waitFor({ state: 'visible' });
    assert.match(await reason.innerText(), /越界写入需要确认/);
    assert.match(await reason.innerText(), /trusted.heredoc.unproved/);
    for (const locator of [reason, page.getByRole('button', { name: '仅允许一次', exact: true }), page.getByRole('button', { name: '拒绝', exact: true })]) {
      const box = await locator.boundingBox();
      assert(box && box.x >= 0 && box.x + box.width <= width && box.y >= 0 && box.y + box.height <= 768);
    }
    assert.equal(await page.getByRole('button', { name: '信任本会话并允许' }).count(), 0);
    await page.screenshot({ path: `${artifacts}/reason-${theme}-${width}.png` });
  }
  console.log(JSON.stringify({ status: 'pass', artifacts, checks: ['human-reason', 'rule', 'light-dark', 'desktop-narrow', 'buttons-visible'] }));
} finally { if (browser) await browser.close(); server.kill(); await writeFile(`${artifacts}/vite.log`, log); }
