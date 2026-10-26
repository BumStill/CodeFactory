# 会话记忆系统：现状、风险与分层恢复设计

> T2 文档交付；调研日期：2026-10-26。状态：设计提案，**未实现、未发版**。
> 基线：`c9da423be`。本次不修改产品代码、数据库、权限、压缩策略或场景 trust root。
> 硬约束：任何清理/压缩都不能让「继续」丢掉历史上下文。

## 1. 任务原文（逐字保留）

```text
# T2：会话记忆系统现状梳理 + 深度设计（用户 10-22 要求「有空时」做）

## 背景
用户要求：梳理 CodeFactory 会话记忆系统现在的各个环节，并做深度分析与设计；硬约束——任何清理/压缩都不能让「继续」丢掉历史上下文。已有素材：`/Users/leo/.codefactory/agent-tools/research-session-memory.md`。
## 要做（只产出文档，不改代码）
- 现状：消息持久化、上下文窗口/压缩与摘要、重启续跑（thin restart、history_repair）、工作区/objective 绑定、跨会话知识（如有）——每个环节的数据流、存储表、触发条件、已知缺陷（引用 10-21~10-26 的真实事故：残缺历史重放、续跑丢中断说明、分裂工作区等）。
- 业界全景：Claude Code / Codex / Cursor / Cline / Aider 等对会话记忆、压缩、长期记忆的做法（WebSearch 核实，给出处）。
- 设计：目标、分层记忆模型、压缩与可追溯原文、继续时的上下文重建、跨会话记忆、可观测性与测试方案、迁移步骤；列出需要用户拍板的决策点。
- 产出：`docs/specs/feature-specs/session-memory-design.md`（中文），PR 交付。
## 约束
- task-template；只改文档；授权交付（ceiling: merged）。

Release-Urgency: default
```

上述本机素材路径仅为任务出处，不作为产品寻址方式。以下 Req ID 是对原文的文档验收映射，不授权后续产品实现。没有新增 Spec Amendment。

## 2. 结论与边界

1. **持久历史、请求上下文、执行状态、长期知识是四类不同资产。** SQLite 里有消息，不代表下一次请求带了这些消息；协议合法也不代表模型理解中断前做了什么。
2. 当前压缩核心是确定性的截断/淘汰，不是持久化的语义摘要。必须先补可追溯性、再考虑摘要模型和检索优化。
3. 继续不能只重放对话：必须同时恢复目标、工作区、交付所有权、未确定副作用和最新验证证据。记忆不得升级授权，也不能替代实时 Git/PR 状态。
4. 原文可恢复是存储不变量；关键约束进入本轮请求是装配不变量；模型语义不遗忘需要场景验收。不能承诺有限窗口同时容纳无限历史或任意摘要绝不损失信息。
5. 推荐先做非破坏性影子装配与回执，再上线带来源的摘要，最后做跨会话学习。下文所有新表、接口、阈值、UI 都是**拟议**，不是已交付能力。

### Requirements Traceability

| Req ID | 原文要求 | 本文位置 | 最低证据 / 本次判据 |
| --- | --- | --- | --- |
| T2-R1 | 梳理所有现状环节 | §3–4 | 每环节有源码入口、存储、触发条件与损失边界 |
| T2-R2 | 真实事故引用 | §5 | 仓库事故规格引用；已知修复与剩余风险分开 |
| T2-R3 | 五类业界做法核实 | §6 | 五个产品官方 URL、核实日期与不可推断项 |
| T2-R4 | 完整深度设计 | §7–11 | 分层、原文引用、重建、跨会话、安全、可观测与迁移 |
| T2-R5 | 测试及决策点 | §12–13 | 可执行 oracle 设计、真机分工、待决策清单 |
| T2-R6 | 仅文档 PR 交付 | §14 | diff 仅本文；文档验收和治理 gate；合并后 §8 收尾 |

### Applicable Harnesses

- **Spec Harness**：原文保留、需求追溯、设计与已实现状态分离。
- **AI Collaboration Harness**：context scope 为任务素材、仓库源码、事故规格和官方公开文档；assumptions 为事故规格是历史记录而非本次重现；review point 为保留策略、授权边界、来源可信度；validation result 由本次 PR 记录。
- **Compatibility / Observation / Payload Harness（设计覆盖）**：旧库迁移、恢复回执、原文归档与敏感字段约束。当前不改这些运行时 surface，不声称完成产品级验收。
- 本次没有 UI 或 release artifact 变更，Viewport / Release 的运行验收不适用；未来实施各阶段需按实际触发重新启用，不借文档绕过场景注册。

