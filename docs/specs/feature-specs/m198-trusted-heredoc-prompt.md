# M198：信任模式解释器 heredoc 与审批理由

## 用户原始规格（需求与约束原文）

- R1 根因并修复：信任模式下，命令只在会话已登记工作区内读写文件（包括 python/node 等解释器 heredoc 改源码）时不得弹审批；M182 的危险清单（推送、改远端、破坏性 git、越界路径、外发网络等）照旧拦截。
- R2 审批意图必须落库「拦截理由 + 命中的规则」，界面审批卡片上显示人话理由；无法给出理由的拦截视为缺陷（fail-closed 仍可，但必须有理由）。
- R3 回归：把上面原文作为固定 fixture 加入测试（先失败）；再加一组「解释器 heredoc 写工作区文件 / 写越界路径 / 内含 git push」的对照，越界与 push 仍拦。
- R4 真实路径：e2e（合成会话、trusted）跑上面原文命令 → 零审批、文件被改写；附越界对照。

约束：task-template（harness + SDD + TDD）；合成 fixture；不改生产数据。授权交付（ceiling: merged）。Release-Urgency: immediate。

## Requirements Traceability

| Req ID | 最低证据 |
| --- | --- |
| R1 | heredoc 解析与文件操作证明测试；未知语法、符号链接、凭据、网络、子进程对照 |
| R2 | SQLite prompt_args_json 内持久化 rule/reason；审批卡片渲染断言与真实浏览器截图 |
| R3 | 固定事故 fixture，先红后绿；越界和 git push 保持拦截 |
| R4 | 合成工作区与 trusted 网关集成，真实解释器执行，断言零审批和内容改写；越界不执行 |

## Applicable Harnesses

Spec、Compatibility、Payload、Observation、Viewport、Release、AI Collaboration。合并不等于发版；安装版真实应用验收由编排方发版后完成。

## 追加规格（编排方 10-29 更新，原文）

信任模式下这条命令弹审批：`git fetch --prune origin main && git merge-base --is-ancestor origin/main HEAD; rg ...; cat docs/repo-governance-profile.md; ls ...; git config core.hooksPath .githooks`。全是只读操作，外加仓库规定的 `git config core.hooksPath .githooks`（工作区本地配置），也应纳入同一修复与回归测试。

需求补充：仓库规则要求的基线同步（fetch/merge origin/main）在信任模式下绝不弹审批；回归测试要覆盖这条与上一条原样命令。

这是信任门禁整体回归，不是单个 heredoc：先做家族审查，拉出 10-24 前后合入的、改动 trusted_git.rs / 审批判定的 PR，找出是哪次改动扩大了拦截面；把这些天全部被拦命令（只取命令形状，脱敏）做成表驱动回归 fixture，逐条标注「应放行/应拦」及理由。只有真正的破坏性远端/不可逆动作（force push、删分支/标签、reset --hard 到非本地提交、rm 工作区外路径等）才在信任模式下拦；其余一律放行。

新增原形：`git fetch --prune origin main && git merge origin/main && git diff --check`、`git merge --autostash origin/main && git status --short && git diff --check`。

会话已授权交付时，对本会话自己 PR 的标签/评论/推送不应再问：`gh pr edit <自己的PR> --remove-label 'merge-queue: armed'`。

## 家族审查

- `a726f6fff`（10-24，M166）：复合 Git 命令走 `sensitive -> backup_registered -> prove`，整条命令任何非白名单段失败都转审批，这是拦截面扩大的入口；不是 M182 单次提交造成。
- `e56d7ad72`（10-26，M172 / #870）：修正敏感词整段扫描，但正文仍按 shell 引号解析，不能识别 heredoc 的语言边界。
- `cf61acb98`（10-26，M177）、`f6527fa9d`（M180）：补齐只读 worktree 与部分同步组合；仍漏 `diff --check`、merge-base、本地 hooks 配置、混合 rg/ls。
- `6e391534f`（10-29，M182）：改为部分 Git denylist，仍在整条命令 prove 中保留只读过滤白名单，且 config 全部拒绝。更新测试已先失败定位到本地 hooks＋混合发现命令，修复后通过。
- `shell_policy` 对 `gh pr edit` 一律 Ask，发生在会话交付授权检查之前；新增精确标签/评论解析，并要求持久化本会话 PR 编号、登记分支和交付授权同时成立。其他 PR、仓库覆盖、复合命令不继承此授权。

已知脱敏命令形状固定在 `src-tauri/tests/fixtures/m198-trusted-family.json`；不读取生产数据库。未取得的逐日全部审批命令不冒充已收集。支持 Python 和 Node 静态文件编辑；未知语法仍 fail-closed 并显示理由，不能把此实现宣称为对任意程序的安全证明。

## Implementation Notes

- 任务提供的复现 JSON 在 classify_extension_error 处包含 `…<truncated>`，缺少 Python 脚本结尾和 heredoc 终止符，不能作为可执行程序。保留原始截断证据，另用完整、合成的同形文件替换命令作可执行验收；不伪称截断命令执行成功。
- 安全证明必须基于完整 quoted heredoc 与解释器语法，不得仅靠搜索路径或危险单词来放行任意程序。不能证明时保留审批并记录原因。
- 后台操作入口沿用现有 dispatch / pending approvals，不依赖 GUI 点按。
- Context scope：可信 shell、登记工作区、审批持久化与卡片。Assumptions：仅证明受支持的静态文件编辑语法，不扩大外部执行权限。Review point：安全反例与真实文件结果。Validation result：待验证。
