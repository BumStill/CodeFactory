# M265：拒绝审批时附一句说明，模型能在被拒结果里看到

## 现场（2026-11-08）

- M261 规格 R4 要求 dispatch 的 `resolve_approval` 支持可选字段 `note`，但没有实现：
  `{"operation":"resolve_approval","approval_id":"…","approve":false,"note":"…"}` 返回
  `unknown field note`（`Request` 使用 `deny_unknown_fields`）。
- 编排方拒绝审批时只能另发一条 steer 讲原因。模型先拿到的是一个没有理由的「被拒」，
  常常换个办法重试同样的事（11-07 M260 的 `/dev/null`、钩子权限，11-08 M264 的
  `git reset --hard`）。
- 结果：一条本来只需一句解释就能收敛的拒绝，变成了又一轮试探 + 又一次审批。

## 用户决定

- 用户在拒绝时写一句说明，这句话必须和「被拒」一起到达模型，而不是另开一条消息。
- 说明只是话，不是权限：写不写、写什么，都不改变这次审批的判定结果，也不带来任何新授权。
- 不写说明时行为与现在完全一致。
- 界面上也能写这句说明（同一路径），并且要能在亮/暗两种主题、桌面和手机宽度下用。

## 决策

1. **说明随判定同行**：进程内审批通道从 `bool` 改成 `PermissionDecision { allow, note }`，
   拒绝分支把 note 拼进模型看到的那条被拒结果文本里，并明确标注「这是用户给的说明」。
2. **只有一条路径**：dispatch 的 `resolve_approval`、界面的审批卡片、界面 store 都调用同一个
   `respond_to_permission(intent_id, allow, note)`；不新增第二条「发说明」的旁路。
3. **长度受限**：note 只接受最多 1000 个字符（去掉首尾空白后），空字符串等价于没写；
   超长在入口就被拒（`invalid_request`），不静默截断。
4. **说明不是授权**：note 不进入策略判定、不写进 `permission_intents` 的判定字段、
   不参与 `action_signature`，也不影响允许/拒绝的记账。批准时带 note 只作为附带说明。

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| R1 | dispatch `resolve_approval` 接受可选 `note`（字符串，≤1000 字符）；批准和拒绝都可以带；超长返回 `invalid_request` | `src-tauri/src/headless_dispatch.rs` 单元测试（解析 + 边界校验）；`src/lib/desktopDispatch.note.test.ts` |
| R2 | 拒绝带 note 时，模型拿到的被拒工具结果包含这段说明，并标明是用户给的说明 | `src-tauri/src/agent/permission_gateway.rs` 单元测试：拒绝内容含 note 原文与说明标记 |
| R3 | 界面审批卡片拒绝时可填说明，走同一路径；通过 Viewport Harness | `src/components/PermissionDialog.test.tsx`；`src/stores/chat.permission.test.ts`；真浏览器 headless 验收（亮/暗、桌面/手机宽度） |
| R4 | note 只作为说明给模型看，不改变任何权限判定，也不带来新授权 | permission_gateway 单元测试：带 note 的拒绝仍是 `DeniedByUser`，允许仍是允许；note 不写入 intent 判定列 |
| R5 | 不写 note 时行为不变 | permission_gateway 单元测试：无 note 的拒绝文本与当前常量逐字相同；dispatch 无 note 的解析结果与现在一致 |

## Applicable Harnesses

- **Spec Harness**：本文件 + PR body 的 Req↔测试映射。
- **Compatibility Harness**：`resolve_approval` 新增字段是可选的（`#[serde(default)]`），
  旧编排方的请求体仍然被接受；`respond_to_permission` 的 `note` 也是 `Option`，旧界面调用不变。
- **Viewport Harness**：审批卡片新增说明输入框，需在亮/暗主题、1280 与 390 宽度下真实浏览器验收。
- **Observation Harness**：dispatch 审计行记录 `approval_id` 与是否带说明（不写说明正文以外的内容）。
- **AI Collaboration Harness**：本规格由编排方给出，实现只覆盖 R1–R5；规格与代码冲突时写 Spec Amendment，不自行改需求。

## 测试矩阵

| 场景 | 层级 | 断言 |
| --- | --- | --- |
| dispatch `resolve_approval` 带 note 解析成功 | Rust 单元 | `note` 落到 `Request::ResolveApproval.note` |
| dispatch note 超长 | Rust 单元 | `invalid_request`，错误里点名 `note` |
| dispatch 不带 note | Rust 单元 | `note == None`，与旧请求等价 |
| 带 note 拒绝 | Rust 单元（permission_gateway） | 结果 `Deny`，`reason == DeniedByUser`，`content` 含 note 原文与「user note」标记 |
| 不带 note 拒绝 | Rust 单元（permission_gateway） | `content` 与当前常量逐字相同 |
| 带 note 允许 | Rust 单元（permission_gateway） | 仍为 `Allow`，note 不改变判定 |
| 前端解析 note | Vitest | `note` 透传到 `resolveApproval`，缺省为 `undefined` |
| store 拒绝带 note | Vitest | `invoke("respond_to_permission", { intentId, allow:false, note })` |
| 卡片填写说明后拒绝 | Vitest（RTL） | `onDeny` 收到输入框内容；不填则为空 |
| 卡片真浏览器渲染 | headless 验收 | 亮/暗 × 桌面/手机截图，说明输入框可见可用 |

## Implementation Notes

- note 只在「有活着的进程内等待者」的路径上送达模型；应用重启后靠 `reconcile_orphaned_response`
  结算的孤儿路径仍然只记 `denied`，因为 `permission_intents` 没有存放说明的列。这条留作后续
  （需要一次迁移），本轮不改表结构。
- 长度限制在 dispatch 边界与 `respond_to_permission` 命令两处共用同一个归一化函数，
  避免只在一个入口上限制。

## 实测记录（本地）

- `cargo test --lib m265`：3 passed（带 note 拒绝 / 无 note 拒绝逐字不变 / 带 note 允许仍为允许）。
- `cargo test --lib` 全量：2377 passed / 0 failed / 12 ignored。
- `npx vitest run`：167 files / 1135 tests pass（含 PermissionDialog、chat store、desktopDispatch 新用例）。
- `npx tsc --noEmit`：pass（含 `src/acceptance/m265-reject-note.tsx`）。
- `node scripts/verify-m265-reject-note-headless.mjs`：该会话沙箱不允许 Chrome 启动（Mach port 被拒），
  本地未取到截图；脚本与验收页已入库，由 CI 执行。
- `validate_repo_governance_baseline.py`：pass；`validate_scenario_test_governance.py --ci --base-ref origin/main`：OK（33 scenarios）。
