## M121 家族修复与 CI 追踪

R1：已枚举 30 个 headless 脚本；14 天完整失败统计尚未完成，不宣称已完成家族归因。已知失败 run：39143263152、39184049777、39189448369、39199076473、39202990975。最近一次 Windows Rust 失败仍为空页面，否定了此前仅凭 URL/readyState 的充分性假设。

R2：共享 gotoScenario/waitForAppBoot 提供页面加载、地标等待及 DOM/console/pageerror/requestfailed 诊断；启动后的产品断言保留 10s。60s 启动预算仍待 Windows 数据支持，不能用本机 35–42ms 推导其合理性。

R3：实际依赖 chromiumoxide 0.9.1 中 new_page 仅创建 target，goto 提供完整导航语义。生产 open 改为先创建 about:blank，再显式 goto 目标一次，等待导航完成后读取；失败回收，不重放目标导航。原有 thin_browser_open 内容断言未削弱。

R4：最终 revision 的 Windows 重复 ×5 证据待完成；不能用旧 head 或本机重复替代。

R5：工作流、场景注册表及验证器保持 base 原样，不增加 continue-on-error，不绕过门禁。

## 验证与需求映射

- R3：browser_open_ 四项真实浏览器测试在最新生产修改后通过；覆盖原始 receipt-free、延迟正文、重复打开内容与授权行为。
- R2：src/acceptance/headlessReadiness.test.ts 七项 helper/家族守卫；完整前端历史结果 1024 pass，最终 head 由 CI 重验。
- 历史全量 Rust 1768 pass 属上一个 revision，不冒充本次最终源码验证。
- R1/R4 仍待验收，不声称任务完成。

## Applicable Harnesses / AI Collaboration

Spec、Compatibility、Observation、AI Collaboration：上下文限定 browser 驱动、session 测试与 headless 支持脚本；假设由真实 Windows CI 反证后修正；review point 为导航完成因果关系与不重放；validation result 为最新四项真实浏览器测试通过、Windows 尚待验证。未启用并行实现，因生产导航与单一 PR 修复链存在顺序依赖。

真机验收由编排方发版后完成：打开受管浏览器，确认首次返回标题与正文、错误页面不误报目标内容、关闭后无残留进程。本任务只授权合并，不发版。

Scenario-Test: HLT-005, RTE-001, RTE-003
README-Update: reviewed
README-Update-Reason: 内部浏览器导航正确性与验收辅助脚本修复，不改变安装、隐私、平台或公开功能承诺。
