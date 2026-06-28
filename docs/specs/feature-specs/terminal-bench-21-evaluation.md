# Terminal-Bench 2.1 能力评估规格

## 范围

本规格定义 CodeFactory 以 Terminal-Bench 2.1 为产品能力评估目标的首期能力。首期目标不是冲榜，而是建立可复现运行、结果导入、失败分类和产品改进闭环。

相关设计：

- `docs/principles/systematic-agent-evaluation.md`
- `docs/design/terminal-bench-21-business-design.md`
- `docs/design/terminal-bench-21-architecture-design.md`
- `docs/design/terminal-bench-21-ux-design.md`

## Requirements Traceability

| Req ID | User request | Normalized requirement | Surfaces | Validation method | Owner |
| --- | --- | --- | --- | --- | --- |
| CF-TB-R1 | 瞄准 Terminal-Bench 2.1 评估能力 | 仓库内有 Terminal-Bench 2.1 业务、架构、UX 和规格文档，明确官方约束和产品目标 | docs | 文档审查 + governance baseline | planning |
| CF-TB-R2 | 评估我们的能力 | CodeFactory 能保存 benchmark run、trial、reward、artifact 和 build 信息 | backend + sqlite + UI | fake Harbor job 导入测试 + UI summary |
| CF-TB-R3 | 不能只看总分 | 系统生成 capability profile 和 failure taxonomy | backend + UI | fixture run 分类断言 |
| CF-TB-R4 | 能被 Terminal-Bench 2.1 跑 | 提供 Harbor custom agent adapter、显式 env 驱动的 model-backed headless runner，以及从当前 CodeFactory provider 到 benchmark env 的显式授权桥接 | adapter + agent loop + backend command | Python adapter smoke + Harbor CodeFactory baseline run + fake model headless runner integration test + provider bridge unit test + real model smoke |
| CF-TB-R5 | 改进后能回归 | 支持同一 subset 的 baseline/head run 对比 | backend + UI | compare run fixture test |
| CF-TB-R6 | 保持可审计和安全 | benchmark policy 只在 Harbor sandbox 生效，不污染普通项目权限和长期 memory | permission + memory + audit | policy unit test + memory write guard |
| CF-TB-R7 | 区分 agent 评估和模型评估 | 所有 run、PR、证据包和 UI 都必须声明 evaluation axis、evaluation subject、fixed variables、changed variables 和 result attribution | docs + backend + UI | spec review + fixture attribution test |

## Primary User Path

P-TB-1: 用户打开 CodeFactory 的 `Benchmarks / Terminal-Bench 2.1` 页面。系统检查 Harbor、Docker 和 CodeFactory agent adapter 状态。用户启动 smoke run 前，系统基于当前 endpoint/model 生成 provider bridge preview，展示不可修改的官方 dataset `terminal-bench/terminal-bench-2-1`、agent/model、policy preset、artifact path、redacted env 和命令 preview。用户必须确认授权短语后，后端才从 OS credential store 读取当前 endpoint key，并只把它临时注入本次 Harbor child process env。run 完成后，CodeFactory 导入 Harbor job 目录，展示 reward、trial 列表、verifier 输出、trajectory 和 failure class。用户选择失败类别，创建后续产品改进 slice，并能用同一 subset 在修复后回归对比。

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

- `Evaluation axis`: `codefactory-agent-capability`、`model-backend-ablation`、`agent-scaffold-comparison` 或 `evaluation-infrastructure-smoke`。
- `Evaluation subject`: 被评价对象；默认是 `codefactory-headless` 或具体 CodeFactory agent。
- `Fixed variables`: 为了归因而固定的 benchmark subset、model backend、policy、runner、build 等变量。
- `Changed variables`: 本 PR 或本次实验实际改变的 build、model、adapter、policy 或 runner。
- `Result attribution`: 结论归属给 CodeFactory agent、model backend、agent scaffold 还是 evaluation infrastructure。
- `Benchmark hypothesis`: 本 PR 预计改善的 failure class。
- `Benchmark scope`: smoke、targeted subset、regression subset、full 或 not run。
- `Baseline`: 对比基线 run id 或明确 `not available`。
- `Result`: reward/failure/cost 变化和 artifact path。
- `Interpretation`: 为什么可以合并，或为什么只能作为实验合并。

## 系统化评估矩阵

本规格继承 `docs/principles/systematic-agent-evaluation.md`。Terminal-Bench 结果默认是 agent 系统结果，不是单独的模型结果。

| Evaluation axis | 固定什么 | 变化什么 | 允许结论 | 禁止结论 |
| --- | --- | --- | --- | --- |
| `codefactory-agent-capability` | Terminal-Bench subset、model backend、policy、runner | CodeFactory build、agent loop、context/tool/policy 实现 | CodeFactory agent 能力变化 | 某模型独立能力排名 |
| `model-backend-ablation` | CodeFactory build、agent adapter、subset、policy、runner | provider/model | 模型作为 CodeFactory 组件的影响 | CodeFactory 产品能力整体提升 |
| `agent-scaffold-comparison` | provider/model、subset、runner | CodeFactory adapter、simple baseline、oracle 或其他 scaffold | agent scaffold / 产品机制强弱 | provider/model 本身优劣 |
| `evaluation-infrastructure-smoke` | oracle 或 no-model diagnostic、runner | Harbor、Docker、importer、schema、UI | 评测基础设施是否打通 | CodeFactory agent 已具备任务能力 |

