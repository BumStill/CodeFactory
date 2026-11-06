# M248：M224 上线后，任务开局同步 main 仍然要审批

## 现场（2026-11-06，v1.95.0）
- 会话 c5b6a28e（M245）开局的命令被 `trusted.git.unproved` 拦到审批：
  `git branch --show-current; git fetch --prune origin main && git merge origin/main && git config core.hooksPath .githooks; rg -n ... ; cat ...`
- 会话 eaa974eb（M242）收尾时，下面的命令也被拦：
  `git merge-base --is-ancestor <sha> origin/main && git show -s ... && git push origin --delete <本单已合并分支>; mkdir -p .codefactory-cache/gh-cache; gh run view ... > .codefactory-cache/m242-windows.log`
- 这两步都是任务模板要求的动作，每单至少打断一次，影响 2.0 的「连续零介入」指标。M204 修过同一类，M224 又说本地命令由沙箱保证，却还在发生。

## 用户决定（必须遵守）
2026-11-04：信任模式靠 OS 沙箱，不再补文字规则，不加危险词表。沙箱不可用时退回审批，绝不裸跑。

## 规格
- R1：先用数据定位。查 `permission_intents`，统计 M224 上线后 `trusted.git.unproved` 的命令形状（只留匿名聚合，不写原始路径和凭据），写进规格。
- R2：用上面第一条命令的合成版本复现，找到 `permission_gateway.rs` 里 `covers_local_command` 为什么没放行（`rewrite`、`remote`、`outside_write_sandbox_scope` 哪一项）。写清根因。
- R3：沙箱生效时，在本会话登记工作区内的 `git fetch`、`git merge origin/<默认分支>`、`git config core.hooksPath .githooks` 与其它本地命令组合，不产生审批。修法优先让判定依赖沙箱和结构化事实，不要新增按命令文字匹配的规则。
- R4：删除远端分支仍要有交付授权。只有分支属于本会话、对应 PR 已合并时才可以免审批，并且判定依据是真实状态，不是命令文字。如果做不到，保留审批，并在规格里写明，这一项算 deferred。
- R5：不放宽：沙箱不可用时退回审批；推默认分支、`reset --hard`、删别人分支、改写历史照旧；M245 的 `core.hooksPath` 判定不动。

## 测试
- R3：真实 git 仓库 + 沙箱生效的集成测试，原样组合命令不产生审批。
- R5：沙箱不可用时同一命令进入审批；每条不放宽的命令一条反例。

## 验收（真机，编排方发版后做）
下一单任务全程 `trusted.git.unproved` 为 0。

## 交付
授权交付。PR body 写 `Scenario-Test`，按任务模板收尾。

## Implementation Notes
- R1：只读查询本机 `permission_intents`，以 M224 合并提交时间为下界，`prompt_args_json` 含该原因码共 2 条：fetch + merge + hooks 配置组合 1 条；merge 查询 + 删除远端分支组合 1 条。不持久化原始命令、路径、凭据或会话标识。
- R2：真实 Git fixture 复现 `registered_rewrite=true`、`remote=false`；该字段实际表示“可在登记工作区证明并备份”，不是破坏性意图，却被直接传给 `covers_local_command` 的 `destructive_rewrite` 参数，导致正常同步无法使用沙箱豁免。同时原范围判定扫描整段文字，grep/rg 的引用参数含远端关键词时 `outside_write_sandbox_scope=true`。修复复用现有分段解析、共享只读查询分类及 Git 参数校验，不增加危险词表。继承未知外层沙箱仍不冒充本应用建立的 Enforced。
- R3/R5 执行边界：免审批不等于沙箱允许修改 config。原样组合中 hooks 配置写入仍会被内核阻止；集成测试断言 fetch/merge/后续查询完成且 config 保持不可写。此安全边界不在本任务放宽；若希望重复 hooks 设置也执行成功，需独立设计幂等的结构化设置入口。
- R4：本任务保留远端混合命令审批，标记 deferred；不扩张远端授权范围。
- 真机安装版验收由编排方发版后完成，本任务不发版（not live）。

## Requirements Traceability 与 Harness
| Req | 最低证据 |
| --- | --- |
| R1 | 上述匿名数据库聚合 |
| R2 | 合成组合的结构化判定断言 |
| R3 | 真实 Git fixture、权限决策及沙箱执行断言 |
| R4 | 远端删除仍走现有授权路径，混合形状 deferred |
| R5 | 无沙箱及危险 Git 反例、既有 M245 测试 |

适用 Spec、Compatibility、Payload、Observation、Release（交接而非发版）、AI Collaboration Harness。无 UI 变更，无 Viewport Harness。后台入口沿用 bash 与现有权限网关，无新增 GUI 依赖。
