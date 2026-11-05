// SPDX-License-Identifier: Apache-2.0
// Run with Vite on 1468. Production Workspace + synthetic IPC; not desktop E2E.
import { chromium } from 'playwright-core';
import { readFile, mkdir } from 'node:fs/promises';
import assert from 'node:assert/strict';
const output = process.env.SPECIALIST_ARTIFACT_DIR ?? '/tmp/specialistbots-ui';
await mkdir(output, {recursive:true});
const browser = await chromium.launch({ executablePath: process.env.CHROMIUM_PATH ?? '/usr/bin/chromium', headless:true, chromiumSandbox:true });
try {
const page = await browser.newPage();
page.on('pageerror', e => console.error('PAGEERROR', e.message));
await page.addInitScript(() => {
  let callback = 0;
  window.__TAURI_INTERNALS__ = { transformCallback: () => ++callback, unregisterCallback: () => {}, invoke: async (cmd) => {
    if (cmd === 'plugin:event|listen') return ++callback;
    if (/list_|get_models/.test(cmd)) return [];
    if (cmd === 'get_git_status') return {is_repo:false, staged:[],unstaged:[],untracked:[]};
    if (cmd === 'get_message_page') return {messages:[],has_more:false};
    return null;
  }};
});
const fixture = JSON.parse(await readFile('docs/acceptance/specialistbots-dm993-projection.json','utf8'));
const input = () => page.getByLabel('导入本地 QA/PM 报告');
for (const [width, theme] of [[1280,'dark'],[375,'light']]) {
  await page.setViewportSize({width,height:850});
  await page.goto('http://127.0.0.1:1468/specialistbots-acceptance.html');
  await page.evaluate(theme => document.documentElement.setAttribute('data-theme',theme),theme);
  const opener = page.getByRole('button',{name:'QA/PM 证据',exact:true});
  await opener.click();
  await input().setInputFiles({name:'real-993.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify(fixture))});
  await page.getByText('待补执行证明',{exact:true}).waitFor();
  assert(await page.getByText('自主执行关闭',{exact:true}).isVisible());
  assert(await page.getByText(fixture.pm.title,{exact:true}).isVisible());
  assert(await page.getByText(/当前项目关联未验证/).isVisible());
  await page.screenshot({path:`${output}/${width}-${theme}.png`,fullPage:true});
  const geometry = await page.getByRole('region',{name:'QA/PM 证据内容'}).boundingBox();
  assert(geometry.x >= 0 && geometry.x + geometry.width <= width+1);
  await page.keyboard.press('Escape');
  assert.equal(await opener.getAttribute('aria-expanded'),'false');
  assert(await opener.evaluate(el => document.activeElement === el));
  await opener.click();
  await input().setInputFiles({name:'forged.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify({...fixture,autonomous_dispatch_enabled:true}))});
  await page.getByRole('alert').waitFor();
  assert.equal(await page.getByText(fixture.pm.title,{exact:true}).count(),0);
  await input().setInputFiles({name:'real-993.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify(fixture))});
  await page.getByText('待补执行证明',{exact:true}).waitFor();
  await page.keyboard.press('Escape');
  await page.getByRole('button',{name:'验收：切换项目'}).click();
  await opener.click();
  assert.equal(await page.getByText(fixture.pm.title,{exact:true}).count(),0);
  console.log(JSON.stringify({width,theme,import:true,forgeryRejected:true,escapeFocus:true,projectReset:true}));
}
} finally {await browser.close();}
