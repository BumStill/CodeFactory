import { spawn } from "node:child_process";
import { mkdir, access } from "node:fs/promises";
import { chromium } from "playwright-core";
import path from "node:path";
const root = process.cwd();
const artifacts = path.join(root,".codefactory-cache/window-screenshots");
const port = 1459;
const url = `http://127.0.0.1:${port}/model-usage-windows-acceptance.html`;
const assert = (condition, message) => { if (!condition) throw new Error(message); };
let browser;
let log = "";
const server = spawn(process.execPath,["node_modules/vite/bin/vite.js","--host","127.0.0.1","--port",String(port),"--strictPort"],{cwd:root,stdio:["ignore","pipe","pipe"]});
server.stdout.on("data",data => {log += data;});
server.stderr.on("data",data => {log += data;});
console.log(`server PID=${server.pid}; log captured in verifier output`);
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
  browser = await chromium.launch({executablePath,headless:true});
  for (const theme of ["light","dark"]) {
    const page = await browser.newPage({viewport:{width:1366,height:768}});
    await page.goto(`${url}?theme=${theme}`);
    await page.getByRole("heading",{name:"模型使用时段"}).waitFor();
    // CF-WINDOW-R3：没有官方节假日数据的年份必须被如实说明。
    assert(await page.getByText("已内置节假日／调休数据的年份：2026。其余年份按保守口径（工作日算高峰）。").isVisible(),"conservative fallback statement missing");
    // CF-WINDOW-R8：每个模型都要显示"现在是否可用、下一次切换时间"。
    const status = await page.getByRole("status").first().innerText();
    assert(/现在可用|当前不在允许时段/.test(status),`model status missing: ${status}`);
    assert(/无切换时间|下一次切换：/.test(status),`next transition missing: ${status}`);
    assert(status.includes("下一次切换："),`the preset model must show its next transition: ${status}`);
    assert(await page.getByLabel("deepseek pro 使用时段").inputValue() === "official_off_peak","pro preset missing");
    assert(await page.getByLabel("deepseek flash 使用时段").inputValue() === "anytime","flash must be unrestricted");
    await page.getByLabel("deepseek flash 使用时段").selectOption("custom");
    await page.getByLabel("flash 时区").waitFor();
    assert(await page.getByLabel("deepseek pro 使用时段").inputValue() === "official_off_peak","editing flash changed pro");
    assert(await page.getByLabel("deepseek flash 使用时段").inputValue() === "custom","custom did not persist");
    for (const width of [1366,375]) {
      await page.setViewportSize({width,height:768});
      assert(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth + 1),`${theme}/${width} horizontal overflow`);
      await page.screenshot({path:path.join(artifacts,`windows-${theme}-${width}.png`),fullPage:true});
    }
    await page.close();
  }
  console.log(JSON.stringify({status:"pass",artifacts}));
} finally {
  if (browser) await browser.close();
  server.kill("SIGTERM");
  await new Promise(resolve => { if(server.exitCode !== null) resolve(); else {server.once("exit",resolve);setTimeout(() => {server.kill("SIGKILL");resolve();},3000).unref();} });
  console.log(log);
}
