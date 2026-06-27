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
      -> Capability Profile
```

## 模块边界

| 模块 | 职责 | v1 数据来源 |
| --- | --- | --- |
| Benchmark Registry | 固化 benchmark 名称、版本、官方命令、约束和证据要求 | 本地 profile + 官方链接 |
| Harbor Environment Probe | 检查 `harbor`、Docker、provider key、磁盘空间和网络 | shell command + config |
| Harbor Runner | 生成 job config，执行 smoke/subset/full run，记录 stdout/stderr | `harbor run` |
| CodeFactory Agent Adapter | 让 Harbor 以自定义 agent 方式调用 CodeFactory | Python adapter + headless CodeFactory runner |
| Benchmark Policy | 在 sandbox 内自动允许合理文件/命令操作，同时保留 hard deny | settings + tool permission engine |
| Run Ledger | 保存 run、trial、artifact、成本、版本和 policy | SQLite |
| Result Ingestor | 解析 Harbor job 目录、trial reward、trajectory 和 verifier 输出 | `jobs/<job>` |
| Failure Classifier | 将失败归类为可改进产品能力 | trajectory + verifier + tool audit |
| Capability Profile | 生成按类别、工具、失败类型的能力画像 | run ledger + classifier |

## Agent 接入方式

### v1: External Agent Adapter

优先实现 Harbor `BaseAgent` adapter。它通过 Harbor `BaseEnvironment.exec` 与任务容器交互，调用 CodeFactory headless runner 决策下一步命令和文件操作。

当前 adapter 是 `codefactory_bench.agent:CodeFactoryAgent`。历史第一步跑通的是 no-model baseline；当前实现已增加 model-backed headless loop：仅从显式 `CODEFACTORY_BENCH_*` 读取模型配置，通过 OpenAI-compatible chat-completions 生成 `run_shell` tool call，经 `benchmark-sandbox` command gate 后用 Harbor `BaseEnvironment.exec` 在 task container 内执行，并写出 trajectory。

优点：

- 不需要把完整桌面 app 安装到每个 task container。
- 更容易收集 trajectory、tool calls、policy decision 和 stdout/stderr。
- 和 Harbor 官方自定义 agent 入口匹配。

限制：

- 真实 model-backed 分数需要显式 benchmark model env；不能隐式读取用户桌面设置、keychain 或通用 provider env。
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

## 数据模型

```rust
struct BenchmarkRun {
  id: String,
  benchmark_id: String,
  dataset: String,
  dataset_version: String,
  agent_name: String,
  agent_version: Option<String>,
  model: Option<String>,
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
