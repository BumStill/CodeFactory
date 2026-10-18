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
    const status = await page.getByRole("status").nth(1).innerText();
    assert(/现在可用|当前不在允许时段/.test(status),`model status missing: ${status}`);
    assert(/无切换时间|下一次切换：/.test(status),`next transition missing: ${status}`);
    assert(status.includes("下一次切换："),`the preset model must show its next transition: ${status}`);
    assert(await page.getByLabel("deepseek pro 使用时段").inputValue() === "official_off_peak","pro preset missing");
    assert(await page.getByLabel("deepseek flash 使用时段").inputValue() === "anytime","flash must be unrestricted");
    await page.getByLabel("deepseek flash 使用时段").selectOption("custom");
    await page.getByLabel("flash 时区").waitFor();
    assert(await page.getByLabel("deepseek pro 使用时段").inputValue() === "official_off_peak","editing flash changed pro");
    assert(await page.getByLabel("deepseek flash 使用时段").inputValue() === "custom","custom did not persist");
    await page.getByRole("button", { name: "恢复继承服务商" }).last().click();
    await page.getByLabel("deepseek 服务商默认 使用时段").selectOption("official_off_peak");
    assert(await page.getByLabel("deepseek flash 使用时段").inputValue() === "official_off_peak", "inherited endpoint default missing");
    await page.getByRole("button", { name: "恢复继承服务商" }).first().click();
    assert(await page.getByText("deepseek / pro · 继承服务商").isVisible(), "pro did not restore inheritance");
    // CF-WINDOW-R5（M86 返修）：等待说明必须出现在**会话界面**上，不是只进日志。
    const waitNotice = page.getByTestId("usage-window-wait-notice");
    assert(await waitNotice.isVisible(),`${theme}: the usage-window wait notice is not visible in the session`);
    const waitText = await waitNotice.innerText();
    for (const expected of ["已达订阅用量上限","高峰时段","尚未配置凭据","最早","自动继续","不计入技术恢复次数"]) {
      assert(waitText.includes(expected),`${theme}: wait notice is missing 「${expected}」: ${waitText}`);
    }
    assert(await page.getByText(/模型连接不稳定/).count() === 0,`${theme}: a usage-window wait must not be described as an unstable connection`);
    // CF-WINDOW-R6（M86 返修）：提示是应用内对话框（含一次性会话豁免），不是原生弹窗。
    const windowDialog = page.getByTestId("usage-window-dialog");
    assert(await windowDialog.isVisible(),`${theme}: the in-app usage-window dialog is not visible`);
    assert(await page.getByRole("button",{name:/本会话忽略时段限制/}).isVisible(),`${theme}: the one-time per-session override option is missing`);
    await page.screenshot({path:path.join(artifacts,`wait-and-dialog-${theme}.png`),fullPage:true});
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
