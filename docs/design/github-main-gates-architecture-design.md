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
  -> GITHUB_TOKEN / GitHub Actions App 15368
  -> ruleset-only machine bypass
  -> version files + tag
  -> explicit release.yml dispatch
```

required checks 全部绑定 Integration `15368`，避免同名外部 status 冒充。`governance-baseline.yml` 的 `push` 与 `pull_request` 都限定 `main`，从而避免 PR 分支 push 和 pull_request 在同一 SHA 产生两个同名 context。

## 同步协议

- `validate`：离线验证安全不变量；不访问或修改 GitHub。
- `plan`：读取 GitHub，报告 `converged` 或 `drift`；不修改。
- `apply`：先创建或更新 active ruleset，再开启 auto-merge，最后移除旧的 1-review 规则；中途失败时保持 fail-closed。
- `verify`：重新读取 GitHub；ruleset、auto-merge、classic cleanup 任一不一致即退出非零。

同步器拒绝以下策略：非默认分支目标、少于四项检查、非 strict check、人工/管理员 bypass、非 GitHub Actions 来源、允许非 squash、审批数不为 0。

## 发布兼容性

ruleset bypass 识别 actor，而当前 `RELEASE_PAT` 的 actor 是维护者本人。因此 checkout/push 必须显式使用 `secrets.GITHUB_TOKEN`。GitHub 防递归语义意味着该版本 bump push 不会重新触发普通 push workflow；这是预期行为，因为 PR 已完成完整 CI，Auto Release 自己验证 `main` 的 governance 状态，并显式 dispatch `release.yml`。

Integration bypass 适用于拥有 `contents: write` 的 GitHub Actions workflow，不只 Auto Release。风险控制点是仓库默认 workflow permission 保持 read、写权限只在具体 workflow 声明，同时 workflow 文件本身必须经过同一 PR 与四项 required checks。

## 回滚

若规则误配导致交付阻塞，先提交修正规则文件和脚本；紧急时由仓库管理员把 ruleset 暂时切为 `evaluate`，不得把维护者或 RepositoryRole 加进长期 bypass。恢复 active 后运行 `verify` 并用新的 probe PR 重验等待与合并行为。
