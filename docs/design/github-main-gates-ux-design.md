# GitHub 主分支门禁：运维 UX 设计

## 日常交付体验

贡献者仍执行原有 `PR -> CI -> squash merge`。区别是 `gh pr merge --auto --squash --match-head-commit <SHA>` 现在表示“登记合并意图”：检查未完成时 PR 保持打开；检查失败时明确停住；候选过期时必须更新分支；只有四项检查全绿且讨论已解决后才合并。

单人维护不需要制造形式化的自我 approval。独立审查证据继续由 QA/sub-agent、测试结果和真实主路径验收承担；GitHub approval 数只在出现第二个真实维护者后调整。

## 管理命令

```bash
python3 tools/governance/manage_main_branch_ruleset.py validate
python3 tools/governance/manage_main_branch_ruleset.py plan
python3 tools/governance/manage_main_branch_ruleset.py apply
python3 tools/governance/manage_main_branch_ruleset.py verify
```

`validate` 和 `plan` 安全只读。`apply` 是唯一写操作，输出仍必须由 `verify` 复读确认。命令不打印 token 或 secret。

## 可理解的状态

工具只输出两种运行时结论：

- `converged`：auto-merge 已启用、active ruleset 与仓库文件一致、旧 1-review 已移除；
- `drift`：至少一项不一致，禁止把聊天约定或本地测试当成已经生效。

真实 probe PR 的验收报告同时记录 PR 编号、head SHA、pending 时的 `OPEN + autoMergeRequest`、四项 check 结论和最终 merge SHA。若下一次真实 Auto Release 尚未发生，发布直推只能标为“配置兼容已验证，live bump 待下一次 releasable batch”，不能包装成已跑通。
