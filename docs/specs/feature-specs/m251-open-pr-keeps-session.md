# M251：升级重启后，PR 还开着的会话被结束

## 背景
2026-11-06 21:31，安装 v1.97.0 重启后，M249、M250 两个会话的 objective 都变成 `legacy_orphan`。M241 新增的 `settle_objectiveless_wake` 看到没有未结算的 objective，就把 M250 的会话结束了，而它的 PR #1010 还开着，并且被合并队列退回。

## 根因
`legacy_orphan` 是重启时退休 objective 的状态，不代表任务完成。瘦会话的 PR 记录在 `thin_delivery_refs`，不在 `delivery_runs`，所以 M241 用的 `open_delivery_objective_id_in_tx` 查不到这个 PR。

## Requirements Traceability
| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| R1 | 会话还有未完成的 PR 时，系统唤醒不结束会话，也不取消其它唤醒 | `m251_open_pr_keeps_session_when_objective_is_retired`，修复前失败 |
| R2 | PR 已完成且 objective 已结算时，M241 的结束行为不变 | `m251_finished_pr_still_ends_a_settled_session` 和全部 `m241_` 测试 |

## Applicable Harnesses
Spec Harness、Compatibility Harness、Release Harness、Observation Harness。

## 真机验收
PR 未合并时安装新版，该会话不被结束，保持等待 PR。

## 遗留
重启为什么退休还在等 PR 的 objective，以及系统唤醒被 objective 状态拒绝后如何继续处理开着的 PR，记在 M247。
