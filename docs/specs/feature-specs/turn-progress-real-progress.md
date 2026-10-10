# M37：顶部进度条真实进度与中性后台检查

## 背景与用户决策
任务运行中全 pending 的计划曾显示 `0/5 0%`，后台补检查曾显示警告和「验证证据不足」。这会把正常执行误报为无进展或故障。后台控制回路只影响下一步执行，不应泄漏到 UI；优先删除无意义 UI，不增加指标。

## 决策
- 与 TurnResultSnapshot 的 planTracked 规则一致：任一步非 pending 才显示已完成/总数、百分比及进度条。
- 未跟踪计划只显示已用时间与当前活动，不编造文件/检查计数。
- 后端段落检查点继续执行时发布「正在补跑检查」，waiting reason 为 None。
- 兼容旧会话的中文原因与 completion_evidence_incomplete：过滤文本，不触发警告或终止状态。
- 保留授权、业务决策和真正停止的原有语义与警告。

## Requirements Traceability
| Req ID | 要求 | 最低证据 |
| --- | --- | --- |
| M37-R1 | 未跟踪计划无假计数、假百分比；跟踪计划保留真计数 | TurnProgress 组件测试；真实浏览器未跟踪/跟踪状态 |
| M37-R2 | 后台补检查不出现内部用语或警告色；源头无等待原因 | completion_checkpoint_activity_is_not_a_warning；组件与旧会话集成测试；浏览器 |
| M37-R3 | 授权/决策/真正停止保留警告 | TurnProgress 参数化警告测试；浏览器授权场景 |
| M37-R4 | 不新增内部用语 | 既有状态条词汇守卫与 M37 黑名单测试；浏览器禁词断言 |
| M37-R5 | 浅深色及窄视口无横向溢出 | Chrome headless 字段与布局断言、浅深色截图 |

## Applicable Harnesses
Spec Harness：本规格和需求映射。Compatibility Harness：已持久化旧等待原因与新代码兼容。Viewport Harness：真实浏览器、1100px 与 430px、浅深主题。Observation Harness：真实 DOM 的文本、tone、aria-valuenow、overflow。AI Collaboration Harness：单执行流审查恢复补丁、记录假设与失败修复。Release Harness：仅创建 PR，不合并或发版。

## 主路径与测试矩阵
用户打开任务会话：全 pending 计划显示当前活动与已用时间；步骤被跟踪后显示实际比例；后台补检查仍为普通进行中；需要用户授权/决策时仍警告。

| 场景 | 单元/集成 | 真实浏览器 |
| --- | --- | --- |
| 全 pending 五步 | TurnProgress 隐藏计数与 progressbar | 未跟踪区域无比例、保留 7m29s |
| 2/4 完成 | TurnProgress 已跟踪测试 | 2/4、50%、aria-valuenow=50 |
| 补检查及旧会话 | TurnProgress；MessageList.progress；Rust 检查点测试 | 中性 tone、无禁词 |
| 授权/决策/失败 | TurnProgress 参数化测试 | 授权保持 warning |
| 主题与宽度 | lightModeAudit | dark/light/narrow 截图及 overflow 断言 |

全量入口：pnpm test；pnpm build；pnpm test:turn-progress:headless。Rust 使用任务专属 .codefactory-cache/cargo-target-m37，运行 cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast。场景声明由登记表及带 --ci 的 validator 判断，不修改登记表或验证器。

## 约束与实现说明
只在当前受管分支、最新 main 上恢复备份。禁止修改 M36/U25 并行文件，禁止调用 delegate_tasks。最新 main 的无计划活动条色调由 src/lib/statusBanner.ts 决定，恢复后的集成测试先失败，需一行修正：用 humanWaitingReason 过滤后的原因决定警告色；未改 MessageList.tsx。

本任务仅改变被动展示，不新增模型选择、授权、停止或设置操作，不改变既有后台操控入口；验收可由 CLI headless 在锁屏时执行。不将后台补检查引入任何需要用户确认的操作。

## 验收与交付边界
PR 正文按 task-template 第 5 节记录自验收、Req ID 映射、实际红绿证据、Harness、AI Collaboration、真实浏览器截图、Scenario-Test 与 README-Update 两行。只交付至 pr_only，开好 PR 即停止。上会话红绿历史不可仅凭旧 PR 草稿当作本次证据；如未复现必须如实标注。
