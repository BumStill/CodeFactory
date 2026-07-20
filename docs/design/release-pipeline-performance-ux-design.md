# CodeFactory 发布流水线性能：UX 设计

## 用户体验

本改动没有新增产品界面。用户仍通过应用内更新或 GitHub Release 获得同样的 Windows/macOS 产物；变化是功能合并后等待正式版本的时间缩短。

## 发布者体验

- `Auto Release` 继续作为按需和每日批量发版入口。
- 版本 commit/tag push 成功后，任务日志明确显示被启动的 `Release` tag。
- `Release` 的 job 图中 Windows 与 macOS 并列，方便直接识别是否真实并行。
- `prepare-release` 独立显示 draft 创建、复用或“已公开不可覆盖”的状态。
- 失败后可用同一 tag 手动重跑，无需重新 bump 版本；若 release 已公开则必须走新版本。

## 状态语义

| State | Meaning |
| --- | --- |
| `prepare-release` success | tag 已校验，draft 可供双平台上传 |
| 单个平台 success | 该平台产物和预发布 smoke 通过，不代表版本已公开 |
| `finalize` success | 双平台资产和 updater manifest 已公开 |
| `verify-published-macos` success | 用户公开下载路径上的 macOS DMG 已复验 |

不得把“并行 job 已启动”表述为发布完成，也不得把首次 main-scoped 冷缓存写入表述为已经产生跨版本命中。
