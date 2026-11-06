import { spawn } from "node:child_process";
import { mkdir, access } from "node:fs/promises";
import { chromium } from "playwright-core";
import path from "node:path";
const root = process.cwd();
const artifacts = path.join(root,".codefactory-cache/window-screenshots");
const port = 1469;
const url = `http://127.0.0.1:${port}/settled-turn-status-acceptance.html`;
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
    for (const [scenario,expected] of [["settled",null],["waiting","waiting_wake"],["running","running"]]) {
      const page = await browser.newPage({viewport:{width,height:768}});
      await page.goto(`${url}?theme=${theme}&case=${scenario}`);
      await page.getByText("合成回答").waitFor();
      const box = await page.getByText("合成回答").boundingBox();
      assert(box && box.y >= 0 && box.y + box.height <= 768,"answer is not on screen, so the page proves nothing");
      const strip = page.getByTestId("thin-state");
      if (expected) assert(await strip.getAttribute("data-thin-state") === expected,`${scenario} lost its status`);
      else {
        await page.getByTestId("settled-turn-status").waitFor();
        assert(await strip.count() === 0,"settled turn still claims it is running");
        assert(await page.getByRole("button",{name:"重置会话状态"}).count() === 0,"settled turn still offers reset");
      }
      assert(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),`horizontal overflow at ${width}`);
      await page.screenshot({path:path.join(artifacts,`settled-turn-status-${scenario}-${theme}-${width}.png`),fullPage:true});
      await page.close();
    }
  }
  console.log("settled turn status headless acceptance passed");
} finally {
  await browser?.close();
  server.kill();
}
