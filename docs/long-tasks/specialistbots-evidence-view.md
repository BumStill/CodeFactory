# SpecialistBots 项目内证据查看切片

## Basics
- Task ID: CF-SPECIALISTBOTS-UI-S7
- Updated at: 2026-11-05
- Branch: feat/dot-admission-ledger
- Spec: docs/specs/feature-specs/specialistbots-evidence-view.md
- Related Req IDs: R1..R5

## Completion Standard
用户从现有项目工作区查看真实QA材料与本地PM待办，始终明确关闭自治；失败测试、独立QA、回归、本地提交。真实UI验收须另获浏览器证据。

## Current State
实现已落本地，70项回归、类型与构建通过，独立QA复验13项通过；浏览器环境阻塞，因此只交接待实地验收切片，不宣称完整验收或上线。

## Completed Items
- 现有Workspace辅助面板新增入口，显式文件选择，无后台文件扫描/IPC权限扩展。
- 解析展示schema及关闭状态，项目关联和执行证明明确未验证。
- 本地PM待办、限制与原head可见，不持久修改任务。
- 文件限额、异步竞态、项目变更、恶意文本回归。
- 独立QA发现普通对象继承键渲染漏洞，先红后修自有属性查询。

## Remaining Items
- 有效浏览器沙箱上的真实用户路径、窄屏/主题/焦点验证及Tauri桌面验收。
- 可信来源/项目岗位绑定、权限/隔离与执行签发、持久PM生命周期；本片不扩展这些能力。

## Blockers
Chromium SUID sandbox helper配置错误，启动即SIGABRT。未关闭沙箱或修改系统权限；浏览器脚本尚未执行到UI断言，不能把jsdom当实地验证。无新增外部写入权限需求。

## Evidence
详见 docs/acceptance/specialistbots-evidence-view.md。真实历史输入：docs/acceptance/specialistbots-dm993-projection.json。日志前缀 /workspace/s0-evidence/specialistbots-ui-*；浏览器失败 specialistbots-browser.log。

## AI Collaboration
- context scope: 现有项目UI/辅助面板、S6投影、仓库harness；无真实用户会话或凭据。
- assumptions: 本地导入仅格式展示，既不认证来源也不授权派发。
- review point: 独立 /root/review_admission 审查输入、真假状态、项目切换；继承键问题已按反馈修复并经复核通过。
- validation result: 以最终测试日志和门禁结果为准；浏览器未通过，自治关闭。

## Stop Boundary
只提交本地分支；不推送、合并、部署。保留浏览器及桌面未验标记，不将手动导入/证据转换称为新岗位自治。
