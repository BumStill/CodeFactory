# M127 — 同一 head 上的重新运行与阻断自愈

## 事故（2026-10-20，#769）

PR #769（M126 修复）在 head `aec05b32` 上连续三次跑 CI：

1. 第一次 `governance-baseline` 因 PR body 缺 `Scenario-Test` 而失败 → 修 body，无提交；
2. 第二次 `check-frontend` 因 README contract 缺 `README-Update:` 行失败 → 补 body，无提交；
3. 第三次同一 head 全绿（`check-frontend` 在 05:33Z SUCCESS，`scenario-gate-pr`、`check-rust`、`execute-windows` 均 pass），`mergeStateStatus=CLEAN`、`mergeable=MERGEABLE`。

但队列仍拒绝合并，原因有两处互相叠加：

- **R1（re-run 不可见）**：`evaluateEligibility` 用 `onHead.find(...)` 取该名称下的**第一条** check-run。同一 head 上的旧 `failure` 永远排在前面，后来的 SUCCESS 被忽略，队列认定该 head 永久红。
- **R2（阻断永不解除，历史缺陷而非操作要求）**：第一次失败写入 `merge-queue: blocked` 和绑定 head 的标记。旧规则对相同 head 保持阻断；body 契约修复不产生新提交，因此会遗留陈旧标签。本规格要求系统识别检查恢复并自动清除陈旧阻断，不向用户移交技术恢复动作。

两处都不是代码缺陷导致的，却让一个已全绿的 PR 卡死：这正是 M126 要消灭的「绿灯不动」的同一族问题的第二阶段。

## 需求

- **M127-R1**：对每个必需检查，判定依据必须是**该 head 上最新的一次** check-run（按 `completed_at`，其次 `started_at`），与 API 返回顺序无关。较新的 failure 仍然胜出（不得乐观取旧 success）。
- **M127-R2**：阻断标记必须记录**导致drop的检查名**（`check=<name>`，URI 编码）。
- **M127-R3**：当阻断记录中的 head 等于当前 head、且记录的检查在该 head 上最新一次为 success 时，视为**陈旧阻断**：`eligible: true` + `clearedBlock: true`，理由写明「同一 head 上该检查已恢复绿色」。
- **M127-R4**：无 `check=` 的历史标记（M101 遗留）保持保守阻断，不猜测失败原因；记录中的检查仍红或未跑时同样保持阻断。
- **M127-R5**：`headChecks` 适配器必须保留 `started_at` / `completed_at`，否则 R1 无法判定。

## 判定顺序

```
armed? → hold? → blocked?
  blocked 且有记录:
     记录 head ≠ 当前 head  → 重新评估（M101-R2，保留）
     记录 head = 当前 head:
         记录带 check 且该 check 最新一次 = success → 重新评估（M127-R3）
         否则                                     → 保持阻断
  无记录（遗留标签）: 记录时间早于 head 提交 → 重新评估；否则保持
→ 必需检查：取该 head 上最新一次运行判定 running / failed / success
→ DIRTY → terminal
```

## 测试映射

| 需求 | 测试（`scripts/merge-queue.test.mjs`） |
|---|---|
| M127-R1 | `M127 — the newest run for a head decides, not the oldest`；`M127 — a later failure still wins over an earlier success (no wishful reading)` |
| M127-R2/R5 | `M127 — runQueue records the failing check so the block can self-heal`；`M127 — the blocked marker round-trips the failing check name` |
| M127-R3 | `M127 — a block on the same head clears once its recorded check is green again` |
| M127-R4 | `M127 — the same-head block survives while its check is still red or unnamed` |

## 边界

- 重新评估不是放宽门禁：进入合并前仍要求全部必需检查在该 head 上最新一次全绿，失败会重新阻断（并记录新的 head + 检查名），因此不存在无界重试。
- 队列自身逻辑只从默认分支运行（`pull_request_target` 检出默认分支），候选 PR 无法自证。
