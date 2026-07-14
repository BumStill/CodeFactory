# Terminal-Bench 2.1 对齐业务设计

## 背景

用户对 CodeFactory 当前产品设计和能力不满意，明确提出要瞄准 Terminal-Bench 2.1 评估能力。这个判断是对的：Terminal-Bench 2.1 衡量的是 agent 在真实 terminal 环境里完成端到端任务的能力，而不是聊天体验、工具陈列或本地 UI 完成度。

CodeFactory 的定位需要从“本地 AI 编程工作台”升级为：

> 可被 Terminal-Bench 2.1 量化的本地 terminal agent 能力系统。

这不意味着 CodeFactory 变成 Harbor 或 benchmark UI 的复制品。CodeFactory 的业务目标是把产品能力、执行证据、失败原因和后续改进闭环绑定到同一套可复现实验体系。

## 外部基准事实

截至 2026-06-27 已核准：

- Terminal-Bench 用真实 terminal 环境评估 AI agents，任务覆盖编译代码、训练模型、搭服务器等端到端工作。
- Terminal-Bench 2.1 官方运行路径是 Harbor，数据集为 `terminal-bench/terminal-bench-2-1`。
- 官方 leaderboard 已有 Terminal-Bench 2.1 live 榜单；提交说明要求不能修改 timeouts 或 resources。
- Harbor 支持内置 agents，也支持通过 import path 接入自定义 agent。
- Harbor job 会产出 `config.json`、`result.json`、trial 结果、agent trajectory、verifier 输出和 artifacts。

参考入口：

- `https://www.tbench.ai/docs/run-terminal-bench-2-1`
- `https://www.tbench.ai/leaderboard/terminal-bench/2.1`
- `https://harborframework.com/docs/run-jobs/run-evals`
- `https://harborframework.com/docs/agents`
- `https://github.com/harbor-framework/terminal-bench`

## 业务目标

| ID | 目标 | 验收方式 |
| --- | --- | --- |
| CF-TB-B1 | CodeFactory 有明确的 Terminal-Bench 2.1 能力基线 | 能从产品或 CLI 生成一次可复现 Harbor smoke run，并保存完整 job artifact |
| CF-TB-B2 | 评估不只看总分 | 展示按任务类别、难度、失败类型、工具路径、耗时和 token/cost 的能力画像 |
| CF-TB-B3 | CodeFactory agent 可被 Harbor 调用 | 提供 `BaseAgent` 或 `BaseInstalledAgent` adapter，运行时不依赖桌面手动点击 |
| CF-TB-B4 | 失败能转成产品改进队列 | 每个失败 trial 归类到 tool/runtime/planning/context/verification/policy/environment 等原因 |
| CF-TB-B5 | 每次产品改进可回归验证 | 关键 PR 能选定 subset 或 smoke suite，对比改动前后 reward、error、cost、latency |
| CF-TB-B6 | 防 benchmark contamination | 不把 Terminal-Bench 任务数据写进训练语料、长期 memory、默认 prompt 或产品示例 |
| CF-TB-B7 | 分数必须衡量产品能力 | adapter 通过污染扫描，禁止按 task、repo、artifact、领域答案或 instruction fingerprint 注入专用 hint/repair；主产品与 headless 使用同一执行完成契约 |

## 产品原则

