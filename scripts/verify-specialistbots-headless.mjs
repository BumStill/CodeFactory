// SPDX-License-Identifier: Apache-2.0
// Self-hosted Vite + real browser, synthetic read-only IPC; not desktop E2E.
import { chromium } from 'playwright-core';
import { readFile, mkdir } from 'node:fs/promises';
import assert from 'node:assert/strict';
import { spawn, spawnSync } from 'node:child_process';
import { access, writeFile } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';
import { allocateLoopbackPort, waitForAcceptanceDocument } from './headless-acceptance-support.mjs';
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const output = process.env.SPECIALIST_ARTIFACT_DIR ?? path.join(process.env.RUNNER_TEMP ?? os.tmpdir(),'specialistbots-ui');
await mkdir(output, {recursive:true});
const preferred = process.env.CHROMIUM_PATH;
const candidates = preferred ? [preferred] : process.platform === 'win32' ? [
  path.join(process.env['PROGRAMFILES(X86)'] ?? 'C:\\Program Files (x86)','Microsoft/Edge/Application/msedge.exe'),
  path.join(process.env.PROGRAMFILES ?? 'C:\\Program Files','Google/Chrome/Application/chrome.exe'),
] : process.platform === 'darwin' ? ['/Applications/Google Chrome.app/Contents/MacOS/Google Chrome','/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge','/Applications/Chromium.app/Contents/MacOS/Chromium'] : ['/usr/bin/google-chrome','/usr/bin/chromium','/usr/bin/chromium-browser'];
let executablePath;
for (const candidate of candidates) { try { await access(candidate); executablePath=candidate; break; } catch {} }
if (!executablePath) throw new Error('No supported system Chrome/Edge available');
const port = await allocateLoopbackPort(1468);
const baseUrl = `http://127.0.0.1:${port}/specialistbots-acceptance.html`;
const vite = spawn(process.execPath,[path.join(root,'node_modules/vite/bin/vite.js'),'--host','127.0.0.1','--port',String(port),'--strictPort'],{cwd:root,detached:process.platform!=='win32',stdio:['ignore','pipe','pipe'],env:{...process.env,BROWSER:'none'}});
let viteLog='';
vite.stdout.on('data',chunk=>{viteLog=(viteLog+chunk.toString()).slice(-65536);});
vite.stderr.on('data',chunk=>{viteLog=(viteLog+chunk.toString()).slice(-65536);});
let browser;
try {
await waitForAcceptanceDocument(vite,baseUrl,'specialistbots.tsx');
browser = await chromium.launch({ executablePath, headless:true, chromiumSandbox:true });
const page = await browser.newPage();
const pageErrors = [];
page.on('pageerror', e => pageErrors.push(e.message));
await page.addInitScript(() => {
  let callback = 0;
  const key='specialist-readonly-synthetic';
  const projects=JSON.parse(sessionStorage.getItem(key) ?? '{}');
  const save=()=>sessionStorage.setItem(key,JSON.stringify(projects));
  const state=project=>projects[project] ?? (projects[project]={epoch:0,records:[]});
  const control=window.__SPECIALIST_FIXTURE__={deferQuery:false,releaseQuery:null,failQuery:false,failList:false,pendingQueries:0,settledQueries:0};
  window.__TAURI_INTERNALS__ = { transformCallback: () => ++callback, unregisterCallback: () => {}, invoke: async (cmd,args={}) => {
    if (cmd === 'plugin:event|listen') return ++callback;
    if (cmd === 'list_git_remotes') return [{id:'synthetic-github',name:'Synthetic GitHub',provider:'github',base_url:'https://api.github.com'}];
    if (cmd === 'list_specialist_observations') {
      if(control.failList){control.failList=false;throw new Error('synthetic list failure');}
      return JSON.parse(JSON.stringify(state(args.project)));
    }
    if (cmd === 'bind_specialist_project') {
      const current=state(args.project);
      if(args.expectedEpoch!==current.epoch || args.remoteId!=='synthetic-github' || args.repo!=='fixture/repo' || args.pr!==7)throw new Error('synthetic binding rejected');
      const binding={id:`${args.project}:${current.epoch}`,repo:args.repo,pr:7,repo_id:7,actor_id:9,head:'a'.repeat(40),role:'qa_read_pm_observe'};
      if(!current.records.some(r=>r.binding.id===binding.id))current.records.push({binding,active:true,observations:[]});
      save();return binding;
    }
    if (cmd === 'query_specialist_observation') {
      const current=state(args.project),epoch=current.epoch;
      const record=current.records.find(r=>r.binding.id===args.bindingId && r.active);
      if(!record)throw new Error('synthetic revoked binding');
      const deferred=control.deferQuery;
      if(deferred)control.pendingQueries++;
      try {
      if(deferred)await new Promise(resolve=>{control.releaseQuery=resolve;});
      if(current.epoch!==epoch || control.failQuery){control.failQuery=false;throw new Error('synthetic source or revocation failure');}
      if(!record.observations.length)record.observations.push({id:'synthetic-observation',observed_at:'2026-11-06T00:00:00Z',pm_state:'needs_execution_provenance',runs:[{id:42,attempt:1,status:'completed',conclusion:'success'}]});
      save();return record.observations[0];
      } finally {if(deferred){control.pendingQueries--;control.settledQueries++;}}
    }
    if (cmd === 'stop_specialist_observations') {
      const current=state(args.project);current.epoch++;current.records.forEach(r=>r.active=false);save();return;
    }
    if(cmd.includes('specialist'))throw new Error('unexpected specialist command: '+cmd);
    if (/list_|get_models/.test(cmd)) return [];
    if (cmd === 'get_git_status') return {is_repo:false, staged:[],unstaged:[],untracked:[]};
    if (cmd === 'get_message_page') return {messages:[],has_more:false};
    return null;
  }};
});
const fixture = JSON.parse(await readFile(path.join(root,'docs/acceptance/specialistbots-dm993-projection.json'),'utf8'));
const input = () => page.getByLabel('导入本地 QA/PM 报告');
for (const [width, theme] of [[1280,'dark'],[375,'light']]) {
  await page.setViewportSize({width,height:850});
  await page.goto(baseUrl);
  await page.evaluate(()=>sessionStorage.removeItem('specialist-readonly-synthetic'));
  await page.reload();
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
  const overflow = await page.getByRole('region',{name:'QA/PM 证据内容'}).evaluate(el => el.scrollWidth - el.clientWidth);
  assert(overflow <= 1, 'evidence content overflows horizontally');
  await page.keyboard.press('Escape');
  assert.equal(await opener.getAttribute('aria-expanded'),'false');
  assert(await opener.evaluate(el => document.activeElement === el));
  await opener.click();
  await input().setInputFiles({name:'forged.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify({...fixture,autonomous_dispatch_enabled:true}))});
  await page.getByRole('alert').filter({hasText:'无法显示报告：'}).waitFor();
  assert.equal(await page.getByText(fixture.pm.title,{exact:true}).count(),0);
  await input().setInputFiles({name:'real-993.json',mimeType:'application/json',buffer:Buffer.from(JSON.stringify(fixture))});
  await page.getByText('待补执行证明',{exact:true}).waitFor();
  // Synthetic project-change event while the report remains mounted. The overlay
  // intentionally prevents clicking background controls, so invoke only this harness control.
  await page.getByRole('button',{name:'验收：切换项目'}).evaluate(button => button.click());
  await opener.click();
  await page.getByText('/acceptance/project-b',{exact:true}).waitFor();
  assert.equal(await page.getByText(fixture.pm.title,{exact:true}).count(),0);
  assert.deepEqual(pageErrors, [], 'browser runtime errors');
  // Real DOM interactions against explicit synthetic IPC. Not live GitHub/Tauri.
  await page.goto(baseUrl);
  await page.evaluate(() => { sessionStorage.removeItem('specialist-readonly-synthetic'); });
  await page.reload();
  await page.evaluate(theme=>document.documentElement.setAttribute('data-theme',theme),theme);
  assert.equal(await page.evaluate(()=>document.documentElement.getAttribute('data-theme')),theme);
  await opener.click();
  const bind=page.getByRole('button',{name:'绑定并授权只读观察',exact:true});
  const query=page.getByRole('button',{name:'查询此 head',exact:true});
  const stop=page.getByRole('button',{name:'停止并撤销本项目查询授权',exact:true});
  const fillBinding=async()=>{
    await page.getByLabel('GitHub 连接').selectOption('synthetic-github');
    await page.getByLabel('仓库 owner/repo').fill('fixture/repo');
    await page.getByLabel('PR 编号',{exact:true}).fill('7');
    await bind.click();
    await page.getByText('仅授权只读取证',{exact:true}).waitFor();
  };
  await fillBinding();
  await query.click();
  await page.getByText('GitHub run 42 / attempt 1：completed / success',{exact:true}).waitFor();
  assert.equal(await page.getByText('待补执行证明',{exact:true}).count(),1);
  await page.keyboard.press('Escape');await opener.click();
  await page.getByText('GitHub run 42 / attempt 1：completed / success',{exact:true}).waitFor();
  await page.reload();
  await page.evaluate(theme=>document.documentElement.setAttribute('data-theme',theme),theme);
  assert.equal(await page.evaluate(()=>document.documentElement.getAttribute('data-theme')),theme);
  await opener.click();
  await page.getByText('GitHub run 42 / attempt 1：completed / success',{exact:true}).waitFor();
  await page.evaluate(()=>{window.__SPECIALIST_FIXTURE__.deferQuery=true;});
  await query.click();
  await page.waitForFunction(()=>typeof window.__SPECIALIST_FIXTURE__.releaseQuery==='function');
  await stop.click();
  await page.getByText('历史绑定：需重新授权',{exact:true}).waitFor();
  await page.evaluate(()=>{window.__SPECIALIST_FIXTURE__.deferQuery=false;window.__SPECIALIST_FIXTURE__.releaseQuery();});
  await page.waitForFunction(()=>window.__SPECIALIST_FIXTURE__.pendingQueries===0 && window.__SPECIALIST_FIXTURE__.settledQueries===1);
  // Let the rejected invocation reach the component catch/finally before asserting.
  await page.evaluate(()=>new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve))));
  assert(await query.isDisabled());
  assert.equal(await page.getByText('待补执行证明',{exact:true}).count(),1);
  await fillBinding();
  await page.evaluate(()=>{window.__SPECIALIST_FIXTURE__.failQuery=true;});
  const activeQuery=page.locator('article').filter({hasText:'仅授权只读取证'}).getByRole('button',{name:'查询此 head'});
  await activeQuery.click();
  await page.getByRole('alert').filter({hasText:'查询或授权未完成'}).waitFor();
  assert.equal(await page.getByText('GitHub run 42 / attempt 1：completed / success',{exact:true}).count(),1);
  await page.evaluate(()=>{window.__SPECIALIST_FIXTURE__.failList=true;});
  await stop.click();
  await page.getByText('正在处理只读观察…',{exact:true}).waitFor({state:'hidden'});
  await page.getByRole('alert').filter({hasText:'查询或授权未完成'}).waitFor();
  assert(await bind.isDisabled());
  assert.equal(await page.getByText('仅授权只读取证',{exact:true}).count(),0);
  for(const button of await query.all())assert(await button.isDisabled());
  assert.equal(await page.evaluate(()=>document.documentElement.getAttribute('data-theme')),theme);
  await page.screenshot({path:path.join(output,`${width}-${theme}-native-observation.png`),fullPage:true});
  await page.getByRole('button',{name:'验收：切换项目'}).evaluate(button=>button.click());
  await opener.click();
  await page.getByText('/acceptance/project-b',{exact:true}).waitFor();
  assert.equal(await query.count(),0);
  assert.equal(await page.getByText('待补执行证明',{exact:true}).count(),0);
  assert.deepEqual(pageErrors, [], 'browser runtime errors');
  console.log(JSON.stringify({width,theme,import:true,forgeryRejected:true,escapeFocus:true,projectReset:true,bindQuery:true,reopenSyntheticHistory:true,stopLateReply:true,sourceFailure:true,stopRefreshFailure:true,evidence:'browser-with-synthetic-ipc',liveTauri:false}));
}
} finally {
  try { await browser?.close(); }
  finally {
    try {
      const running=()=>vite.exitCode===null && vite.signalCode===null;
      const waitExit=async()=>{
        const deadline=Date.now()+2000;
        while(running() && Date.now()<deadline)await new Promise(resolve=>setTimeout(resolve,50));
      };
      if(running()){
        if(process.platform==='win32')spawnSync('taskkill',['/pid',String(vite.pid),'/t','/f'],{stdio:'ignore',timeout:5000});
        else { try{process.kill(-vite.pid,'SIGTERM');}catch{vite.kill('SIGTERM');} }
        await waitExit();
        if(running()){
          if(process.platform!=='win32'){try{process.kill(-vite.pid,'SIGKILL');}catch{vite.kill('SIGKILL');}}
          await waitExit();
          if(running())throw new Error('Vite process did not exit; cleanup unverified');
        }
      }
    } finally {await writeFile(path.join(output,'vite.log'),viteLog);}
  }
}
