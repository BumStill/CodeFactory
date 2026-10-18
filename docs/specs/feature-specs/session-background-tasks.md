# CF-BG 会话级通用后台任务能力（后台命令 + 定时唤醒）

- 状态：阶段 1 已实现（R1 / R3 / R4 / R7 核心）；阶段 2（R2 / R5）与阶段 3（R6）待实现
- 任务来源：`task-cf-bg-background-tasks.md`（用户 2026-10-18 决策）

## 背景与业务目的

用户需要一个**通用**的后台任务能力，让 agent 自己当编排方：提交、CI、合并、长构建、长测试、线上状态巡检这类"要等"的流程，agent 自己挂后台命令、自己被唤醒、自己继续下一步，而不是每件事都由外部编排方（Claude Code 的 `Bash run_in_background` + 定时心跳）代劳。

现状痛点：会话交付到"开 PR"之后，等 CI、转述失败、打 `merge-queue: armed`、合并后发版装机，全靠外部编排方人工介入；仅 2026-10-17/18 两天就介入十余次，2.0 门禁①（连续 10 单零介入）因此长期停在 0–3。产品内逐情况补"什么情况唤醒会话"（M104/M108）治标不治本——问题不是缺某一种唤醒源，而是缺一个通用能力。

## 用户决策（2026-10-18）

1. 后台能力必须是**通用工具**，不是只盯交付的固定流程；什么时候开、开什么命令、设什么唤醒由 agent 根据当前情况自主判断。
2. 用户可以用自然语言直接要求（"盯着这个 PR，合并了告诉我"、"每小时检查一次线上状态"、"这个测试放后台跑，跑完叫我"），agent 据此创建任务，并在回复里告诉用户建了什么、怎么停。
3. 两种来源（`agent_decided` / `user_requested`）走同一套权限，任务记录里标明来源，界面可见可停。
4. 会话被用户停止/结案时，回收其全部后台任务与定时器。
5. 结束回合前若仍有外部进行中的事项（CI、合并队列、发版），必须有后台任务或定时唤醒兜底，否则完成门禁不放行；该规则只约束 agent 下一步，不写进对话。

## 决策

### D1 工具面（会话级、按 task_id 管理）

| 工具 | 作用 |
| --- | --- |
| `background_task_start` | 在**本任务受管工作区**启动 shell 命令，立即返回 `task_id`，不阻塞当前回合 |
| `background_task_list` | 列出**本会话**后台任务（状态、退出码、命令、创建/结束时间） |
| `background_task_output` | 读取某任务输出尾部（有长度上限） |
| `background_task_stop` | 按 `task_id` 结束任务，杀**整个进程树** |

`background_task_start` 接受 `command`、可选 `description`、可选 `timeout_sec`、`source`（`agent_decided` | `user_requested`，默认 `agent_decided`）。返回体只含 `task_id`、pid、输出文件路径和一行使用提示——回合不等待命令结束。

### D2 唤醒入队（与用户消息同一条入队机制）

命令结束（成功 / 失败 / 超时 / 被停止）后，系统把一条**内部唤醒输入**入队到**原会话**，走与用户消息完全相同的入队与调度路径（因此会按会话串行、会被停止栅栏与完成门禁看到），但：

- 不写成 `user` 角色的聊天消息，不进入用户消息流的视觉呈现（阶段 3 的界面标记也遵循这一点）；
- 唤醒文本固定形状：`后台任务 <id> 已结束` + 退出码（或"超时" / "被停止"）+ 输出尾部（受长度上限截断）+ 输出文件路径；
- 同一任务的唤醒只发一次（`wake_state = pending | enqueued`），重启对账不会重复补发；
- 会话正在跑回合时，唤醒排在该回合之后（同一条入队通道，不抢占当前回合）。

### D3 数据模型（SQLite，`background_task`）

