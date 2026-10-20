# M130：不再「按字面猜权限」——本地改动随写权限放行，远端动作由会话级「允许交付」决定（用户 10-20 拍板）

## 证据
- 用户截图（/Users/leo/.codefactory/agent-tools/m130-screenshot.jpg，Windows 企业仓库，master + MR，模型 dfa_self_deepseek_v4_flash）：用户授权「实施」，模型在 shell 里 `git commit` 被拒：「当前意图只授权本地实施，已跳过提交、推送、PR、合并或发布动作；需要交付时必须获得对应意图并调用受控交付工具。」模型只能请用户说「执行交付」这类口令。连**本地** commit 都被拦。
- 机制：`src-tauri/src/agent/dispatch.rs` 的 `steer_capability_override*`、`policy::capability_denial` 按每轮用户字面把能力判为 ReadOnly/Implement/Deliver；源码注释记载 09-08 已有 7 个会话因「继续」被判 Implement 而无法交付——同族反复按词表修补（违反「根因优先，不往词表加词」）。

## 用户决定（10-20）
- 本地改动（含本地 `git commit`、建本地分支、stash 等不触及远端的 git 操作）**随会话写权限放行**，不再看字面意图。
- 触及远端的动作（push、开 PR/MR、合并、打标签、发版）由**会话级「允许交付」开关**决定：开着直接走受控交付；关着而模型需要时，进入 `waiting_user`，界面给出一个明确问题+按钮（例：「允许推送到 origin 并开 MR 吗？」），点一下即开开关并继续；不再要求用户说口令。
- 不再从字面「升级」权限；只保留字面「降级」（「只读/别改/先别动代码」→ 本轮只读），以及显式撤销（「别推了/先不要提交到远端」→ 关闭开关）。

## 需求
- R1 删除/旁路每轮按字面判 Implement vs Deliver 的逻辑；能力 = 写权限（会话审批模式）+ 会话级 delivery_authorized。保留 ReadOnly 降级与撤销识别。
- R2 `capability_denial` 不再拦本地 git 写操作；只拦远端变更且仅当 delivery_authorized=false。拦截时**不返回内部词汇**给模型/用户，而是触发 R3。
- R3 远端动作未授权 → 会话进入 waiting_user（阶段 2 四态），界面显示问题与「允许」按钮；允许后持久化开关并自动继续同一动作；拒绝则模型收到「用户不允许推送，请在本地完成并说明」。`--dispatch` 入口支持对应的批准操作（编排方后台可点）。
- R4 shell 里的 `git push`/`gh pr create`/`glab mr create` 等与 `deliver_changes` 共用同一判定（不能一边放行一边拦）。GitLab MR 路径同样适用。
- R5 测试（先失败）：「实施」后本地 git commit 成功；未授权时 push → waiting_user 且无内部词汇；点允许 → push 继续；「只读」→ 本地写被拒；「别推了」→ 开关关闭；回放 09-08「继续」案例 → 已授权会话照常交付。
- R6 文档：docs/specs 浏览/权限相关规格更新两级模型；删除旧词表测试中依赖「字面升级」的断言（属规格变更，PR 说明）。

## 约束
- 安全边界改动已获用户批准（10-20）。合成 fixture，不写生产 DB。授权交付（ceiling: merged）。PR 正文带 `Release-Urgency: immediate`。不要超出规格自加范围。

## Implementation Notes
- Req R1–R6 原文如上。Spec、Compatibility、Payload、Viewport、Observation、Release、AI Collaboration Harness 适用。
- 本地/远端动作统一由共享 policy 分类；会话授权在 desktop gateway 实时读取，不以模型文字或缓存能力作为远端授权。
- 最低验收：共享 policy 本地/远端与只读断言；合成 SQLite 审批持久化与拒绝；headless 浏览器审批布局；完整 Rust、前端测试和构建。安装版真机验收由编排方发版后完成。
