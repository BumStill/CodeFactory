# 方案：`deliver_changes` 把合并死锁报成「等待中」，且 PR 标题与分支内容脱钩

> 状态: **待批准**（提案，不是已批准规格）
>
> 提出日期: 2026-07-30
>
> 触发：一次 `deliver_changes` 停在 merge 阶段，报"已登记 auto-merge，等待远端门禁"，
> 用时 11m36s，结论 `blocked` / `not live`。

---

## 一句话结论

工具报的是"**等待中**"，实际是"**死锁**"。按它给的下一步（等 GitHub 自动合并）等下去，
PR 永远不会动。同时它开出的 PR 标题与分支内容完全不符，合并后会**错误地把版本推成 minor**。

---

## 证据（实测，非截图推断）

```
仓库 ruleset: main-pr-and-ci-gate (active)
  required_status_checks:
    strict_required_status_checks_policy = True   ← 要求分支必须与 main 同步
    contexts: agent-bridge-linux, check, governance-baseline, remote-real-app-gui

PR #290  head=fix/auto-release-reconcile-sigpipe
         title="feat: add on-demand embedded browser pane"     ← 与内容不符
         commits=["ci: avoid auto-release reconcile sigpipe"]  ← 真实内容
         mergeStateStatus=BEHIND   autoMerge=True   state=OPEN

PR #281  head=feat/on-demand-embedded-browser-pane
         title="feat: add on-demand embedded browser pane"
         mergeStateStatus=BEHIND   autoMerge=False  state=OPEN
```

---

## 缺陷 1（最严重）：死锁被报成等待

`strict_required_status_checks_policy = True` 意味着 **分支落后于 main 时不可合并**。
而 GitHub 的 auto-merge **不会自动更新落后的分支**——它只是挂在那里等 PR 变成可合并。

于是形成闭环：

```
分支 BEHIND ──→ 不可合并 ──→ auto-merge 挂起等待
     ↑                                    │
     └──── 没有任何东西会更新分支 ←────────┘
```

**在 strict 策略的仓库里，对 BEHIND 分支登记 auto-merge 是无效手段**，不是"稍等即可"。
工具却把它当成正常等待，打印"等待 GitHub 远端门禁完成并自动合并"就终止了回合。

这跟 `#268` 修掉的那个缺陷是同一族：**输出一个诚实但无用的终止态，而不是把能做的做完**。
区别在于这次更糟——它给出的下一步是**错的**，会让人（或下一轮 agent）无限期空等。

### 修复：merge 阶段必须区分三种状态，并主动解开可自动解的那种

读 `mergeStateStatus`，分流：

| 状态 | 含义 | 正确动作 |
| --- | --- | --- |
| `CLEAN` / `HAS_HOOKS` | 可以合 | 直接合 |
| `BEHIND` | 落后于 base，strict 策略下不可合 | **自动 update-branch**（`gh pr update-branch`，或本地 merge base 后 push），等 CI 重新变绿，再合。全程无需人工 |
| `UNSTABLE` | 有非必需检查未过 | 按必需检查判定，可合则合 |
| `BLOCKED` | 缺审查或缺必需检查结论 | 真 blocked，报具体缺什么 |
| `DIRTY` | 存在冲突 | 真 blocked，报需人工解冲突 |
| `UNKNOWN` | GitHub 仍在计算 | 短轮询重取，不要当结论 |

配套两条：

- **登记 auto-merge 之后不得就地终止。** 要么继续轮询到有界超时，要么根本不登记。
- **strict 策略探测。** 通过 `gh api repos/{owner}/{repo}/rulesets` 判定；探测不到就
  按最保守假设处理（即：BEHIND 一律先 update-branch，不指望 auto-merge）。

### 术语必须改

`blocked` 现在同时表示"需要人来做点什么"和"在等机器"。至少要拆成
`blocked`（需人工）与 `waiting`（有界等待，会自动续跑）。前者才该触发"需要处理"红条。

---

## 缺陷 2：PR 标题由会话上下文决定，而不是由分支提交决定

PR #290 的唯一提交是 `ci: avoid auto-release reconcile sigpipe`，标题却是
`feat: add on-demand embedded browser pane`——与 #281 一字不差。标题显然来自当时的
任务上下文，没有看过分支里到底有什么。

**这不只是难看。** 本仓库一律 squash 合并，squash commit 的标题就是 PR 标题；
而发版 slot 正是按 conventional commit 前缀算的：

- 真实内容 `ci:` → 按 `docs/principles/release-cadence.md` 第 3 条，**不该单独触发发版**
- 错误标题 `feat:` → 触发 **minor** 版本

也就是说，这个错标题一旦合并，会凭空推出一个 minor 版本，且变更日志里写着一个
根本不存在的功能。**它直接污染我们刚立的发版节奏原则。**

### 修复

1. PR 标题默认从**分支相对 base 的提交**生成，不从会话上下文取。
2. 调用方显式传标题时，加一道断言：标题的 conventional 前缀必须与分支提交算出的
   最高 slot 一致（`feat` ≥ `fix` > 其他）。不一致就**拒绝开 PR** 并说明原因——
   理由要写清楚是"会污染发版 slot"，而不是笼统的"标题不匹配"。
3. 该断言同样适用于 squash 合并时的标题复验（`#269` 已为 trailer 做过同类复验，
   这里是同一类问题的另一个字段）。

---

## 缺陷 3：`deliver_changes` 在错误的分支上工作

会话意图是**恢复 PR #281**，工具却在当时 cwd 所在的 `fix/auto-release-reconcile-sigpipe`
分支上开了一个新 PR #290。结果是两个标题相同、内容不同的 open PR。

根因：恢复既有交付时按"任务标题"而不是按 **PR 号 / head 分支**定位。

### 修复

开工前对齐三者并断言一致：**cwd 当前分支** × **要恢复的 PR 号** × **任务意图**。
不一致就停下报告，而不是在当前分支上按标题另开一个。恢复既有 PR 一律按 PR 号或
head 分支定位。

---

## 当前这两个 PR 的清理（与代码修复独立，可立即做）

1. **#290 改标题**为 `ci: avoid auto-release reconcile sigpipe`。
   不改就合会错误触发 minor 发版。
2. **#290 update-branch** 后才能合（现在 BEHIND，auto-merge 永远不会触发）。
3. **#281 同样 BEHIND**，也需要 update-branch；它是真正的 browser pane 功能。
4. 确认 #290 的 `ci:` 改动是否还需要——它修的是 auto-release 的 sigpipe，
   而 auto-release 在 `#268`/`#269` 之后已有较大改动。

---

## 验证方式

- 单元测试覆盖 `mergeStateStatus` 五种分流，尤其 `BEHIND → update-branch → 重测 → 合并`
  这条自动路径必须有测试，且断言**不产生 `blocked` 终止态**。
- 标题断言的测试：分支提交为 `ci:`、显式标题为 `feat:` → 必须拒绝开 PR。
- 分支绑定的测试：请求恢复 PR #N 而 cwd 在别的分支 → 必须停下报告，不得另开 PR。
- 真实路径证据：在一个 BEHIND 的 PR 上跑一次 `deliver_changes`，证明它自动
  update-branch → CI 绿 → 合并，全程不需要人介入。