| 字段 | 说明 |
| --- | --- |
| `id` | 任务 id（`bg-<uuid>` 短前缀），主键 |
| `session_id` | 所属会话；列表/停止/回收都按它过滤 |
| `root_turn_id` | 创建它的回合（审计用，不参与回收判定） |
| `command` / `cwd` | 原样命令与**绝对** cwd（受管工作区） |
| `source` | `agent_decided` / `user_requested` |
| `status` | `running` / `exited` / `failed` / `timed_out` / `stopped` / `lost` |
| `exit_code` | 退出码（`null` 表示未知） |
| `pid` / `process_started_at` | 进程 id 与**进程启动时间**；对账时两者同时匹配才认为"仍是同一个进程"（防 pid 复用） |
| `output_path` | stdout+stderr 合并落盘文件，位于应用数据目录内的 `background-tasks/` |
| `created_at` / `finished_at` | 创建/结束时间（Unix 秒） |
| `timeout_sec` | 生效的单任务超时 |
| `wake_state` | `pending` / `enqueued` |

行的写入口径：启动时插入 `running` 行（先落库再 spawn，避免"进程已起但无记录"）→ 结束时由监视任务原子更新状态 + 退出码 + `finished_at`。数据库为应用现有 SQLite 库（迁移只在 `migrations/` 追加），测试 fixture 用临时库，不写生产 DB。

### D4 重启对账

应用启动时对账所有非终态行，三分类：

| 分类 | 判定 | 处理 |
| --- | --- | --- |
| running | `pid` 存在且进程启动时间与记录一致 | 继续监视：重新挂上等待，结束照常写状态并发唤醒 |
| finished | 进程不存在（或启动时间不匹配且进程已不在） | 读输出文件补写终态，`wake_state = pending` 时补发一次唤醒（含输出尾部） |
| lost | 无法确认（无 pid、无启动时间、pid 复用但无输出可判定、输出文件缺失） | 标记 `lost`，唤醒会话说明"无法确认该后台任务的结果"，并附已读到的输出（可能为空） |

对账是**幂等**的：重复执行只做"未完成→终态"的单向转换，`wake_state` 已是 `enqueued` 的行不再补发。

### D5 进程树回收

- 每个任务在独立**进程组**里启动（Unix：`setsid`/进程组；Windows：`Job Object`/`taskkill /T`），因此停止与超时都能杀整棵树，不留孤儿；
- 停止 = 先 `SIGTERM` 给进程组 → 有界等待 → 仍在则 `SIGKILL`；
- 超时 = 同一套回收路径，终态记 `timed_out`；
- 会话被停止/结案 → 回收该会话全部 `running` 任务（终态记 `stopped`），不再发唤醒（会话已结案）。

### D6 上限（超限明确报错，不静默排队）

- 每会话并发 `running` ≤ 3；
- 全局并发 `running` ≤ 8；
- 单任务最长时长 3 小时（`timeout_sec` 上限即 10800）；
- 超限返回可读错误句（哪个上限、当前占用、可执行的下一步），不创建行、不 spawn。

### D7 安全（与前台 bash 完全一致，不放宽）

- 后台命令复用前台 `bash` 的**命令分类与拒绝清单**（`tools::shell_policy`）：被前台拒绝的命令在后台同样被拒，且拒绝发生在落库/spawn 之前；
- 审批/信任模式由同一 permission gateway 决定，后台不新增任何权限类别、不跳过审批；
- 只在本机运行（不引入远程执行路径）；
- 输出只写应用数据目录内 `background-tasks/`，不落到工作区外任意路径，也不写入工作区（避免污染 diff）。

### D8 与 objective / 完成门禁的关系

- 后台任务属于**会话**，不属于 objective 断点；objective 交付状态机不因后台任务存在而前进或回退；
- 唤醒输入进入会话入队后，按普通回合处理：仍受停止栅栏、完成门禁、交付授权约束；唤醒本身不构成交付授权，也不能替代 CI 证据；
- "结束回合前必须有唤醒源"只在**阶段 2**（R5）接进完成门禁；阶段 1 只提供能力，不改门禁判定，避免半成品门禁放行/拦截错误。

