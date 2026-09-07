# E2E-001 可信执行升级本地证据

## 结论

Bootstrap-1a 的 case planner/adapter/executor/aggregate/final verifier 本地实现和真实定向集成通过，**not live**：只有经过新的外部信任根迁移批准、默认分支合入、恢复规则与 canary 后，才能称线上可信门禁生效。

普通前置 [PR #507](https://github.com/BumStill/CodeFactory/pull/507) 已合并，将正式 binary 的 unattended CLI 放在首个分支，并通过独立模块直接挂载 driver。可信升级为 [PR #508](https://github.com/BumStill/CodeFactory/pull/508)。已复用 #506 的 macOS nightly 改动，不重复实现、不删除它仍保留的 PR/UI/release 缺口。

## 失败优先

1. 新增集成测试先因不存在 `case_plans` 和执行输入验证器出现 9 项失败。
2. 独立 QA 实测旧 builder 接受 `ok="false"`、bool 计数和 float 计数；9 个子反例先失败，严格类型修复后通过。
3. 原聚合器接受 failed+passed 重复 target、错 runner 与额外目标；现要求计划与回执完整且唯一。
4. 独立 QA 找到 Cargo runner 配置绕过；extensionless 配置负例先失败，保护配置及拒绝新变体后通过。
5. 非 case 字段夹带私密原文、`detail={}` 触发 TypeError 均有反例；现输出安全字段或匿名失败诊断。
6. tracked tree 变脏和 Cargo 构建缺少 `--locked` 的负例先失败；修复后拒绝源树漂移和隐式锁文件更新。

## 本地测试

- 新 case execution 测试 **19/19**：含真正的 driver 文件束、symlink 父目录、CLI 前缀、Cargo 配置/入口、实际 HEAD/平台、完整集合、畸形输入、隐私、非零退出和临时目录回收。
- receipt contract **29/29**。
- 可运行的 Python discovery 测试集 **286/286**。唯一未装载的模块为 `test_codefactory_bench_agent`：本机没有 Harbor；临时 Python 3.12 环境安装 `harbor==0.15.0` 的依赖下载超时，离线缓存不完整。未修改系统 Python，也未把该模块记作通过；完整含 Harbor 的验证由升级 PR 的 `agent-bridge-linux` required check 补齐。
- governance baseline 与 `git diff --check` 通过。
- 前置入口的实际 Rust 分发测试、正式 binary failure-receipt 测试、真实成功 smoke 与 TypeScript 检查，见前置 PR evidence pack。

## 真实回执链

在干净提交 `e1edb7d98b8bb83fcf972008a43f229970db832d` 上，复用现有 `_execute_concrete_target` 实际构建/运行正式 binary，再由 `build_case_entry`、`aggregate_receipts` 和 `validate_aggregate_receipt` 完成重算。执行前后使用真正文件束、HEAD、tracked tree 与实际 macOS/arm64 检查，未用假 driver bytes 或虚假平台替代。

- scope：`local_targeted_integration_not_github_attestation`；runner 名称为 `local-macos`。
- source HEAD 与 binary 编译时 build SHA 均为 `e1edb7d98b8bb83fcf972008a43f229970db832d`。
- target 成功、case outcome 为 `passed`、最终验证错误列表为空。
- durable-state/process/side-effects/delivery 四类 oracle 通过。
- 单用户消息、零人工 prompt、真实 hard kill、worker reap、不同进程替换、单副作用 receipt、双 replay link、completed 终态。
- cleanup_attempted/orphan_sweep_performed 均为 true，descendant/leak 均为 0。
- UI 为 `not_required_for_stage`，executable/artifact digest、version、tag 为空；不是 GitHub required check、Windows proof 或 release artifact 验收。

先前提交 `338ae23c0c9488488fef3b293e3e06b086ea14cf` 上的首轮真实回执，还由独立 QA 从 raw 重新计算，核对 driver/verifier/fixture/run digest 全部一致。补上 tracked-tree/locked 守门后又执行了上述干净提交的最终定向集成。

## 审查与上线边界

独立 QA 先指出入口截获、Cargo runner 覆盖、类型混淆和隐私旁路；修复后复核无新的假绿或信任根绕过问题，并额外执行 10 项畸形/隐私负例。最后一个 `detail` 对象鲁棒性问题已补红转绿测试。

本轮没有修改线上 ruleset。只读对账确认 six required contexts 仍为 active/strict/no-bypass，均绑定 GitHub Actions app 15368。网络 EOF/TLS 错误只通过单次命令的 HTTP/1.1/直连参数及已连接 GitHub 服务回退，不修改代理、不跳过同步 hook。

## 服务端交付检查点

- #507 测试 head 为 `aa115e322fb9e557dd6c20254e85e483865b81ca`，base 为 `9097a86bfa059e02f38b03cb8501732fd16027ed`。[执行 run 34102523569](https://github.com/BumStill/CodeFactory/actions/runs/34102523569) 与 [gate run 34102522048](https://github.com/BumStill/CodeFactory/actions/runs/34102522048) 均成功，六项 required checks 全绿。独立 QA 以 base 一致的 schema v1 verifier 确认 69/69 targets（Windows 60、macOS 9），完整唯一集合、command digest、runner/final 并集均匹配。
- 首轮暴露的 Windows Skill symlink 零测试问题已修成真正双平台合成夹具；新 Windows 日志明确为该命名测试 `1 passed; 0 failed; 0 ignored`。不改 registry 或路由绕过测试。#507 合并 SHA 为 `b7f65f5fb149f8f792e73460f76781cb8d4f63c0`，hold trailer 已读回复验；只回收了其 clean worktree 与本地分支，源代码可从已合并 PR 恢复。
- #508 实现版 head `5ee2c1ea95b6c1cee0b5b41cd4188b661b791f97` 的五项普通 required checks 全通过。[CI run 34102604063](https://github.com/BumStill/CodeFactory/actions/runs/34102604063) 的 Linux 完整集报告 313 项：303 通过，10 项 macOS 专用测试跳过，后者已由本地 macOS 286 项集覆盖；Harbor 相关模块不再是未验证项。
- 同一 #508 实现版 head 又执行真实本地定向 probe：source/build SHA 均为 `5ee2c1ea95b6c1cee0b5b41cd4188b661b791f97`，target/case passed、errors 为空、四类非 UI oracle 与 cleanup 通过。仍标记 `local_targeted_integration_not_github_attestation`，不冒充线上新 judge 证明。
- [旧 gate run 34102604124](https://github.com/BumStill/CodeFactory/actions/runs/34102604124) 仅报告 `run_scenario_harness_gate.py` 与 `scenario_execution.py` 两个既有 trust-root 文件不可在普通 PR 中自修改。该拒绝符合设计；五项普通检查通过不授权旁路合并。
- #508 已同步 #507 合入后的 main，最终差异只保留 11 个可信升级/测试/文档文件。同步提交改变 HEAD，必须重新审查其 exact-head CI，不能复用上一 head 的绿灯作为合并依据。

下一步：待用户另行批准仅针对 #508 的最小 bootstrap，且最终 exact-head 五项普通 required checks 全绿后，迁移并立即恢复原规则集，再用新默认分支 canary 验证 schema v2 receipt。完整 11 case、真实 WebView、release/nightly 及任意敌对构建代码的 OS 级隔离均未在本证据中宣称完成；registry 仍为 10 个部分实现、1 个设计中、26 项 remaining gaps。

## AI Collaboration

- context scope：合成场景、源码、测试、公开 PR/CI、匿名回执；无生产聊天或凭据读取。
- assumptions：同一个 Harness；PR slice 不等于完整 E2E；fixture digest 不等于执行证明。
- review point：独立设计审查、漏洞反例、入口改造审查、真实 receipt 独立复算。
- validation result：本地上述证据通过；默认分支升级和服务端 canary 尚待完成。
