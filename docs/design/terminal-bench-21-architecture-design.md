# Terminal-Bench 2.1 对齐架构设计

## 架构目标

CodeFactory 增加一个 `benchmark-evaluation` 子系统，把 Harbor/Terminal-Bench 2.1 的运行、结果、证据和失败分析纳入本地产品能力闭环。

```text
CodeFactory UI / CLI
  -> Benchmark Evaluation Service
      -> Benchmark Registry
      -> Harbor Runner
      -> CodeFactory Agent Adapter
      -> Run Ledger
      -> Result Ingestor
      -> Failure Classifier
      -> Evaluation Attribution
      -> Capability Profile
```

## 模块边界

| 模块 | 职责 | v1 数据来源 |
| --- | --- | --- |
| Benchmark Registry | 固化 benchmark 名称、版本、官方命令、约束和证据要求 | 本地 profile + 官方链接 |
| Harbor Environment Probe | 检查 `harbor`、Docker、provider key、磁盘空间和网络；将 Harbor 的有效 network policy 注入共享 Agent core | shell command + config |
| Harbor Runner | 生成 job config，执行 smoke/subset/full run，记录 stdout/stderr | `harbor run` |
| CodeFactory Agent Adapter | 让 Harbor 以自定义 agent 方式调用 CodeFactory | Python adapter + headless CodeFactory runner |
| Benchmark Policy | 在 sandbox 内自动允许合理文件/命令操作，同时保留 hard deny | settings + tool permission engine |
| Run Ledger | 保存 run、trial、artifact、成本、版本和 policy | SQLite |
| Result Ingestor | 解析 Harbor job 目录、trial reward、trajectory 和 verifier 输出 | `jobs/<job>` |
| Failure Classifier | 将失败归类为可改进产品能力 | trajectory + verifier + tool audit |
| Evaluation Attribution | 区分 agent 能力、模型后端影响、agent scaffold 对比和评测基础设施 smoke | run config + comparison plan |
| Capability Profile | 生成按类别、工具、失败类型的能力画像 | run ledger + classifier |

## Agent 接入方式

### 评测完整性与共享执行契约

`codefactory-headless` 必须和桌面主产品共享 `agent_contracts/execution_completion.md`，并由同一个 Rust `codefactory-agent-core` 执行 policy、tool outcome 分类和 completion gate，至少统一以下完成语义：

- 源码构建任务必须完成 build、install、离开源码目录的 runtime/import smoke、项目测试四段证据。
- 后台服务任务必须记录 PID、日志、bounded readiness 和真实 client/functional probe。
- 最终回复前必须存在晚于最后一次实现修改的成功验证；失败必须继续迭代或形成明确 blocker。

Harbor 的有效 Agent wall timeout 由 thin bridge 原样传给 Rust sidecar。sidecar 以单一 `Instant` 计算剩余时间：进入总预算后 2/3 区间即持续提示收敛，进入最后 1/3 后 completion budget 拒绝新的范围扩张，最后 30 秒不再启动模型或工具调用而用于写出结构化 `Finished`。模型 transport 的有限重试共享同一个总 deadline，单次工具 timeout 也被剩余墙钟预算裁剪，避免各层 timeout 相加后由 Harbor 强杀而丢失结果与 usage。

`CompletionGate` 对源码交付维护独立 sequence：最后源码修改、成功 source install、install 后在源码目录外的 runtime/import smoke、项目验证。兼容性扫描必须递归覆盖构建配置引用的 `.py`、`.pyx`、`.pxd`、生成 `.c` 等输入，并通过明确的 `exit 1/0`、`sys.exit` 或 `test ! -s` 契约表达残留命中；正常输出 `PASSED` 或摘要不应被误判为失败。

当原始需求明确要求项目测试时，`CompletionGate` 额外记录最后一次成功项目测试，并要求其 sequence 晚于最后源码修改、安装和外部运行。已获准的复合工具调用只要包含明确文件修改，即使后续 build/install/runtime 失败，也必须推进最后源码修改 sequence 并使旧交付证据失效；纯依赖安装或 policy deny 不得误记为修改。