## Requirements Traceability（阶段 1）

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| R1 | `background_task_start` 立即返回 task_id 不阻塞；结束/超时/失败后以"任务已结束 + 退出码 + 输出尾部"唤醒原会话 | 集成测试：启动后立即返回、监视任务写终态、唤醒回调收到的文本含退出码与输出尾部；快速命令与慢命令两种 |
| R1-持久化 | 任务记录落 SQLite（会话、命令、cwd、pid+进程启动时间、状态、退出码、输出路径、创建/结束时间） | 读取库行断言字段齐全；进程内重启（重新打开库）后行仍在 |
| R1-对账 | 重启后三分类：运行中继续监视 / 已结束补发唤醒 / 无法确认标记 lost 并唤醒 | 三个独立测试，分别构造 running、finished、lost 行，断言终态与唤醒条数 |
| R3-管理 | 列出、读取输出尾部、停止（杀进程树） | 测试：list 只见本会话；output 截断到上限；stop 后子进程（含孙进程）全部退出且终态 `stopped` |
| R3-回收 | 会话停止/结案回收其全部任务 | 测试：两个任务的会话被回收后无 `running` 行、无存活进程 |
| R3-上限 | 每会话 ≤3、全局 ≤8、单任务 ≤3h，超限明确报错 | 测试：第 4 个会话任务与第 9 个全局任务被拒且错误句可读；`timeout_sec` 超 10800 被拒 |
| R4 | 与前台 bash 同一套分类/审批，后台不新增权限 | 测试：前台被拒命令在 `background_task_start` 同样被拒，且未创建行；后台分类函数就是前台那一个（同一单元被两侧调用） |
| R7 | 先失败后通过；端到端闭环 | PR 内记录每个 Req 的首个失败运行与转绿运行 |

## Applicable Harnesses

- **Spec Harness**：本文件为权威规格；实现只做 R1/R3/R4/R7（阶段 1），R2/R5/R6 明确留给后续 PR。
- **Payload Harness**：输出落盘与尾部截断（长度上限、多字节字符边界不切坏 UTF-8）、命令文本与 cwd 落库。
- **Compatibility Harness**：新增表/列只在 `migrations/` 追加；旧库（无此表）启动时迁移后可用；不改变既有 `bash` 行为与返回结构。
- **Observation Harness**：启动对账、上限拒绝、停止回收都要有可读的日志/事件，便于事后追溯（阶段 3 再把这些接到界面）。
- **Viewport Harness**：阶段 1 不改界面（界面标记属阶段 3 R6），无需浏览器截图；将在阶段 3 PR 内补。
- **AI Collaboration Harness**：context scope、assumptions、review point、validation result 见 PR body。

## Implementation Notes：阶段 1 Windows 续做（2026-10-18）

取代 #724，范围仍为 R1/R3/R4/R7，不实现阶段 2/3。

- 原 Windows CI job 117571544611：进程树测试在 PowerShell 中执行 POSIX `sleep 60 & echo $!`，未产生 child.pid；对账测试启动 `/bin/sh`，Windows 返回 OS error 3。两者均为真实失败，不能用 cfg 排除测试。
- 平台 fixture 必须使用与生产相同的 shell_invocation；Windows 用 Start-Process 派生真实子进程并以 ASCII 写 pid，Unix 对路径做 shell 引号转义，覆盖含空格与单引号的路径。
- Windows 继续使用 `taskkill /PID <pid> /T /F` 回收整棵进程树；测试必须断言父子进程实际退出，而非仅检查命令成功。
- Windows 进程身份改用 OpenProcess + GetProcessTimes 获取创建 FILETIME，避免每次额外启动 PowerShell、区域格式和快速退出竞态；探活用进程句柄查询退出状态，不解析本地化 tasklist。不能取得身份时仍 fail closed 为 lost。
- 对账 fixture 使用真实运行过且 wait 回收的本机 shell，不用虚构不存在的固定 PID。
- 真机验收由编排方发版安装后完成：后台命令完成唤醒、停止父子进程、应用重启三分类与审计记录；会话内以真实子进程集成测试与 Windows CI 提供证据。

## 测试矩阵（阶段 1）

