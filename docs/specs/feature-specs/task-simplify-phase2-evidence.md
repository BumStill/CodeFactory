# 阶段2剩余分片验收证据（2026-10-20）

## 覆盖需求
R1/R7 四态权威存储、R4 心跳监督、R5 连续失败与模型恢复、R6 界面投影、R8 回放。

## 机器证据
- `cargo test --lib thin`：55 passed（含 `phase2_state_tests`、`settle_tests`、既有 thin 回归）。
- `cargo test --lib`：完整 Rust 单测。
- `npx tsc --noEmit`：通过。
- `npx vitest run src/lib/thinState.test.ts src/components/MessageList`：95 passed。
- `pnpm build`：前端生产构建通过。

## 真机/回放语义
- 最终回答存在且无外部等待 → `ended`，写入一次，不重写答案。
- 外部等待有唤醒源 → `waiting_wake`；无唤醒源/自我编造确认 → `waiting_user` 并给出问题。
- 同一技术失败 3 次 → `ended`，reason 用人话写明失败；模型不可用 → `waiting_wake`。
- 唤醒源 30 分钟后消失 → 心跳转 `waiting_user` 并说明「已失效」。
- thin 结算不产生 `objective_remediations`/recovery 行，界面不显示 legacy 失败码。
