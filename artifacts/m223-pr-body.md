## M223 发布基础设施重试修复

按原任务 R1–R5 实施，不绕过任何必需检查或可信 validator。

### 需求与验收
- R1：GitHub API 读取五次有限退避；PR 观测失败不是测试失败。shell 功能验收 test_unreadable_pr_is_not_a_verdict 通过。
- R2：基础设施判定读取 jobs/steps 和数值 job URL 对应的 check-run 注解，真实测试失败优先；每个 PR 检查最多三次重跑，所有退避和等待受 60 分钟步骤上限约束。test_automatic_retries_are_bounded、test_runner_capacity_annotation_is_retried 及 tests.test_m223_requirements 通过。
- R3：Release Recovery 使用默认分支 workflow_run，在 Release 完结后最多两次重跑失败作业，之后明确失败而不是把 Draft 当成功；真实构建失败不重跑。ReleaseRecoveryTests 通过。
- R4：记录原因和第几次重试，区分测试失败、不可判定、基础设施耗尽。
- R5：tests.test_ci_infra_retry、tests.test_auto_release_wait_loop 覆盖 HTTP 502、runner not acquired/capacity、真实测试失败；真实 workflow shell 与合并门禁一起执行。相关 91 项测试通过。

### CI 自愈与实际功能证据
第一次 Windows smoke 在 phase-two 的硬编码 45s 超时。一次重跑后 execute-windows 通过，但旧失败收据被可信聚合器拒绝；未修改 validator，采用同步 main 后的新 head 完整验证。phase-two/three/four 已统一取现有可配置 worker 预算。
正式 codefactory 二进制构建后在 /tmp 运行 unattended-long-task-smoke：objective_status=completed、side_effect_receipt_count=1、replay_call_link_count=2、process_restart_observed=true、leaked_resource_count=0，机器断言通过；之后 agent-core smoke_worker 项目单测通过。
此前过滤 unattended_smoke::tests 得到零个测试，不作为有效证据。

### TDD 与治理
新增 tests.test_m223_requirements 先看到三项失败，随后修复通过。旧的两次 PR 重试/120 分钟设计不符合任务，已纠正为三次/60 分钟。源码预算修复映射 CF-USB-1/2/3，详见 unattended-smoke-shared-phase-budget.md。
Applicable Harnesses: Spec、Compatibility、Release、Observation、AI Collaboration。
AI Collaboration: context scope 为 release workflows/纯判定器/跨进程 smoke；assumption 为观测失败不得推断测试失败；review point 为首版预算、REST identity 与合并门禁；validation result 为 91 项测试、正式二进制 smoke 和治理校验。未启动子代理：任务模板明确禁止 delegate_tasks，本轮采用单执行流。

### 发布后验收分工
本次只授权合并，不触发发布。下一次真实发版由编排方发起，核对自动重试日志、最终非 Draft、跨平台资产和 updater metadata；安装版真实应用验收由编排方发版后完成。当前 not live，真实发版验收仍待后续发版任务。
Spec Amendment: 无；本轮撤回首版对原需求的错误扩张。
Follow-up: 可信收据聚合器对旧异常尝试的处理作为独立建议记录，不在本任务削弱它。

Scenario-Test: HLT-001, HLT-004, UI-005
README-Update: reviewed
README-Update-Reason: 内部发布恢复和 CI smoke 预算修复，不改变公开产品、安装或隐私承诺。
