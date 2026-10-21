// SPDX-License-Identifier: Apache-2.0
import { spawnSync } from 'node:child_process';
import { mkdtemp, writeFile, rm, access } from 'node:fs/promises';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { chromium } from 'playwright-core';
import assert from 'node:assert/strict';
const root = process.cwd();
const site = await mkdtemp(path.join(tmpdir(), 'cf-m144-'));
const artifacts = process.env.M144_ARTIFACT_DIR ?? path.join(tmpdir(), 'codefactory-m144-evidence');
const { mkdir, readdir } = await import('node:fs/promises');
await mkdir(artifacts, { recursive:true });
function run(cmd, args) {
  const result = spawnSync(cmd,args,{cwd:root,encoding:'utf8'});
  assert.equal(result.status,0,result.stderr || result.stdout);
}
let server, browser;
try {
  const packages = await readdir('node_modules/.pnpm');
  const esbuildPackage = packages.find(name => name.startsWith('esbuild@'));
  assert.ok(esbuildPackage);
  const { buildSync } = await import(`../node_modules/.pnpm/${esbuildPackage}/node_modules/esbuild/lib/main.js`);
  buildSync({entryPoints:['src/acceptance/m144-storage.tsx'],bundle:true,format:'esm',platform:'browser',jsx:'automatic',loader:{'.css':'empty'},outfile:`${site}/app.js`});
  run(process.execPath,['node_modules/tailwindcss/lib/cli.js','-i','src/styles/globals.css','-o',`${site}/styles.css`]);
  await writeFile(`${site}/index.html`,'<!doctype html><html data-theme="light"><head><meta charset="UTF-8"><link rel="stylesheet" href="styles.css"></head><body><div id="root"></div><script type="module" src="app.js"></script></body></html>');
  server=createServer(async(req,res)=>{try { const name=req.url==='/'?'index.html':req.url.slice(1); assert.ok(['index.html','app.js','styles.css'].includes(name)); res.setHeader('Content-Type',name.endsWith('.js')?'text/javascript':name.endsWith('.css')?'text/css':'text/html'); res.end(await readFile(path.join(site,name))); } catch {res.writeHead(404);res.end();}});
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  const candidates = process.platform==='darwin' ? ['/Applications/Google Chrome.app/Contents/MacOS/Google Chrome','/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge'] : process.platform==='win32' ? [`${process.env.PROGRAMFILES}/Google/Chrome/Application/chrome.exe`,`${process.env['PROGRAMFILES(X86)']}/Microsoft/Edge/Application/msedge.exe`] : ['/usr/bin/google-chrome','/usr/bin/chromium'];
  let executablePath;
  for(const candidate of candidates){try{await access(candidate);executablePath=candidate;break;}catch{}}
  assert.ok(executablePath,'system browser required');
  browser=await chromium.launch({executablePath,headless:true,args:['--no-sandbox','--no-proxy-server']});
  const page=await browser.newPage({viewport:{width:1100,height:800}});
  await page.goto(`http://127.0.0.1:${server.address().port}/`);
  for(const theme of ['light','dark']){
    await page.evaluate(theme=>document.documentElement.dataset.theme=theme,theme);
    await page.getByText('本地数据暂时无法保存，本次操作未完成；已有内容已保留。',{exact:true}).waitFor();
    assert.equal(await page.getByText('已停止。已有内容和本地改动已保留。',{exact:true}).count(),1);
    const text=await page.locator('main').innerText();
    assert.doesNotMatch(text,/database is locked|code: 5|编排方|PR/);
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),true);
    await page.screenshot({path:path.join(artifacts,`m144-${theme}.png`)});
  }
  console.log(`M144 browser acceptance PASS; screenshots: ${artifacts}`);
} finally {await browser?.close();await new Promise(resolve=>server?server.close(resolve):resolve());await rm(site,{recursive:true,force:true});}
