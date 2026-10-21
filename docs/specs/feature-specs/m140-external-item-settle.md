# M140：外部事项（PR）已完成后 thin 会话仍停 waiting_wake，且唤醒源消失后心跳未处理

## 证据（v1.91.12）
- M139 会话 aafbe027：PR #809 于 18:5x 合并并发版；thin_session_state 自 19:06:40 起为 `waiting_wake / 交付 PR / CI`，20:0x 仍未变化；该会话 running 的 background_task 数为 0、无未来 wakeup。
- M138 会话 ece03bce 同样（PR #807 合并后停在 waiting_wake「外部事项未挂监视」），需编排方发消息才收尾。
## 需求
- R1 derive_state 的「外部进行中事项」必须以**真实状态**判定：会话交付映射（thin_delivery_refs）对应 PR 已 MERGED/CLOSED 且 ceiling 已达到（merged；或 through_release 时对应版本已发布）→ 不再算外部进行中事项。由心跳（或交付映射刷新）以低频（≥180s，遵守 API 预算）查询，不在每次推导时调用 GitHub。
- R2 外部事项完成后：会话被唤醒一次做收尾（§8），收尾回答后 ended；若模型无事可做直接 ended(完成)，原因写「PR #N 已合并」。
- R3 心跳：waiting_wake 且无活着的唤醒源（无 running 后台任务、无未来 wakeup、外部事项已完成或无可监视对象）→ 立即按 R2 处理（外部事项完成）或转 waiting_user（无法判定时），不得无限期停留。
- R4 测试（先失败）：PR 合并事实注入 → 下一次心跳后会话被唤醒收尾并 ended；无唤醒源 + 外部未完成 → waiting_user；有运行中的监视任务 → 保持 waiting_wake。
## 约束
- GitHub 调用遵守全局预算（≤600/h，轮询 ≥180s）。授权交付（ceiling: merged）。不要超出规格自加范围。

## Implementation Notes
- 原任务未指定文件名，逐字保存于本文件。
- Harness：Spec、Compatibility、Observation、Release、AI Collaboration；不涉及布局，不启用 Viewport。
- 需求映射：R1 真实外部事实缓存与观测限流；R2 完成事实持久化与一次性收尾唤醒；R3 心跳基于活着的唤醒源收敛；R4 合成 SQLite fixture 回归。
- 主路径：后台心跳刷新交付事实 → 持久一次性唤醒 → 模型执行 §8 收尾 → 最终回答 ended。无需 GUI，锁屏可用。
- 真机验收由编排方发版后完成：已合并 PR 的 thin 会话在下一次低频观测后自动唤醒收尾；收尾后状态 ended，重启无重复唤醒。
- AI Collaboration：context scope 为 thin 推导、心跳、交付引用与持久唤醒；assumptions 为 GitHub 观测失败不视为完成；review point 为去重、取消优先与 release ceiling；validation result 随 PR 测试证据更新。任务模板禁止 delegate_tasks，本次单执行流。