1. **Score 是结果，不是产品。** 首期先建立可复现评估、失败解释和改进闭环；不要做只展示分数的 vanity dashboard。
2. **headless 是硬门槛。** Terminal-Bench 不会等用户在桌面 UI 里逐个审批；CodeFactory 必须有可审计的 benchmark sandbox policy。
3. **能力画像比榜单名次更重要。** 用户需要知道 CodeFactory 输在哪里：上下文、终端操作、文件编辑、测试判断、环境安装、长任务漂移还是权限策略。
4. **官方约束不可绕开。** 不能修改 Terminal-Bench 2.1 的 timeouts/resources；不能把 benchmark canary 或任务内容纳入训练/记忆。
5. **真实运行证据优先。** 本地单元测试、UI 页面或配置存在都不能替代 Harbor job artifact 和 verifier reward。
6. **评估主体必须明确。** Terminal-Bench 评的是 agent system；`CodeFactory agent using DeepSeek` 是 CodeFactory agent 的一次运行配置，不是 DeepSeek 本身的 benchmark 结果。
7. **被专用修复污染的分数无效。** 如果 adapter 读取 hidden verifier、按固定题指纹分支，或内置某题答案/脚本，该 run 只能标记为 `benchmark-contaminated diagnostic`，不得进入产品能力基线、发布门禁或对外水平判断。
8. **能力必须在真实产品路径复用。** 例如“修改后重新验证”和“服务启动后做 bounded readiness/client probe”必须同时改善桌面 CodeFactory 的普通开发任务；仅在 Harbor adapter 中出现的行为不计产品提升。
9. **确定性改进必须先交付。** 通用能力切片通过独立回归后，先完成真实 App、PR/CI、合并和适用发布，再开始下一轮评分优化；位于主产品源码但尚未发布的改动仍是 `not live`。
10. **失败轨迹必须保留真实修改事实。** Agent 把编辑、构建、安装和运行合在一个命令中时，后半段失败不能抹掉前半段已发生的源码修改；产品必须据此重新扫描、安装和验证，不能沿用过期成功证据。
11. **中文任务不得降低完成标准。** CodeFactory 的中文用户要求兼容迁移、源码安装或项目测试时，必须启用与英文任务相同的结构化完成证据；界面语言不能成为绕过验证门禁的条件。

## 系统化评估矩阵

本项目的通用评估原则固化在 `docs/principles/systematic-agent-evaluation.md`。Terminal-Bench 2.1 首期按以下矩阵落地：

| 目的 | 固定什么 | 变化什么 | 结论归属 |
| --- | --- | --- | --- |
| 评 CodeFactory agent 能力 | Terminal-Bench subset、model backend、policy、runner | CodeFactory build、agent loop、context/tool/policy 实现 | CodeFactory agent |
| 评模型后端影响 | CodeFactory build、agent adapter、subset、policy、runner | DeepSeek / Claude / GPT 等 provider/model | model backend 作为组件 |
| 评 agent scaffold 强弱 | 同一个 model backend、subset、runner | CodeFactory adapter、simple baseline、oracle 或其他 scaffold | agent scaffold / product mechanism |
| 评 benchmark 基础设施 | oracle 或 no-model diagnostic、runner | Harbor、Docker、importer、schema、UI | evaluation infrastructure |

业务口径：

- 第一次有效能力结果必须写成 `agent=codefactory-headless`，模型只写在 `model_backend` 或 `model` 字段。
- Oracle smoke 只证明基础设施，不证明 CodeFactory agent 能力。
- No-model smoke 只证明 adapter/import 链路，不证明 model-backed agent 能力。
- Provider bridge test 只证明产品授权路径，不证明真实 benchmark 分数。

## v1 范围

包含：

- Terminal-Bench 2.1 benchmark profile。
- Harbor 环境检查和 smoke run 指引。
- CodeFactory headless agent adapter 设计。
- Run ledger 和结果导入设计。
- 失败分类、能力画像和改进队列设计。
- UI 的 Benchmark Runs / Failure Triage / Capability Profile 设计。

不包含：

- 不承诺首期进入官方 leaderboard。
- 不在首期做云端大规模并发平台；先支持 local Docker，后续再接 Daytona 等 provider。
- 不把 Terminal-Bench 任务内容作为产品示例、skill 训练材料或 memory。
- 不以牺牲安全边界换 benchmark 分数。

## 成功标准

- 用户可以在 CodeFactory 里看到 Terminal-Bench 2.1 作为能力评估目标，而不是只看到普通聊天任务。
- 开发者可以用同一份配置复现一次 Harbor smoke run。
- 每个 run 都能保存 dataset、agent、model、CodeFactory build、policy、job artifact path、reward 和失败摘要。
- 至少能把失败 trial 映射到 CodeFactory 的具体能力缺口和后续 PR backlog。
- 后续产品能力变化可以用相同 subset 做回归对比。
