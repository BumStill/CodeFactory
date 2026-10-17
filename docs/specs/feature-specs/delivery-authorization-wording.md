# M48：任务消息里有问句（如「能否…」）就丢了交付授权

## 已批准任务（原文）

## 现象（10-09 真机，需先确认是否仍存在）
- 会话 d074ba4a：开工消息明确写「授权交付，ceiling: pr_only」，但因其中有「先核实个人账户仓库能否用 merge queue」，`sessions.delivery_authorized` 仍为 0——交付意图被问句线索压过。
- 违反原则：只许从措辞猜「姿态」，不许猜「写权限」。显式授权语句必须胜过任何问句/犹豫线索；后台入口现在显式传 delivery_authorized，但界面输入路径仍走措辞推断。

## 需求
- R0 用合成 fixture 复现界面输入路径（或证明已不可能）；已修复则补回归测试锁住并说明来源。
- R1 消息中出现明确的交付授权声明（如「授权交付」「ceiling: merged/pr_only」）时，delivery_authorized=1，与同条消息里的问句、疑问词无关。
- R2 授权判定不再依赖问句关键词降级；如保留姿态推断（计划/执行），它只影响姿态，不影响写权限。
- R3 测试：授权句 + 「能否/是否/可不可以」问句 → 已授权；无授权句的纯咨询 → 未授权；授权句在消息末尾/中间/英文写法各一条。先失败。

## 约束
- 不改变无授权时的默认（不交付）。合成 fixture，不写生产 DB。

## Requirements Traceability

| Req ID | 实现与最低证据 |
| --- | --- |
| R0 | 合成 SQLite `sessions` fixture 走与 `send_message` 相同的解析函数 `resolve_turn_delivery_authorization`；`gui_input_path_keeps_delivery_authorization_when_message_also_asks` 修复前 `delivery_authorized=0`（期望 1），修复后通过 |
| R1 | `is_explicit_delivery_authorization` 识别中文授权声明、`ceiling: merged/pr_only/through_*` 与英文写法；授权句存在时 `contract.capability=Deliver`，`sessions.delivery_authorized` 落 1 |
| R2 | `is_delivery_request` 先判授权、后判姿态线索；`planning_cue_present` 只保留「只分析 / 不要修改」这类显式只读约束作为权限否决，普通问句/「分析一下」不再降级 |
| R3 | 单元测试矩阵：授权+能否/是否/可不可以、无授权纯咨询、授权句在末尾/中间、英文写法；全部合成文本 |
| 约束 | 无授权句时仍默认不交付（`plain_consultation_without_authorization_stays_undelivered`）；`is_delivery_revocation` 仍可显式撤回 |

## Applicable Harnesses

Spec Harness；Compatibility Harness（不改变 DB schema 与既有 `delivery_authorized` 语义）；Observation Harness（会话持久化字段可核对）；AI Collaboration Harness。

## R0 结论与 Implementation Notes

- 修复前根因：`is_delivery_request` 在匹配交付线索前先调用 `planning_cue_present`，该表包含「分析一下 / 给个方案」等**姿态**线索；一条既写「授权交付」又写「先核实…能否…，再分析一下」的消息因此被判成非交付，`sessions.delivery_authorized` 保持 0。
- 修复方式：新增显式授权识别，并让它先于姿态线索；姿态线索只在消息同时含显式只读约束（只分析 / 不要修改 / don't implement 等）时否决写权限，问句本身不再影响授权。
- 界面路径（`send_message`）与匿名路径（`send_message_anonymous`）改为共用 `resolve_turn_delivery_authorization`，避免两处推断漂移；`delivery_stop_fences` 的清除条件仍只在「本轮新消息明确授权」时触发。
- 后台 dispatch 入口继续使用结构化 `delivery_authorized`，本次不改变其语义。
- 未修改场景注册表、validator 与 `.github/` 工作流。
- 真机验收由编排方在发版安装后完成：新建会话发送同时含「授权交付，ceiling: merged」与「能否…」问句的开工消息，检查会话 `sessions.delivery_authorized=1` 且交付动作不被拒绝；再发一条纯咨询消息（无授权句）确认仍为 0。
