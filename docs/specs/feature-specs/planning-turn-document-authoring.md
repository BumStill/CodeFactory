# 规划意图下的文档产出权与长方案落盘

> Req ID: `CF-PLAN-DOC`
>
> 状态: approved
>
> 决策日期: 2026-07-30

## 背景

2026-07-30 现场报告：用户在会话里提出一个产品设计（内置浏览器 pane 的展开策略），
并要求「这种长的方案，应该生成一个文件来存储，而不是每次都展示在会话窗口里」。
Agent 识别出用户当前明确要求不修改产品，并以内部兼容类型
`TurnCapability::ReviewOnly` 表示这项动作约束。但随后它三次尝试落盘同一份规格，
全部被结构性门禁拦下，而且错误文案把动作约束描述成整回合固定模式：

| 尝试 | 结果 |
| --- | --- |
| `python3 - <<'PY' … spec.write_text(…)` | 旧文案错误声称“本回合是只读审视”并阻止 `bash` |
| `apply_patch <<'PATCH' *** Add File: docs/specs/feature-specs/…md` | 同上 |
| 放弃落盘，把完整方案平铺进会话 | 用户看到的正是他刚要求避免的形态 |

两个缺陷叠加：

1. **能力粒度错误。** `ReviewOnly` 把「当前不要改产品」等同于「什么都不许落盘」。
   而规划意图的交付物本身就是文档；禁止写文档等于禁止交付。
   `is_review_safe_named_tool` 里没有 `write_file`/`edit_file`，模型在该约束下
   连写工具都看不到，只能用 shell heredoc 硬撞，撞上的又是同一道 Mutation 门禁。
2. **门禁只挡不指路。** 拒绝消息只说「已阻止」，没有给出允许的路径。模型因此换
   三种写法重试同一个动作，最后把阻塞甩给用户处理。

`ReviewOnly` 仍是 `decide_chat_contract` 的内部默认兜底类型，但不是对整回合的永久
定性；后续消息、用户 steer 和每个动作都会重新评估，已有持续交付授权也不会被
普通追问覆盖。

## Requirements Traceability

| Req ID | 用户要求 | 规范化要求 | Surface | 验证方法 | Owner |
| --- | --- | --- | --- | --- | --- |
| CF-PLAN-DOC-R1 | 不改代码也要能写文档 | `ReviewOnly` 回合允许写入规划文档；`write_file`/`edit_file` 在该回合可见 | agent-loop policy | Rust 单元测试 | agent |
| CF-PLAN-DOC-R2 | 举一反三，不只放开一个文件 | 放行由路径白名单判定（文档目录 + prose 扩展名），不是单点特例 | agent-loop policy | 表驱动 Rust 测试 | agent |
| CF-PLAN-DOC-R3 | 明确只分析时仍不得改产品 | 代码、配置、测试、迁移与 agent 指令文件（`AGENTS.md`/`CLAUDE.md`/`README.md`）在当前动作约束下仍被拒 | agent-loop policy | 表驱动 Rust 测试 | governance |
| CF-PLAN-DOC-R4 | 不要三次撞墙后甩锅 | 结构性拒绝消息必须写明允许的工具与路径 | agent-loop policy | Rust 单元测试 | agent |
| CF-PLAN-DOC-R5 | 长方案落盘而不是平铺会话 | 系统提示要求超过一屏的方案写成仓库文档，会话只留摘要、路径与待决问题 | agent prompt | Rust prompt 断言 | agent |
| CF-PLAN-DOC-R6 | 不用 shell 写文件 | 提示与门禁一致地把落盘动作收敛到 `write_file`/`edit_file` | agent prompt + policy | Rust 单元测试 | agent |

## 能力边界（权威定义）

`TurnCapability::ReviewOnly` 下**唯一允许的 mutation** 是「规划文档写入」，其判定为
路径白名单，三个条件同时成立：

1. 工具是 `write_file` 或 `edit_file`——即写入范围被单个 `path` 参数完全限定；
2. `path` 位于文档目录之内：`docs/`、`doc/`、`design/`、`designs/`、`spec/`、
   `specs/`、`plan/`、`plans/`、`planning/`、`rfc/`、`rfcs/`、`adr/`、`adrs/`、`notes/`
   （按路径段匹配，`mydocs/` 与 `src/docsystem/` 不算）；
3. 扩展名是 `.md`、`.markdown` 或 `.txt`。

以下仍然拒绝，且属于**故意的 fail-closed**：

