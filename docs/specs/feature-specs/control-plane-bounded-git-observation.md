# ControlPlane 有界 Git 观察规格

## 范围

本规格定义 CodeFactory AI Coding OS ControlPlane 对项目 Git 状态的有界观察和降级行为。目标是让慢仓库、网络盘、损坏仓库或异常 Git 进程只影响 Delivery 数据的完整性，不得让整个控制面永久停在“加载控制面…”。

这项能力位于 CodeFactory 正常产品路径，不读取 Terminal-Bench task name、runner artifact 或 benchmark 专用配置。

## Requirements Traceability

| Req ID | Normalized requirement | Surface | Validation method | Owner |
| --- | --- | --- | --- | --- |
| CF-CP-GIT-R1 | 所有 ControlPlane Git 子进程必须异步执行，每个探测最多 2000ms | Tauri backend | Tokio timeout tests + elapsed assertion | development |
| CF-CP-GIT-R2 | 超时必须显式 kill 并 wait 回收 Git 进程树（含仍持有输出管道的后代进程），不能留下后台 Git 继续占用仓库或资源 | process lifecycle | sentinel child cleanup test + implementation review | QA |
| CF-CP-GIT-R3 | 单个 Git 探测失败或超时仍返回 Authority、Memory、Capabilities、filesystem delivery fields 和可用 Git 字段 | snapshot API | partial probe unit test + real app check | development |
| CF-CP-GIT-R4 | API 必须区分 `ok`、`partial`、`not_repository`、`unavailable`、`not_checked`，并列出 timed-out/failed probe names | Tauri contract | serialization and classifier tests | QA |
| CF-CP-GIT-R5 | UI 必须显示“Git 状态部分可用”及风险，不能把 timeout/unavailable 误显示为 `not a git repo` | ControlPlane page | React test + real app timeout path | QA |
| CF-CP-GIT-R6 | 刷新期间保留上一份快照；超时降级完成后停止 spinner，用户可以再次刷新恢复 | ControlPlane page | refresh interaction + real app recovery | QA |
| CF-CP-GIT-R7 | 不新增持久化字段或 migration；旧 snapshot fixture 缺少 `git_probe` 时前端仍能显示 legacy fallback | compatibility | React compatibility test | development |

## Primary User Paths

1. 正常仓库：用户打开 ControlPlane，Git observation 显示 `complete`，branch、dirty tree、hook 和 latest tag 正常显示。
2. 慢或挂起的 Git：页面在有界时间内显示其余控制面数据，Git observation 显示 `partial`，Risks 列出 timed-out probe；spinner 停止。
3. 非 Git 目录：页面显示 `not a git repository`，不把它归类为 timeout 或 app error。
4. 恢复：Git 恢复正常后用户点击刷新，partial risk 消失，Git observation 回到 `complete`。

## Applicable Harnesses

- Spec Harness：本规格、Req ID、测试矩阵和证据随实现提交。
- Compatibility Harness：API 只做 additive contract，不新增数据库 migration。
- Observation Harness：risk 必须带失败类别和 probe name，不能只显示模糊错误。
- Viewport Harness：partial risk、Delivery rows 和刷新状态在真实窗口中不得遮挡或溢出。
- Release Harness：发布后在安装版或 dev app 走正常、超时和恢复路径。
- AI Collaboration Harness：独立后端与 QA 角色复核 timeout、process cleanup 和用户状态语义。

## Testing Matrix

| Scenario | Expected result | Evidence |
| --- | --- | --- |
| child process exceeds timeout | returns timeout before budget and child cannot write delayed sentinel | Rust async test |
| normal Git repo | `git_probe.status=ok`; available fields populated | Rust repo fixture |
| non-Git directory | `git_probe.status=not_repository`; snapshot succeeds | Rust temp directory fixture |
| one probe timeout | `git_probe.status=partial`; timeout probe listed; filesystem fields retained | Rust probe aggregation test |
| Git executable unavailable | `git_probe.status=unavailable`; explicit risk | classifier test |
| partial snapshot render | header and Delivery row say partial; risk visible; loading gone | React test |
| recovered refresh | next complete snapshot replaces partial snapshot | React interaction/live app |

## Product Boundary

这项修复提升的是 CodeFactory 控制面和长任务工作流的可靠性，不直接提升模型推理或 Terminal-Bench 分数。非评测示例：大型 monorepo 的 `git status` 卡住时，用户仍能看到规则、记忆、能力和发布 workflow 状态，并知道只有 Git delivery observation 不完整。

## AI Collaboration Record

- context scope: ControlPlane Tauri command、React page、现有 AI Coding OS 业务/架构/UX 设计和真实 dev app 路径。
- assumptions: 单探测 2000ms 足以覆盖正常本地仓库；多个独立 probe 应并行，避免 timeout 累加。
- review point: timeout 后必须显式 kill/wait 整个进程树，`kill_on_drop` 只能兜底；UI 必须区分 timeout 与 non-repository。
- validation result: RED 已复现永久 loading、错误 clean/not-configured 结论和后代进程持管道问题；GREEN 已通过前后端回归、真实 app 正常/超时/进程清理/恢复刷新路径。PR CI 和 release proof 仍由交付流程验证。
