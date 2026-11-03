# Unattended smoke：重启阶段共享同一个可配置等待预算

## 问题

`src-tauri/src/agent/unattended_smoke.rs`（E2E-001「Unattended long-task cross-process smoke」的驱动）
里有两类等待：

- `wait_for_fault_point` 使用 `worker_observation_budget()`：默认取
  `codefactory_agent_core::smoke_worker_budget_secs`（120s），并可用
  `CODEFACTORY_SMOKE_WORKER_TIMEOUT_SECS` 覆盖（负向测试需要短 deadline）。
- phase-two / phase-three / phase-four 的重启等待各自硬编码 `Duration::from_secs(45)`。

硬编码的后果在 2026-11-03 的一次真实失败里出现：PR #962 的 `scenario-execution`
在 Windows 冷 runner 上以
`Unattended long-task smoke failed: phase-two worker did not settle within 45s`
退出，`execute-windows` 失败，随后 `receipt` 缺收据、`scenario-gate-pr` 判定执行工作流失败
（三项检查是同一根因的级联，不是三个独立缺陷）。同一文件里 `worker_observation_budget()`
的文档已经写明冷 runner 的现实（大 debug 二进制加载 + SQLite + provider fixture），
但重启阶段没有继承这条结论，也没有任何可调旋钮。

## 需求与追踪

| Req ID | 需求 | 实现 | 验证 |
|---|---|---|---|
| CF-USB-1 | 重启阶段等待必须取用同一个可配置预算，不得硬编码数值 | `unattended_smoke.rs` 三处 phase 等待改为 `worker_observation_budget()` | `cargo check --bins`（该文件只在非 `cfg(test)` 的正式可执行文件中编译）+ E2E-001 场景执行 |
| CF-USB-2 | 默认预算必须严格大于曾失败的 45s 墙，覆盖值仍可下调 | `codefactory_agent_core::smoke_worker_budget_secs` 语义不变（默认 120s、非法覆盖回落默认） | `agent-core` 单测 `the_smoke_worker_budget_leaves_room_for_a_cold_runner`（新增 `> 45` 断言） |
| CF-USB-3 | 失败必须可诊断：超时要能区分「产品卡住」与「预算不够」 | 超时消息保留 phase 名与预算值，且预算可用环境变量调整 | 失败日志中的 `phase-two worker did not settle within <budget>` |

## 关键约束：这个 driver 不能被单元测试覆盖

`src-tauri/src/unattended_smoke_cli.rs` 以 `#[cfg(not(test))] #[path = "agent/unattended_smoke.rs"] mod driver;`
引入该文件，原因写在文件注释里：parent 与内部 worker 必须是**同一个正式可执行文件**的副本，
不能替换成测试 EXE。因此该文件在 `cargo test` 构建中根本不参与编译，
任何写在它内部（或按 `--lib` 过滤）的测试都不会运行。断言必须放在
`src-tauri/crates/agent-core`（预算旋钮所在 crate）或场景执行侧，
不能放进这个 driver 文件里假装被覆盖。

## Primary User Path

不新增用户可见路径。这条预算只影响 E2E-001 场景在 CI 冷 runner 上的判定质量：
用户侧看到的是「同一次会话恢复后 Objective 收敛为 completed、两条 tool 调用折叠到一张
durable receipt」，而不是因为 runner 排队抖动导致的假失败。

## Applicable Harnesses

- Spec Harness（本文件）
- Release Harness：`scenario-execution` / `scenario-gate-pr` 是合并必需检查，本缺陷直接表现为必需检查假红。
- Observation Harness：失败可见性依赖 `phase-*` 超时消息与 `error: process didn't exit successfully` 行。

## 测试矩阵

| 层 | 命令 | 断言 |
|---|---|---|
| 单元（可运行） | `cargo test -p codefactory-agent-core smoke_worker` | 默认值 > 45s，覆盖值可生效，非法覆盖回落 |
| 编译（driver 唯一的编译期覆盖） | `cargo check --bins` | `#[cfg(not(test))]` 的 driver 与正式二进制一起编译 |
| 场景 | `scenario-execution` → E2E-001 / HLT-001 | Windows/macOS 上 smoke 通过并产出 exact-head receipt |

## Evidence Pack Requirements

- 失败日志原文（含 `phase-two worker did not settle within 45s`）与对应 run/job id。
- 修复后同 head 的 `execute-windows` 成功与 `receipt` 聚合成功。
- `cargo check --bins` 与 `agent-core` 单测输出。

## 兼容性与发布边界

- 不改任何用户可见行为、协议字段或存储 schema。
- 环境变量语义不变（`CODEFACTORY_SMOKE_WORKER_TIMEOUT_SECS`）。
- 单独的效果是：冷 runner 上多等一会儿换取真实判定；通过路径不额外消耗时间（等待是条件轮询）。
