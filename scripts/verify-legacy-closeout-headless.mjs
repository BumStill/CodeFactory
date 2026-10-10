// SPDX-License-Identifier: Apache-2.0
import { chromium } from 'playwright-core';
import { spawn } from 'node:child_process';
import { mkdir, writeFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
const artifacts = '/tmp/codefactory-legacy-closeout';
await mkdir(artifacts, { recursive: true });
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', '--host', '127.0.0.1', '--port', '1457', '--strictPort'], { stdio: ['ignore','pipe','pipe'] });
let log = ''; server.stdout.on('data', d => log += d); server.stderr.on('data', d => log += d);
console.log(JSON.stringify({ service_pid: server.pid, log: `${artifacts}/vite.log` }));
let browser;
try {
  for (let i=0; i<100; i++) { try { if ((await fetch('http://127.0.0.1:1457')).ok) break; } catch {} await new Promise(r=>setTimeout(r,100)); }
  browser = await chromium.launch({ executablePath: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', headless: true });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
  await page.goto('http://127.0.0.1:1457/legacy-closeout-acceptance.html');
  for (const theme of ['light', 'dark']) {
    await page.evaluate(theme => document.documentElement.dataset.theme = theme, theme);
    await page.getByRole('dialog').waitFor();
    assert.equal(await page.getByRole('button', { name: '信任本会话并允许' }).count(), 0);
    assert.equal(await page.getByLabel('等待批准').count(), 1);
    await page.screenshot({ path: `${artifacts}/approval-${theme}.png` });
    await page.getByRole('button', { name: '切到 B', exact: true }).evaluate(b => b.click());
    await page.waitForFunction(() => document.querySelector('[data-testid="active-session"]').textContent === 'B');
    assert.equal(await page.getByRole('dialog').count(), 0);
    assert.equal(await page.getByLabel('等待批准').count(), 1);
    await page.getByRole('button', { name: '模拟合并刷新' }).click();
    const status = page.getByRole('button', { name: /会话交付状态/ });
    assert.match(await status.innerText(), /已合并/);
    assert.doesNotMatch(await status.innerText(), /CI 失败/);
    assert.notEqual(await status.getAttribute('data-status-tone'), 'danger');
    await page.screenshot({ path: `${artifacts}/merged-${theme}.png` });
    await page.getByRole('button', { name: '切回 A', exact: true }).click();
    await page.getByRole('dialog').waitFor();
    assert.match(await page.getByRole('dialog').innerText(), /printf synthetic/);
  }
  console.log(JSON.stringify({ status: 'pass', artifacts, checks: ['switch-away-back-retains-exact-approval', 'sidebar-waiting-mark', 'trusted-prompt-no-retrust', 'merged-beats-old-ci', 'light-dark'] }));
} finally { if (browser) await browser.close(); server.kill(); await writeFile(`${artifacts}/vite.log`, log); }
