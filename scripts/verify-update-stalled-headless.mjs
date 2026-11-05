import { spawn } from "node:child_process";
import { mkdir, access } from "node:fs/promises";
import { chromium } from "playwright-core";
import path from "node:path";
const root = process.cwd();
const artifacts = path.join(root,".codefactory-cache/window-screenshots");
const port = 1468;
const url = `http://127.0.0.1:${port}/update-stalled-acceptance.html`;
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
    for (const scenario of ["quiet","error"]) {
      const page = await browser.newPage({viewport:{width,height:768}});
      await page.goto(`${url}?theme=${theme}&case=${scenario}`);
      await page.getByText("更新没有开始").waitFor();
      assert(await page.getByText(/本地执行仍在运行/).count() === 0,"stalled update still claims running work");
      assert(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),`horizontal overflow at ${width}`);
      await page.getByRole("button",{name:"手动下载"}).waitFor();
      await page.screenshot({path:path.join(artifacts,`update-stalled-${scenario}-${theme}-${width}.png`),fullPage:true});
      await page.getByRole("button",{name:"重试"}).click();
      await page.waitForFunction(() => window.updateCalls.includes("reserve_update_install"));
      const calls = await page.evaluate(() => window.updateCalls);
      assert(calls.indexOf("retry_update_install") >= 0 && calls.indexOf("retry_update_install") < calls.lastIndexOf("reserve_update_install"),`retry order wrong: ${calls}`);
      await page.getByText("更新没有开始").waitFor({state:"detached"});
      await page.close();
    }
  }
  console.log("update stalled headless acceptance passed");
} finally {
  await browser?.close();
  server.kill();
}
