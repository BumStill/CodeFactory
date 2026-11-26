# M291：会话运行中排队的消息在 app 重启后丢失

## 现场（2026-11-26，只读核实）
- 21:59 编排方通过 dispatch `send(mode=queue)` 给运行中的会话 6e140abf 发 R6 修改要求，回执 `status: queued, ahead: current_run`。22:12 app 因安装 v1.128.0 重启。此后该会话从未收到这条消息：`messages` 表无此内容，`queued_dispatch_messages` 也无记录；会话一直以为工作已完成，挂轮询等编排方 5 小时。
- 代码：`src/lib/dispatchActions.ts` 约 275–290 行，运行中会话走 `useChatStore.getState().enqueueMessage(...)`，消息只进前端内存 `runtime.queue`；持久化表 `queued_dispatch_messages`（`src-tauri/src/commands/queued_delivery.rs`）没有覆盖这条路径。界面里用户在运行中输入的排队消息很可能同样受影响（请核实）。
- 历史同族：排队消息延迟投递（M287 规格追加现场：会话空闲但队列消息不触发新回合）。

## 规格（放到 `docs/specs/feature-specs/m291-durable-queued-messages.md`）
- R1：凡是回执为 `queued` 的消息（dispatch 与界面输入两条入口），在回执返回前必须已持久化（复用 `queued_dispatch_messages` 或同等表），带会话、内容、客户端消息 id、创建时间。
- R2：app 重启后，未投递的排队消息按原顺序投递：会话空闲时立即开新回合，运行中则排在当前回合之后；同一消息不得投递两次（客户端消息 id 幂等）。
- R3：会话空闲（等待唤醒、等待外部、后台任务中）而队列非空时，不得一直等待——必须在一个监督周期内投递（与 M287 活性 owner 对齐：待投递消息本身是推进者）。
- R4：投递失败（会话已结束、被删除）时消息不静默丢弃：记录明确状态，并在 dispatch `list`/界面可见。

## 测试（先看到失败）
- 真实入口：运行中会话 dispatch 一条 queue 消息 → 模拟 app 重启（重建 store/重新初始化 DB 连接）→ 断言消息被投递一次且只一次。
- 空闲会话有队列消息 → 一个监督周期内开新回合。
- 界面输入入口同样覆盖。反例：重复投递被幂等拒绝；会话已结束时状态可见。

## 约束
改文件只用 edit_file / write_file。不改信任根；场景缺口只追加 `docs/testing/scenario-mappings.json`。不碰凭据、settings.json、其他会话 PR，不动任何 merge-queue: hold 标签。规格文件在工作区外，用 bash cat 只读查看。

## 交付
PR 正文 `R | 是否完成 | 证据` 列 R1–R4；`Release-Urgency: immediate`（编排指令在升级重启时静默丢失，导致会话长时间空转）。

Release-Urgency: immediate
