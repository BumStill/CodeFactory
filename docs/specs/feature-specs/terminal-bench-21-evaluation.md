# Terminal-Bench 2.1 能力评估规格

## 范围

本规格定义 CodeFactory 以 Terminal-Bench 2.1 为产品能力评估目标的首期能力。首期目标不是冲榜，而是建立可复现运行、结果导入、失败分类和产品改进闭环。

相关设计：

- `docs/design/terminal-bench-21-business-design.md`
- `docs/design/terminal-bench-21-architecture-design.md`
- `docs/design/terminal-bench-21-ux-design.md`

## Requirements Traceability

| Req ID | User request | Normalized requirement | Surfaces | Validation method | Owner |
| --- | --- | --- | --- | --- | --- |
| CF-TB-R1 | 瞄准 Terminal-Bench 2.1 评估能力 | 仓库内有 Terminal-Bench 2.1 业务、架构、UX 和规格文档，明确官方约束和产品目标 | docs | 文档审查 + governance baseline | planning |
| CF-TB-R2 | 评估我们的能力 | CodeFactory 能保存 benchmark run、trial、reward、artifact 和 build 信息 | backend + sqlite + UI | fake Harbor job 导入测试 + UI summary |
| CF-TB-R3 | 不能只看总分 | 系统生成 capability profile 和 failure taxonomy | backend + UI | fixture run 分类断言 |
| CF-TB-R4 | 能被 Terminal-Bench 2.1 跑 | 提供 Harbor custom agent adapter 设计和 headless runner 接口 | adapter + agent loop | Harbor smoke run 或 adapter integration test |
| CF-TB-R5 | 改进后能回归 | 支持同一 subset 的 baseline/head run 对比 | backend + UI | compare run fixture test |
| CF-TB-R6 | 保持可审计和安全 | benchmark policy 只在 Harbor sandbox 生效，不污染普通项目权限和长期 memory | permission + memory + audit | policy unit test + memory write guard |

## Primary User Path

P-TB-1: 用户打开 CodeFactory 的 `Benchmarks / Terminal-Bench 2.1` 页面。系统检查 Harbor、Docker 和 CodeFactory agent adapter 状态。用户启动 smoke run，系统展示不可修改的官方 dataset `terminal-bench/terminal-bench-2-1`、agent/model、policy preset、artifact path 和命令 preview。run 完成后，CodeFactory 导入 Harbor job 目录，展示 reward、trial 列表、verifier 输出、trajectory 和 failure class。用户选择失败类别，创建后续产品改进 slice，并能用同一 subset 在修复后回归对比。

## 开发内嵌评估节奏

Terminal-Bench 2.1 不是发版前偶尔运行的榜单检查，而是 CodeFactory 能力开发的反馈系统。所有面向 agent 能力的非平凡 PR 都必须声明它预计改善哪类 benchmark failure，并选择对应评估层级。

| 阶段 | 何时运行 | 评估范围 | 必须回答的问题 | 产物 |
| --- | --- | --- | --- | --- |
| Baseline | Terminal-Bench 2.1 支持落地后、每个 release baseline 或重大 agent loop 改动前 | 当前 `main` 或 release build 的 smoke/subset | CodeFactory 现在主要输在哪类能力？ | baseline run、failure taxonomy、artifact refs |
| PR planning | PR 开发前 | 不运行或导入已有失败集 | 这个 PR 预计改善 planning/context/tool-use/verification/policy/environment 中哪一类？ | PR 假设、目标 subset |
| Inner loop smoke | adapter、runner、policy、importer、agent loop 改动中 | 1 到 5 个 task 或 fake Harbor fixture | Harbor -> CodeFactory -> verifier -> import 链路有没有断？ | smoke job、import result |
| Targeted subset | 能力 PR 合并前 | 5 到 20 个历史失败同类 task | 原目标失败是否改善？是否转移成其他失败类型？ | baseline/head 对比 |
| Regression subset | 触碰共享 agent loop、tool runtime、context builder、permission、verification 时 | 固定代表性 subset | 核心能力有没有退化？cost/latency 有没有恶化？ | regression report |
| Main scheduled | `main` 定期运行，默认每日或每周 | 固定 subset + rotating subset | 多个 PR 叠加后的真实趋势是什么？ | trend snapshot、failure queue |
| Release candidate | 发版候选或 leaderboard 相关准备 | 更大 subset，必要时接近完整 Terminal-Bench 2.1 | 相比上个 release 是否可接受？是否满足 comparable 约束？ | release evidence pack |

合并标准不是“分数一定上涨”，而是必须解释变化：

