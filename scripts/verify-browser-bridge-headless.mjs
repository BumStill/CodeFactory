// SPDX-License-Identifier: Apache-2.0
import { mkdtemp, readFile, readdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { chromium } from 'playwright-core';
const root = process.cwd();
const artifacts = await mkdtemp(join(tmpdir(), 'm136-viewport-'));
const modules = join(root, 'node_modules/.pnpm');
const name = (await readdir(modules)).find(n => /^esbuild@/.test(n));
const { build } = await import(join(modules, name, 'node_modules/esbuild/lib/main.js'));
await build({ entryPoints: ['src/acceptance/browser-bridge.tsx'], outfile: join(artifacts, 'app.js'), bundle: true, format: 'iife', jsx: 'automatic', plugins: [{ name: 'synthetic-tauri', setup(b) {
  b.onResolve({ filter: /^@tauri-apps\/api\/(core|event)$/ }, a => ({ path: a.path, namespace: 'fixture' }));
  b.onLoad({ filter: /.*/, namespace: 'fixture' }, () => ({ contents: `export class Resource {} export class Channel {} export async function invoke(command) { if(command==='browser_bridge_reconnect') window.reconnected=true; return {dir:'C:/Synthetic/extension',port:47615,token:'SYNTHETIC-SECRET-DO-NOT-COPY',connected:false,chrome_available:true,diagnostics:{events:[{at:1,side:'app',event:'heartbeat_timeout',code:4002,reason:'heartbeat timeout'}]}} } export async function listen(){return ()=>{}};`, loader: 'js' }));
} }] });
const css = spawnSync(process.execPath, ['node_modules/tailwindcss/lib/cli.js', '-i', 'src/styles/globals.css', '-o', join(artifacts, 'style.css')], { encoding: 'utf8' });
if (css.status !== 0) throw new Error(css.stderr);
await writeFile(join(artifacts, 'index.html'), `<html><meta charset="utf-8"><link rel="stylesheet" href="style.css"><div id="root"></div><script src="app.js"></script></html>`);
const browser = await chromium.launch({ executablePath: process.env.CODEFACTORY_CHROME, headless: true });
try {
  const page = await browser.newPage();
  for (const theme of ['light','dark']) for (const width of [1366,768]) {
    await page.setViewportSize({width,height:768});
    await page.goto('file://' + join(artifacts,'index.html'));
    await page.evaluate(t => {document.documentElement.dataset.theme=t; Object.defineProperty(navigator,'clipboard',{value:{writeText:async value=>{window.copied=value}}});}, theme);
    await page.getByRole('button',{name:'复制诊断信息'}).click();
    const copied = await page.evaluate(()=>window.copied);
    if (!copied || copied.includes('SYNTHETIC-SECRET') || JSON.parse(copied).events[0].code !== 4002) throw new Error('diagnostics not redacted or wrong event');
    await page.getByRole('button',{name:'重新连接'}).click();
    if (!await page.evaluate(()=>window.reconnected)) throw new Error('reconnect not invoked');
    await page.getByText('最近 50 条连接事件').click();
    if (!await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth)) throw new Error('horizontal overflow');
    await page.screenshot({path:join(artifacts,`${theme}-${width}.png`),fullPage:true});
  }
  console.log('PASS: real production panel diagnostics/reconnect, light/dark, 1366/768; screenshots:', artifacts);
} finally { await browser.close(); }