命名规则：

- 正确：`CodeFactory agent using DeepSeek`、`agent=codefactory-headless model_backend=DeepSeek`。
- 错误：`DeepSeek 跑出了 CodeFactory 的 Terminal-Bench 结果`。
- 第一次有效 CodeFactory 能力结果必须满足 `evaluation_axis=codefactory-agent-capability` 且 `agent_name=codefactory-headless`。

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

### Harbor Command Semantics

当前本地验证使用 Harbor 0.15.0。命令语义必须按实际 CLI 处理：

- `-d` / `--dataset`: 选择数据集，例如 `terminal-bench/terminal-bench-2-1`。
- `-l` / `--n-tasks`: 限制 task 数量，smoke run 默认用 1 到 5。
- `-k` / `--n-attempts`: 每个 trial 的 attempts，不得误用为 task 数量。
- `--agent-import-path`: 自定义 CodeFactory agent adapter 的 import path。
- `-a oracle`: 只用于验证 Harbor/Docker/dataset/verifier/import 链路，不代表 CodeFactory agent 能力。

当前已验证的 CodeFactory import path 是 `codefactory_bench.agent:CodeFactoryAgent`。历史首个真实 run 使用 `codefactory-headless-baseline` / `baseline-no-model`，只证明 Harbor 能运行 CodeFactory-owned adapter 并把结果导回 CodeFactory；不得把该 0 分 baseline 声明为完整 CodeFactory agent 能力。

当前 adapter 名称为 `codefactory-headless`，支持两种模式：

- `baseline-no-model`: 未提供显式 benchmark model env 时，只跑 sandbox 诊断和导入链路。
- `model-backed`: 提供 `CODEFACTORY_BENCH_API_KEY` 且提供 `CODEFACTORY_BENCH_MODEL` 或 Harbor `-m <model>` 时，调用 OpenAI-compatible chat-completions 接口，通过 `run_shell` 工具在 Harbor task container 内执行。

Model-backed 模式只能读取显式 `CODEFACTORY_BENCH_*` 配置，不读取 CodeFactory desktop settings、macOS keychain、通用 provider env 或用户凭据。

Model-backed loop 必须把 task container 内的 `environment.exec` 异常记录成 trajectory 中的 `exec-error` tool result，而不是让整个 trial 直接变成 Harbor agent exception。至少记录：

- `status=exec-error`
- `error_type`: `command-timeout`、`environment-exec-error` 或 `exec-runtime-error`
- `timeout_sec`
- 原始 command 的单行摘要
- `context.metadata.exec_errors` 和 `context.metadata.command_timeouts`

对于自检命令返回非零且 stdout/stderr 包含 pytest failure、traceback 或 assertion 失败时，adapter 必须追加 verifier-repair 提示，要求模型基于失败断言修改实现并重跑最小失败检查后再结束。

对于明显的前台服务启动命令，例如 `python -m http.server`、`uvicorn`、`flask run`、`npm start`、`redis-server` 等，adapter 必须要求后台启动、日志重定向、pid 记录和 bounded readiness check；不得直接执行会常驻到 tool timeout 的前台服务命令。已显式后台化、`nohup`、`setsid`、`timeout` 或 daemon 模式的命令不在该拦截范围内。

### Provider Bridge

产品侧允许用户把当前 CodeFactory endpoint/model 用于一次 benchmark run，但必须经过显式授权桥接：

- `preview_benchmark_provider_bridge(request)` 只读取 settings 中的 endpoint/model/key_ref，返回 redacted env、command preview、job path 和授权短语；不得读取或返回 raw API key。
- `start_benchmark_provider_run(request)` 只有在授权短语完全匹配时才读取 OS credential store，并把 key 作为 `CODEFACTORY_BENCH_API_KEY` 注入 Harbor child process env。
- raw key 不写入 command preview、frontend state、SQLite run record、Harbor args、日志或 evidence pack。
- 当前 bridge 只支持 OpenAI-compatible `chat/completions` endpoint；DeepSeek 这类 direct provider 需要用 `normalize_model_id` 去掉 OpenRouter vendor 前缀。
- ChatGPT OAuth、Anthropic 原生 Messages API、需要浏览器会话或非 API key 的 provider 暂不支持 benchmark bridge。
- `concurrency` 是 Harbor `-n` / `--n-concurrent`，不是 trial count；`trial_count` 只作为旧客户端兼容 alias。
- `task_names` 使用 Harbor `--include-task-name` 过滤固定 subset；当提供 `task_names` 且未显式提供 `task_limit` 时，默认 `task_limit=task_names.length`，避免固定 subset 被默认 smoke limit 截断。

### Regression Subset

