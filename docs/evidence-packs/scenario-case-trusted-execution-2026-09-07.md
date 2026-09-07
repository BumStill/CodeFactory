# E2E-001 可信执行升级本地证据

## 结论

Bootstrap-1a 的 case planner/adapter/executor/aggregate/final verifier 本地实现和真实定向集成通过，**not live**：只有经过新的外部信任根迁移批准、默认分支合入、恢复规则与 canary 后，才能称线上可信门禁生效。

依赖普通前置 [PR #507](https://github.com/BumStill/CodeFactory/pull/507)，将正式 binary 的 unattended CLI 放在首个分支，并通过独立模块直接挂载 driver。已复用 #506 的 macOS nightly 改动，不重复实现、不删除它仍保留的 PR/UI/release 缺口。

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

下一步：先普通合并 #507；升级 PR 与 `main` 同步、审查全部 CI；再取得仅针对该升级 PR 的最小 bootstrap 批准，迁移后立即恢复原规则集，并用新默认分支 canary 验证 schema v2 exact-head receipt。完整 11 case、真实 WebView、release/nightly 及任意敌对构建代码的 OS 级隔离均未在本证据中宣称完成。

## AI Collaboration

- context scope：合成场景、源码、测试、公开 PR/CI、匿名回执；无生产聊天或凭据读取。
- assumptions：同一个 Harness；PR slice 不等于完整 E2E；fixture digest 不等于执行证明。
- review point：独立设计审查、漏洞反例、入口改造审查、真实 receipt 独立复算。
- validation result：本地上述证据通过；默认分支升级和服务端 canary 尚待完成。
