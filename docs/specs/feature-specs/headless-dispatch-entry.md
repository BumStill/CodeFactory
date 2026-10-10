# 本地非界面派单入口（锁屏也能派单）

### Background and user decision
The user expects CodeFactory to keep working unattended for long stretches (e.g. overnight) while an orchestrator (Claude/Codex) keeps dispatching tasks to it. Dispatching must not depend on the screen being unlocked, and must not weaken any existing safety boundary.

### Decision
Provide a **local-only** task entry point that the running CodeFactory accepts and that works without the GUI. Tasks submitted through it go exactly the same way as ones typed in the GUI (same session/objective/permission/delivery/completion gates); there's no bypass channel.

### Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| CF-HDE-R1 | Through this entry point, the local user can: create a session in a named project and send the first message; send a message to a named existing session; set that session's permission mode (standard/trusted); query a session's state (objective state, latest reply summary, PR number). Works when the screen is locked or the window is on another display | Integration tests + one real-machine record: submitting a task with the screen locked succeeds |
| CF-HDE-R6 | **Everything an orchestrator needs can be done from the background** (user's default requirement, 2026-10-10): through this entry point you can also — pick the model when creating a session and change an existing session's model / set the default model; switch the session currently shown in the GUI (M27: menu switching doesn't work); stop the current run; list and approve/deny pending approval requests (approving must still go through the same permission rules: high-risk ones are only approved after an explicit choice, no blanket approval); move the main window back to the main display (M42). None of these depend on a pop-up menu, coordinate clicks or the screen being unlocked | Integration test for each capability + one real-machine record with the screen locked |
| CF-HDE-R2 | **Local only, no remote attack surface**: no network port is opened; only the current OS user can submit (enforced via file permissions or equivalent); requests from other users / other machines are rejected | Counter-example tests: other users / world-writable path / any network access → rejected |
| CF-HDE-R3 | **Doesn't weaken anything**: messages submitted this way go through exactly the same admission as GUI input (wording-inferred permissions, delivery authorization, permission prompts, completion gate); delivery authorization must be stated explicitly in the request (a structured field, not inferred from the wording, see M48); high-risk operations still need confirmation | Tests: same input via GUI and via this entry point produces identical admission results |
| CF-HDE-R4 | Every submission is traceable: who (local entry point) / when / which session / the original text, written to the audit log; the session shows "this message came from the local task entry point" | Test + plain-language wording |
| CF-HDE-R5 | Usage docs: one command or one example file can submit a task; the docs state the security boundary | Docs + a runnable example |

### Applicable Harnesses
Spec Harness; Compatibility Harness; Observation Harness (submission audit); AI Collaboration Harness. **Touches a security boundary**: the PR must have a dedicated "Threat model" section (who can call it, what they can do, how it's limited), and counter-example tests are the focus of acceptance.

### Test matrix
- Normal: screen locked → create session + send + set trusted + query state.
- Counter-examples: another user, a file with wrong permissions, a request with no delivery authorization trying to deliver, a non-existent project/session.
- Consistency: GUI vs this entry point, admission results identical.

## Implementation Notes

These notes add technical constraints only. They do not change any requirement,
acceptance criterion or minimum evidence above.

### Transport
A **Unix domain socket** at `<app data dir>/dispatch.sock`, created with mode
`0600` inside a `0700` directory. No TCP/UDP listener is ever opened. On Windows
the equivalent private named pipe is not implemented in this change — see the
leftover section of the PR.

### Protocol
One JSON object per line, one response line per connection. `operation` is the
discriminator; the schema is closed (`deny_unknown_fields`). `delivery_authorized`
is a **required** boolean with no default, so an unstated delivery request does
not parse at all.

Operations: `create_and_send`, `send`, `set_permission`, `status`, `set_model`,
`switch_session`, `stop`, `list_approvals`, `resolve_approval`,
`focus_main_display`.

### Who executes what
Transport and security live in Rust (`headless_dispatch`). Behaviour lives where
the GUI's behaviour already lives: the app forwards each request to the interface
over one event and waits for the reply, so the entry point cannot drift from the
GUI code path. `focus_main_display` is handled natively in Rust (window
placement) because it has no interface-side equivalent.

### Audit
Every **parsed** request is appended to `<app data dir>/dispatch-audit.log`
(JSON lines, mode `0600`) before it is executed: timestamp, source, operation,
session, project and the original text.

### Entry-point routing
`src-tauri/src/main.rs` is the E2E-001 trust root (a `DRIVER_FILES` member in
`tools/governance/scenario_case_execution.py`): a normal PR must keep it
byte-identical, and it must keep dispatching to the canonical unattended module
first. The Rust client mode (`CodeFactory --dispatch '<json>'`), therefore, is
**not** a new branch in `main.rs`; it is resolved at the top of the library app
entry `lib.rs::run()` — the one place the frozen `main` falls through to — before
any Tauri/GUI work. The socket client lives in `headless_dispatch::run_client_cli`.
The documented, orchestrator-facing command remains `node scripts/dispatch-task.mjs`,
which speaks the same protocol directly to the private socket.

## 第二阶段（U34b，2026-10-10 真机实测缺口）

第一阶段让锁屏派单成为可能；真机长时间运行又暴露了四处"入口回报与事实不符"的
缺口。本节把第二阶段的需求固化下来，实现只覆盖这里的 Req ID。

### 问题与证据（v1.83.2 真机，全程锁屏）
- 锁屏下 `create_and_send`、`status`、`list_approvals`、`switch_session`、`stop`
  全部可用 —— 第一阶段目标已达成。
- **M56（送达撒谎 + 作用对象错，最严重）**：给空闲会话 5bb60a3a 发 `send`，入口
  返回 ok，但消息始终没有进入会话；`switch_session` 之后仍然没有；直到 `stop`
  （objective 变 failed）后再发，才在 6 分钟后送达。
  - **真因**：对已有会话的 `send` / `stop` 不认 session_id，作用在界面当前显示的
    会话上。14:30 发给 5bb60a3a 的修复指令（带 `delivery_authorized=true`）落进了
    当时显示的 be9674c5；14:42 发给 5bb60a3a 的 `stop` 停掉了当时显示的 40704077；
    14:42 那条 `send` 进了队列，14:43:41 被冲进 40704077。跨会话投递且带交付授权，
    属于安全边界缺陷。
- **M55（能力缺口）**：`status` 的 `latest_reply` 恒为 null；文档写着
  "`set_model` 省略 session_id 即设置默认模型"，实现在没有目标会话时报
  "no session is open to change the model on"；运行中发的 `send` 只会排队，
  没有"插话引导当前执行"的选项，与界面 Enter / ⌘Enter 的语义不一致。
- **M53（状态不可信）**：会话已经在 DeepSeek 上连续调用，objective 状态和顶部提示
  仍停留在"等待自动重试"。

### Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| CF-HDE-R0 | **作用对象必须正确（最高优先）**：所有带 session_id 的操作（send / stop / set_permission / set_model / status / resolve_approval）只作用于该 session_id 指定的会话，与界面当前显示哪个会话无关；不存在或无法定位时返回 not_found，绝不退回「当前会话」。待发队列必须按会话隔离，绝不能把 A 的消息冲进 B | 集成测试：界面显示 B 时对 A 执行 send / stop / set_permission / set_model，断言只有 A 收到或停止、B 毫无变化；复现 14:30 与 14:42 两条时序 |
| CF-HDE-R12 | **待审批全量可见**：`list_approvals` 列出所有会话里所有待审批的请求（含 bash 等工具权限），带会话 id、工具名、参数摘要、过期时间；`resolve_approval` 仍然一次只处理一条，并且走同一套权限规则 | 集成测试：多个会话各有待审批时全部列出，批准或拒绝只影响指定那一条 |
| CF-HDE-R7 | **送达如实回报**：`send` 的应答必须明确是「已送达并开始处理」还是「已排队及排在什么之后」。发给空闲会话时，一定立即开始新的一轮，不能因为界面残留的旧回合状态而挂起；挂起超过有界时间要回报失败，不能回报 ok | 集成测试：空闲会话、残留回合状态、运行中三种情况各自的应答与实际落库一致 |
| CF-HDE-R8 | **插话与排队可选**：`send` 支持「插话引导当前执行」和「本轮结束后再发」两种方式（默认值自定并说明理由），与界面上 Enter / ⌘Enter 的语义一致 | 测试：两种方式行为与界面一致 |
| CF-HDE-R9 | **状态可信且够用**：`status` 返回的 objective_state 与真实执行情况一致（例如在模型调用就不能显示等待重试）；latest_reply 给出最近一条回复的摘要；pr_number 取自交付记录；另外返回最近一次模型调用的上下文大小（token 数），供编排方判断是回原会话追加还是另开新会话 | 测试：断言各字段与库内事实一致 |
| CF-HDE-R10 | **默认模型可设**：`set_model` 省略 session_id 时，设置新会话的默认模型（端点与模型保持配套，不能出现「端点是 deepseek、默认模型是 gpt」这类错配），与文档一致 | 测试 + 文档 |
| CF-HDE-R11 | **界面状态同源**（M53）：顶部状态提示与会话列表的转圈，和 R9 用同一个真实来源，不再停在过期的「等待自动重试」 | 组件测试 + 真浏览器截图（light/dark） |

### Applicable Harnesses
Spec Harness；Compatibility Harness；Observation Harness；Viewport Harness（R11）；AI Collaboration Harness。
本任务碰安全边界：沿用第一阶段的威胁模型（仅本机当前 OS 用户的 `0600` Unix socket、
不推断交付授权、闭 schema），新增字段（`mode`、`delivery`）不放宽任何关卡。

### 测试矩阵
- R0：界面显示 B，对 A 执行 send / stop / set_permission / set_model；对不存在的会话执行同样操作。
- `send`：空闲会话、残留旧回合状态的会话（复现 M56）、运行中会话 × 插话 / 排队两种方式。
- `status`：运行中、等待、已结束、开过 PR 的会话。
- `set_model`：带 session_id 和不带 session_id 两种。

### Implementation Notes（新增技术约束，不改变上面任何需求或最低证据）

- **单一真实状态源**：新增纯函数模块 `src/lib/sessionTurnState.ts`，把「这一轮到底
  是不是在跑 / objective 到底是什么 / 最近一条回复是什么」从权威证据（服务器
  `session.is_running`、消息里的 root turn 归属与 turnActivity、权限等待）里算出来。
  `status`（R9）、顶部提示与侧栏转圈（R11）、以及 `send` 的送达判定（R7）都读它，
  所以三处不可能各说各话。
- **`send` 的 `mode`**：可选字段，取值 `steer`（插话引导当前执行，等价界面 Enter）与
  `queue`（本轮结束后再发，等价界面 ⌘/Ctrl+Enter）。**默认 `steer`**：它是界面里
  不加修饰键时按 Enter 的语义，也是"我说话就是要它现在就听"的默认期待；空闲会话下
  `steer` 与 `queue` 都表现为"立即开始新的一轮"。
- **`send` 的应答**：`result.delivery` 是 `{ status: "delivered" | "queued", ... }`。
  `delivered` 附 `into: "new_turn" | "current_run"`；`queued` 附 `queue_position` 与
  `ahead`（排在什么之后）。无法在有界时间内真正开始一轮时返回结构化失败
  `delivery_failed`，绝不回 ok。
- **残留回合状态**：`chat` store 的 `sendMessage` 只把"未释放的进行中回合"当作阻塞；
  一个已经结算（objective 终止）的旧 `streaming` 残留不再吞掉新消息（M56 根因）。
- **`set_model` 省略 session_id**：写入新会话默认（`activeModel` + 草稿模型），并在
  能确定端点时保持端点与模型配套；无法配套时 fail-closed 报错，不做静默错配。
