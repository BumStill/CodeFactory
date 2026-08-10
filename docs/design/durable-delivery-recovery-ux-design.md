# 持久交付恢复：UX 设计

## 状态语言

| System state | 用户可见文案 | 是否终态 |
| --- | --- | --- |
| running | 正在执行当前步骤 | 否 |
| waiting | 正在等待远端状态；系统会自动续接 | 否 |
| recovering | 已接管上次未完成任务，正在核对同一 PR | 否 |
| needs_business_decision | 需要你选择会改变业务结果的方案，并展示推荐与影响 | 是，业务决策 |
| core_input_required | 缺少无法自动获取的核心外部输入；一次列全，提供后自动续接 | 否，objective 保持运行 |
| failed_internal | 自动恢复未能解决；这是系统失败 | 是，系统失败 |
| platform_incident | 平台正在修复执行环境；无需用户推动 | 否，系统持有 |
| completed | 已达到请求边界，并附证据 | 是 |

## Rules

- 心跳不得刷新“最后实质进展”；两者分开展示。
- 不显示内部 completion gate prompt，不要求用户仅回复“继续”。
- transport stream 结束但业务仍恢复时，状态卡保持 active/waiting。
- completion warning 改为进度提示；不得与绿色完成图标同时出现。
- 同一 PR 始终显示 canonical PR、当前 head、requested/reached ceiling 和下一步 owner。
- 不得用“请重试、回复继续、稍后再来”处理技术问题；业务决策卡必须含推荐方案与各选项业务影响。
- 用户已要求搞定或表示离开时不展示普通决策卡，直接采用推荐配置；核心输入卡必须一次列全，且不能提供降级完成按钮。

## Primary path acceptance

1. 用户授权交付并看到 canonical PR。
2. CI 失败后状态切到“正在修复同一 PR”，不出现完成或重复授权提示。
3. App 被结束并重启后，状态切到“已恢复”，同一 PR/head 信息不变。
4. 只有达到请求边界后出现完成状态和可核对证据。