- reward delta、pass/fail delta、cost/duration delta。
- 原失败 task 是否改善。
- 是否新增 regression。
- failure class 是否从一种产品问题转移成另一种。
- 如果没跑对应 subset，PR 必须说明 blocker 和替代证据。

PR 描述必须包含：

- `Benchmark hypothesis`: 本 PR 预计改善的 failure class。
- `Benchmark scope`: smoke、targeted subset、regression subset、full 或 not run。
- `Baseline`: 对比基线 run id 或明确 `not available`。
- `Result`: reward/failure/cost 变化和 artifact path。
- `Interpretation`: 为什么可以合并，或为什么只能作为实验合并。

## Applicable Harnesses

- Spec Harness: 本规格、Req ID、主路径、测试矩阵和证据要求必须存在。
- Compatibility Harness: 新表、新 settings、新 agent adapter 不得破坏旧 session、tool runtime、permissions、memory。
- Observation Harness: run/trial/artifact/reward/failure classification 必须可审计。
- Payload Harness: Harbor artifacts、trajectory、verifier output、result JSON 都是 payload，导入和导出必须脱敏并记录来源。
- AI Collaboration Harness: 失败分类和改进建议必须记录 assumptions、review point、validation result。
- Release Harness: 如果 benchmark runner 进入安装包或公开 release，必须验证真实 packaged app/headless runner。

## 数据契约

### Benchmark Profile

| Field | Terminal-Bench 2.1 value |
| --- | --- |
| `id` | `terminal-bench-2.1` |
| `dataset` | `terminal-bench/terminal-bench-2-1` |
| `harness` | `harbor` |
| `official_url` | `https://www.tbench.ai/docs/run-terminal-bench-2-1` |
| `leaderboard_url` | `https://www.tbench.ai/leaderboard/terminal-bench/2.1` |
| `comparable_constraints` | no timeout/resource changes, dataset fixed, agent/model/build recorded |

### Run Summary

每次 run 至少记录：

- benchmark id、dataset、dataset version 或 resolved package id。
- agent name、agent version、model、provider。
- CodeFactory app version、git sha、build time。
- Harbor version、Docker/provider 类型。
- full command、job path、started/finished time、status。
- comparable flag 和不 comparable 的原因。
- reward summary、pass/fail/partial counts、cost/duration。

### Trial Summary

每个 trial 至少记录：

- task name、category、difficulty、tags。
- reward、duration、verifier exit status。
- trajectory path、verifier stdout/stderr path、artifacts path。
- failure class、classifier confidence、human review status。

## 测试矩阵

| Path type | Scenario | Expected result | Evidence |
| --- | --- | --- | --- |
| Primary | 导入完整 Harbor job fixture | run 和 trial 入库，summary 正确 | unit/integration output |
| Primary | 页面展示 latest run | reward、task count、artifact path、failure classes 可见 | UI test |
| Primary | 同一 subset 对比两个 run | reward delta、regression task、improved task 可见 | compare test |
| Adapter | custom agent adapter command 生成 | 使用 `terminal-bench/terminal-bench-2-1` 和 import path | command assertion |
| Policy | benchmark-sandbox policy in task container | workspace command/file edit 自动允许，host path/secret deny | policy unit test |
| Failure | 缺失 `result.json` | 标记 `partial_import`，列出缺失文件 | importer test |
| Failure | Harbor 不存在 | UI 显示 blocker，不影响其他页面 | environment probe test |
| Failure | timeout/resource 被修改 | comparable=false，官方可比状态标红 | config validation test |
| Observation | classifier 输出 failure class | 每个失败 trial 有 evidence refs 和 assumptions | classifier fixture |
| Payload | trajectory/artifact 导出 | 不写入长期 memory，不自动复制任务全文 | memory guard test |

## Evidence Pack Requirements

- 官方资料核准时间和链接。
- 环境检查输出：Harbor、Docker、provider、dataset。
- run command preview 和实际 command。
- Harbor job path。
- `config.json`、`result.json`、trial result、trajectory、verifier output 摘要。
- CodeFactory build identity。
- comparable flag 和约束检查结果。
- failure taxonomy summary。
- 改进前后 subset 对比报告。

## 发布边界

- 在 headless runner 和 Harbor adapter 真正可跑前，产品只能声明 `design ready`，不得声明 Terminal-Bench 2.1 已支持。
- 在至少一次真实 Harbor smoke run 成功导入前，不能声明 `evaluation path verified`。
- 在 packaged app 或 release artifact 中验证前，不能声明 `live`。
- 官方 leaderboard submission 需要单独 release/QA gate；本规格首期只覆盖本地可复现能力评估。