- `bash` 的任何 Mutation。一条 shell 命令即使指向 `docs/`，同一行仍可触及任何其他
  位置，无法用路径界定——这是把模型推回 `write_file` 的手段，不是遗漏。
- 文档目录内的非 prose 文件（`docs/config.json`、`docs/scripts/build.sh`）：
  文档树里同样存放构建脚本与 fixture。
- `AGENTS.md`、`CLAUDE.md`、`README.md`、`CONTRIBUTING.md`、`.cursorrules`、
  `copilot-instructions.md`：改这些文件**改变行为**，正是当前显式约束要防的事，
  即使它们位于文档目录之内。
- 含 `..` 的路径。

## Primary User Path

1. 用户在会话里提出设计、评估或「先给方案，不要改代码」类请求。
2. `decide_chat_contract` 判定当前动作约束，并用内部类型 `ReviewOnly` 表达；它不把
   后续整回合永久锁定。
3. 方案超过一屏时，Agent 用 `write_file` 把完整方案写入仓库文档（优先遵循仓库既有约定
   `docs/specs/`、`docs/design/`、`docs/long-tasks/`；无约定时用 `docs/plans/<slug>.md`）。
4. 会话里只留：一句问题陈述、3–6 条决策要点、文档路径、待决问题。
5. 当前消息仍明确要求不要改代码时，门禁拒绝对应动作并说明原因。
6. 用户表达执行意图后，下一动作重新评估为 `Implement`，Agent 按已落盘的文档实施。

## Applicable Harnesses

- **Spec Harness**：本文件是该能力边界的权威；`repository-owned-specifications.md`
  中「计划只在当前对话中表达」一句仅适用于**执行期任务状态**，长方案以本文件为准。
- **AI Collaboration Harness**：context scope 为 `ReviewOnly` 能力定义与系统提示；
  关键假设是「文档写入不改变产品行为」，由 R3 的排除项守住。
- **Compatibility Harness**：`capability_denial` 增加 `args` 参数，唯一调用方是
  `run.rs`；`Implement`/`Deliver` 两个分支行为不变。

## 测试矩阵

| 场景 | 期望结果 | 证据 |
| --- | --- | --- |
| 明确只分析时写 `docs/specs/feature-specs/*.md` | 允许 | `review_only_allows_the_planning_document_it_exists_to_produce` |
| 明确只分析时写 `src/main.rs`、`package.json`、`migrations/*.sql` | 拒绝 | `review_only_still_refuses_code_config_and_agent_instruction_writes` |
| 明确只分析时写 `AGENTS.md`/`CLAUDE.md`/`README.md`（含 `docs/` 内） | 拒绝 | 同上 |
| 明确只分析时用 heredoc / `apply_patch` / `echo >` 写 `docs/` | 拒绝 | `review_only_keeps_shell_writes_blocked_because_a_command_has_no_path_bound` |
| 路径白名单近似匹配 `mydocs/a.md`、`src/docsystem/a.md` | 拒绝 | `planning_document_detection_is_a_whitelist_and_fails_closed` |
| 拒绝消息内容 | 含 `write_file` 与 `docs/` | `review_denial_names_the_route_instead_of_only_saying_no` |
| 明确只分析时的工具清单 | `write_file`/`edit_file` 可见且描述含路径约束 | `review_turns_expose_the_write_tools_scoped_to_documents` |
| 三种 mode 的系统提示 | 含长方案落盘契约 | `a_long_plan_is_persisted_as_a_document_instead_of_flattened_into_chat` |

## 兼容与迁移

- `capability_denial` 的签名新增末位 `args: &serde_json::Value`；唯一生产调用方是
  `agent-loop/src/run.rs`，该处已有 `args` 在作用域内。无持久化格式、无数据库、
  无前端契约变更。
- `Implement` 与 `Deliver` 分支逐字未改，交付门禁（提交/推送/PR/发布）不受影响。
- 桌面权限策略无需改动：`write_file`/`edit_file` 原本就在默认放行名单内，
  safe 模式下仍走用户确认。

## Evidence Pack Requirements

- 失败→通过的 `policy::` 单元测试（首轮红：`is_planning_document_path` 不存在、
  编译器报 `args` 未使用，即门禁不看路径）。
- `agent::` prompt 断言，覆盖 Interactive/Execute/Autonomous 三种 mode。
- 相关 Rust 测试、前端 typecheck 与治理 validator 结果。
- PR/CI 状态；未合并、未发布时标记 `not live`。
