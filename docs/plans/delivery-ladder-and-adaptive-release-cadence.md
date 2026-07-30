# 方案：交付梯子按级降级 + 发版节奏自适应化

> 状态: **A 已发布于 v1.73.4；B 已批准，实施中**
>
> 提出日期: 2026-07-30
>
> 涉及同一交付链上的两件事：A 是缺陷修复，已通过独立 PR 发布；
> B 是最高原则与执行门禁修订，独立 PR 交付，避免把未审查治理草案混入紧急修复。

---

## A. `deliver_changes` 全有或全无预检（缺陷）

### 现象

`deliver_changes` 在 `through_release` 预检处阻断：

> 交付预检未通过：目标 `through_release` 缺少 live verifier；尚未执行 stage、commit 或 push。

活儿已经写完、验证完，工具连 `git commit` 都不肯做，最终状态 `not live`。

### 根因

`src-tauri/src/agent/delivery.rs` 的 `delivery_preflight` 是一道**全有或全无的闸门**：只要目标 ceiling 所需能力链上缺任何一环，就返回 `Err(blocked)`，`deliver()` 立刻 `return outcome.blocked_at(...)`，
stage → commit → push → PR → CI → merge **一级都不执行**。

而这条梯子的下层完全可用，且用户要的正是这些。**顶层缺一颗螺丝，不该让整架梯子作废。**

### 为什么是主路径而不是边缘情况

三个默认值相乘：

| 因素 | 默认值 | 后果 |
| --- | --- | --- |
| `DeliveryCeiling` 默认 | `ThroughRelease`（`config/settings.rs`，标了 `#[default]`） | 预检要求 live verifier |
| `GhCliRemote`（GitHub + `gh` 的默认适配器） | `live: false` | 适配器提供不了 |
| `.codefactory/delivery.json` | CodeFactory 本仓库不存在 | 仓库也补不上 |

于是**任何 GitHub 仓库在默认配置下，每一次 `deliver_changes` 都被整条拒绝**。

### 已裁定的安全边界：无 review 通道仍然前置阻断

旧的 `deliver()` 文档注释写着：

> `remote` is `None` when no git remote token is configured — **local steps still run** and the PR step reports a clear, non-looping blocker.

这条旧注释与既有正式规格 `REQ-DEL-002/009` 冲突。最终裁定是：无 remote
或无 review adapter 时继续保持无副作用前置阻断，避免留下无法形成评审的
半交付状态；本次只修复**已有 review 通道、但更高层执行器或 live verifier
缺失时整条梯子被取消**的问题。

### 修复原则（本方案的核心判断）

> **已有 review 通道时：缺更高层执行器 → 降级动作；缺验证器 → 降级结论，不降级动作。**

- **review adapter / remote channel 缺失**：继续在首次 stage/commit/push 前阻断。
- **更高层执行器**（CI observer / merge adapter / release adapter）缺失：把 ceiling 降到能达到的最高一级，跑完，如实报告降到哪、因为缺什么、怎么解锁。
- **验证器**（live verifier）缺失：**照常执行到发布**，但不得声称"已上线"。

第二条不是新发明——`delivery.rs` 已有 `block_unverified_release(outcome, "未配置 live verifier")` 专门处理这件事。也就是说 **preflight 里的 live 检查与下游既有守卫重复**，而且重复得更早、更粗暴：下游只是不让你吹牛，preflight 直接不让你干活。删掉 preflight 里的 live 检查，让既有守卫工作即可。

### 改动清单

1. `delivery_preflight` 由 `Result<(), StepResult>` 改为返回**可达 ceiling + 降级原因**；无 review 通道或配置不可读仍可安全前置阻断。
2. 能力缺失逐级降级：
   - 无 remote / 无 review adapter → 无副作用前置阻断
   - 缺 CI observer → 降到 `PrOnly`
   - 缺 merge adapter → 降到 `ThroughCiGreen`
   - 缺 release adapter → 降到 `ThroughMerge`
   - **缺 live verifier → 不降级**，执行到发布，由 `block_unverified_release` 标 `not live`
3. `preflight` step 从"blocked"变为"ok + 降级说明"，`next_action` 给出解锁所需的具体配置或命令。
4. 最终 summary 必须同时说清：**实际到达哪一级**、**为什么停在这**、**下一步做什么能更进一步**。

### 验证

- 先写失败测试：对每种能力缺失组合，断言 commit 真的发生、且 ceiling 降到预期级别（当前实现会在 stage 之前就返回，测试必然红）。
- 补一条专门复现截图场景的测试：`GhCliRemote` 能力 + 无 `.codefactory/delivery.json` + `ThroughRelease` → 必须完成 commit/push/PR，且结论为 `not live` 而非 `blocked at preflight`。
- 恢复 `remote: None` 的文档契约测试。

---

## B. 发版节奏：从"刻意攒批"到"按严重性与规模自适应"

### 现状与缺口

`docs/principles/release-cadence.md` 是跨仓最高原则，只有两个速度：`workflow_dispatch`（按需）和 daily cron（每日 01:00 UTC），一个闸门（有无 `feat`/`fix`）。

它**没说什么时候该去拉那根按需的杆**。结果实践中一切都落到 daily cron —— 一个把用户卡死的 P0 崩溃修复，和一个错别字修复，排在同一班车上，最长等 24 小时。原则很好地防住了更新疲劳，却没管严重缺陷的送达时间。

**更关键的缺口：这条原则根本不在 AI Coding OS canon 里。** 它只存在于 CodeFactory 的 `docs/principles/`，`canon/global-principles.md` 里一条发版节奏都没有。所以 Codex、OpenClaw 拿不到它——"其他 code 工具也可以执行"这一环目前是断的。

