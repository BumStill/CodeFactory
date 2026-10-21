# README 更新（10-21 已发布能力）

按仓库 README 约定（README-Update 规则）更新 Features / Roadmap，使其与已发布版本一致（v1.91.13）：
- web_fetch 读网页（HTML/JSON/PDF，压缩页面，受管浏览器回落）；
- 两级权限：本地改动随写权限，远端动作由会话级「允许交付」+ 一键批准；只读命令按片段判定、信任模式免审批；
- 会话四态（运行中/等唤醒/等你/已结束）、从事实推导、重启自动续跑、外部事项完成后自动收尾；
- 无项目对话可上传附件、对话内文件链接可打开；
- 浏览器扩展重连加固与连接诊断；
- 合并队列与「立即发版」由派单携带。
只写对用户有意义的能力，不写内部机制细节；Roadmap 删掉已完成项。授权交付（ceiling: merged）。

## Implementation Notes

原任务全文保留如上；未提供单独的 Req ID 或指定规格路径，以下仅作验收索引，不修改原要求。

| Req ID | 要求 | 文档验收 |
| --- | --- | --- |
| README-01 | 读网页 | Features 覆盖 HTML/JSON/PDF、压缩页面和浏览器回落 |
| README-02 | 两级权限 | 区分本地写权限、远端会话授权、一键批准和信任模式只读命令 |
| README-03 | 会话状态 | 覆盖四态、实际任务进展、重启续跑与外部事项收尾 |
| README-04 | 附件与文件链接 | 无项目对话上传附件及打开对话文件链接 |
| README-05 | 扩展连接 | 重连及连接诊断 |
| README-06 | 交付 | 按序合并，派单携带交付授权与立即发版选项 |
| README-07 | 常青与 Roadmap | 不固定发布版本；核对并仅保留未完成路线图项 |

- Applicable Harnesses：Spec（原文与逐项验收）、AI Collaboration（单流自检、依据既有规格）。仅文档变更，不修改权限、UI、工具、发布机制或治理规则；不触发对应产品 Harness。
- 测试矩阵：修改前执行 README 内容断言并记录失败；修改后原命令全部通过；README contract、scenario governance、仓库治理基线与 diff 检查。
- Roadmap 核对：现有 Welcome page with template projects / Linux build 不属于本次已发布清单，保留，不添加本次已完成项。
- AI Collaboration：context scope 为任务文件、模板、README 与相关已发布规格；assumption 为仅同步现有能力，不重新实现；review point 为权限边界、浏览器回落需显式调用而非自动保证、合并与发版分离；validation result 记录于 PR。
- 按模板禁用 delegate_tasks，且本次为小范围文档更新，不启用并行角色。
