# README 更新机制：业务设计

## 问题

README 是用户发现、安装和判断产品边界的入口，但此前更新依赖作者“顺手记得改”。
在连续能力合并和按批次发版时，README 没有明确 owner、触发条件或复核周期，造成
公开承诺落后于真实 app，且发布时容易产生无价值的版本噪音。

## 目标与非目标

目标：让每个 PR 对 README 影响作出可审计判断；让高风险用户契约变化与代码同 PR
落地；让低价值内部变更不产生 README 噪音；在发版之外提供周期性 stale review。

非目标：自动生成完整 README、把 Release notes 复制到 README、按 tag 自动修改正文、
用静态检查替代产品 owner 对能力描述的判断。

## 业务决策

用户可见能力、安装/更新、平台/provider、数据/隐私、安全和公开 roadmap 变化必须
选择 `README-Update: required` 并在同一 PR 更新 README；其他变更选择 `reviewed` 并
说明理由。发版只处理版本文件和 Release notes。每月复核 issue 负责发现遗漏，但不
直接改正文。

## Primary User Path

作者修改产品 → 阅读 PR 模板中的 README contract → 选择 `required`/`reviewed` 并写理由
→ CI 检查静态契约和 diff → review/merge → Release notes 按既有 cadence 生成；每月
owner 处理复核 issue，必要时另开 README PR。

## 验收

- 真实 PR body 缺决策、重复决策或占位理由时 CI 失败。
- `required` 且未改 README 的 PR CI 失败；`reviewed` 的内部 PR 可通过。
- 版本 bump PR 不修改 README 仍可通过。
- 发布不会自动产生 README commit，月度 review issue 可重复执行且不重复创建。
