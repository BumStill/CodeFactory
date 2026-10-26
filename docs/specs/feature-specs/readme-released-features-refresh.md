# README 已发布功能刷新（T1）

## 背景与用户决策
- 用户偏好：README 的 Features / Roadmap 必须定期与**已发布**能力同步。
- 竞品对比的第一入口是 README；多项已经在用户手里跑起来的能力（视觉桥的页面渲染、回合中途重启续跑、额度硬上限、信任模式本地命令、PR 元数据自动生成、工作区事故自愈、唤醒可靠送达）在 README 里完全没有出现，读者会以为产品还停在上一个形态。
- 决策：本任务只改文档（`README.md` + 校验用的可执行验收），不动产品代码；不写具体版本号（README 是 evergreen 契约，版本细节属于 Release notes）。

## Requirements Traceability
| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| R1 | Features 的「Sight for text-only models」写清：`.pptx` / `.pdf` 先渲染成页面图，逐页可看，纯文本模型也能读幻灯片与扫描件 | `tools/governance/test_readme_released_features.py` 断言 Features 段包含 `page by page` |
| R2 | Features 的 Sessions 写清：被应用重启打断的回合会从断点继续，不丢当前回合 | 同一测试断言 `resumes the interrupted turn` |
| R3 | Features 的 Subscription quota reserve 写清：额度是硬上限，全部端点到达上限时按时间窗等待而不是越限；等待跨重启仍然有效 | 同一测试断言 `hard ceiling` 与 `survives a restart` |
| R4 | Features 的 Safe by default 写清：信任模式覆盖会话自身的本地 git 提交/推送，且复合命令按**每一段**分别判定 | 同一测试断言 `session-owned git` 与 `segment by segment` |
| R5 | Features 的 Controlled delivery 写清：PR 描述里的交付声明、场景覆盖与 release trailer 由真实改动集生成并校验；PR 在等 CI 时系统唤醒不会结束会话 | 同一测试断言 `generated from the change set` 与 `no longer ends a session` |
| R6 | Features 的 Automatic merge closeout 写清：事故留下的脏工作区会被自动抢救，而不是直接删掉 | 同一测试断言 `incident` |
| R7 | Features 的 Sessions 写清：唤醒与后台任务完成通知在重启后仍然送达 | 同一测试断言 `delivered after a restart` |
| R8 | Roadmap 只保留真正未完成项；已完成项必须移除，且不得出现 `[x]` | 现有 `test_roadmap_has_no_completed_items` + README 契约静态校验 |
| R9 | README 保持 evergreen 契约：无具体版本号、必需章节齐全、本地链接可达 | `validate_readme_contract.py` static 校验通过（由 `test_evergreen_contract` 调用） |

## Applicable Harnesses
- **Spec Harness**：本文件即规格；Req ID 与可执行断言一一对应。
- **Compatibility Harness**：README 是公开承诺，措辞不得超出已发布行为（例如 Windows 命名管道仍未实现、Linux 构建仍未提供）。
- 不触发 Release / Observation / Payload / Viewport Harness：本次不产出安装包、不改运行时、不改 UI 布局。

## 测试矩阵
| 用例 | 类型 | 断言 |
| --- | --- | --- |
| `test_released_features_refresh_t1` | 可执行文档验收（Python `unittest`） | Features 段同时包含 R1–R7 的关键短语；先见失败，改 README 后转绿 |
| `test_released_features_and_boundaries` | 既有回归 | 历史承诺短语仍在（防止刷新时删掉旧能力） |
| `test_roadmap_has_no_completed_items` | 既有回归 | Roadmap 无 `[x]`、仍有 `Linux build` |
| `test_evergreen_contract` | 既有回归 | README 静态契约零违规（无版本号 / 章节齐全 / 链接有效） |

## Implementation Notes
- 断言短语刻意选在 README 的**新句子**里，避免与既有措辞重复导致测试在改动前就通过（否则无法先看到失败）。
- 中文叙述留在本规格与 PR，README 正文沿用其现有英文语言。