### 设计要点：把严重性变成机器可读信号，而不是措辞

散文式的"严重就赶紧发"，每个 agent 的解读都不一样，无法执行也无法审计。提议用 **commit trailer** 承载：

```
Release-Urgency: immediate
```

选 trailer 而不是 PR label 的理由：它随 `git log` 走，任何仓库、任何工具、不依赖 GitHub，符合"可原样复制到任何仓库"的要求；而 `auto-release.yml` 本来就在跑 `git log <last-tag>..HEAD`，解析成本接近零。也不能复用 Conventional Commits 的 `!`——那已经表示 breaking change，含义会打架。

| 取值 | 判据 | 发布动作 |
| --- | --- | --- |
| `immediate` | 见下方 rubric | 即使普通交付边界会等待，也要求合并后立即 dispatch |
| 不写（默认） | 无紧急度覆盖 | 跟随仓库配置：已授权的 `through_release` 仍按需 dispatch，否则攒到下一班 cron |
| `hold` | 需与其他改动配套、或需文档/公告先行 | 阻断所有切版；依赖完成并审查全批次后，只能用 `allow_guarded_batch=true` 明确放行 |

`immediate` 是决策信号，不是 merge trigger。现有“合并不自动发版”原则保持不变；
已获授权的合并执行者负责在合并后立即 `workflow_dispatch`。用户已配置的
`deliver_changes -> through_release` 本身也是刻意的按需发布请求，不需要伪装成
紧急变更。Squash 合并时必须把 trailer 写进最终 main commit body，只存在于被丢弃
分支 commit 上的 trailer 无效；CodeFactory 的 GitHub adapter 必须传入最终 body，
合并后读回 commit 验证 trailer，再允许进入 release。

### `immediate` 判据（满足任一）

1. 主路径不可用、产品无法启动或崩溃；
2. 数据丢失、损坏或错误持久化；
3. 安全、凭据或权限绕过；
4. 已发布版本正在向用户暴露该缺陷（回归），用户已能感知；
5. 用户明确表示紧急；
6. 一个完整自洽的大功能落地，攒着只推迟价值、不降低风险。

**存疑时按默认。** 这条是防通胀的关键——`immediate` 一旦泛滥就退化成"每次合并都发"，把原则打回原点。

### `hold` 的真实用途：发版会带走别人的东西

切一版会把**自上个 tag 起的所有合并**都发出去，包括不属于当前任务的改动。所以 agent 自主决定发版有一个真实风险：为了发自己的 P0，顺带把别人半成品的 feat 也发了。

对策：触发 `immediate` 前必须扫 `git log <last-tag>..HEAD`，若批次内存在 `Release-Urgency: hold` 的提交，则不得自动发；改为等待、或请用户裁决。

### 授权边界（需要写清楚，否则下次还会卡）

全局 Security 已把 `PR -> CI -> merge -> workflow_dispatch -> artifact verification` 纳入同一授权链。因此：

- **在已批准的任务内**，按 rubric 判定 `immediate` 并 dispatch，属于授权范围，不需要再问；
- **没有任务在身时主动提议发版**，仍需用户同意。

### 改动清单

| 层 | 文件 | 动作 |
| --- | --- | --- |
| 跨仓原则 | `docs/principles/release-cadence.md` | 新增"自适应节奏"章节 + rubric + trailer 契约；保留现有 7 条规则 |
| 本仓入口 | `AGENTS.md` 最高原则段 | 一句话摘要同步 |
| 本仓执行入口 | `AGENTS.md` 最高原则段 | 同步 trailer、立即发版与 hold 的最小可执行摘要 |
| 全局 canon | OpenClaw AI Coding OS proposal | 作为独立治理推广处理；不在本仓 PR 中越权修改用户级 authority surface |
| 交付集成 | `src-tauri/src/agent/delivery.rs`、`src-tauri/src/tools/delivery.rs`、`src-tauri/src/git_remote/github.rs` | 工具参数写入 trailer；squash 显式传递并读回验证；hold/非法值在 trigger_release 前阻断 |
| 参考实现 | `.github/workflows/auto-release.yml`、`tools/release/plan_release.py` | 解析 `Release-Urgency`；`hold`/非法值阻断所有发版；`force` 只处理无 feat/fix；cron 语义从"唯一常规出口"改为"兜底下限" |
| 可验证 | `tests/test_release_workflow.py` | 用临时 Git 历史实际执行 trailer、版本槽、hold、force 与显式放行策略，不只断言 YAML 文本 |

---

## 已决定（2026-07-30，用户裁定）

### 1. "每天至少发布一次" = 读法 A（正常情况下的等待上限）

在门禁和发布基础设施健康时，daily cron 是**正常等待上限**：有可发的
`feat`/`fix` 时，目标是最多攒 24 小时发出去。红色门禁、显式 hold 或基础设施故障
必须作为可见 blocker 报告，不能假装仍满足了时间目标。
**没有用户可见改动就不产出版本**——不为了凑"每天一发"而强制 force 出只含
`chore`/`docs` 的空版本，那正是原则第 3 条要防的（更新疲劳、版本号无意义）。

因此原则第 3 条（只有 feat/fix 才发）**保持不变**，新增的只是"攒批有 24 小时上限"
这个时间承诺，以及 `immediate` 可以提前出车。

### 2. cron 时刻不改

保留 `0 1 * * *` UTC（北京 09:00）单班兜底，**不加第二班**。当天需要更快送达的，
靠 `Release-Urgency: immediate` 提前出车，而不是靠加密 cron。
