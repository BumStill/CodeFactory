# Chrome attach runtime 回执证据范围修正

## 范围与结论

- 基线：`5c25e49fc1947ea7557ee0d4184129b63400ffae`。
- 对应规格：CF-STG-R10、CF-STG-R30；运行时行为证据不能自行升级为 exact release artifact 证据。
- `--browser-chrome-attach-smoke` 的成功与失败回执原来都硬编码 `evidence_level=exact_release_artifact`。PR 的开发二进制同样执行此入口，因此该标签不成立。
- 两个生产点统一使用 `native_runtime_smoke`，并将诊断中的 release/artifact 措辞改为 runtime/native。
- 本次不改 registry、workflow、ruleset、protected driver 或外部安装包验收器，不提升复杂 E2E 状态。

## 证据责任

runtime smoke 观察 native bridge、真实 synthetic Chrome attach/detach、标签页和租约状态。它不能证明自己的安装包来源；debug/release 编译模式或环境变量也不能提供此证明。

`scripts/verify-macos-release-artifact.sh` 继续负责指定 DMG 的安装路径、bundle 版本/签名、DMG/updater executable 摘要一致性和 expected build SHA。其行为字段校验保持不变，不依赖 runtime 自报的 `evidence_level`。外部 `policy_setup` 失败占位回执只描述验收上下文，不能当作 runtime 成功证明。

## 失败优先与本地验证

1. 新 Python 合同先失败：旧 CLI 没有 runtime-only 常量，两个生产点都宣称正式产物。
2. 新正式二进制测试先失败：实际回执为 `exact_release_artifact`，期望 `native_runtime_smoke`。
3. 修改后 `pnpm cargo:shared -- test --manifest-path src-tauri/Cargo.toml --test chrome_attach_receipt -- --nocapture` 通过（1 项）。测试清除 fixture 环境变量，在浏览器下载、启动、bridge 或 Tauri 初始化前失败；断言退出码 1、有效 JSON、RTE-003、failed 和 runtime-only 范围。进程等待上限为 15 秒。
4. `python3 -B -m unittest tests.test_release_workflow -q` 通过（34 项），包括新生产者合同和原安装包强校验合同。
5. 原 `test_release_gate_requires_exact_artifact_evidence_level` 通过；它检查 registry 的 L4 要求，不依赖这个 JSON 标签。
6. 治理基线和本地统一 scenario harness 均通过。

## 门禁计划与剩余证据

- 最小差异影响 `RTE-003`、`RTE-004` 和 `E2E-009`，现有 PR 计划为 19 个 target；不命中 E2E-004 的路径，没有 missing-gate blocker。
- 变更不属于受保护文件；`lib.rs` 首个无条件 canonical CLI module 声明保持不变。
- Mac managed Chrome 成功 smoke 尚未在本修复分支执行，需要后续 PR 的实际 macOS 结果核对成功回执标签。
- 本次不是正式安装包验收，也不声明修复已发布。按任务边界只先提交到本地分支，不推送或创建 PR。

## AI Collaboration

- context scope：报告标签生产者、全部仓库消费者、最小 diff 的现有门禁。
- assumptions：保留原 runtime 行为和外部 artifact provenance 校验；不以编译模式推断安装包来源。
- review point：独立 QA 发现 PR 开发二进制错误标签；主执行者复核本地 diff 与证据后继续 PR。
- validation result：源码合同和正式二进制失败回执先红后绿；外部安装包合同未放松，成功 runtime smoke 待 PR 补证。
