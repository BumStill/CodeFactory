# Workspace 顶栏收敛架构设计

## 路由归属

`App` 继续持有一级 view state。`WorkspacePage` 不再接收画像、进化、评测、资源和控制平面的打开回调，只保留会话内所需回调。`SettingsPage` 新增「功能」tab，并接收上述一级 view 的路由回调。

```text
Workspace settings button
  -> SettingsPage(capabilities)
      -> Profile / Evolution / Benchmarks / Resources / ControlPlane

Welcome usage details
  -> SettingsPage(usage)
```

设置页只承担导航聚合，不复制各能力的数据与状态。Workspace 的设置按钮显式传入 `capabilities`；其他旧入口不传初始 tab 时继续默认挂载端点页，保留 ChatGPT 模型目录刷新语义。进入能力页后沿用现有 `onBack` 返回 Workspace，避免创建第二套路由栈。

## 规范与计划

采用 `repository-owned-specifications` 已定义的实现：删除 `SpecsPage`、`stores/specs` 和后端 specs commands；Agent 从仓库级 `AGENTS.md`、`README.md`、`docs/` 和用户明确提供的文档提取意图。委派任务是会话内部行为，只有已存在 task run 时才在对话区展示执行详情。

## Welcome 用量摘要

新增 Welcome 专用 `TokenUsageTrend`，按时间从左到右渲染 28 个纵向趋势条。它与 Settings 的 `TokenUsageHeatmap` 分离：

- Welcome：1×28 趋势、左右方向键、紧凑摘要语义。
- Settings：7 行日历、上下/左右跨日历移动、日期下钻与长范围局部滚动。

两个组件共享 `UsageHeatmapDay` 和可访问文案，但不共享布局和键盘拓扑，避免再次把完整日历机械缩小。

## 兼容与回归边界

- `AppView` 与各功能页不变；仅改变入口归属。
- Settings 的 90/180/365 日地图保持 12px 方格与 7 行日历。
- 旧 task run 的 `spec_req_id/spec_title` 继续只读展示，不恢复规范产品面。
- Header 收敛不得改变模型、推理强度、Git、检查点、匿名退出和用量详情行为。
