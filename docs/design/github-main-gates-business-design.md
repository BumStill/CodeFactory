# GitHub 主分支门禁：业务设计

## 问题

CodeFactory 已要求所有交付经过 PR 与 CI，但 GitHub 的真实设置没有 required checks。结果是执行 `gh pr merge --auto` 时，只要当时没有其他未满足的仓库规则，PR 会立刻合并，长耗时 Windows 与 macOS 真实 App 检查仍在运行。聊天中的流程约定和绿色的事后结果都不能阻止一次红灯候选进入 `main`。

## 目标与非目标

目标：

- 每次人工或 agent 改动必须先进入 PR；
- `main` 只在四项固定检查全绿且候选已包含最新 `main` 后接受 squash merge；
- `gh pr merge --auto` 在检查运行中保持排队，而不是立即合并；
- 单维护者仓库不依赖无法获得的自我审批；
- Auto Release 仍能修改版本文件并推送 tag，但版本改动也必须经过 PR 与四项检查；
- GitHub 设置可由仓库文件审计、验证和重复应用。

非目标：

- 不把每次 merge 变成 release；
- 不用管理员 bypass 代替正常交付；
- 不在本次改变产品运行时、UI 或发布节奏；
- 不声称一次规则配置就替代独立 QA、发布产物和真实主路径验收。

## 决策

采用单一 repository ruleset 保护默认分支：必须 PR、只允许 squash、解决全部 review conversation、禁止删除和 force push，并严格要求 `governance-baseline`、`agent-bridge-linux`、`check`、`remote-real-app-gui`。当前只有一个维护者，approval 数为 0；未来存在独立 reviewer 后再提升为 1。

ruleset 不配置任何 bypass actor。GitHub live API 已证明：个人仓库不能把内置 GitHub Actions Integration `15368` 设为 bypass，API 会返回 `422 Actor GitHub Actions integration must be part of the ruleset source or owner organization`。因此 Auto Release 保留 `RELEASE_PAT`，但它只能推送自动化分支并创建版本 bump PR；该 PR 走相同 required checks，合并后 workflow 才能给 merge commit 打 tag。仓库开启 auto-merge，移除与新规则冲突且会造成单人死锁的 classic 1-review 设置。

## 成功指标

- 新 PR 在 required checks pending 时仍为 `OPEN`，且 `autoMergeRequest` 非空；
- 四项检查全部成功后 PR 才能合并；任一失败时不得合并；
- GitHub API 返回的 active ruleset 与 `.github/rulesets/main.json` 一致；
- classic review requirement 不再存在，ruleset 的 bypass actors 为空；
- Auto Release 的版本 PR 通过四项检查并自动合并，tag 精确指向其 merge commit。
- 发布 PR 落后 main 时不扩张已规划批次；PR merge、tag、Release dispatch 任一中断后下一轮可恢复。
