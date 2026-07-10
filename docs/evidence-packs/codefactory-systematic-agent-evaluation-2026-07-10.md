# CodeFactory 系统化 Agent 评估报告（2026-07-10）

## 结论

本轮第一次按同一目标同时完成了外部 terminal-agent 评测和真实 CodeFactory 产品主路径评测。结论不是“已经达到 18/18”，而是：

- Terminal-Bench 2.1 固定 18 题诊断分为 `12 / 18`，mean reward `0.667`。18 个 trial 全部收口，但两个 trial 由 runner watchdog 停止，因此不是 clean official-comparable 成绩。
- 该结果低于 2026-07-06 两轮稳定的 `16 / 18`，说明当前发布前 score-holding 已破坏，不能继续用历史最好分代表当前状态。排除三个有直接环境/验证器证据的 trial 后，可归因 Agent 子集为 `12 / 15`（`80%`）；这是诊断归因，不替代原始分。
- 已安装 v1.42.7 的真实产品编码任务完成 `3 / 3` 测试，产品矩阵评分 `9 / 10`；但首轮暴露了过期模型状态和错误 `/workspace` 假设。
- 通用产品修复已通过 PR `#100` 合并并进入 v1.42.8 发布：所有新项目/快速任务会话在入库前按 endpoint 校正模型，Agent 在首个工具调用前获得准确项目根目录。没有新增 Terminal-Bench task-specific 分支。
- 正式安装的 v1.42.8 已完成同类 P1 复验：新项目发送前显示 `deepseek / deepseek-v4-pro`，首个命令直接使用 `/private/tmp/codefactory-product-eval-context-fix`，Agent 在 51.1 秒内完成分析、修改和内部复验，独立终端再次验证 `3 / 3`。两项产品缺陷均已在发布路径闭环。

当前水平应定义为：**真实产品主路径可用，但 terminal-agent 能力处于中等且不稳定水平**。它能稳定完成常见文件、数据、服务、QEMU、Torch 和 gRPC 类任务，但在源码构建、长安装、服务 readiness 和本机重 verifier 上仍会回退。

## 外部水平参照

