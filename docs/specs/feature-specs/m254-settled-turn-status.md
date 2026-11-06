# M254：已结束的回合不再显示「正在做」

## 背景
用户多次报告同一条消息下同时出现「正在做」和「已结束」，并带着「会话诊断」「重置会话状态」按钮。2026-11-06 的现场是会话 db17572f：25 个回合都有 `turn_settled_at`，但回合行的 `recent_activity_kind` 仍是 `thin_running`。

## 根因
`chat_turn_state` 的状态是写入当时的快照，只有当前回合会被更新。界面却用每条消息自己的快照显示会话状态，所以旧回合和已结束的回合会一直说自己在运行。此前的修复都针对某一种卡住成因，没有处理这个读取方式。

## Requirements Traceability
| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| R1 | 已结束的回合不显示「正在做」，也不显示重置入口 | `does not say running on a turn that has settled`，修复前失败；真实浏览器验收 |
| R2 | 只有最新的回合能显示会话状态 | `keeps a stale waiting status off an older turn`，修复前失败 |
| R3 | 最新回合的等待、需要决定状态照常显示；未结束的回合照常显示「正在做」和自救入口 | `still shows waiting on the latest settled turn`、`stuck session recovery`；真实浏览器验收 |

## Applicable Harnesses
Spec Harness、Viewport Harness、Compatibility Harness、Release Harness、Observation Harness。

## 真机验收
发版安装后打开会话 db17572f 这类有多个旧回合的会话，已结束的回合下不再出现「正在做」。

## 遗留
PR 合并后会话被 `thin-source-repair` 和 `thin-closeout` 反复唤醒，是另一个问题，记在 M255。
