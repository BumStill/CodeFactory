# M248b：信任模式下，真实任务的本地 git 命令还在要审批

## 现场（v1.98.0、v1.99.0，M248 已上线）
M248 的固定组合通过了测试，但真实任务里的命令仍被 `trusted.git.unproved` 拦下，每单 1 到 3 次。样本（路径已省略）：
1. `git fetch --prune origin main && git merge origin/main && git config core.hooksPath .githooks; gh auth status; sed ...; rg ...; cat ...`
2. `git fetch --prune origin main && git merge-base --is-ancestor origin/main HEAD && git config core.hooksPath .githooks; cat ...; rg ...; ls '<应用数据目录>'; ...`
3. `git add docs/specs/... && git commit -m 'docs: ...'; sed ...; grep -n 'fn git' ...; grep -n 'pub.*register\|fn.*execut\|static.*' ...; command -v cargo; df -h .; git status --short`

4. `python3 - <<'PY'` 只读打开应用数据库（`mode=ro`）被 `trusted.heredoc.unproved` 拦：「路径在工作区外」。一单批了 5 次。

## 用户决定（必须遵守）
2026-11-04：信任模式靠 OS 沙箱，不再补文字规则，不加危险词表。沙箱不可用时退回审批，绝不裸跑。
`trusted_git.rs` 的 `outside_write_sandbox_segment` 现在就是一张词表（`gh`、`curl`、`token` 等）。

## 规格（放到 `docs/specs/feature-specs/m248b-trusted-git-real-commands.md`）
- R1：把上面 3 条样本写成测试，确定每条是哪个判断项（`rewrite`、`remote`、`outside_scope`、`is_enforced`）拦下的，结论写进规格。第 3 条必须查清。
- R2：用 `permission_intents` 再统计 M248 上线后的命令形状，只留匿名聚合，确认样本有代表性。
- R3：沙箱生效时，只读取工作区外文件的 heredoc 脚本也不产生审批，写入由沙箱挡下。本地命令组合（`git add`、`commit`、`fetch`、`merge`，以及读取类命令）不产生审批，哪怕同一行混了只读的 `gh` 查询或读工作区外文件。沙箱本身挡写入，读取不需要文字证明。
- R4：判定收敛到结构化事实，能删的词表项要删。不新增按命令文字匹配的规则。如果某项必须保留，写明为什么沙箱管不了。
- R5：不放宽：网络写操作（`git push`、`gh pr edit` 等）和破坏性改写（`reset --hard`、`push --force`、删别人分支）照旧要授权；`core.hooksPath` 门禁、`--no-verify` 门禁不动；沙箱不可用时退回审批。

## 测试
- 3 条样本原样通过，不产生审批。先看到失败。
- R5 每项一条反例，包括一行混合只读和写操作的命令。

## 验收（真机，编排方发版后做）
下一单任务全程 `trusted.git.unproved` 为 0。

## 交付
授权交付。PR body 写 `Scenario-Test`，按任务模板收尾。

## Implementation Notes（不改变需求）

### R1 实测定位
修复前 `m248b_reports_legacy_proof_predicates` 在合成仓库输出：

| 样本 | rewrite | remote | outside_scope | destructive_intent | 结论 |
| --- | --- | --- | --- | --- | --- |
| 1 | true | false | true | false | `gh` 词表取消沙箱豁免；真实路径/组合证明稍有变化又会失去 rewrite fallback |
| 2 | true | false | false | false | 省略路径后的样本本身不复现；真实外部路径使旧范围证明失败，沙箱不可用/继承时没有豁免 |
| 3 | false | false | false | true | `git add`/`commit` 被敏感分类沿用为破坏性；严格组合 parser 不能证明读取组合。不是网络授权 |

`is_enforced` 必须现场探测，不能由命令文本推断；macOS 嵌套 Seatbelt 拒绝时为 false，仍退回原审批/证明路径。测试对真实宿主沙箱与不可用 fallback 分开取证。

### R2 匿名聚合
只读访问 `permission_intents`，统计自 2026-11-04 起的 bash 记录（created_at 毫秒）；不保存命令、路径、会话 ID 或凭据。统计不是按发布身份精确归因，起点采用任务给定的 M248 上线窗口。
- bash 总数 134：`trusted.git.unproved` 37、`trusted.heredoc.unproved` 82、`shell.safety` 11、`trusted.sandbox.bypass` 4。
- 两类 unproved 共 119：fetch+merge 11（含 gh 查询 6）、add+commit 7、heredoc 82（含读取探针 46）、其他 19。
- 三个本地组合家族及 heredoc 在真实审计中均出现，不仅是固定夹具。

### R3–R5 实现边界
- 删除 `outside_write_sandbox_segment` 的 gh/curl/token/路径词表；文件范围由已生效的内核写边界负责。保留现有共享 remote 与 Shell 风险裁决，不新增危险词条。
- `destructive_intent` 使用已有 executable-segment parser，add 与不带 amend 的受支持 commit 不再等同历史破坏；原有 reset/force/delete/配置绕过门禁不变。
- heredoc 在已验证注册工作区和真实沙箱生效时，AST 只证明能力；不再文字证明文件路径。外部 SQLite 连接仅接受 `uri=True` 且 URI `mode=ro`，结果可读取/打印。无网络、反射或子进程能力。
- 仍保留 AST 能力检查：当前文件沙箱网络开启，不能阻止解释器发出远端写请求；因此不能无条件放行任意解释器。未建模能力继续审批，不通过危险词表猜测。
- 无 UI/配置/schema 变更；bash 与后台进程沿用既有沙箱入口，可锁屏操作。

### Requirements Traceability 与测试矩阵
| Req ID | 最低证据 |
| --- | --- |
| R1 | m248b 三原样样本，先红后绿；legacy predicates 输出 |
| R2 | 上述只读匿名聚合 |
| R3 | m248b_sandboxed_external_database_read_has_no_path_proof；permission_gateway 注册身份/零 permission_intents；macOS 实际内核外读/拒写 |
| R4 | 删除旧词表，复用现有结构化 effects/AST/注册身份/沙箱事实 |
| R5 | m248b_history_and_remote_fences；socket/subprocess/rw SQLite 反例；m224_bypass_is_denied_with_recovery_guidance；covers_local_command(false,...) |

Applicable Harnesses：Spec、Compatibility、Payload、Observation、Release（仅合并与验收交接，不发版）、AI Collaboration。无视口变更，Viewport 不适用。

AI Collaboration：context scope 为网关、Git/AST 证明、内核启动器；assumption 为文件沙箱不限制网络；review point 为独立只读安全审查与样本三归因；validation result 以 PR 命令证据为准。

真机验收由编排方发版后完成：安装新版本，可信模式执行下一单任务，统计全程 `trusted.git.unproved=0`；核对本地提交、fetch/merge+只读 gh、外部只读数据库均无额外审批，远端写与破坏性改写仍要求既有授权；禁用/不可用沙箱时不能获得该豁免。本 PR 合并不等于已发布，not live。