| 场景 | 类型 | 断言 |
| --- | --- | --- |
| 快速命令后台运行 | 集成 | 返回 `task_id`；终态 `exited` + `exit_code=0`；唤醒文本含退出码与输出尾 |
| 失败命令 | 集成 | 终态 `failed` + 非零退出码；唤醒文本标明失败 |
| 长命令超时 | 集成 | 终态 `timed_out`；进程组无存活成员 |
| 启动即返回 | 集成 | `background_task_start` 的耗时远小于命令时长（不阻塞回合） |
| 重启对账 running | 集成 | 重新对账后仍 `running`，结束后正常发唤醒 |
| 重启对账 finished | 集成 | 终态补齐 + 补发一条唤醒（含输出尾） |
| 重启对账 lost | 集成 | 终态 `lost` + 唤醒说明无法确认 |
| 唤醒幂等 | 集成 | 再次对账不重复发唤醒 |
| list 隔离 | 集成 | 会话 A 看不到会话 B 的任务 |
| output 截断 | 单元 | 超过上限时只保留尾部且不切坏多字节字符 |
| stop 进程树 | 集成 | 父+孙进程均退出；终态 `stopped` |
| 会话回收 | 集成 | 会话全部任务终态化，无存活进程 |
| 每会话上限 | 单元/集成 | 第 4 个被拒，错误句含上限与当前值 |
| 全局上限 | 单元/集成 | 第 9 个被拒 |
| 时长上限 | 单元 | `timeout_sec > 10800` 被拒 |
| 被拒命令 | 单元/集成 | 与前台同一拒绝理由；未创建行 |
| SQLite 字段完整性 | 集成 | 行内 session/command/cwd/pid/进程启动时间/状态/退出码/输出路径/时间戳齐全 |

## 分阶段计划（每阶段单独 PR）

1. **阶段 1（本 PR）**：规格 + R1（后台命令、持久化、唤醒入队、重启对账）+ R3（管理、回收、上限）+ R4（权限一致）+ R7（测试先失败）。
2. **阶段 2**：R2（`schedule_wakeup` 定时唤醒，含下限/上限与取消）+ R5（模板与系统提示里的使用规矩、结束回合前的唤醒源门禁）。
3. **阶段 3**：R6（会话内"N 个后台任务进行中"标记，可展开、可停止）。

## Implementation Notes（技术约束，不改变任何需求）

- 唤醒入队走与用户消息同一个通道：后台监视器只在**终态写入成功后**调用注入的 `WakeSink`，由调度层负责入队，因此监视任务不直接持有会话锁；
- 为便于 headless 测试，核心逻辑不依赖 Tauri `AppHandle`：进程启动、监视、对账、上限、回收都在纯 Rust 模块内，数据库用 `sqlx::SqlitePool`，测试用临时目录与临时库；
- 进程启动时间在 Unix 上取进程统计里的 starttime（与 pid 合并判定），Windows 取创建时间；取不到时记为 `null` 并在对账时落到 `lost` 分支（不猜 `running`）。

---

# 阶段 2：定时唤醒（R2）+ 使用规矩（R5 / R5b）

- 状态：阶段 2 规格；实现于本 PR
- 任务来源：`task-cf-bg-stage2.md`（总规格 `task-cf-bg-background-tasks.md` 的 R2 / R5 / R5b）
- 前置：阶段 1 已由 #728 合并并在 v1.88.25 上线（R1 / R3 / R4）。本阶段在阶段 1 的实现（`src-tauri/src/agent/background_tasks.rs`、`src-tauri/src/tools/background_task.rs`）上扩展，不另起一套。

## 背景

阶段 1 给了 agent "开一个后台命令、结束被唤醒"的能力，但**只有命令结束才会唤醒**。真实长流程里还有一大类等待没有可结束的进程：CI 排队、合并队列按序跑、发版轮询、线上状态巡检。这类等待需要一个**定时唤醒**：N 秒后、或每 N 秒，用一句话把本会话叫醒，让它自己看一次状态再决定下一步。同时，agent 侧需要一条使用规矩：交付开 PR 后自己挂 CI 监视，结束回合前不能把"外部还在进行中的事项"留在那里没人管。

## 需求（本阶段）

### R2 定时唤醒工具

| 工具 | 作用 |
| --- | --- |
| `schedule_wakeup` | 在 N 秒后（一次性）或每 N 秒（周期性）以一句提示唤醒本会话，返回 `wakeup_id` |
| `wakeup_list` | 列出本会话的定时唤醒（提示、下次到点时间、周期、状态） |
| `wakeup_cancel` | 按 `wakeup_id` 取消，取消后不再触发 |