源码兼容迁移必须在首次昂贵 build/install 前从仓库 import 语句推导全部本地 alias，覆盖构建配置已观察到的源码、生成和编译输入扩展。扫描与替换使用 token boundary 和幂等规则，相关修改应批量完成；最后一次修改后的 clean residual scan 是下一次 build/install 的前置门禁，避免只覆盖常见别名、产生二次替换或在部分扫描后反复重建。门禁激活后仍允许仓库级 alias discovery、纠正性源码修改和带干净退出契约的最终 residual scan，使 Agent 能从不完整别名盘点中恢复；其他探索及 build/install 继续拒绝。

`CompletionGate::new_for_instruction` 的任务意图识别覆盖产品支持的中英文表达。中文“兼容/已移除/弃用/迁移”“从源码安装/源码构建/编译扩展”“项目测试/测试套件”必须映射到与英文 `compatibility`、`install from source`、`project tests` 相同的 gate，任务语言不得改变完成证据强度。

Headless 工具输出进入模型前采用 bounded head/tail compaction：保留命令/阶段开头与错误或成功尾部，压缩中间编译日志；总上下文达到预算时保留共享 contract、原始任务和最近完整 tool round。完整 stdout/stderr 仍留在 trajectory 作为审计证据，不以缩短模型上下文为由删除运行证据。

adapter 可以按通用能力类型维护状态，但不得按 benchmark task name、固定 repo、固定 artifact、领域答案、instruction fingerprint 或成功 marker 选择专用脚本。adapter 也不得读取 `/tests`、verifier 文件或 solution。每个 run 必须记录共享 contract SHA-256 和 contamination scan 结果；缺失任一项时 `evaluation_axis=codefactory-agent-capability` 无效。

运行拓扑固定为 `Desktop AgentLoop -> codefactory-agent-core` 与 `Harbor -> thin Python bridge -> codefactory-agent-headless -> codefactory-agent-core`。Python 只负责把 JSONL `ToolRequest` 转发给 Harbor `BaseEnvironment.exec` 并回传结构化 `ToolResult`，不得包含模型调用、prompt、policy、任务分类或 repair。单次 trial 不允许在 Rust sidecar 失败后静默退回旧 Python solver。

桌面端和 headless 允许有不同外围适配器，但结束判定必须来自同一 `CompletionGate`：桌面纯只读请求可以正常结束；一旦发生 mutation、失败命令或后台服务启动，则必须有更新的成功验证。headless coding run 额外要求至少发生一次真实工具行动，避免空回答被误判完成。

### v1: External Agent Adapter

优先实现 Harbor `BaseAgent` adapter。它通过 Harbor `BaseEnvironment.exec` 与任务容器交互，调用 CodeFactory headless runner 决策下一步命令和文件操作。

当前 adapter 是 `codefactory_bench.agent:CodeFactoryAgent`。它仅从显式 `CODEFACTORY_BENCH_*` 接收启动配置，启动 `codefactory-agent-headless`，并把 sidecar 的 `run_shell` 请求交给 Harbor task container 执行；模型 transport、prompt、benchmark policy 和 completion gate 均位于共享 Rust runtime。no-model baseline 只保留为基础设施诊断，不计 agent 能力分。

产品侧新增 provider bridge，解决“本地 CodeFactory 已配置 DeepSeek，但 benchmark adapter 不能隐式读取桌面设置”的边界：

- `preview_benchmark_provider_bridge` 从 settings 解析当前 endpoint、active model、key_ref、job path 和 Harbor 命令，返回 redacted env 与授权短语。
- `start_benchmark_provider_run` 只有在授权短语完全匹配后才读取 OS credential store，把 key 临时注入本次 Harbor child process 的 `CODEFACTORY_BENCH_API_KEY`。
- raw key 不返回前端、不进入 command preview、不进 Harbor args、不写 SQLite run record。
- direct provider 会复用 `normalize_model_id`，例如 DeepSeek direct API 下把 `deepseek/deepseek-v4-flash` 规范化为 `deepseek-v4-flash`。

