# 交付执行策略与失败恢复：架构设计

## 权威状态

```text
user intent stream
  -> durable task grants + current explicit constraints
  -> per-action intent decision
  -> deliver_changes
  -> PR/check observation
  -> guarded auto-merge
  -> remote reconciliation
  -> release / live observation
```

各层只回答自己的问题：

| 层 | 权威问题 | 不得推断 |
| --- | --- | --- |
| 会话授权 | 用户是否仍授权交付 | GitHub 是否允许合并 |
| 动作意图 | 这个具体动作是否属于已授权任务且未被当前显式约束禁止 | 把整条消息固化为回合模式 |
| 工具权限 | 当前工具是否需确认 | 用户业务意图 |
| CI 观察 | 当前 PR head 的 checks 是否稳定终态 | ruleset 已允许合并 |
| 合并执行 | GitHub 是否接受绑定 head 的 auto-merge | 已经发布 |
| 回执恢复 | 外部动作真实状态 | 根据本地 intent 猜成功或失败 |

## 设计决策

### 1. 从固定回合能力改为逐动作意图

`ReviewOnly / Implement / Deliver` 不能再作为一次根消息生命周期内的互斥锁。
intent policy 需要保留两类输入：

- durable grants：当前任务已经获准实施或交付的范围；
- explicit constraints：当前消息明确提出的“不要修改”“不要合并”“只分析”等限制。

每个工具调用按动作类型单独判断。问句、解释请求或分类器默认值不得生成隐式的
全局只读锁；只有明确约束能拒绝对应 mutation/delivery。中途 steer 立即更新约束，
无需等到所谓下一回合。

### 2. CI 终态需要注册稳定窗口

`wait_for_ci` 不得把第一次 `None` 或第一次 `Success` 直接当作终态。GitHub PR 创建后
workflow/check-suite 注册存在竞态。对具备 CI 能力的 remote，至少经过注册稳定窗口，
并再次观察相同 head 的终态后才能到达 `ci_green`。无 CI 仓库在稳定窗口结束后才接受
`None`。

### 3. 合并由 GitHub 规则集托管

GitHub CLI adapter 使用：

```text
gh pr merge <number> <method> --auto --match-head-commit <expected_sha>
```

Squash subject/body 继续保留发布 trailer。禁止自动添加 `--admin`。合并成功必须以
远端 PR `MERGED` 和 merge commit SHA 为准，不能只看 CLI exit code。

REST adapter 在 merge payload 中绑定 `sha`；受保护分支返回非成功时保持 fail closed。

### 4. intent 回执可核对恢复

为 `DeliveryRemote` 增加 PR 合并状态观察：`OpenSameHead`、`Merged`、
`ClosedUnmerged`、`HeadChanged`、`Unsupported`。

- `Merged`：升级本地回执为 `merged`，继续 release。
- `OpenSameHead`：auto-merge 注册/重试是幂等的，可继续受控合并。
- 其他状态：结构化阻断，禁止自动重试。

## 故障模型

- checks 尚未注册：稳定窗口保持 pending。
- checks 运行中/失败：不进入 merge。
- head 在等待期间变化：`--match-head-commit` 拒绝，重新从新 head 验证。
- CLI 返回失败但请求可能被接收：下一轮先查 PR 状态，不凭本地错误猜测。
- App 重启：DB 中 standing grant 和 git config 回执共同恢复状态。
- GitHub 不可达：保留 intent，明确显示“远端事实无法核对”，不建议 `--admin`。

## 迁移兼容

短期保留 `TurnCapability` 类型作为 provider prompt/finalization 的兼容输入，但结构
门禁改读 `ActionIntentPolicy`。类型注释和用户文案不得再把它称为固定回合边界；待
所有 surface 迁移完成后删除旧枚举。
