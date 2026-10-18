# M115b：统一「这个头是不是本交付的」判定——远端分支检查与 PR 头检查用同一规则（M99/M111/M115 同族第 4 次）

## 现象（真机 2026-10-18 17:07，v1.88.26 已含 M115 #732）
- 会话 e24314f8，PR #735 于 16:59 由合并队列合并（会话自己 armed，交付全程自主）。17:07 会话报 `remote branch codefactory/objective-dda5e759… moved to an unrecognized head bc66161f…; expected persisted e5649e65…` → 恢复耗尽 → objective 显示「失败」（交付其实成功）。bc66161f 是合并队列 update-branch 生成的合并头。
- 同族：M99（自推头）、M111（PR 头检查漏了自推头）、M115（PR 头检查漏了合并队列头）、本次（远端分支检查漏了合并队列头）。每次只修一个检查点。

## 需求
- R1 在 delivery.rs 抽出**唯一**的「头可解释」判定函数（持久化头、已回执父提交、本交付自推、本地预期头的祖先、合并队列/GitHub update-branch 生成且父提交含本交付已知头与 main 的合并提交），所有检查点（远端分支检查、canonical PR 头检查、PR 正文/标题更新对账、takeover 观察）统一调用它；删掉各处各自的判定。
- R2 PR 已合并后：任何头漂移都不得把 objective 置为失败；以合并结果结算。
- R3 测试（先失败）：四种头（自推、本地祖先、合并队列合并头、陌生头）× 四个检查点的矩阵；PR 已合并后分支漂移不判失败。
## 约束
- 陌生头仍失败关闭，不放宽安全边界。合成 fixture，不写生产 DB。

## Implementation Notes
- 在当前受管 worktree/分支实现；不修改数据库 schema、UI 或治理信任根。
- Applicable Harnesses：Spec、Compatibility、Observation、Release（仅交付、不发版）、AI Collaboration。
- 真机验收由编排方发版后完成：安装新版本，触发交付并让队列 update-branch，核对 CI 失败唤醒、合并后 objective 成功、陌生头拒绝的审计记录。
