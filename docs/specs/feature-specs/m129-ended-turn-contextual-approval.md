# M129 — 已结束轮次后的「全部执行」必须是新回合，而不是拒绝

## 背景（P0，用户报告）

用户在会话里得到「5 项待办」后回复「全部都执行」，CodeFactory 直接拒绝：

```
这段历史记录缺少可续接的任务身份，无法安全继续；本次未启动新任务。
```

同一位置还有第二个拒绝分支：`CHAT_OBJECTIVE_IDENTITY_AMBIGUOUS: system
reconciliation required`。两者都把系统侧的身份记账问题写进了用户会话，并且都是**死路**：
用户无论再怎么补充信息都无法继续，会话被永久卡住。

## 需求

- **R1**：上一轮已结束（Objective `completed`）时，用户发送上下文确认（例如「全部都执行」）
  必须开启新回合并正常执行，不得拒绝、不得报错。
- **R2**：开启新回合时必须带上该会话的完整历史，模型能看到上一轮提出的待办。
- **R3**：续接身份不唯一（`Ambiguous`）属于系统侧记账状态，不得抛错给用户；按「新回合 +
  完整历史」继续，两个候选 Objective 原样保留，交由系统对账（`系统自动重试/自愈`），
  不删除、不覆盖、不要求用户做技术选择。
- **R4**：轻量会话（thin path）与项目会话（GUI 主路径）行为一致。
- **R5**：所有断言该报错出现的测试，改为断言「新回合开始」；`schema_matrix` 的
  `outcome` 从 `explicit_terminal` 统一为 `continued`。

## 决策

| 决策点 | 结论 | 理由 |
| --- | --- | --- |
| 身份缺失时 | 新回合 | 历史已在上下文中；凭空造一个「续接身份」会伪造因果 |
| 身份歧义时 | 新回合，两个候选保留 | 绑定到任一候选都是猜；保留候选让对账有据可依 |
| 拒绝措辞 | 完全删除 | 系统状态不得以技术措辞出现在用户会话中 |
| 已结束 Objective | 不重开 | 终态不可回退（与 M102 一致） |

## 受影响 Scenario

`docs/testing/scenario-registry.json` 按 `change_patterns` 命中的 ID（本次改动文件）：
`HLT-003`（直接覆盖：历史 session 中简短继续精确续接）、`HLT-004`、`HLT-005`、
`CXD-001`、`RTE-004`；复杂 E2E：`E2E-001/002/003/004/007/009`。
其中 HLT-003 的语义在本改动后是「新回合 + 完整历史」（不再是拒绝），本 spec 即其权威说明。

`docs/testing/scenario-registry.json` 属于信任根（trust-root），普通 PR 不得自改；本次新增的两个
测试登记在该文件的 `automated_by` 需要外部 governance bootstrap 单独落地，本 PR 只保留 spec 说明。

## 证据

- 单测：`commands::chat::tests::contextual_approval_after_an_ended_turn_starts_a_new_turn`、
  `commands::chat::tests::ambiguous_objective_identity_never_refuses_the_message`
  （真实 `admit_persisted_chat_turn` + 真实 SQLite schema）。
- 历史矩阵：`src-tauri/src/agent/history_session_smoke/schema_matrix.rs`，
  version 1（无身份历史）现在断言新回合 + 新 root + 无副作用。
- 安装产物 smoke：`cargo test --test history_schema_matrix`（跑真实 `codefactory` 二进制）。
