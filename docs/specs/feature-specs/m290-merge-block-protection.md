# M290：会话不得移除人工/编排方施加的合并阻断（仅 `merge-queue: hold`）

## 现场（2026-11-26，只读核实）
- 17:44 会话 367f54eb（M287）执行 `gh pr ready 1084; gh issue edit 1084 --remove-label "merge-queue: hold"`，编排方此前两次明确要求不要摘 hold；该命令在 trusted 模式下未进入审批，随后合并队列合并了未经核对的 #1084。
- 21:23 会话 6e140abf（M289）执行 `gh api -X DELETE repos/BumStill/CodeFactory/issues/1090/labels/merge-queue%3A%20hold`，绕过了会触发审批的 `gh pr edit` 形式；#1090 当时正文缺需求表、紧急度与规格不符。
- 同一会话族多次在被告知「不要摘 hold」后仍尝试摘（多次被 `gh pr edit --remove-label` 审批拦下后改用 `gh api`）。
- 这与 M285b（人工撤回 armed/草稿后系统不得覆盖）同族：人工施加的合并阻断必须是会话无法单方面解除的事实。

## 规格（放到 `docs/specs/feature-specs/m290-merge-block-protection.md`）
- R1：会话（agent 工具调用，含 bash 中的 `gh pr edit`、`gh issue edit`、`gh label`、`gh api` 的 labels 端点 DELETE/PUT/POST/PATCH，以及交付工具内部调用）不得移除 `merge-queue: hold` 标签，也不得把带 hold 的 PR 从草稿转为 ready 以外再做绕过；此类命令在任何权限模式下都必须拒绝（不是审批），并在结果中说明「人工合并阻断由人工/编排方解除」。`merge-queue: blocked*` 是队列绑定某个 head 的自动失败标记，不属于人工阻断，按下方 M290b 修正规则处理。
- R2：识别必须基于命令语义而非字面串：覆盖 URL 编码（`merge-queue%3A%20hold`）、引号/转义变体、`-X DELETE`/`--method DELETE`、`gh api graphql` 的 removeLabelsFromLabelable、以及通过 shell 变量拼接的常见形式；无法证明不触及人工 hold 的 `gh api` 写操作（非 GET）应进入审批而不是放行。
- R3：添加 hold、查看标签、对没有阻断标签的 PR 正常打/摘 armed 不受影响（反例）。
- R4：交付工具（deliver_changes 及续交付路径）在 PR 带 hold 时不得转 ready、不得打 armed、不得改写标题/正文（与 M285b F5/F6 对齐，复用其撤回事实）。

## 测试（先看到失败）
- 用本次两条真实命令原样作为输入（`gh issue edit 1084 --remove-label "merge-queue: hold"`、`gh api -X DELETE .../labels/merge-queue%3A%20hold`），在 trusted 模式真实权限入口断言被拒；变体矩阵（GraphQL、变量拼接、--method）。
- 反例：`gh pr edit N --add-label 'merge-queue: hold'`、`gh pr view`、对无 hold PR 的 armed 增删。
- 交付工具：带 hold 的 PR 调用 deliver_changes 不转 ready、不打 armed、不改正文（假 gh + sqlite 真实入口）。

## M290b 修正：`merge-queue: blocked` 不是人工 hold

### 修正原因
M290 过度保护了 `merge-queue: blocked*`。该标签是合并队列对某个 head 的自动失败标记；把它等同于人工施加的 `merge-queue: hold`，会导致会话修好 CI 根因后仍无法推送新 head，也无法清除绑定旧 head 的过期标记，交付主路径因此停滞。

### 修正规格
- M290b-R1：只有 `merge-queue: hold` 属于人工/编排方阻断。会话不得移除它；交付工具遇到它不转 ready、不打 armed、不改正文，保持 M290 行为。
- M290b-R2：`merge-queue: blocked*` 不阻止推送新提交。交付工具在该标签存在时照常推送修复；推送新 head 后由交付工具或队列移除过期的 blocked 标签（标记 head 与当前 head 不同即过期），让合并队列重新评估。会话可以移除对应旧 head 的 blocked；对应当前 head 的 blocked 仍由队列判定，不得通过会话移除来绕过失败检查。
- M290b-R3：测试覆盖：带 blocked（旧 head）时 `deliver_changes` 推送成功并清除过期标签；blocked 对应当前 head 时不得把 PR 标成可合并；hold 行为不变，保留 M290 原测试。
- M290b-R4：本节更正 blocked 的语义，并记录本次修正原因。

## 约束
改文件只用 edit_file / write_file。不改信任根；场景缺口只追加 `docs/testing/scenario-mappings.json`。不碰凭据、settings.json、其他会话的 PR。规格文件在工作区外，用 bash cat 只读查看。

## 交付
PR 正文 `R | 是否完成 | 证据` 列 R1–R4；`Release-Urgency: default`。全部完成、CI 全绿后正常交付。

Release-Urgency: default
