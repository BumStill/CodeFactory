# AI Coding OS 控制面架构设计

## 架构目标

CodeFactory 增加一个 `control-plane` 聚合层，把现有分散模块统一暴露成可查询、可测试、可展示的快照。

```text
Desktop UX
  -> Control Plane Page
      -> get_control_plane_snapshot(cwd?)
          -> Authority scanner
          -> Memory proposal reader
          -> Capability inventory
          -> Delivery gate scanner
              -> existing SQLite / settings / filesystem / git state
```

## 模块边界

| 模块 | 职责 | v1 数据来源 |
| --- | --- | --- |
| Authority scanner | 发现项目和仓库规则 surface | `AGENTS.md`、仓库约定的 `docs/specs`、`.githooks/pre-commit`、`.github/workflows/auto-release.yml` |
| Memory proposal reader | 汇总待审核记忆/偏好 | SQLite `learning_events` |
| Capability inventory | 汇总可用能力和启用状态 | Skills、MCP settings、Knowledge libraries、Hooks、Git remotes |
| Delivery gate scanner | 汇总交付风险和门禁 | git branch/status、sync gate、release cadence、latest release workflow files |
| Desktop page | 展示控制面状态和下一步入口 | React page + Tauri command |

## 后端接口

新增 Tauri command：

```rust
get_control_plane_snapshot(cwd: Option<String>) -> ControlPlaneSnapshot
```

核心类型：

```rust
struct ControlPlaneSnapshot {
  generated_at: String,
  cwd: Option<String>,
  authority: Vec<ControlPlaneItem>,
  memory: MemoryProposalSummary,
  capabilities: Vec<CapabilitySummary>,
  delivery: DeliverySummary,
  risks: Vec<ControlPlaneRisk>,
}
```

Git delivery observation 是 additive contract：

```rust
struct GitProbeSummary {
  status: ok | partial | not_repository | unavailable | not_checked,
  timeout_ms: u64,
  timed_out: Vec<String>,
  failed: Vec<String>,
}
```

所有 Git probe 使用 Tokio child process 和 2000ms timeout。实现必须显式 `spawn`、并发读取 stdout/stderr，并对 `child.wait()` 计时；超时后终止整个进程树、`wait` 回收直接 child，并有界结束管道 reader，避免后代进程继续持有 stdout/stderr 让请求卡住。`kill_on_drop(true)` 只作为任务取消或 panic 的兜底。probe 固定 `LC_ALL=C`、`LANG=C`、关闭 pager 和 terminal prompt，避免本地化错误误分类或隐式交互。仓库识别完成后，branch、status、hook config 和 tag probe 并行执行；单项失败只降低 `git_probe.status`，不使 `get_control_plane_snapshot` 整体失败。

### Authority

每个 authority surface 返回：

- `id`
- `label`
- `status`: `present | missing | warning`
- `path`
- `detail`

v1 不解析所有规则内容，只判断 surface 是否存在、是否能承担当前控制面职责。

### Memory

复用 `learning_events`：

- `pending`
- `accepted`
- `rejected`
- `preference_pending`
- `latest_pending`

v1 不新增 memory 表。后续如需更严格 lifecycle，可以从 `learning_events` 迁移到 `memory_proposals`。

### Capability

每种 capability 统一为：

- `id`
- `label`
- `total`
- `enabled`
- `status`
- `detail`

v1 类型：

- Skills
- MCP servers
- Knowledge libraries
- Hooks
- Git remotes

### Delivery

Delivery summary 返回：

- `git_branch`
- `is_dirty`
- `dirty_count`
- `sync_gate_present`
- `sync_gate_configured`
- `release_workflow_present`
- `auto_release_present`
- `latest_release_tag`
- `git_probe`

`latest_release_tag` v1 从本地 tag 推断；GitHub release 是否公开仍由发布流程用 `gh release view` 验证。
`sync_gate_present` 只表示 `.githooks/pre-commit` 文件存在；`sync_gate_configured` 还要读取当前仓库的 `core.hooksPath`，确认本 checkout 真正在使用版本化 hook。
`git_probe` 区分完整、部分、非 Git 目录、Git 不可用和未执行；timeout/failed probe names 同时进入 Risks，前端不得用 `not a git repo` 覆盖其他失败类型。

## 存储策略

v1 不新建核心表，原因：

- 当前控制面需要先统一视图。
- 已有 `learning_events`、settings、knowledge、skills、hooks、git_remote 足以支撑 v1。
- 先避免 schema 迁移扩大风险。

后续 v2 再引入：

- `authority_rules`
- `memory_proposals`
- `capability_sources`
- `workflow_runs`
- `evidence_ledger`

## 安全与权限

- 后端只读取当前 cwd 内的规则文件和仓库状态。
- 不读取 secret。
- Git remote 只统计配置数量，不返回 token。
- 控制面不执行 mutating action；所有修改仍回到对应页面或显式流程。

## 兼容性

- 没有 cwd 时返回全局 capability 状态，Authority/Delivery 明确标记缺少项目上下文。
- 非 git repo 时 Delivery 返回 warning，不报错。
- 老用户没有 `.githooks/pre-commit` 时显示 missing/warning，但不阻塞 app 启动。

## 测试策略

- Rust 单元测试：临时目录模拟 authority surface、git repo、dirty 文件、缺失 hook。
- TypeScript/React 测试：ControlPlanePage 渲染 snapshot，展示四类状态和风险。
- 现有构建验证：`pnpm vitest`、`pnpm build`、`cargo test` 或 CI 中 `Cargo test`。