## 3. 现状全景与数据流

```text
用户消息 / 工具往返
  → messages + tool_calls（持久化、脱敏）
  → storage::load_agent_history(session_id)
      排序、剔除 gate_events 指向的 rejected_candidate
  → AgentLoop::build_openai_messages
      replayable_history → repair_incomplete_tool_history → ChatMessage
  → agent-loop::run_agent_loop → history_repair::repair_interrupted_history
  → provider 对应的 context policy / compactor → wire transport

重启 / 心跳 → thin_restart 扫描 → commands/chat 续跑
  → 加载上述历史 + 中断提示 + 当前 route + 执行所有权

长期上下文 → memory.md / CODEFACTORY.md + preferences / accepted learnings
外部知识   → knowledge_* → kb_search / kb_get_chunk（不是会话原文库）
外部执行态 → objective / root turn / worktree / delivery refs（不是摘要文本）
```

### 3.1 证据入口索引（可按符号定位，不依赖漂移行号）

| 环节 | 证据路径 / 符号 | 存储 / 载体 | 触发与边界 |
| --- | --- | --- | --- |
| 原始会话 | `src-tauri/migrations/0001_init.sql`；`storage/db.rs` | `sessions`、`messages`、`tool_calls` | 建库及后续 ensure/迁移；初始 migration 不能单独代表当前 schema |
| 持久化 | `src-tauri/src/agent/persistence.rs::SqlitePersistence`、`persist_message` | messages 的正文、tool_calls JSON、reasoning、有效 route 等 | 模型消息与工具生命周期写入；派生消息会脱敏；anonymous 分支不写，不保证可恢复 |
| 历史读取 | `src-tauri/src/storage/mod.rs::load_agent_history` | messages + gate_events | 每次按 session，`created_at ASC, rowid ASC`；UI 的全记录与 provider 记录刻意不同 |
| 请求装配 | `src-tauri/src/agent/mod.rs::build_openai_messages`、`replayable_history`、`repair_incomplete_tool_history` | 内存 ChatMessage | 过滤 turn_error/notice、gate_*、rejected_candidate、command_recovery 等控制记录；恢复 reasoning/tool_call_id、附件 parts |
| 中断协议修复 | `src-tauri/crates/agent-loop/src/history_repair.rs::repair_interrupted_history` | 请求内派生消息 | loop 入口；补未知结果、丢孤立 tool、非首条 system 降为 user；不等于存储修复 |
| 窗口与压缩 | `src-tauri/src/agent/context.rs`、`context_policy.rs`；`crates/agent-loop/src/context.rs::compress_if_needed_with_tools` | 请求内存 | 根据实际 route/window、系统词和工具定义估算；见 §4 |
| 溢出恢复 | `src-tauri/src/agent/context_recovery.rs`、`agent/mod.rs::claimed_context_compression_authorization` | Objective / recovery permit | 精确授权的恢复 claim 才能强制重压；旧 binding 必须 fence |
| 重启续跑 | `src-tauri/src/agent/thin_restart.rs`；`commands/chat.rs::resume_thin_running_session`、`push_thin_resume_prompt` | thin_session_state、chat_turn_state、chat_run_controls、tool_calls | 启动/心跳扫描；见 §3.2 |
| 目标/工作区 | `commands/chat.rs`；`agent/objective.rs`；`agent/execution_workspace.rs` | objectives、chat_task_segments、会话/目标工作区记录 | 准入、追问续接、分裂修复时处理归属；不能按“继续”关键词猜目标 |
| 交付映射 | `storage/db.rs`；`migrations/0004_delivery_runs.sql` | thin_delivery_refs、session_delivery_refs、delivery_runs、delivery_run_events | PR/分支/ceiling 与执行目标对账，不能拿聊天总结当远端事实 |
| 项目记忆 | `commands/memory.rs`；`agent/mod.rs::build_system_prompt_for` | `.codefactory/memory.md` + legacy `CODEFACTORY.md` | 运行时读入系统词；两个文件各有 4000 字符读取上限，后续还有 prompt block 预算 |
| 用户/学习 | `agent/user_context.rs`、`storage/db.rs` | user_preferences、learning_events、evolution_active_memory | 全局偏好被项目覆盖；active memory 优先，legacy accepted learning 补足最近 20 条 |
| 知识库 | `knowledge.rs::ensure_schema`；`tools/knowledge.rs` | knowledge_libraries、knowledge_documents、knowledge_chunks、retrieval_events | 启用的文档库扫描/索引与按作用域检索；不是自动继承所有旧会话 |

