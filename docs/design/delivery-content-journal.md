# R2：逐次宿主调用的内容证据

## 现有真实身份

普通 thin 会话仍通过 commands/chat.rs 创建/继续 Objective，并通过 resolve_chat_execution_cwd 分配或恢复登记工作区。chat_turn_state 绑定 session/root/Objective；chat_run_controls 绑定精确 run_instance、root、Objective revision 和 Stop 状态；execution_workspaces 固定 base/repo/worktree。ToolCtx 携带 session/root/trajectory session，但没有 run_instance，需要从数据库严格解析唯一 active 且 root/Objective 完全匹配的运行，不回退到其他会话。

thin 的 mutation_preflight 继续原样早退；权限网关、薄会话终态和恢复状态不变。本模块只观察实际执行，不运行工具、不调度、不重放、不授予任何权限。没有完整可信身份就没有证据，不能把接口/mock 当成真实支持。

## 窄 schema

新增 workspace_content_intents：每次实际 dispatch 生成宿主 UUID，关联 normalized tool_call_id、trajectory_session、session/root、Objective/revision、workspace ID/base/repo/worktree/branch、run_instance、permission content epoch、action fingerprint、完整精确 manifest 及前置/预期摘要。内容字段写入后不覆盖。

新增 workspace_content_results：每个 dispatch 至多一条终态观察，状态 applied/conflict/unknown、actual_success、observed_digest、完成时间。崩溃只留下 intent，没有 result；不猜成功、不重放。相同命令的新输入属于新 dispatch，因此旧来源链不会被覆盖。legacy 的 Replay 分支在实际执行边界之前返回，不生成新 dispatch 证据。

session_content_epochs 及 permission_mode 更新 trigger 仅为在途观察提供单调版本。它不改变权限设置或允许/拒绝结果。prepare 与 finish 必须 epoch 相同，捕获 trusted→safe→trusted 的 ABA。真实 schema 缺 permission_mode 或 trigger 不符时不得声称有此防护。已成功完成的历史内容不因之后权限变化被删除或统一撤销；是否可以继续执行/远端交付仍由现行门禁判断。

## 接口与事务

content_provenance::prepare(db, call, args, ctx) 在既有权限/准入之后、实际 executor 前运行：构建 native write/edit 或有界 shell 精确意图，核实当前唯一 active run、Objective revision、登记工作区、未请求取消、tool_call 对应 trajectory owner。插入 intent，不改变旧业务状态。

finish(db, prepared, actual_result, ctx) 只接受该内存中的宿主 prepared handle；重新核实同一 run/root/Objective revision/workspace/epoch 与未 Stop，重新观察完整 manifest。仅实际 Done 且非 error、完整 expected==observed、绑定未变时追加 applied。其他情况为 conflict/unknown，不能用于交付；错误和崩溃不自动恢复为成功。

TaskDeliveryScope::load 读取匹配当前 Objective/workspace/base 的已完成记录，验证 action/toolcall 关联和聚合摘要，再与原有 legacy 来源组一起做读取依赖固定点。来源组不授予新权限。历史已完成输入可连接合法后续调用；不能从未完成行或外来输入自环获得证明。

本轮先覆盖真实 foreground chat thin；task 子会话需要 task_run 的独立活动/取消身份，未接通前不能借 chat 身份降级认领。相同观察器也可用于 legacy 的实际执行，不改变既有 receipt/replay 语义。

## 先红后绿验收

thin 标记必须在写入前设置；实际 chat_run_controls/root/Objective/workspace 均准备完整。真实 backend native/shell 执行→逐次记录→TaskScope→本地 bare remote；不能先用 legacy 写完再切 thin。覆盖相同命令不同输入、多次提交、外来读取源、未知 legacy receipt 不阻断 thin 工具、Stop/换 run/换代次、权限模式 ABA、失败/崩溃未完成行不认领，以及既有 native/legacy/shell 回归。所有数据库仅使用测试夹具。

新的内容 journal 不要求 objective_bindings：fresh thin 的真实写入口尚未创建 legacy binding，测试必须在零 legacy binding/receipt 下运行。run_instance + Objective revision + workspace 注册身份构成观察代次；它不是新增业务权限代次。

## 迁移、保留与未验证边界

生产启动的 `sqlx::migrate!` 包含 0027。现有启动兼容路径可能在迁移错误后继续运行旧 `ensure_schema`；因此观察器在首次准备时仍幂等检查本表和 epoch trigger，缺少前提或 trigger 内容不符时不签发来源证明。这个检查不修复权限、不把旧历史自动标记为已证明，也不把已有启动回退宣称为完整迁移验收。

迁移测试从测试数据库移除仅 0027 的对象和迁移登记，模拟此前 schema，再由当前生产 `storage::db::connect` 升级及二次重开。它覆盖当前版本的迁移幂等与 SQLite 完整性；没有运行旧版 binary，不能据此宣称旧版回退兼容。

已完成内容证明仍依赖 tool_calls/trajectory 的可核验历史。删除所属数据会按外键清理；修改工具参数、状态或关联会令读回验证拒绝。历史日志清理后可能无法自动交付原改动，这是保守拒绝边界，不应重新采样文件来补造历史来源。

POSIX 精确 shell 子集不覆盖任意编译器或脚本输出。Windows 不生成 POSIX shell 意图；Windows 原生文件读取防护仅做源码接线，尚无本轮 Windows 实跑证据。prepare 与实际写入之间也不构成 OS 级独占工作区：外部进程同路径并发写入的丢失更新问题不能以最终摘要一致声称消除。
