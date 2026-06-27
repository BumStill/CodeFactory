# 系统化 Agent 评测机制

## 原则

CodeFactory 的能力评估默认评估 **agent system**，不是单独评估模型。模型后端是 agent 的一个组件，和 prompt、context builder、tool loop、policy、runner、verifier/import 链路共同决定结果。

任何 benchmark run、PR 描述、证据包或产品 UI 都必须明确：

- 评估主体是谁。
- 固定了什么。
- 变化了什么。
- 结论归属给谁。

不能把 `CodeFactory agent using DeepSeek` 写成 `DeepSeek 的 Terminal-Bench 结果`。正确归因是：`CodeFactory agent` 是评估对象，`DeepSeek` 是该次 run 使用的 model backend。

## 评估矩阵

| 目的 | 固定什么 | 变化什么 | 结论归属 |
| --- | --- | --- | --- |
| 评 CodeFactory agent 能力 | benchmark task set、model backend、policy preset、runner 环境 | CodeFactory build、agent loop、context/tool/policy 实现 | CodeFactory agent |
| 评模型后端影响 | CodeFactory build、agent adapter、task set、policy preset、runner 环境 | provider/model，例如 DeepSeek、Claude、GPT | model backend 作为组件 |
| 评 agent scaffold 强弱 | 同一个 provider/model、task set、runner 环境 | CodeFactory adapter、simple baseline、oracle 或其他 agent scaffold | agent scaffold / product mechanism |
| 评 benchmark 基础设施 | oracle 或 no-model diagnostic、task limit、runner 环境 | Harbor/Docker/importer/schema/UI | evaluation infrastructure |

## 结果归因契约

每个 run 至少记录这些字段或等价信息：

- `benchmark_id`、`dataset`、`task_set` 或 subset id。
- `evaluation_axis`: `codefactory-agent-capability`、`model-backend-ablation`、`agent-scaffold-comparison` 或 `evaluation-infrastructure-smoke`。
- `evaluation_subject`: 被评价对象，例如 `codefactory-headless`。
- `fixed_variables`: 本次比较中保持不变的 task set、model backend、policy、runner、CodeFactory build。
- `changed_variables`: 本次比较中刻意改变的 build、model、adapter 或 runner。
- `agent_name`、`agent_version`、`codefactory_version`、`codefactory_git_sha`。
- `model_provider`、`model`、`endpoint_type`，作为 backend attribution。
- `policy_preset`、`runner`、`harbor_version`、`job_path`。
- `claim_allowed`: 这次证据允许做出的结论。
- `claim_forbidden`: 这次证据不允许做出的结论。

## PR 和证据要求

能力相关 PR 必须在描述中写明：

- `Evaluation axis`: 本 PR 属于哪一种评估轴。
- `Evaluation subject`: 评价对象，默认应是 CodeFactory agent。
- `Fixed variables`: 这次为了归因而固定的变量。
- `Changed variables`: 本 PR 或本次实验实际改变的变量。
- `Result attribution`: 结论归属给 CodeFactory agent、model backend、agent scaffold 还是 evaluation infrastructure。

如果缺少真实 benchmark run，必须写 `not run`、blocker 和替代证据；不得把 fake model、oracle smoke、no-model smoke 或 provider bridge test 声明成 CodeFactory agent capability score。
