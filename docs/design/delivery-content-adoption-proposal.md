# R2 兼容性修复：固定内容采用方案（历史提案，未批准实施）

> 此文正文是 tree `033740d4` 阶段的历史方案。后续指令要求优先技术来源证明，当前实现见 `delivery-shell-intent.md` 与 `delivery-content-journal.md`；没有实现本提案的 UI、审批 purpose 或采用权限。下文“当前”均指该历史阶段，不能当作最新候选验收结论。

## 结论与范围

Mac 独立对照确认当前候选不能交付：基线 989c8da9 的 legacy 167/0，候选 141/38；12 个新增测试不能抵消 38 项回归。合法 native write/edit 后预暂存可交付；同一合法宿主通过 bash 写入新文件/修改已有文件，执行成功但精确回执为零，不能交付。本记录引用父会话的 Mac 回执，不声称读取其 /tmp 文件。

最小兼容方案是宿主生成并让用户明确采用固定变更清单。它解决“现有内容是否由用户纳入本任务”，不声称证明 shell 对每个字节的因果作者。没有用户采用或原生精确回执的内容仍不自动纳入。任意 shell 自动归属需要另外的执行隔离工程，不能以全工作区快照差代替。

本轮不改变既有产品授权策略、不增加远端权限、不触碰真实应用数据库；旧候选和 Library 归档保持不变。本方案尚非已实现能力，也不自动解决 thin 脏树提交。

## 源码事实

- `agent/tool_recovery.rs` 的 workspace_git 仅记录聚合变化用于恢复，不提供逐文件归属。
- `tools/bash.rs` Docker 路径将实时工作区读写挂载；macOS `trusted_sandbox` 允许实时 workspace/cache。它们不排除宿主并发写入。
- `agent/permission_intent.rs` 有 Objective revision、binding generation、action signature、prompt generation、恢复 claim/lease 和一次消费。
- `permission_action_signature` 排除 `_permission`；不能把需授权的 manifest 仅放这里。
- `grant_remote_delivery_in_tx` 对 deliver_changes 批准设置 delivery_authorized 并删除 Stop fence。内容采用必须使用独立 purpose，禁止继承该副作用。
- `commands/pending_approvals.rs`、`components/PermissionDialog.tsx` 和 `stores/chat.ts` 可复用持久展示与用户响应，但目前没有内容采用界面/语义。

## 冻结对象与授权边界

宿主从真实绑定生成 snapshot，不接受模型给出的 ownership=true、允许文件列表或快照摘要作为信任来源。对象至少包含：格式版本、Objective/session/revision、binding/resource generation、repo/worktree identity、基线 commit、当前 HEAD/branch、原始 index bytes digest、完整 source manifest digest、目标 tree、逐路径 old/new blob+mode+操作、expiry、随机不可预测 snapshot ID。明确数量/字节上限，超过上限必须返回可操作失败，不能截断后批准。

UI 展示“将以下现有变更纳入本任务”，完整列出 shell/历史改动、已有暂存、未知新文件、删除和 rename 两端，允许查看差异；首次、重开及续期均不得默认勾选未知内容。用户只一次采用所选固定内容，模型不能替用户完成此决定。若支持选择子集，宿主从可信基树加已证明及用户明确采用的 immutable entries 重新生成目标 tree 和摘要；禁止以真实 index 或未经证明的当前 HEAD 为起点，否则 foreign staging/历史提交仍可能夹带。任何 selection 更改重新确认。未采用文件留在原处，不能删除或挪动以让 gate 通过。

采用回执与原生写入回执分开存储和展示。它表示用户确认纳入，而不是伪造 applied tool receipt。采用仅覆盖精确内容；不批准执行、push、PR、合并、发版，不开启会话信任，不删除 Stop fence。

## 状态与恢复