首个固定回归子集为 `docs/benchmark-subsets/terminal-bench-21-regression-subset-v1.json`。

该子集来自第一次完整 CodeFactory Terminal-Bench 2.1 run `7ff6ef13-4488-4e0f-afd0-a1f9bd16d561`，包含 18 个任务，覆盖：

- passed smoke: `write-compressor`, `extract-elf`, `filter-js-from-html`, `nginx-request-logging`
- verifier-zero: `circuit-fibsqrt`, `configure-git-webserver`, `mteb-retrieve`, `sanitize-git-repo`, `query-optimize`
- tool-use: `count-dataset-tokens`, `install-windows-3.11`, `protein-assembly`
- command-timeout: `build-cython-ext`, `kv-store-grpc`, `sparql-university`, `torch-tensor-parallelism`
- environment/resource: `caffe-cifar-10`, `qemu-startup`

后续 agent-loop、tool runtime、verification repair、resource/preflight 改动默认至少跑该 subset 或说明 blocker。

### Run Summary

每次 run 至少记录：

- benchmark id、dataset、dataset version 或 resolved package id。
- evaluation axis、evaluation subject、fixed variables、changed variables、result attribution。
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
| Adapter | CodeFactory baseline adapter smoke | Harbor 能 import `codefactory_bench.agent:CodeFactoryAgent`，trial 无 exception，CodeFactory importer 读回 agent identity 和 reward | Harbor job + ignored real import test |
| Adapter | Model-backed headless loop | fake OpenAI-compatible server 返回 `run_shell` tool call，adapter 执行 Harbor environment command 并写 trajectory | Python integration test |
| Adapter | Artifact enforcement loop | 初始 inspection 后，重复读文件、复合只读命令和无关实现命令会被压回 artifact 生成；有目标产物前空回复会恢复为 tool-call 要求 | Python loop tests + real provider smoke trajectory |
| Adapter | Protocol auto-repair | candidate artifact 自检出现 decompressor crash、size limit 或协议失败时，adapter 能记录自动修复轨迹并产出可验证 artifact；修复不得依赖 task container 中不存在的 Python runtime | Python loop test + real provider smoke reward |
| Adapter | Exec timeout recovery | `environment.exec` 抛出 command timeout 时，adapter 写入 `exec-error/command-timeout`、更新 metadata，并继续给模型修复机会，不直接 Harbor exception | Python loop test |
| Adapter | Failed self-check repair | pytest/assertion/traceback 类自检失败会生成具体 repair reminder，要求修改实现并重跑最小失败检查 | Python loop test |
| Adapter | Foreground service supervision | 前台服务启动命令被 suppress，并提示后台启动、日志、pid 和 readiness check，不消耗完整 tool timeout | Python loop test |
| Adapter | Provider tool-choice compatibility | provider 拒绝 forced `tool_choice` 时自动降级为 `auto` 重试，不把兼容性错误误记为 agent 能力结果 | Python provider fallback test |
| Adapter | Provider bridge preview | 当前 DeepSeek endpoint/model 生成 redacted env 和 Harbor command preview，不暴露 raw key | Rust unit test |
| Adapter | Provider bridge authorization | 授权短语不匹配时不得 lookup secret；匹配后只把 key 放入 child env | Rust unit test |
| Attribution | Evaluation axis contract | run/PR/evidence 区分 CodeFactory agent 能力、模型后端影响、agent scaffold 对比和评测基础设施 smoke | spec review + fixture test |
| Policy | benchmark-sandbox policy in task container | workspace command/file edit 自动允许，host path/secret deny | policy unit test |
| Policy | network/secret deny | fake model 请求 `curl` 或 credential path 时不调用 environment.exec | Python policy test |
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
- 在至少一次真实 Harbor smoke run 成功导入前，不能声明 `evaluation path verified`。oracle smoke 只能证明 Harbor 环境和导入链路，不能证明 CodeFactory agent 能力。
- `codefactory-headless-baseline` 成功运行后，可以声明 `CodeFactory-owned adapter path verified`，但在 model-backed headless runner 跑通前，不能声明 `CodeFactory agent capability evaluated`。
- fake model 测试通过后，只能声明 `model-backed runner implementation verified locally`；在显式模型 env 下跑完真实 Terminal-Bench smoke 前，不能声明 `model-backed CodeFactory score available`。
- provider bridge 测试通过后，只能声明 `current provider can be authorized for CodeFactory agent benchmark launch by backend contract`；在真实 Harbor run 完成并导入前，不能声明当前本机已产生 CodeFactory agent Terminal-Bench 分数。
- 使用 DeepSeek/Claude/GPT 等模型后端完成的 run，结果仍归属 `CodeFactory agent using <model backend>`；不得写成模型本身的 Terminal-Bench 结果，除非 evaluation axis 明确是 `model-backend-ablation` 且 CodeFactory build/agent adapter/subset/policy/runner 已固定。
- 在 packaged app 或 release artifact 中验证前，不能声明 `live`。
- 官方 leaderboard submission 需要单独 release/QA gate；本规格首期只覆盖本地可复现能力评估。