### 3.2 恢复与 thin 状态

- 启动使用 `running_interrupted_by_restart`，选 thin `running`、无活执行者、没有已请求停止的会话；不能套用心跳 10 分钟宽限。心跳 `running_without_executor` 需要宽限并核对最近进展/控制行，防止抢占活回合。
- `resume_thin_running_session` 加载持久历史，`push_thin_resume_prompt` 补最后一步/时间和“结果未知”；提示用 user wire role，避免覆盖完整 system prompt。它是系统来源的恢复信息，不是用户新授权。
- `mark_interrupted_tool_calls` 将无结果的 pending/approved 调用写成 error，并保留“可能已执行也可能未执行”。这只是生命周期标记，不证明工具真实失败。
- 路由解析使用 `resolve_turn_route`，续跑前置失败必须结算并保留真实错误，不能留下永远 running 的回合。
- thin 四态与 legacy Objective/turn 投影必须区分；legacy 终态不应替代 thin 的真实终止事实。恢复是否活着由执行所有权和状态事实决定，不由摘要说“完成了”决定。

## 4. 压缩、摘要及其真实损失边界

`compress_if_needed_with_tools` 在估算超过窗口的 `COMPRESSION_TRIGGER`（当前 75%）时执行两遍处理：

1. assistant/tool 超大正文无论新旧都按动态字符上限保留头尾；较老半区的中等大正文替换成 elided 标记、字节/token 估计和 120 字符预览。首遍不修改用户正文。
2. 若仍超硬上限，删除最早 user 及其后的整回合，直到低于预算或仅剩两个 user 的最小骨架。**因此“用户正文首遍不改”不等于“所有用户历史都保留”。** 最小骨架本身仍可能超限，不能据此宣布请求必定成功。

这些转换是请求投影，不能据此断言数据库原文被删除；同样，数据库还在也不能断言模型看到了淘汰内容。在本次检查的压缩路径中，没有“语义摘要 + 被覆盖原文区间 + 持久引用 + 可重读接口”的完整协议。

`agent/mod.rs::run_openai` 默认启用压缩、退避与窗口扩展；`run_anthropic` 默认不启用压缩，使用不同窗口策略。恢复 claim 可在严格许可下开启强制压缩。不能将一个 provider 的默认策略泛化到所有 route。

主要风险：用户最早的禁令被第二遍删除；工具输出中部的验证失败被头尾裁掉；旧 assistant 的未完成事项变成无来源预览；后续恢复反复重读大历史但仍在同一点丢关键内容；装配前协议合法不能自动证明压缩后的 wire 仍合法。

## 5. 10-21～10-26 事故窗口：证据与解释

本次不读取或复制生产会话原文/真实 ID、不操作生产库。以下是仓库既有事故记录的匿名归纳，不伪称本次重现，也不强行给未注明日期的事故补日期。

| 事故 / 文档证据 | 记忆层失败模式 | 已记录修复 | 仍需设计兜底 |
| --- | --- | --- | --- |
| [M154 中途被杀后继续失败](m154-resume-after-midturn-kill.md) | 悬空调用/孤立结果使 provider 拒绝；末尾 system 恢复提示覆盖系统词 | loop 幂等修复；恢复提示 user role；保留真实错误 | 协议修复被丢的内容也要可追溯，不能把占位结果当成功 |
| [thin 重启续跑](thin-restart-resume.md) | legacy 终态把活回合结束；66 秒的重启被心跳宽限挡住；中断说明不可见 | 启动独立扫描、未知结果标记、保护 thin 事实 | 将中断来源与装配回执一起持久化，重复重启不丢说明 |
| [M157，明确记录 2026-10-22](m157-mid-turn-restart-liveness.md) | 执行者已死但会话仍 running；幽灵控制行挡住继续 | 真实跨进程 SQLite smoke，覆盖重复重启幂等 | 后端接管证据不等于真实桌面用户看到完整上下文 |
| [M155，明确记录 2026-10-22](m155-restart-route-single-entry.md) | 恢复选择无凭据 route，尚未模型调用就退出且不结算 | 统一 route、排除无凭据候选、结算前置失败 | route 变小需要重新预算；不能用历史模型名决定预算 |
| [M169 追问及分裂工作区](m169-followup-continuity-and-split-workspace-repair.md) | 追问另起 Objective；PR 归属与工作区分离 | session 内状态续接、歧义自愈、工作区跟 PR 持有者 | 稳定身份、版本化交接与原文关系图，禁止按当前 cwd 猜归属 |

