# GitHub 主分支门禁：架构设计

## 权威与边界

`.github/rulesets/main.json` 是 CodeFactory 的期望 GitHub 状态，GitHub repository ruleset 是运行时强制面，`tools/governance/manage_main_branch_ruleset.py` 是两者之间的校验与同步器。仓库文档不替代 GitHub runtime，GitHub runtime 也不得成为无法 review 的手工配置。

## 规则模型

```text
PR head
  -> strict/up-to-date
  -> governance-baseline
  -> agent-bridge-linux
  -> check
  -> remote-real-app-gui
  -> conversations resolved
  -> squash merge to main

Auto Release workflow
  -> RELEASE_PAT pushes automation/release-next
  -> version bump PR + same four checks
  -> squash merge, no bypass
  -> tag the merge commit
  -> explicit release.yml dispatch
```

required checks 全部绑定 Integration `15368`，避免同名外部 status 冒充。`governance-baseline.yml` 的 `push` 与 `pull_request` 都限定 `main`，从而避免 PR 分支 push 和 pull_request 在同一 SHA 产生两个同名 context。

## 同步协议

- `validate`：离线验证安全不变量；不访问或修改 GitHub。
- `plan`：读取 GitHub，报告 `converged` 或 `drift`；不修改。
- `apply`：先创建或更新 active ruleset，再开启 auto-merge，最后移除旧的 1-review 规则；中途失败时保持 fail-closed。
- `verify`：重新读取 GitHub；ruleset、auto-merge、classic cleanup 任一不一致即退出非零。

classic review 清理用 REST `DELETE` 执行，但状态验证读取 GraphQL 中 `refs/heads/main` 的 effective `branchProtectionRule` 实时 review 字段，避免漏掉 `*`、`m*` 等命中 main 的通配规则。GitHub 的专用 REST review GET 在成功返回 `204` 后仍可能长期返回删除前 payload，不能把该缓存响应作为 drift 依据。

同步器拒绝以下策略：非默认分支目标、少于四项检查、非 strict check、任何 bypass actor、非 GitHub Actions 检查来源、允许非 squash、审批数不为 0。

## 发布兼容性

个人仓库的 repository ruleset 拒绝把内置 GitHub Actions Integration 加入 bypass；真实 POST 返回 HTTP 422。直接给 `BumStill`、RepositoryRole 或 DeployKey bypass 都会把相同能力扩散给本地 agent、PAT 或持钥者，因此不采用。

Auto Release 使用 `RELEASE_PAT` 推送固定的 `automation/release-next` 并创建或更新 PR；tag 也由 checkout 持久化的同一 PAT 推送。PAT 没有 bypass，但它创建的 PR 会正常触发 required workflows；若使用 `GITHUB_TOKEN` 创建 PR，GitHub 会把这些 workflow 置于 approval-required，无法无人值守完成。

release job 串行执行，但版本 PR 不预先登记 auto-merge：workflow 持续确认 head SHA 未变化，并等待四项指定 check 全绿后，才用 `--match-head-commit` 请求 ruleset 保护下的 squash merge。若 PR 落后 `main`，workflow 不得 rebase，因为这会把未重新评估的 `hold` 或更高 semver slot 带入已经规划的 tag；本轮直接失败，下一轮从最新 `main` 完整重算。固定分支的 force-with-lease 永远绑定首次读取的远端 SHA，远端被其他操作者改动时失败，不能 fetch 后接受新 lease。

每轮先暂停历史版本 PR 可能残留的 auto-merge并确认 checkout 仍是最新 main；随后 reconcile 最近一个已合并版本 PR：缺 tag 时验证 merge commit 只改四个版本文件且四处版本一致，再从该 SHA 补 tag；tag 已存在时必须精确指向该 SHA；没有已发布 release 或在途 Release run 时补 dispatch。这样 PR merge、tag push、workflow dispatch 三个动作之间的任何中断都能在下一轮恢复。只有 merge SHA 被确认包含于 `origin/main` 后才允许打 tag。

## 回滚

若规则误配导致交付阻塞，先提交修正规则文件和脚本；紧急时由仓库管理员暂时 disable ruleset，不得把维护者、RepositoryRole 或 PAT 加进长期 bypass。恢复 active 后运行 `verify` 并用新的 probe PR 重验等待与合并行为。
