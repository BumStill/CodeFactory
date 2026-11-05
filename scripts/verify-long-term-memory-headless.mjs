import { spawn } from "node:child_process";
import { mkdir, access } from "node:fs/promises";
import { chromium } from "playwright-core";
import path from "node:path";
const root = process.cwd();
const artifacts = path.join(root,".codefactory-cache/window-screenshots");
const port = 1467;
const url = `http://127.0.0.1:${port}/long-term-memory-acceptance.html`;
const assert = (condition, message) => { if (!condition) throw new Error(message); };
let browser;
let log = "";
const server = spawn(process.execPath,["node_modules/vite/bin/vite.js","--host","127.0.0.1","--port",String(port),"--strictPort"],{cwd:root,stdio:["ignore","pipe","pipe"]});
server.stdout.on("data",data => {log += data;});
server.stderr.on("data",data => {log += data;});
try {
  await mkdir(artifacts,{recursive:true});
  let ready = false;
  for (let i=0;i<100;i++) {
    if (server.exitCode !== null) throw new Error(`Vite exited: ${log}`);
    try { if ((await fetch(url)).ok) {ready=true;break;} } catch {}
    await new Promise(resolve => setTimeout(resolve,200));
  }
  assert(ready,"Vite readiness timeout");
  let executablePath;
  for (const candidate of ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome","/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge","/usr/bin/chromium","/usr/bin/google-chrome"]) {
    try {await access(candidate); executablePath=candidate;break;} catch {}
  }
  assert(executablePath,"No installed browser found");
  browser = await chromium.launch({executablePath,headless:true,args:process.env.CI ? ["--no-sandbox"] : []});
  for (const [theme,width] of [["light",1366],["dark",1366],["light",375],["dark",375]]) {
    const page = await browser.newPage({viewport:{width,height:768}});
    await page.goto(`${url}?theme=${theme}`);
    await page.getByText("这个项目的回复结尾不要总结").waitFor();
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),`horizontal overflow at ${width}`);
    await page.getByRole("button",{name:"查看来源"}).first().click();
    await page.getByText("记住：回复用中文").waitFor();
    await page.getByRole("button",{name:"修改"}).first().click();
    await page.getByLabel("修改记忆内容").fill("token=synthetic");
    await page.getByRole("button",{name:"保存"}).click();
    await page.getByRole("alert").waitFor();
    await page.getByLabel("修改记忆内容").fill("回复一律用中文");
    await page.getByRole("button",{name:"保存"}).click();
    await page.getByText("回复一律用中文").waitFor();
    await page.screenshot({path:path.join(artifacts,`long-term-memory-${theme}-${width}.png`),fullPage:true});
    await page.getByRole("button",{name:"删除"}).nth(1).click();
    await page.getByRole("button",{name:"确认删除"}).click();
    await page.getByText("这个项目的回复结尾不要总结").waitFor({state:"detached"});
    await page.getByRole("checkbox",{name:"使用长期记忆"}).uncheck();
    assert(!(await page.getByRole("checkbox",{name:"使用长期记忆"}).isChecked()),"switch did not turn off");
    await page.close();
  }
  console.log("long-term memory headless acceptance passed");
} finally {
  await browser?.close();
  server.kill();
}