10-21～10-26 是任务要求的调查窗口，不表示每天都有一条已核实事故。素材中的“残缺历史、丢中断说明、工作区分裂”均有上列对应仓库依据；没有额外宣称 10-26 发生了新的生产事故。

## 6. 业界全景（官方来源核实）

核实方式：2026-10-26 直接读取官方公开文档（当前工具没有 WebSearch，使用 HTTP 文档取证，不伪称执行搜索）。资料是当日可访问页面，不是历史版本快照；营销性“记住一切”不作为正确性证明。

| 产品 | 经来源支持的机制 | 可借鉴 / 不可推断 |
| --- | --- | --- |
| Claude Code | [Memory](https://code.claude.com/docs/en/memory)：CLAUDE.md 用户/项目/组织指令与 auto memory 分离；auto memory 属于按仓库、跨 worktree 的笔记；起始加载有大小边界 | 借鉴显式指令与自动学习分层；不是无限会话原文保留的证明。该页不用于推断 compaction 的事务或恢复实现 |
| Codex | [AGENTS.md](https://learn.chatgpt.com/docs/agent-configuration/agents-md.md)（由 developers.openai.com/codex/guides/agents-md 重定向）：run 起始构建全局到 cwd 的指令链、override、总字节上限 | 借鉴版本化仓库意图与作用域；AGENTS.md 不等于聊天日志，本次来源不能证明 Codex 任意压缩后无损续跑 |
| Cursor | [Rules](https://cursor.com/docs/rules)：`.cursor/rules` 的 always / glob / relevance / manual 应用，用户/团队规则与 AGENTS.md | 借鉴按需注入以节约窗口；规则是 prompt context，不是可恢复执行 checkpoint。旧 docs.cursor.com/context/rules 重定向首页，已改查实际 rules 页；不臆测内部摘要算法 |
| Cline | [Auto Compact](https://docs.cline.bot/features/auto-compact)：接近窗口时生成摘要并替换请求历史，显示摘要调用；支持 checkpoint 回退；页面同时注明其他模型可能回退规则截断 | 借鉴可见压缩、状态恢复入口；“preserves all”是官方描述，不是独立无损验证，checkpoint 回滚不保证副作用可逆 |
| Aider | [In-chat commands](https://aider.chat/docs/usage/commands.html)：`/tokens`、`/map`、`/copy-context`、`/save`/`/load`、`/clear`/`/reset`；中断的 partial response 留在 conversation | 借鉴上下文可检查及显式重建；`/save` 保存重建文件集合的命令，不等于完整运行态备份；该页不足以确认内部摘要持久化策略 |

比较维度必须分开：静态指令（CLAUDE.md/AGENTS.md/rules）、摘要（Cline）、工作树/代码状态、原文日志、检索知识、自动笔记。没有一种静态指令文件可以替代历史工具结果和交付所有权。优先复用本仓库已有 memory/preferences/knowledge/Evolution 能力，而非再引入一套无来源的全局“记忆库”。

## 7. 目标模型：五层资产、一个装配入口

### 7.1 不变量

- **I1 原文可追溯**：正常持久化会话的压缩/缓存清理不删唯一来源；用户显式删除/隐私清除是独立、明确确认的产品动作。anonymous 会话不承诺重启可恢复。
- **I2 关键事实不淘汰**：当前目标、明确禁令/接受标准、有效授权范围、待办、失败证据、结果未知的副作用必须保留来源并进入有效上下文；不能只剩模型猜测的摘要。
- **I3 身份一致**：session/objective/root/workspace/delivery 组成版本化绑定；跨 session 共享只能分享被允许的知识，不能共享执行权或发布许可。
- **I4 诚实恢复**：未知≠失败≠成功；旧测试绿≠当前 head 绿；摘要中的 PR 状态要实时重查。
- **I5 可重建**：给定同一历史水位、绑定版本、政策与预算，确定性选择相同来源；摘要内容可由模型生成，但选用版本和事实引用可重放。
- **I6 技术态自愈**：索引缺失、摘要损坏、内部绑定歧义由系统自动重试/自愈；只有保留策略、跨项目共享、删除和权限扩大等产品级决策交给用户。

### 7.2 分层

| 层 | 权威性 / 内容 | 保留与重建 |
| --- | --- | --- |
| L0 持久轨迹 | 已脱敏消息、工具调用/结果、附件引用、控制来源与审计事件 | 作为可追溯原文，追加与版本化修订；不保存凭据，也不声称恢复脱敏前秘密 |
| L1 执行事实 | 当前目标/接受标准、停止意图、有效 authorization receipt、binding、外部 PR refs、验证 head | 来自既有控制面真相源，不复制成第二个可写状态机；读取时记版本和时间 |
| L2 会话摘要 | 覆盖指定 L0 区间的目标、决策、约束、待办、引用与不确定性 | 可再生、有版本、失效原因、校验和；不能自行增加权限或宣告完成 |
| L3 项目/用户长期知识 | 现有 memory/preferences/accepted learnings 与被批准的稳定结论 | 明确作用域、来源、有效期/撤销；不存活任务状态或原始工具参数 |
| L4 本轮上下文 | 固定系统词 + L1 + L2 + 最近完整往返 + 有界检索原文 | 临时投影；记录本轮 manifest，清理后可从 L0–L3 重建 |

## 8. 拟议存储与压缩协议

### 8.1 最小数据契约（实现阶段再定 migration 编号）

优先让既有 `messages`、`tool_calls`、Objective 和交付表保持权威；新增派生表只保存来源与装配记录：

- `session_memory_checkpoints`：id、session_id、scope/binding_version、covered_through_seq、source_digest、schema_version、summary_version、summary_json、status、created_at、superseded_by。
- `session_memory_sources`：checkpoint_id、source_type、source_id、source_revision、digest、顺序区间；不能只有一个可能移动的文件路径。
- `context_assembly_receipts`：run/root、trigger（normal/continue/restart/overflow）、history_watermark、binding_version、route/window、token_estimate、checkpoint_id、included/excluded source refs、排除原因、unknown_effect_refs、assembly_digest。
- 大结果归档 manifest：blob id、内容 hash、脱敏版本、大小、所有权、引用计数、完整性与可读状态。复用已有安全文件路径/资产权限，不允许模型提供任意本机路径读取。

稳定 seq 是拟议的 session 内单调顺序，不把 `created_at` 当唯一顺序。旧库的 `created_at,rowid` 排序仅用于首次建 seq 快照，不暴露 rowid 为跨备份身份。原文有更正时新增 revision 并使相关摘要失效，不静默重写历史。

### 8.2 原子流程

1. 在一致读快照捕获历史水位和 binding_version，形成 source manifest；新到达消息放在水位之后作为 tail。
2. 脱敏后生成摘要，保留逐条来源。必含 goals、constraints、decisions、pending、unknown_effects、verification（含 head/时间）、delivery_refs 和 omission/retrieval hints；无法确认的内容标 unknown，不推测补齐。
3. 校验引用完整、digest 对得上、关键事实覆盖、工具往返闭合。摘要生成超时/空/格式错误不会替换既有有效版本；系统自动重试/自愈，回退原文或旧摘要加 tail。
4. 短事务 CAS 发布 checkpoint：binding 与水位来源未变化才可 ready。仅允许完整 ready 版本被装配使用；崩溃留下 building 可回收但不删 L0。
5. 从模型请求投影中淘汰旧正文前，必须确认原文可读且摘要/来源索引 ready。归档失败则不淘汰唯一副本。
6. 若关键事实与 tail 仍放不进窗口，按授权选择可用更大窗口，或做有界分段取原文；返回有具体恢复 owner 的系统等待，不能无摘要丢最老回合来“解决”。

### 8.3 原文读取与清理

拟议的 session history read 接口按 source id / seq 范围、字节分页读取，返回 digest、has_more、omitted_ranges；必须经过本 session 权限和审计，不能跨项目自由检索。摘要引用可由 UI 或 headless dispatch 跳到相同原文。

清理分三类：L4 缓存可删；L2 索引/摘要可再生；L0 唯一副本不得自动删。归档必须先写、校验、事务切换引用，再删冗余副本；磁盘不足优先释放可再生产物并提示存储事实，绝不静默删历史。用户显式删除会话/附件时同步清理引用、检索索引、长期知识衍生物和备份保留说明，不用“无限保留”覆盖隐私删除权。

## 9. 「继续」的统一上下文重建

拟议单一装配接口 `assemble_context(session, trigger, route_budget, ownership_snapshot)`，普通发送、用户继续、自动唤醒、重启和溢出恢复都调用；保留现有 route 与执行 fence，不创建旁路执行入口。

1. **确定身份**：按 session 找合法可续接 Objective/root，验证 worktree/branch 与交付持有者。歧义时正常接收用户消息、只暂停歧义相关副作用，系统自动对账/自愈，不按关键词或“最新 PR”乱绑。
2. **处理停止与授权**：停止意图优先；自动恢复不添加真实用户消息、不扩大 delivery ceiling；既有 user wire 恢复提示携带 system provenance，使其不能成为新用户目标/授权依据。
3. **一致读取**：捕获水位、最后中断事件、tool unknown、有效 checkpoint；控制行修复和历史快照需避免“先加载旧 pending，再标未知”造成不一致。
4. **构造优先级**：固定系统约束 → 当前用户请求和 L1（含关键旧约束）→ 有效摘要 → 最近完整工具往返 → 与当前工作相关的原文片段。预算包含工具 schemas、附件、provider overhead 和输出余量。
5. **按实际 route 重预算**：route/window 改变必须重算，不能复用上次较大模型的 manifest。必要时重建摘要/分段，而不是重复发送相同超长 body。
6. **协议校验**：装配和压缩之后再次验证 system 位置、tool_call_id 配对、reasoning 兼容；孤立结果不送 provider，但保留原文引用及排除原因。
7. **恢复前探测副作用**：对 unknown 的写入/提交/交付先读当前现场，按 action receipt/idempotency key 判断是否已发生；仅能证明未执行时重做。只读工具可按政策重跑，不能靠“结果未知”盲目重复提交或删除。
8. **发布回执并执行**：保存装配 manifest，绑定 run/head/version；后续工具 mutations 使旧验证证据失效。外部等待有系统唤醒 owner，不能让模型空转，也不能要求用户发送“继续”修系统状态。

摘要作为派生资料不能覆盖仓库 AGENTS/规格，不得把引用文档里的指令提升为工具命令。此接口应可通过本地 dispatch 在锁屏下调用，不依赖 GUI 点按。

## 10. 跨会话知识与安全边界

- 已有 `.codefactory/memory.md`、preferences 和 Evolution 是长期知识入口；新摘要默认只属本 session，不自动提升为项目/全局记忆。
- 晋升需稳定事实、来源与 scope，沿用 existing accepted learning / Eval activation，保留撤销与 supersedes。项目事实绑定仓库身份（含 worktree 关系），不只绑定 cwd 字符串；跨项目共享默认关闭，用户偏好全局与项目 override 分开。
- 活 PR、最新 head、测试是否通过、临时凭据、审批许可不晋升为稳定知识。旧结论随版本/文件变更失效，检索前过滤 scope/status，再相关度排序。
- 冲突优先级：当前有效用户/仓库约束与控制面授权高于旧笔记；来源冲突显式展示，不以“最新摘要”抹掉用户禁令。
- 密钥、cookie、真实生产参数在进入原文持久化前遵循现有脱敏，摘要/索引/日志不能绕过。跨 session 检索需独立授权与审计；导出和备份沿用隐私边界。
- 原文中的网页/工具内容是低信任数据，摘要需保留来源类型，不能把 prompt injection 提升为项目规范。删除某条知识后需使其衍生摘要失效，避免重生成时“复活”。

## 11. 可观测性与用户控制

拟议用户面板显示“本轮带入：目标、来源摘要版本、最近原文范围、未决副作用、裁剪原因、预算与 route”，支持查看来源、修正稳定知识、撤销学习和导出回执。后台提供同等 inspect/rebuild/revoke 入口；UI 操作不是唯一入口。

回执字段至少包含：`session_ref`（受控 ID）、`run_ref`、`trigger`、`binding_version`、`history_watermark`、`checkpoint_ref`、`source_digest`、`route_ref`、`window_tokens`、`estimated_input_tokens`、`reserved_output_tokens`、`included_ranges`、`omitted_ranges`、`repair_actions`、`unknown_effect_count`、`freshness`、`assembly_digest`。默认日志仅 ID/hash/计数/原因，不记录正文和凭据；详细原文查看走受控读取。

指标建议：关键事实覆盖率、引用完整率、恢复首请求失败率、未知副作用重复率、无用户重提示续跑率、压缩/检索延迟、摘要成本、装配窗口占比。不能把“HTTP 200、非空摘要、tokens 变少”作为记忆正确率。

## 12. 测试矩阵与真实应用验收分工

以下为**未来实施验收**，本次未运行这些产品行为用例；实施时先造 synthetic fixture 见红，再改最小路径并见绿。需要绑定仓库已有场景及真实测试入口，不在本次擅自注册/标 implemented。

| Case | 对应不变量 | 可执行输入与 oracle（必须字段级断言） |
| --- | --- | --- |
| MEM-01 压缩保义 | I1/I2 | 多回合变量长度文本，最早含唯一禁令和接受标准；压缩后断言两者在请求关键事实区，source digest 可逐字重读，完整原文 hash 不变 |
| MEM-02 输出中部失败 | I1/I4 | 工具长输出中部有非零失败；头尾裁剪后摘要仍引用失败原文，不能判验证通过 |
| MEM-03 中断三形态 | I4/I5 | 悬空调用、孤立结果、末尾 system；断言最终 wire 配对、系统词保留、未知字段为真、修复幂等且原文不丢 |
| MEM-04 真实重启 | I3/I4/I6 | 两独立进程/真实 SQLite：工具开始后杀进程；重启选择同 session/root/workspace、无新用户消息、未知结果被注入，最终有真实输出及状态结算 |
| MEM-05 副作用幂等 | I4 | 写操作已落盘但结果未提交；重启先查 receipt，断言外部实际副作用计数恰为 1，而非只断言模型没说重做 |
| MEM-06 分裂与并发 | I3/I5 | 两 session、追问、PR 持有段交接；断言无串绑、branch/workspace 随 PR 所有权；binding CAS 失败不发布旧摘要 |
| MEM-07 归档崩溃 | I1/I5 | 在 blob 写入、索引切换前后注入退出/磁盘满；恢复后每个 source 至少一份可校验原文；不引用半成品 |
| MEM-08 route 缩窗 | I2/I5 | 同历史切到小窗口，工具 schema/附件加入预算；断言重装配、关键事实覆盖、最终 wire 合法，必要时系统等待而非静默丢历史 |
| MEM-09 隐私与撤销 | I1/I3 | 合成秘密、恶意网页指令、撤销知识；断言日志/摘要无秘密、不执行注入、跨 scope 拒绝，撤销后所有读取不复活 |
| MEM-10 旧库与匿名 | I1/I5 | 旧 schema 回填后 seq/source 稳定；匿名会话重启明确无恢复承诺；缺摘要可回原文，不能伪造历史 |
| MEM-11 可见与后台 | I2/I6 | 真浏览器 headless 验收 inspect/rebuild/revoke 和 light/dark 布局；dispatch 返回同源回执，锁屏无需弹窗 |
| MEM-12 语义续接 | I2/I4 | fixture 随机给目标/禁令/已做事项/待办，真实模型继续；机器断言下一步符合约束且能检索抽样原文；独立于摘要自评打分 |

补充 property/fault tests：随机 message 顺序、时间戳碰撞、任意压缩边界、Unicode、大工具 schema、重复恢复；断言来源覆盖与原文可读性、闭合协议、无扩大授权。在正常、继续、重启三入口比较相同 snapshot 的 assembly digest，排除 trigger/provenance 等明确允许差异。

**真实安装版验收由编排方在未来实施发版安装后完成**：准备合成目标并记最早禁令 → 制造多次压缩 → 工具中途重启 → 无需再发“继续”接管 → 检查恢复说明/原文跳转/审计回执 → 对账 PR/worktree/head → 撤销知识、重启确认不再注入。安装后 UI 截图/点按/审计不能由本次文档治理通过替代；当前没有新增 UI、无需截图，也不声称已修复真实应用行为。

## 13. 迁移阶段与待用户拍板

### 13.1 推荐实施顺序（后续独立授权任务）

1. **阶段 A：来源与影子回执**。为现有排序建立稳定 seq/revision，记录不改变请求的 assembly manifest；完成旧库/并发/匿名兼容，feature flag 默认关闭。旧路径仍是执行者，不双跑模型或工具。
2. **阶段 B：原文保全与摘要**。先上大结果可读归档及校验，再生成结构化摘要；影子比较关键事实覆盖，故障注入验证原文不丢。摘要失败回退原文/上个 ready 版本。
3. **阶段 C：统一重建入口**。逐个接 normal/continue/restart/overflow，共用来源选择和最终协议校验；按 provider、thin/legacy 分批开启。以真实跨进程和外部副作用证据验收，不只用单测。
4. **阶段 D：长期晋升与可见控制**。接入现有 preferences/Evolution 和 scope 检索、撤销 UI/dispatch；只有隐私/注入/撤销测试通过才允许自动学习。
5. **阶段 E：可回滚推广**。比较恢复失败率、关键事实覆盖与成本。降级只关闭新投影/学习，不删 L0、不降授权 gate；旧版不能理解新 schema 时保留可读取原文与兼容视图，禁止破坏性 down migration。导出/备份需包含 source manifests，回滚版本不得写坏新引用。

每阶段独立 Spec/Req/Test/Scenario/PR；只有真实代码与 gate 绑定后才可写“已实现”。本次不规定迁移编号、默认配额或发布版本，不为未来大改预授权。

### 13.2 决策清单

**已决：用户 10-26 按推荐**（五项全部采纳推荐默认，无修改）。下表「状态」列即生效结论。

| 决策 | 已决结论 | 状态 |
| --- | --- | --- |
| D1 原文保留与磁盘配额 | 正常会话不自动删唯一原文；可归档、显式删除 | 已决：用户 10-26 按推荐 |
| D2 跨会话/跨项目共享 | 项目内显式晋升；跨项目默认关闭 | 已决：用户 10-26 按推荐 |
| D3 摘要模型/费用 | 先确定性关键事实提取，模型摘要可选且有预算上限 | 已决：用户 10-26 按推荐 |
| D4 语义无损标准 | 原文可回读 + 关键事实硬断言 + 随机化真实模型续接评测 | 已决：用户 10-26 按推荐 |
| D5 用户控制面 | GUI 与 dispatch 同源；可查来源、撤销、重建 | 已决：用户 10-26 按推荐 |

决策已落定，实施按 §13.1 顺序分阶段推进。阶段 A（稳定来源与影子回执）的独立规格见
`docs/specs/feature-specs/session-memory-stage-a-provenance-shadow.md`。容量、备份、加密/保留时长等
具体配额仍属后续阶段的产品参数，不得在实现中以配额默默破坏「继续」。

## 14. 本次文档验收与交付记录规则

- 首先执行目标文档存在性验收并观察失败；这只证明交付缺口，不是产品 TDD。写入后做独立机器校验：原文逐字包含、T2-R1～R6、各设计章节/测试与决策覆盖、源码和相对链接存在、diff 仅目标文档。
- 本次执行文档内容检查、治理基线、统一 local scenario gate、PR 最终 body 的 scenario/readme validator；实际结果列在 PR。文档-only 不要求 Rust/前端构建，不把没跑的产品测试列为 pass。
- `README-Update: reviewed`：本次仅提案，不改变公开产品承诺。Scenario-Test 仅声明 validator 确认的 impacted IDs；无影响时不编造场景 ID。
- 独立后台 agent 未启用：任务模板禁止 `delegate_tasks`，本次为文档调研的单执行流；并非独立 QA 背书。
- 合并授权仅 through_merge：CI 全绿后标 `merge-queue: armed`，由队列 squash 合并，不手工 merge、不自动发版。
- 合并后按模板 §8：API 确认 merged、squash commit 在 origin/main、检查/删除精确远端分支、检查本地无未提交/未推送、清理任务临时文件和自启进程；保留应用管理的工作区目录。本次无 release urgency trailer，不凭空设置 immediate/hold。

## Implementation Notes

本次只有设计文件；不改现有压缩、history_repair、thin 四态、数据库或权限。官方资料核实仅支撑 §6 对应事实，不能作为本产品正确性证明。后续实施需独立规格和授权；当前关于“继续不失忆”的硬约束仍是待实现系统验收目标，而非本 PR 已修复的行为。
