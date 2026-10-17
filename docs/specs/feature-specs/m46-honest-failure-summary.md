# M46：失败终态摘要必须来自实测 git 状态

## 背景与用户决策

10-09 真机会话 `0508c0e1` 中，任务失败后的系统摘要写「改动已经提交在本机的分支
`codefactory/objective-d19afe4e…` 上（还没推送）」，而现场 `git log origin/main..HEAD`
为空，改动全部是未提交的工作区修改。用户因此以为改动已进入提交、清理工作区也安全，
实际上一次工作区清理就会全部丢失。同族问题：U1b（诚实终态）、「不许造用户看不懂的
中间态」。

决策：

- 摘要里关于「工作成果在哪」的每一句话都必须来自实测 git 状态，不允许由模板文案或
  配置推断。
- 未提交修改、本地未推送提交、已推送分支/PR 是三种不同的事实，必须分开如实列出。
- 有未提交修改时必须明确说清「还没提交、清理工作区会丢失」，并给出工作区路径。
- closeout 遇到未提交或未推送内容必须保留，并在会话里可见（与 CF-CLOSEOUT 一致）。

## Requirements Traceability

| Req ID | requirement | minimum evidence |
| --- | --- | --- |
| R0 | 用合成 fixture 复现（或证明不可能）；已修复则补回归测试锁住 | 合成 git 仓库单元测试 `m46_*`，四个状态各一条，断言摘要文本与 `git` 实测一致 |
| R1 | 失败/停止/恢复耗尽等终态摘要中，工作成果描述必须来自实测 git 状态；未提交修改（文件数）、本地未推送提交（数量+最新提交标题）、已推送分支/PR 三者分开列出 | `failure_summary` 渲染测试 + `objective` 观测函数测试断言三段分别出现且不互相冒充 |
| R2 | 有未提交修改时明确说「还没提交，工作区清理会丢失」并给出工作区路径；系统自动保留了什么也要写清 | 渲染测试断言出现「还没提交」「清理工作区就会丢失」和工作区路径；保留说明由应用自身行为（工作区保留、不自动提交）决定 |
| R3 | 工作区清理（closeout）遇到未提交或未推送内容必须保留并在会话里可见 | `execution_workspace` closeout 测试：脏工作区/未推送提交的候选保持 `cleanup_pending` 且记录可见原因 |
| R4 | 测试覆盖仅未提交 / 仅本地提交 / 已推送 / 三者混合四种状态，先失败 | 四个测试先红后绿 |

## Implementation Notes

- 根因是观测范围错误：`unpushed_commits` 用 `git rev-list --count HEAD --not --remotes`
  统计整个分支历史上所有不在远端的提交（包括工作区基线之前、与本任务无关的本地提交），
  再配合 `base_sha..HEAD` 的 `commits_ahead` 就得出「本机有提交、还没推送」的错误结论。
  观测改为把两者都限定在 `base_sha..HEAD` 之内，因此本任务没有提交时不可能再声称有提交。
- 未提交修改的文件数通过 `git status --porcelain` 实测，与提交计数互不冒充。

## Applicable Harnesses

- Spec Harness：本规格 + Requirements Traceability。
- Compatibility Harness：结论只依赖 `git rev-list` / `git status`，对旧数据库与旧会话保持仅观测、不迁移。
- Observation Harness：摘要与 closeout 都记录可核对的实测结果。
