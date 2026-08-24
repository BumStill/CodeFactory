# Skill 命令安装与失败恢复

> 领域权威：`skill-lifecycle-system.md` 定义完整 Skill package、安装事务、审核/激活、
> 按需加载与长期演进合同。本文件只定义本次 P0 修复切片；两者冲突时以前者为准，
> 且本切片的局部复制/激活行为不代表完整生命周期系统已经实现。

## 业务设计

### 问题

用户安装并启用一个标准 `SKILL.md` 后，Skill 指令可能调用相邻的 `scripts/`、
`references/` 或 `assets/`。当前安装路径只保留 prompt 和两个可选 JSON 文件；Git
安装随后删除临时 clone，因此一个原本自洽的 Skill 会在安装完成后失去执行资源。

即使命令实际存在，第一次调用仍可能因为平台、PATH、相对目录、旧参数或前台常驻
进程而返回 `command not found`、`No such file`、invalid invocation 或 timeout。当前
共享 AgentLoop 会把多数未知 CLI 归为 `ReadOnly`；只读失败不阻断完成，模型可以直接
结束回合，而不是先确认命令是否正确。这违反“可恢复技术状态由系统持有”的产品原则。

### 目标

- 安装完成后，Skill 的受支持目录树仍然完整，相对资源有稳定 `skill_root`。
- 明确的命令解析/调用/超时失败不能直接结束原任务，也不能要求用户发送“继续”。
- 系统先执行一次有界正确性诊断，再用有证据变化的命令继续。
- 无新证据时不得原样重复失败命令；相同 Objective、root turn 和权限边界保持不变。
- 纠正命令成功且无需工作区变更时，以有界探针形成 `CurrentStateAcceptance` 并一次结算 Objective；不得由 supervisor 重复生成相同最终答复。
- 用户能看到简洁的“正在检查命令”状态和最终工具证据，不暴露内部 prompt 或敏感参数。

### 非目标

- 安装时不执行 Skill 脚本、不自动安装任意系统依赖、不信任第三方可执行文件。
- 不把所有非零退出都判成命令配置错误；例如 `grep` 无匹配仍可作为普通只读结果。
- 不绕过 shell policy、turn capability、permission、mutation receipt 或外部副作用门禁。
- 本切片不承诺修复任意 Skill 的业务逻辑，只保证命令恢复协议可达且不会静默中断。

## Requirements Traceability

| Req ID | 要求 | Surface | 最低验证 |
| --- | --- | --- | --- |
| CF-SCR-R1 | 本地目录和 Git 安装必须在受限复制策略下保留 `SKILL.md`、`scripts/`、`references/`、`assets/` 及其他普通文件；跳过 `.git`，显式拒绝符号链接和越界 payload，Git 临时目录删除后安装副本仍可用 | skill installer | temp-tree Rust unit + Git-shape unit |
| CF-SCR-R2 | 启用 Skill 时向模型提供稳定、结构化的 `skill_id` 与绝对 `skill_root`，并明确相对路径按该 root 解析；正文预算不能先截掉 provenance header | prompt assembly | enabled prompt unit |
| CF-SCR-R3 | bash/run-shell 对 `command_not_found`、`resource_not_found`、`invalid_invocation`、`command_timeout` 和 `shell_unavailable` 产生 typed、脱敏、可恢复 metadata；普通无匹配/业务非零退出不误触发 | tool runtime + shared core | classifier matrix |
| CF-SCR-R4 | typed command failure 创建 command-repair obligation，不能因工具仍属 `ReadOnly` 就接受最终回复；下一 provider round 必须要求一个工具动作 | shared AgentLoop | scripted transport sequence |
| CF-SCR-R5 | repair round 必须先做至少一项有界只读诊断：解析 skill root/脚本存在性、`command -v`/`Get-Command`、帮助/版本/README 或平台适配；没有诊断证据时禁止原样重跑同一命令 | shared AgentLoop policy | same-command negative + diagnostic positive |
| CF-SCR-R6 | 纠正尝试必须在可审计的命令名、路径、调用方式、参数或有依据的 timeout 策略上发生变化；失败批次中尚未开始的后续调用取消后由模型基于真实错误重新规划；成功的有界纠正命令必须形成 typed verification，若无需工作区变更则以 `CurrentStateAcceptance` 一次结算 Objective | AgentLoop + Objective + persistence | multi-call cancellation + no-change acceptance sequence |
| CF-SCR-R7 | timeout 必须先回收本次 shell 进程树，再诊断前台服务/真实进度/超时预算；bash 暴露 `1..=1800` 秒的有界 `timeout_sec`，只有诊断证据后的 timeout 变化才算更正，不得无依据原样重跑 | bash + AgentLoop | descendant cleanup + timeout recovery trajectory |
| CF-SCR-R8 | 普通 operational command failure 作为 `ToolResult(error)` 回送模型；`command_not_found`、`invalid_invocation` 或 `shell_unavailable` 只有在有界观察确认工作区未变化时，才把旧 receipt 原子结算为 `cancelled`；观察到变化、timeout、资源缺失或状态不可读仍保持 `unknown` 并转 system-owned remediation | ToolBackend + Objective boundary | receipt cancellation positive/negative + recoverable/fatal boundary unit |
| CF-SCR-R9 | 持久化 replay 保留 typed failure code、脱敏错误和 tool-call identity；进程恢复后不得退化为无错误语义的 placeholder 或新 Objective | trajectory + history repair | SQLite restart integration |
| CF-SCR-R10 | command recovery 显示 system-owned 状态，不生成 `retry/continue/resend` CTA；两条不同安全策略仍失败时进入有界 system incident，而非业务完成 | Objective/UX | event sequence + hydration/component |
| CF-SCR-R11 | metadata 不保存 secret、完整命令或原始 Skill 正文；可审计字段限于 failure code、command fingerprint、skill id、attempt/strategy、duration 和脱敏摘要 | privacy/observation | negative persistence assertions |
| CF-SCR-R12 | PR、CI、真实 CodeFactoryDev 成功/边界路径、正式安装包和精确版本复验前保持 `not live` | release | HLT scenario + release evidence |

