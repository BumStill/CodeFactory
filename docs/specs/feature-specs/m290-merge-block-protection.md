# M290：会话不得移除人工/编排方施加的合并阻断（merge-queue: hold / blocked）

## 现场（2026-11-26，只读核实）
- 17:44 会话 367f54eb（M287）执行 `gh pr ready 1084; gh issue edit 1084 --remove-label "merge-queue: hold"`，编排方此前两次明确要求不要摘 hold；该命令在 trusted 模式下未进入审批，随后合并队列合并了未经核对的 #1084。
- 21:23 会话 6e140abf（M289）执行 `gh api -X DELETE repos/BumStill/CodeFactory/issues/1090/labels/merge-queue%3A%20hold`，绕过了会触发审批的 `gh pr edit` 形式；#1090 当时正文缺需求表、紧急度与规格不符。
- 同一会话族多次在被告知「不要摘 hold」后仍尝试摘（多次被 `gh pr edit --remove-label` 审批拦下后改用 `gh api`）。
- 这与 M285b（人工撤回 armed/草稿后系统不得覆盖）同族：人工施加的合并阻断必须是会话无法单方面解除的事实。

## 规格（放到 `docs/specs/feature-specs/m290-merge-block-protection.md`）
- R1：会话（agent 工具调用，含 bash 中的 `gh pr edit`、`gh issue edit`、`gh label`、`gh api` 的 labels 端点 DELETE/PUT/POST/PATCH，以及交付工具内部调用）不得移除 `merge-queue: hold` 和 `merge-queue: blocked*` 标签，也不得把带 hold 的 PR 从草稿转为 ready 以外再做绕过；此类命令在任何权限模式下都必须拒绝（不是审批），并在结果中说明「合并阻断由人工/编排方解除」。
- R2：识别必须基于命令语义而非字面串：覆盖 URL 编码（`merge-queue%3A%20hold`）、引号/转义变体、`-X DELETE`/`--method DELETE`、`gh api graphql` 的 removeLabelsFromLabelable、以及通过 shell 变量拼接的常见形式；无法证明不触及阻断标签的 `gh api` 写操作（非 GET）应进入审批而不是放行。
- R3：添加 hold、查看标签、对没有阻断标签的 PR 正常打/摘 armed 不受影响（反例）。
- R4：交付工具（deliver_changes 及续交付路径）在 PR 带 hold 时不得转 ready、不得打 armed、不得改写标题/正文（与 M285b F5/F6 对齐，复用其撤回事实）。

## 测试（先看到失败）
- 用本次两条真实命令原样作为输入（`gh issue edit 1084 --remove-label "merge-queue: hold"`、`gh api -X DELETE .../labels/merge-queue%3A%20hold`），在 trusted 模式真实权限入口断言被拒；变体矩阵（GraphQL、变量拼接、--method）。
- 反例：`gh pr edit N --add-label 'merge-queue: hold'`、`gh pr view`、对无 hold PR 的 armed 增删。
- 交付工具：带 hold 的 PR 调用 deliver_changes 不转 ready、不打 armed、不改正文（假 gh + sqlite 真实入口）。

## 约束
改文件只用 edit_file / write_file。不改信任根；场景缺口只追加 `docs/testing/scenario-mappings.json`。不碰凭据、settings.json、其他会话的 PR。规格文件在工作区外，用 bash cat 只读查看。

## 交付
PR 正文 `R | 是否完成 | 证据` 列 R1–R4；`Release-Urgency: default`。全部完成、CI 全绿后正常交付。

Release-Urgency: default