- 参数：`prompt`（唤醒时注入的那句话）、`after_sec`（一次性）**或** `every_sec`（周期性），二选一必填；`source`（`agent_decided` | `user_requested`，默认 `agent_decided`）。
- 间隔下限 60 秒、上限 6 小时（21600 秒）；越界**明确报错**（说明下限/上限与合法示例），不创建记录。
- 每会话最多 8 条待触发唤醒，超限明确报错并给出可执行下一步（先取消一条）。
- 唤醒文本固定形状：`定时唤醒 <id>：<prompt>` + 周期信息（一次性 / 每 N 秒）；周期性唤醒带"第 n 次"。
- **持久化**：记录落 SQLite；应用重启后按原计划继续——到点时间在应用关闭期间已过的，启动后立即补一次并在文本中说明"计划时间已过，启动后补发"，然后按周期继续。
- **回收**：会话被用户停止/结案时，取消该会话全部待触发唤醒，与阶段 1 的后台任务回收同一入口。

### R5 使用规矩（系统提示 + 任务模板）

1. 交付开 PR 后，用后台任务监视 CI（`gh pr checks <n> --watch`，或间隔 ≥180 秒的轮询脚本；注意 GitHub API 额度，见 M114），被唤醒后：失败 → 修复并推送；只差信任根自修改 → 明确"等编排方放行"；全绿 → 打 `merge-queue: armed`。
2. 结束回合前若仍有外部进行中的事项（CI、合并队列、发版），必须已经挂了后台任务或定时唤醒；否则不放行。这条只约束 agent 的下一步，**不写进对话、不展示内部词汇**。
3. 规矩落在两处并且必须同时存在：agent 系统提示（交互 / 执行 / 自主三种模式）与仓库内任务模板文档（`docs/agent-task-template.md`，编排方同步 `~/.codefactory/agent-tools/task-template.md`）。

### R5b 谁来决定开后台任务

- `agent_decided`：agent 自己判断什么时候开后台任务 / 定时唤醒（等 CI、等长构建、等部署、定期检查状态、长时间测试放后台跑同时继续别的事），不限于交付流程。
- `user_requested`：用户用自然语言要求（"盯着这个 PR，合并了告诉我"、"每小时检查一次线上状态"、"这个测试放后台跑，跑完叫我"）时，agent 创建对应任务/唤醒，并在回复里说明建了什么、怎么停（`background_task_stop` / `wakeup_cancel`）。
- 两种来源走同一套权限与审批，任务记录里标明来源，界面可见可停（界面属阶段 3 R6）。

## 决策

### D9 数据模型（SQLite，`wakeup_schedules`）

| 字段 | 说明 |
| --- | --- |
| `id` | `wk-<uuid 简写>`，主键 |
| `session_id` | 所属会话；列表/取消/回收都按它过滤 |
| `root_turn_id` | 创建它的回合（审计用） |
| `prompt` | 唤醒时注入的那句话 |
| `source` | `agent_decided` / `user_requested` |
| `after_sec` | 一次性唤醒的延迟（一次性时为非空） |
| `every_sec` | 周期（周期性时为非空，两者互斥） |
| `next_at` | 下次到点的 Unix 秒（周期唤醒每次触发后推进 `every_sec`） |
| `status` | `pending` / `done` / `cancelled` |
| `fire_count` | 已触发次数 |
| `created_at` / `last_fired_at` | 创建 / 最近触发时间 |

一次性唤醒触发后转 `done`；周期性唤醒保持 `pending` 并推进 `next_at`；取消转 `cancelled`。表只在 `ensure_schema` 里 `CREATE TABLE IF NOT EXISTS` 追加，旧库启动自动补齐（Compatibility）。

### D10 触发与投递

- 调度是一个每秒执行一次的纯 Rust 函数 `drain_due_wakeups_at(pool, sink, now)`：取 `status='pending' AND next_at<=now` 的行，逐条投递到与用户消息相同的会话入队通道（复用阶段 1 的 `WakeSink`）。
- 投递失败（会话正在跑回合）保持 `pending`，下一轮重试，不丢；投递成功才推进 `next_at` / `fire_count`，因此重启不会重复触发同一次。
- 周期性唤醒的"晚到"语义：应用关闭期间错过多次，只补**一次**并把 `next_at` 推到 `now + every_sec`（不在启动瞬间灌一堆唤醒）。

### D11 与完成门禁的关系（R5 第 2 条）