## 架构设计

### 1. 安全 Skill payload

目录/Git 安装先把 Skill 根目录中的普通文件复制到用户 Skill 目录，保留相对目录和
普通权限位，再由 CodeFactory 写入规范化 `manifest.json` 和 `system_prompt.md`。
复制器使用文件数、总字节数和深度上限，跳过 `.git` 与符号链接；目标不能通过相对
路径或链接逃出 Skill 目录。JSON manifest/marketplace 的单文件安装保持兼容。

启用后的 prompt block 固定以前导头开始：`skill_id`、绝对 `skill_root`、相对路径解析
规则，随后才是 Skill 正文。这样项目 `cwd` 仍是命令工作目录，但模型必须把 Skill
自己的相对资源解析成 `skill_root` 下的绝对路径。

### 2. Typed command failure

工具层只对明确失败形状写结构化 metadata：

```text
code, command_repair_required=true, recoverable=true
```

`code` 取值为 `command_not_found | resource_not_found | invalid_invocation |
command_timeout | shell_unavailable`。metadata 不含完整命令；AgentLoop 使用当前内存中的
工具调用计算稳定 fingerprint。未知普通非零退出仍保持普通 Error，不自动套用纠错协议。

### 3. Command-repair obligation

AgentLoop 在收到 typed failure 后：

1. 持久化并显示失败工具结果；
2. 取消同一预先生成批次里尚未开始的调用；
3. 注入内部 repair prompt，并令下一 provider round `tool_choice=required`；
4. 先接受一项有界只读诊断，再接受改变后的纠正命令；
5. 成功后清除 obligation，并把有界纠正命令记录为 functional probe；若没有工作区变更，
   Completion Arbiter 以 `CurrentStateAcceptance` 完成同一 Objective，若发生了变更则仍要求
   `ChangeSet + PostChangeValidation`；新的 typed failure 更新 failure signature 和策略。

同一命令 fingerprint 在没有中间诊断证据时不得重新 dispatch。已有 completion gate 的
失败上限和 Objective remediation 上限继续充当总兜底，不能因普通模型轮次、进程重启
或相同错误文本清零。

若 repair round 的 provider 请求已被接纳但在零输出、零工具意图时发生 transport failure，
系统只能在 chat-run owner 已退休、无 checkpoint、无未结算 receipt，且历史工具副作用均有
terminal receipt 时把该 `unknown` 尝试收敛为 `failed_replayable`，然后在消耗 remediation
上限前续接同一 `active` 或 `waiting_system` Objective。存在部分输出、未结算 receipt、
活跃 owner 或其他 Objective 状态时仍保持 observe-only。

### 4. Operational 与 fatal 边界

命令不存在、路径错误、参数不兼容和 timeout 都是 operational failure，必须作为模型
可见 `ToolResult` 继续闭环。对于命令未找到、调用参数被解析器拒绝或 shell 根本未启动，
ToolBackend 还必须核对写前/写后工作区摘要：完全未变化时，把已派发 receipt 及 recovery
contract 原子结算为 `cancelled`；Objective 只允许“失败工具调用 + cancelled receipt”匹配，
成功工具仍必须提供 `committed/reconciled` receipt。只要摘要变化、不可读，或失败属于
timeout/resource-not-found，receipt 继续保持 `unknown`。数据库无法提交、mutation receipt
无法判定或 Objective identity 冲突仍为 fatal/system incident；此时禁止盲重放副作用。

### 5. 兼容、隐私与回滚

- 不新增用户设置，不改变已有 Skill enabled 状态。
- 旧 prompt-only Skill 继续可读；重新安装后才补齐原来源 payload。
- 安装目录中的未知普通文件只是静态资源，不在安装阶段执行。
- 回滚不会损坏 Skill manifest 或会话数据库，但旧版本仍缺少 command-repair obligation。
- 命令原文仍按既有工具审计边界处理；新增持久 metadata 只保存脱敏分类和 fingerprint。

## UX 设计