优点：

- 不需要把完整桌面 app 安装到每个 task container。
- 更容易收集 trajectory、tool calls、policy decision 和 stdout/stderr。
- 和 Harbor 官方自定义 agent 入口匹配。

限制：

- adapter 仍只接受显式 `CODEFACTORY_BENCH_*` env；读取当前 CodeFactory provider 只能发生在产品后端的显式授权 bridge 中。
- 需要把 adapter-local `benchmark-sandbox` command gate 后续沉淀为共享 policy preset，避免把 benchmark run 的自动授权带回普通用户项目。

### v2: Installed Agent Adapter

如果 CodeFactory 后续有轻量 CLI binary，可以实现 Harbor `BaseInstalledAgent`，在 container 内安装并运行。

适用场景：

- 需要测试 CodeFactory 自带工具链在 container 内的真实兼容性。
- 需要更接近官方 leaderboard 的 agent 运行方式。

## Benchmark Policy

Terminal-Bench 任务发生在隔离 container 中，默认不能沿用普通项目的逐步人工审批，否则无法自动评估。

新增 policy preset：

```text
benchmark-sandbox
  scope: Harbor task container only
  allow: read/write workspace files, run build/test/install commands
  ask: network egress beyond task expectation, long-running background services
  deny: host filesystem, credential stores, destructive host commands, data exfiltration, benchmark task export
```

关键约束：

- `benchmark-sandbox` 只能由 Harbor adapter 或明确 benchmark run 创建。
- policy 必须绑定 run id、container id、dataset、task name。
- 离开 sandbox 后立即失效。
- 不允许把 benchmark task instruction、solution、hidden test 或 canary 写入长期 memory。

## Evaluation Attribution

Terminal-Bench 2.1 的首要评估主体是 CodeFactory agent system。模型后端是 agent 的一个组件，不能把 `CodeFactory agent using DeepSeek` 的结果写成 `DeepSeek 的结果`。

归因类型：

| `evaluation_axis` | 固定变量 | 变化变量 | 结论归属 |
| --- | --- | --- | --- |
| `codefactory-agent-capability` | task set、model backend、policy、runner | CodeFactory build、agent loop、context/tool/policy 实现 | CodeFactory agent |
| `model-backend-ablation` | CodeFactory build、agent adapter、task set、policy、runner | provider/model | model backend 作为组件 |
| `agent-scaffold-comparison` | provider/model、task set、runner | CodeFactory adapter、simple baseline、oracle 或其他 scaffold | agent scaffold / product mechanism |
| `evaluation-infrastructure-smoke` | oracle 或 no-model diagnostic、runner | Harbor、Docker、importer、schema、UI | evaluation infrastructure |

Run ledger 和 UI 必须能展示：

- evaluation axis。
- evaluation subject，例如 `codefactory-headless`。
- fixed variables 和 changed variables。
- allowed claim 和 forbidden claim。
- model provider/model 作为 backend attribution，而不是默认评价主体。

## 数据模型

```rust
struct BenchmarkRun {
  id: String,
  benchmark_id: String,
  dataset: String,
  dataset_version: String,
  evaluation_axis: String,
  evaluation_subject: String,
  fixed_variables_json: String,
  changed_variables_json: String,
  result_attribution: String,
  agent_name: String,
  agent_version: Option<String>,
  model: Option<String>,
  model_provider: Option<String>,
  codefactory_version: String,
  codefactory_git_sha: Option<String>,
  policy_preset: String,
  harbor_version: Option<String>,
  command: String,
  job_path: String,
  status: BenchmarkRunStatus,
  started_at: String,
  finished_at: Option<String>,
}

struct BenchmarkTrial {
  id: String,
  run_id: String,
  task_name: String,
  category: Option<String>,
  difficulty: Option<String>,
  reward: f64,
  duration_ms: Option<i64>,
  error_kind: Option<String>,
  failure_class: Option<String>,
  trajectory_path: Option<String>,
  verifier_stdout_path: Option<String>,
  verifier_stderr_path: Option<String>,
}
```