截至 2026-07-10，[Terminal-Bench 2.1 官方榜单](https://www.tbench.ai/leaderboard/terminal-bench/2.1)展示 13 个完整评测项，准确率范围为 `58.7%` 到 `83.4%`，并明确不允许修改 timeout 或资源。CodeFactory 本轮 `12 / 18 = 66.7%` 只来自固定诊断子集且启用了 watchdog，任务选择和运行约束均不同，**不能映射为官方榜单名次**。它只能说明当前重点能力家族的通过率；要判断外部真实水平，仍需在 clean Linux/x86 环境完成新的 89 题、无资源/timeout 修改的正式 run。历史 `6 / 89` 已不能代表当前产品，但在新 full run 前也不能用 18 题比例替代它。

## 评估矩阵

| 轴 | 评估对象 | 固定条件 | 结果 | 允许结论 |
| --- | --- | --- | ---: | --- |
| 外部 Agent 能力 | v1.42.7 `codefactory-headless` + `deepseek-v4-pro` | 固定 18 题、concurrency 2、1200s watchdog | `12 / 18`，`0.667` | 当前环境下的完整诊断基线 |
| 产品主路径 | `/Applications/CodeFactory.app` v1.42.7 | 固定 npm fixture、DeepSeek、默认权限 | `9 / 10`，外部复验 `3 / 3` | 已安装 App 能完成普通代码修复闭环 |
| 产品修复验证 | CodeFactoryDev（PR #100） | stale Claude store + DeepSeek endpoint | 发送前 UI/SQLite 均为 `deepseek-v4-pro` | 新会话模型权威状态已前移到创建阶段 |
| 发布版 P1 复验 | `/Applications/CodeFactory.app` v1.42.8 | 全新失败 fixture、DeepSeek、两次单次项目权限 | 发送前模型正确；首命令 cwd 正确；内部及外部测试均 `3 / 3` | 模型与 cwd 修复已在正式安装版真实用户路径生效 |
| 发布交付 | PR #100 / v1.42.8 | PR、CI、刻意发版 | PR/CI、Windows/macOS build、macOS artifact smoke、正式发布均通过 | 产品修复已进入正式发布 |

## 18 题结果

通过 12 题：

`circuit-fibsqrt`、`count-dataset-tokens`、`extract-elf`、`kv-store-grpc`、`mteb-retrieve`、`nginx-request-logging`、`protein-assembly`、`qemu-startup`、`sanitize-git-repo`、`sparql-university`、`torch-tensor-parallelism`、`write-compressor`。

失败 6 题：

| Task | 本轮分类 | 与历史 16/18 的关系 | 产品改进方向 |
| --- | --- | --- | --- |
| `build-cython-ext` | policy | 历史通过，本轮回归 | 依赖/源码构建计划必须包含 install、import smoke、最终 verifier 三段闭环 |
| `configure-git-webserver` | verification | 历史通过，本轮回归 | 后台服务必须记录 PID/log/readiness，并在 verifier 前证明端口可用 |
| `install-windows-3.11` | verifier/environment ambiguity | 原始 reward 回落，但 3/4 verifier 通过；最后一项因 VNC 截图尺寸 `400×720` 与 `480×640` 无法广播比较而失败 | 固定截图尺寸/归一化后在 Linux/x86 重跑，不计为已确认 Agent 回归 |
| `caffe-cifar-10` | verification | 历史剩余失败 | 构建、运行、训练小样本、模型产物四阶段验证，禁止把流水线返回 0 当成功 |
| `filter-js-from-html` | environment | 历史通过，本轮本机 watchdog | 把浏览器 verifier 移到稳定 Linux/x86 runner；不作为本机 Agent 回归直接计零 |
| `query-optimize` | environment | 历史剩余失败 | 在 Linux/x86 clean runner 单独验证，分离 SQL 实现正确性与本机 QEMU verifier 卡死 |

## 已落到产品的提升

### 1. 会话模型权威状态

旧行为：DeepSeek endpoint 新建会话时，前端可能先写入 stale Claude model，直到首次发送才由后端修复。用户会看到错误模型，首次请求还承担路由失败风险。

新行为：项目会话、复用快速任务、新建快速任务都在写 SQLite 前按 endpoint 解析；前端立即采用后端返回的 `session.model_id`。OpenRouter 会保留会话明确选择的 `provider/model`，同时修复来自 DeepSeek/ChatGPT 的无前缀残留。

### 2. 首轮工作目录上下文

旧行为：工具本身有 cwd 安全边界，但 system prompt 没告诉模型真实路径。真实 v1.42.7 任务首个命令猜了 `/workspace/...`，失败并额外消耗一次授权。

新行为：准确项目根目录进入不可被 context budget 淘汰的基础 system prompt，要求使用该路径或相对路径。安全边界没有放宽，只减少错误路径和无效工具调用。

## 系统性改进路线

### P0：恢复 score-holding

下一道发布门不是直接宣称 `18 / 18`，而是先让**同一发布版**恢复 `>= 16 / 18` 且历史 pass set 不再回退。顺序：

1. `build-cython-ext`：补通用源码构建状态机，验证包安装、模块 import 和测试，不写 task answer。
2. `configure-git-webserver`：把后台服务生命周期和 readiness 变成 AgentLoop 通用能力。
3. `install-windows-3.11`：先修复或隔离截图尺寸 verifier，再判断是否还存在 Agent 长安装缺陷。
4. 两个确认回归 canary 与一个环境复测全部收口后，跑同口径 18 题 aggregate；任何历史通过项回落都不允许发布能力声明。

### P1：隔离环境噪声

- 建立 Linux/x86 clean runner，专跑 `filter-js-from-html` 和 `query-optimize`，关闭 runner watchdog 后才产生 official-comparable gate。
- 本机 Mac/QEMU run 保留为快速诊断层；watchdog/environment 失败单列，不和 Agent verifier failure 混成一个分数。
- 同一 commit 同时保存 product build、agent、model、runner、task set、资源和 watchdog 元数据，避免再出现“历史最好分等于当前版本”的错误归因。

### P2：固化每轮评估机制

每个用户可见 Agent 能力版本至少执行：

1. **产品 P1 smoke**：固定真实项目，验证模型/cwd、权限、文件 diff、内部测试、外部 held-out 测试和最终回复。
2. **目标 canary**：只验证本轮要修的能力家族，用来快速反馈，不作为总体分。
3. **18 题 score-holding**：canary 后必须跑 aggregate，比较 pass set、mean reward、failure class 和历史回归。
4. **发布版复验**：安装正式 artifact 后重复产品 P1 smoke；仅代码、CI、健康接口或 dev app 不算上线完成。
5. **定期 89 题**：固定 18 题在发布版稳定 `>= 16 / 18` 且 clean runner 可用后再跑，长期目标仍是 `18 / 18` 和显著提高完整 89 题结果。

## 证据边界

- 固定 18 题原始结果：run `3f86d0e1-e7a9-465e-9deb-034ee38d4d1a`。
- 证据：`docs/evidence-packs/terminal-bench-21-regression-subset-2026-07-10T07-41-20Z.md`。
- 该 run 18/18 trial 已收口，但 `filter-js-from-html`、`query-optimize` 由 1200s watchdog 停止，因此 `official_comparable: no`。
- `12 / 18` 是当前完整诊断分；`16 / 18` 是历史 score-holding 参照，不是本轮结果。
- 产品修复 PR：`https://github.com/BumStill/CodeFactory/pull/100`，合并 commit `8073de9c5a9dfa5acf6fa27574ddde4d3e3d26ef`；PR CI 和合并后 main CI 均通过。
- 正式发布：`https://github.com/BumStill/CodeFactory/releases/tag/v1.42.8`。Windows、macOS build、DMG 临时安装、bundle/version/arm64、真实窗口稳定和隔离数据库初始化均通过。
- 同一 DMG 已安装到 `/Applications/CodeFactory.app`，本机 bundle version 为 `1.42.8` 且进程从该路径稳定启动。解锁后完成交互式新项目 P1：外部基线 `0 / 3`，发送前 UI 为 `deepseek / deepseek-v4-pro`，首个 bash 命令为 `cd /private/tmp/codefactory-product-eval-context-fix && npm test 2>&1`，Agent 修改后内部复验通过，独立终端复验为 `3 / 3`。