1. 只读准备快照并保存，建立独立 content-adoption purpose 的持久审批；purpose 本身纳入签名域并持久化，将完整授权摘要放入签名输入，不放可剔除的展示字段。后端 grant_remote_delivery_in_tx 及前端信任会话/自动批准分支均显式排除此 purpose。
2. 允许/拒绝响应绑定 intent+prompt generation+snapshot。仅宿主命令接收 UI 响应；不能成为模型工具的自批准参数。
3. 批准时重新核对绑定、任务状态及所有冻结输入。SQLite 中原子消费审批并写 adoption receipt；失败回滚两者。文件系统与 SQLite 不构成共同事务，因此写回执不代表立即执行：交付前仍须再次核对冻结内容，最终仅提交固定 Git tree、推送固定 SHA。写真实 index 必须先持有 index.lock，再校验原始字节并原子安装；更新 HEAD 使用固定 old OID 的 CAS。仅重读后裸写仍有竞态，不能作为通过证据。
4. Stop、撤权、变更代次、任何未记录的路径/暂存/HEAD/branch变化使快照失效；受批准的固定树生成唯一 child commit 等自身合法转换须由持久意图列出精确前后身份，不能因此误杀正常提交。拒绝、过期、旧响应、重复点击不得扩大或复用批准。重启恢复原快照及真实状态，不重新捕获内容套用旧批准。普通权限流程支持晚到响应，但 content-adoption 必须独立强制 expiry；在批准事务与实际派发前均核验持久 Stop/revocation generation/fence，恢复不得清除 fence 或复活旧快照。
5. 同一审批响应重试只幂等返回同一回执；回执只能恢复同一固定交付操作，不可开始新操作。恢复分别记录尚未执行、精确提交已完成、外部结果未知；未知结果先对账，不重复发起。
6. 批准提交后发生外来写入时停止交付，保留其文件和原 index；不能为恢复而 reset 用户数据。

## 分片与真实验收

A. 当前安全候选保留原12生产验收；增加合法 native 预暂存正例及合法真实bash正例，后者当前必须保持红灯。记录 Mac baseline 对照，不批量删除/豁免旧38断言。
B. 宿主 snapshot+独立审批 purpose+原子 adoption receipt+现有 UI 接线，形成一个可用切片。要求真实 SQLite/进程恢复、真实生产 handlers/本地 bare origin，以及完整可审阅列表的原生或浏览器验收；合成 provider 不冒充真实认证。
C. thin 精确提交事务必须保持 Git hooks 和未知 index 内容，之后才能恢复脏树。不能简单转用现有 commit-tree 并声称执行了普通 commit hooks。仓库 ensure_branch_current 拒绝 detached HEAD；可验证候选是受管的临时具名分支/worktree，固定在原 HEAD，仅放入已证明树，执行正常 git commit 后检查 parent/tree/hooks 配置，再通过已有 receipted index-lock/ref CAS 安装。hook 改动树时停止，不自动采用其新内容；不得关闭 hook。当前 worktree-start 固定从 origin/main 起步，需要受管 pinned-base 入口。必须实测路径/分支相关 hooks；不假设临时工作树中的所有 hook 语义天然与原路径相同。
D. 原38测试逐项恢复：远端行为单元测试需使用明确真实fixture或只在其单元层提供scope输入，但生产路径必须由新增真实host/adoption验收兜底。不得在生产cfg(test)关闭门禁，或把成功断言统一改为拒绝。

验收必须含：合法新文件/修改/删除/rename/历史变更/任务预暂存通过；未采用unknown及foreign staging不进入目标树；不同项目、旧epoch/旧snapshot、内容变更/新增文件均拒绝；批准不修改delivery授权/Stop；拒绝与过期无副作用；消费与receipt持久化前后崩溃可恢复且不重复；hooks拒绝/修改内容时不推送；真实index字节和未知文件保留。单纯mock批准不算生产采用路径通过。

## 自动 shell 的后续边界

若要求无需人工采用任意 shell 产物，需私有的每次执行快照/写层，禁止共享实时工作区读写挂载；所有后代结束后收集私有delta，绑定输入树及执行epoch，逐文件CAS应用到未变的实时preimage并持久化真实回执。必须处理进程后代、symlink/hardlink、rename/delete、资源/网络边界和平台差异。现有沙箱不具备这一证明，不能在本修复中假设已完成。

## 当前阻塞

内容采用是新的用户可见权限语义，需要实现并审查上述完整切片，现有普通“允许交付”不可直接复用；thin hook事务另缺实现。没有外部授权或环境安全设置变更可以自动补齐这些代码缺口。当前不申请更改保护规则，也不以全面阻断作为最终修复。