- 完成门禁只约束 agent 的下一步：当会话有外部进行中的事项时，要求 `wake_source_cover(session)` 至少有 1 个后台任务在跑或 1 条待触发唤醒；
- 缺唤醒源时给出可执行的一句话（"给这个等待挂一个 `schedule_wakeup` 或 `background_task_start`"），不写进对话、不展示内部词汇；
- 该判断是纯查询，不新建任何状态机，也不改变交付授权、CI 证据链或 objective 状态推进。

## Requirements Traceability（阶段 2）

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| R2-触发 | 到点触发一次，唤醒文本含提示与 id | 集成：到点后 `drain_due_wakeups_at` 投递 1 条，未到点投递 0 条 |
| R2-周期 | `every_sec` 在每次触发后推进 `next_at`，可再次触发 | 集成：连续两次推进时间点各投递 1 条，`fire_count=2` |
| R2-取消 | `wakeup_cancel` 后不再触发 | 集成：取消后到点投递 0 条，`status=cancelled` |
| R2-重启继续 | 重启后按原计划继续，错过的只补一次 | 集成：模拟重启（重新打开库）后到点行仍触发；关闭期间错过 3 次只投递 1 条且 `next_at=now+every_sec` |
| R2-上下限 | <60s / >6h 明确报错，且不建记录 | 单元：59 与 21601 被拒，错误句含边界；60 与 21600 通过 |
| R2-回收 | 会话停止/结案取消其全部唤醒 | 集成：回收后该会话无 `pending` 行，投递 0 条 |
| R5-规矩 | 系统提示与任务模板都含交付后 CI 监视、结束回合前的唤醒源规矩 | 单元：三种系统提示都含规矩锚点；`docs/agent-task-template.md` 含同一规矩 |
| R5-门禁 | 缺唤醒源时门禁给出可执行提示，有则放行 | 单元：`wake_source_cover` 在有/无任务与唤醒时分别返回 covered / missing |
| R5b-来源 | 两种来源都落库并在列表可见 | 集成：`user_requested` 与 `agent_decided` 各建一条，列表显示来源 |

## Applicable Harnesses（阶段 2）

- **Spec Harness**：本文件为权威规格；实现只做 R2 / R5 / R5b，R6 留给阶段 3。
- **Payload Harness**：唤醒提示文本落库、长度上限（4 000 字符）与唤醒文本固定的头部形状。
- **Compatibility Harness**：新表只 `CREATE TABLE IF NOT EXISTS` 追加；旧库启动自动补齐；阶段 1 的 `background_task_*` 行为与返回结构不变。
- **Observation Harness**：触发、取消、回收、晚到补发都写 `tracing` 事件，便于事后追溯。
- **AI Collaboration Harness**：context scope / assumptions / review point / validation result 见 PR body。
- **Viewport Harness**：本阶段不改界面（"N 个后台任务"标记属阶段 3），无需浏览器截图。

## 测试矩阵（阶段 2）

| 场景 | 类型 | 断言 |
| --- | --- | --- |
| 到点触发 | 集成 | 投递 1 条，文本含 `定时唤醒 <id>` 与提示原文 |
| 未到点不触发 | 集成 | `now < next_at` 时投递 0 条 |
| 周期推进 | 集成 | 两次推进各 1 条，`fire_count=2`，`next_at` 递增 `every_sec` |
| 取消 | 集成 | 取消后投递 0 条，`status=cancelled`，重复取消幂等 |
| 重启继续 | 集成 | 重新打开库后待触发行仍在并能触发 |
| 晚到只补一次 | 集成 | 错过 3 个周期只投递 1 条，`next_at≈now+every_sec` |
| 上下限 | 单元 | 59 / 21601 被拒；60 / 21600 通过 |
| 每会话上限 | 单元/集成 | 第 9 条被拒，错误句含上限与下一步 |
| 会话回收 | 集成 | 回收后无 `pending` 行、投递 0 条 |
| 来源标注 | 集成 | `user_requested` / `agent_decided` 各自落库且列表可见 |
| 系统提示规矩 | 单元 | 三种提示都含交付后监视 CI 与结束回合前唤醒源锚点 |
| 任务模板规矩 | 单元 | `docs/agent-task-template.md` 含同一规矩 |
| 唤醒源门禁 | 单元 | 有任务/唤醒 → covered；两者皆无 → missing + 可执行提示 |
