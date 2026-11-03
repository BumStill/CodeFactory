# M208：已授权交付的会话操作自己 PR（打标签/评论）仍被 shell.safety 拦

Release-Urgency: immediate

## 证据（v2-tasks.log 已累计 4 次人工批准）

- 10-29 23:37 会话 a06be073：`gh pr edit 929 --add-label 'merge-queue: armed'`
- 10-30 05:2x 会话 05638da0：`gh pr view 933 ...; gh pr edit 933 --remove-label 'merge-queue: blocked' --add-label 'merge-queue: armed'; gh pr comment 933 --body '...'`

两者都是信任模式、dispatch 时 `delivery_authorized: true`（ceiling merged），PR 由本会话交付工具创建。拦截理由：`命令命中安全规则：remote write: gh pr edit`。

## 规则（已拍板，见 10-20「权限两档」）

远端动作看「会话级允许交付」：会话已授权交付时，对**本会话创建/登记的 PR** 的以下操作不弹审批：加/摘 `merge-queue: armed`、`merge-queue: blocked` 标签、评论、标记 ready、更新标题/正文。目标 PR 是否「本会话的」按交付工具记录或本会话登记工作区分支 == PR head 判定，不按文本猜。

不放宽：别人的 PR、close/merge/删除分支（未合并）、改仓库设置、改 ruleset、给非本会话 PR 打标签仍拦；未授权交付的会话仍拦。

## 要求

1. 夹具：上面两条原样命令 + 同样命令作用于他人 PR 号 + 未授权会话，各判一次，先红。
2. 复合命令里 gh 段与其它段按各自单独判定取最严（与 M207 同一原则）。

## 验收

装机后授权交付的信任模式会话给自己 PR 打 armed 标签不产生 permission_intents。

## 补充（10-30 05:56，会话 05638da0）

§8 收尾删除**本会话已合并 PR** 的远端分支 `git push origin --delete codefactory/objective-…` 同样弹审批。按同一规则：授权交付 + PR 已 MERGED + 分支是本会话登记工作区分支 → 放行；其余删除仍拦。

## 补充（10-31）

- 交付已授权时，`git push origin HEAD` / `git push -u origin <本会话分支>`（推本会话已登记分支，非强推、非主分支）在 trusted 模式下也被 `trusted.git.unproved` 拦截（会话 1ec2435a，10:14）。同样纳入本单：推本会话自己的分支不应询问。

## Requirements Traceability

| Req | 需求 | 实现 | 证据（测试） |
| --- | --- | --- | --- |
| R1 | 授权交付会话对本会话 PR 加/摘 armed·blocked 标签不询问 | `trusted_git::own_remote_actions` + `DesktopPermissionGateway::own_remote_owned` | `m208_own_remote_writes_need_only_delivery_authority`（allow 组，`permission_intents` 保持 0） |
| R1b | 评论、标记 ready、更新标题/正文同样不询问 | 同上（`comment --body`、`ready`、`edit --title/--body`） | 同上 |
| R1c | 他人 PR 号 / 未授权会话仍询问 | 归属查询 `thin_delivery_refs ⋈ execution_workspaces` + `sessions.delivery_authorized` | 同上（negative 组） |
| R2 | 复合命令逐段判定、取最严 | `own_remote_actions` 逐 segment：自有远端动作之外的段必须单独 Allow、非 delivery、非 sensitive，否则整体回落到原门禁 | `m208_compound_segments_are_judged_one_by_one`、gateway negative 组 |
| R3 | 本会话已 MERGED PR 的交付分支可远程删除；未合并/他人分支仍拦 | `OwnRemoteAction::DeleteBranch` + `thin_external_observations.merged_at IS NOT NULL` | 同上（delete 三段：未合并拦、合并放行、他人分支拦） |
| R4 | 推本会话已登记分支（`HEAD`、`-u origin <branch>`）不询问；强推/主分支仍拦 | `OwnRemoteAction::PushBranch`（语法层拒绝 force/`:`/main/master，归属层要求登记分支） | 同上 |

## Protected behaviors（不得放宽清单，均为负向断言）

- `gh pr merge` / `gh pr close` / `gh pr edit <他人 PR>` / `gh auth token` / `gh ruleset` 仍询问或拒绝。
- `git push --force origin <branch>`、`git push origin main`、`git push origin --delete <未合并或他人的分支>` 仍询问。
- 未授权交付（`delivery_authorized = 0`）时以上全部操作仍询问。
