# Terminal-Bench 2.1 对齐 UX 设计

## UX 目标

Benchmark UX 不是营销页，也不是单纯排行榜。用户进入后应能回答四个问题：

- CodeFactory 现在能不能被 Terminal-Bench 2.1 跑起来？
- 最近一次 run 的真实结果和 artifacts 在哪里？
- 失败主要卡在哪类能力？
- 下一步应该修哪个产品能力，修完如何回归？

## 信息架构

新增 `Benchmarks` 工作区页面，作为 AI Coding OS 控制面的能力评估分支。

```text
Benchmarks
  Terminal-Bench 2.1
    Overview
    Runs
    Trials
    Failure Triage
    Capability Profile
    Environment
```

## 首屏布局

```text
┌────────────────────────────────────────────────────────────┐
│ Benchmarks / Terminal-Bench 2.1                  Refresh   │
├────────────────────────────────────────────────────────────┤
│ Environment: Harbor OK | Docker OK | Dataset 2.1 | Policy  │
├────────────────────────────────────────────────────────────┤
│ Latest Run                                                 │
│ status reward tasks pass/fail duration cost build artifact │
├────────────────────────────────────────────────────────────┤
│ Capability Profile                                         │
│ planning context tool-use verification policy environment  │
├────────────────────────────────────────────────────────────┤
│ Failure Triage                                             │
│ task name | reward | class | evidence | suggested slice    │
└────────────────────────────────────────────────────────────┘
```

视觉原则：

- 紧凑表格、状态 badge、分段 controls，不使用 hero。
- 重点是 evidence path、failure class、diff between runs。
- 分数展示为事实，不做鼓励性文案。
- 长路径、任务名、模型名必须可换行或截断，悬停显示完整值。

## 关键流程

### 1. 环境检查

用户打开 Terminal-Bench 2.1 页面：

- 系统检查 `harbor --help`。
- 系统检查 Docker 是否运行。
- 系统显示 dataset：`terminal-bench/terminal-bench-2-1`。
- 系统显示官方约束：不能修改 timeouts/resources。
- 缺失项显示 blocker 和下一步命令。

### 2. Smoke Run

用户点击 `Run Smoke`：

- 默认 `-l 5`，用于确认 Harbor、Docker、agent adapter 可跑。`-k` 保留为 attempts，不用于限制 task 数量。
- run 前显示 command preview、policy preset、agent/model、artifact path。
- run 中显示 stdout/stderr 摘要和 trial 进度。
- run 后导入 result，展示 reward、失败摘要和 job path。

### 3. Subset Regression

用户选择一个失败类别或任务集合：

- 页面生成 subset run config。
- 与 baseline run 对比 reward delta、failure class delta、cost/duration delta。
- 如果 reward 下降，显示具体退化任务。

### 4. Failure Triage

用户打开失败 trial：

- 左侧：task metadata、reward、verifier output。
- 中间：agent trajectory 和关键 command/file edits。
- 右侧：failure class、证据、建议产品改进 slice。
- 可创建 backlog item 或 long task，但不自动写长期 memory。

## 状态

| State | UI behavior |
| --- | --- |
| Harbor missing | 显示安装命令和 blocker，不显示 run 按钮 |
| Docker stopped | 显示 Docker blocker，保留 import job 功能 |
| Dataset unavailable | 显示 registry/dataset blocker，允许导入已有 job |
| No agent adapter | 显示 adapter blocker，提供设计文档链接 |
| Running | 禁用 mutating controls，显示 cancel/stop 但解释可能只停止本地 orchestrator |
| Partial import | 展示已导入字段和缺失 artifacts |
| Failed verifier | 显示 verifier stdout/stderr，不把失败等同于系统崩溃 |
| Official constraint risk | timeouts/resources 被修改时标红，禁止标记为 official-comparable |

## 用户可见术语

- `Run`: 一次 Harbor job。
- `Trial`: 一个 task 的一次尝试。
- `Reward`: verifier 给出的 0 到 1 结果。
- `Comparable`: 是否保持官方 dataset、timeout、resource、agent/model 记录完整。
- `Failure Class`: CodeFactory 对失败原因的产品归类。
- `Artifact`: Harbor 产出的 result、trajectory、verifier 输出和文件证据。

## 不做

- 不做“CodeFactory 已达到某榜单水平”的静态宣传。
- 不把 benchmark task 列表作为训练示例浏览器。
- 不默认展示或复制完整任务数据到聊天上下文。
- 不在普通用户项目里复用 benchmark-sandbox 自动授权。

## 验收

- 用户能看到 Terminal-Bench 2.1 环境是否可跑。
- 用户能启动或导入一次 run，并看到真实 job artifact path。
- 用户能从失败 trial 进入 trajectory/verifier/evidence。
- 用户能看到能力画像，而不只看到总分。
- UI 明确区分 official-comparable run 和本地实验 run。