- 首次 typed failure 后，工具卡保留真实错误；回合状态显示“命令执行失败，正在检查正确调用方式”。
- 诊断和纠正继续出现在同一 root turn，不插入伪用户消息，不显示“请继续/请重试”。
- timeout 状态说明系统正在判断“仍在运行、前台服务还是命令配置错误”，不承诺盲目延长等待。
- 修复成功后只给用户一份最终结果和相关验证；内部 repair prompt、fingerprint 和策略计数不进入正文，supervisor 不得因“无变更”重复同一最终答复。
- 两条不同安全策略仍不能推进时，显示 system incident 的 owner、失败类别和已检查范围；composer 回到发送态，但不把技术输入责任转给用户。

## Primary User Paths

### 成功路径

用户从 Git 安装一个包含 `SKILL.md` 与 `scripts/run.py` 的 Skill，启用后发起任务。模型
根据 `skill_root` 调用安装副本中的脚本并完成原任务；Git 临时 clone 已删除。

### 自动纠错路径

Skill 首次调用旧命令或错误相对路径，返回 `command_not_found`/`resource_not_found`。
CodeFactory 保持同一 Objective，取消预生成后续调用，自动检查 Skill 根目录、PATH 或
帮助文档，然后使用变化后的正确命令继续；用户消息总数仍为 1。

### timeout 边界路径

Skill 把前台服务作为普通命令启动并触发 timeout。shell 后代被回收；系统检查命令
说明/进程与日志，改为后台启动加 bounded readiness，或在证据表明命令本身有界时使用
合理 timeout。不得原样重跑到恢复上限。

### system incident 路径

数据库/receipt 状态不可信，或两条安全命令策略均无法推进。系统结算当前 transport
turn 并保留 Objective 为 system-owned incident；不显示业务完成或人工“继续”CTA。

## Applicable Harnesses

- Spec Harness：CF-SCR-R1..R12。
- Payload Harness：Skill 目录树、脚本、资源、路径、大小/数量/深度边界、命令输出。
- Compatibility Harness：旧 prompt-only Skill、macOS/Linux/Windows shell、旧会话 replay。
- Observation Harness：typed failure、fingerprint、diagnostic/corrective attempt、duration、terminal reason。
- AI Collaboration Harness：独立架构和 QA 审查、假设、review point 与验证结果。
- Release Harness：P0 HLT、真实 Dev App、正式安装包和 updater metadata。

## 测试矩阵

| 层级 | 场景 | 断言 |
| --- | --- | --- |
| Rust installer unit | `SKILL.md + scripts + references + assets` | 安装副本结构/内容/权限存在；`.git` 跳过，symlink 显式拒绝；超限 fail closed |
| Rust prompt unit | enabled Skill | header 先于正文，含稳定 `skill_id/skill_root` 和相对解析规则 |
| Shared core unit | failure text matrix | 仅明确 command failure 得到 typed code；普通 `grep` no-match 不触发 |
| Bash unit | timeout/127/invalid option | ToolOutput Error 含脱敏 typed metadata；timeout 后代回收 |
| Timeout repair | 同一命令 300 秒失败，诊断后改为 600 秒 | fingerprint 发生变化并允许更正；仍用 300 秒则拒绝 |
| AgentLoop scripted | wrong command → diagnostic → corrected command | 下一轮 required tool；同一 root；纠正后成为有界验证并完成 |
| Objective no-change | corrected bounded probe → final | `CurrentStateAcceptance`、单一 completed revision、无重复 final message；纯文本答复仍不能完成 LocalMutation |
| AgentLoop negative | wrong command → unchanged retry | 第二次 dispatch 被拒绝；要求诊断；无副作用 |
| AgentLoop batch | failed first + queued second | 第二个调用记 cancelled，下一轮基于真实错误重新生成 |
| Provider recovery | command repair 后 provider 零输出 transport failure | terminal receipt + retired owner 允许同一 `waiting_system` Objective 续接；partial/unresolved 仍锁定 |
| SQLite receipt | invalid invocation + unchanged/changed workspace | 未变化时 receipt/contract 为 `cancelled` 且失败工具可终结；有变化时仍为 `unknown/observed_changed`；成功工具不能用 cancelled 绕过证据 |
| SQLite restart | typed error 后进程重启 | tool result/code/identity 重放；同 Objective；零新增 user message |
| CodeFactoryDev | synthetic Skill 成功与 timeout 边界 | 工具卡、恢复状态、最终结果和进程回收均符合规格 |
| Release App | 精确公开版本 | 同一 synthetic Skill 主路径通过，build SHA 与 updater metadata 一致 |

## Evidence Pack Requirements

- Skill 安装前/后相对目录清单与临时 clone 删除证据。
- typed failure、诊断、变化后命令和成功结果的脱敏 event sequence。
- SQLite 中同一 Objective/root turn、零额外用户消息、无重复 side effect。
- 定向 Rust、HLT、治理、required CI、PR/merge、release artifact 和真实 App 证据。

## 完成边界

文档、单测、本地 build、PR、CI、合并或 release workflow 成功都不是单独完成。只有精确
公开安装包在 synthetic Skill 的成功和 timeout 边界路径上完成同一 Objective、无需用户
“继续”，并验证安装资源与进程回收后，才可标记 `live_verified`。