## 后端接口

| Command | Purpose |
| --- | --- |
| `list_benchmark_profiles()` | 返回支持的 benchmark profile，首期包含 Terminal-Bench 2.1 |
| `probe_benchmark_environment(profile_id)` | 检查 Harbor、Docker、provider、磁盘和网络 |
| `preview_benchmark_provider_bridge(request)` | 基于当前或指定 endpoint/model 生成 redacted env、授权短语和 Harbor command preview，不读取 raw key |
| `start_benchmark_provider_run(request)` | 授权短语匹配后临时注入 provider key，启动 Harbor run，完成后导入 job |
| `create_benchmark_run(profile_id, run_config)` | 创建 run record 和 job config |
| `start_benchmark_run(run_id)` | 启动 Harbor run，流式记录输出 |
| `import_benchmark_results(job_path)` | 导入已有 Harbor job |
| `get_benchmark_run(run_id)` | 读取 run、trial、summary、artifact refs |
| `classify_benchmark_failures(run_id)` | 生成失败分类和能力画像 |
| `compare_benchmark_runs(base_run_id, head_run_id)` | 对比 reward、失败类型、耗时、成本 |

## 结果导入契约

Harbor job 目录至少需要导入：

- `config.json`
- `result.json`
- 每个 trial 的 `config.json`、`result.json`
- `agent/trajectory.json` 或 agent log
- `verifier/reward.txt`
- `verifier/test-stdout.txt`
- `verifier/test-stderr.txt`
- `artifacts/manifest.json` 和实际 artifacts，如果存在

导入失败不能默默吞掉；必须记录为 `partial_import`，并指出缺失文件。

## Failure Taxonomy

| Class | Meaning | Product owner |
| --- | --- | --- |
| `planning` | 任务分解或策略错误 | agent loop |
| `context` | 没读到关键文件、说明或环境状态 | context builder |
| `tool-use` | 命令、文件编辑、路径、shell 使用错误 | tool runtime |
| `verification` | 没运行正确测试或误判完成 | verification engine |
| `policy` | 权限策略阻碍或放行不当 | permission system |
| `environment` | Docker/dependency/network/resource 问题 | runner |
| `long-horizon` | 长任务中途漂移、重复、遗忘目标 | scheduler + memory |
| `model-limit` | 模型能力、上下文或预算不足 | model routing |

## 兼容性

- 普通 CodeFactory session 不自动启用 benchmark policy。
- 旧 session、tool_call、learning_events 不需要迁移。
- 新 SQLite 表必须 idempotent 创建。
- 没有 Harbor 或 Docker 时，Benchmark 页面显示 blocker，不影响普通聊天和项目任务。

## 安全

- 不保存 API key、provider token、Docker registry credential。
- 不把 benchmark task text、hidden tests、solution、canary 写入 long-term memory、默认 prompt、skills 或示例库。
- Harbor job artifacts 可能包含模型输出和任务文件；导出前必须提示用户检查敏感内容。
- 官方 submission 要求不能修改 timeouts/resources；产品 UI 必须把这些字段锁定或标红。

## 测试策略

- Unit: benchmark profile parsing、command generation、result import、failure taxonomy。
- Integration: fake Harbor job fixture 导入。
- CLI smoke: `harbor run -d terminal-bench/terminal-bench-2-1 -a oracle -l 1` 或等价可用 smoke。注意 Harbor 0.15.0 中 `-l` 是 task 数量上限，`-k` 是每个 trial 的 attempts。
- Product smoke: CodeFactory UI 导入 job，展示 run summary 和 trial failure。
- Regression: 同一 subset 对比两个 CodeFactory builds 的 reward 和 failure delta。
