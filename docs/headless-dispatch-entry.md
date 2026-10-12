# 本地非界面派单入口（锁屏也能派单）

CodeFactory 运行时会开一个**仅本机、仅当前用户**的本地通道，编排方（Claude /
Codex / 你自己的脚本）可以用它派单，**不需要窗口在前台，也不需要解锁屏幕**。

- 规格与验收标准：`docs/specs/feature-specs/headless-dispatch-entry.md`
- 传输与安全边界实现：`src-tauri/src/headless_dispatch.rs`
- 界面侧接线（复用界面同一条代码路径）：`src/lib/desktopDispatch.ts`

## 为什么需要它

macOS 锁屏后，窗口级的无障碍通道（菜单项、合成点击、键盘事件）全部失效：
系统会直接拒绝，报 `... actions need the screen unlocked`。菜单栏（M7）因此
只覆盖"屏幕还亮着"的场景；夜里无人值守时，派单请求会一直卡到有人解锁。
这个入口走的是文件系统上的 socket，不经过窗口服务器，所以锁屏、窗口在副屏、
窗口被最小化都不影响。

## 安全边界（先读这一节）

- **仅本机**：只有 Unix domain socket，从不打开任何 TCP/UDP 端口。没有可以被
  别的机器连上的地址。
- **仅当前用户**：socket 是 `0600`，放在 `0700` 的应用数据目录里；服务端还会
  用内核给出的对端 uid（macOS `LOCAL_PEERCRED` / Linux `SO_PEERCRED`）复核。
  换一个用户、全局可写目录、被别人替换过的 socket、伪装成 socket 的普通文件
  一律拒绝。
- **不放宽任何关卡**：请求交给界面已经在用的那条代码路径执行——建会话、发消息、
  权限模式、审批、停止、模型都走界面按钮同一个函数。**交付授权只能来自结构化
  字段**（`delivery_authorized`），绝不从消息措辞推断；字段缺失的请求连协议都
  过不了。审批一次只处理一条，不会批量放行。
- **可追溯**：每个通过协议校验的请求都会先写进
  `<应用数据目录>/dispatch-audit.log`（JSON 行，`0600`）：时间、来源、操作、
  会话、项目、原始文本。

## 一个命令派单

```bash
# 在已命名项目里新建会话并发送第一条消息（交付授权必须显式写出）
node scripts/dispatch-task.mjs '{
  "operation": "create_and_send",
  "project": "/Users/you/Projects/CodeFactory",
  "message": "修复 #590 的登录超时",
  "model": "deepseek/v4-pro",
  "permission_mode": "trusted",
  "delivery_authorized": true
}'
```

也可以直接用应用自带的客户端（同一个协议、同一套校验）：

```bash
CodeFactory --dispatch '{"operation":"status","session_id":"<会话 id>"}'
echo '{"operation":"stop"}' | CodeFactory --dispatch -
```

请求也可以从 stdin 读（`node scripts/dispatch-task.mjs -`），方便编排脚本拼接。

## 支持的操作

| operation | 作用 | 关键字段 |
| --- | --- | --- |
| `create_and_send` | 在指定项目里新建会话并发送第一条消息 | `project`, `message`, `delivery_authorized`；可选 `model`, `permission_mode` |
| `send` | 给已存在的会话发消息 | `session_id`, `message`, `delivery_authorized` |
| `set_permission` | 设置权限模式 | `session_id`, `permission_mode`（`safe`/`standard`/`trusted`） |
| `status` | 查询会话状态 | `session_id` |
| `set_model` | 改会话模型，或设置默认模型（省略 `session_id`） | `model`；可选 `session_id` |
| `switch_session` | 切换界面当前显示的会话 | `session_id` |
| `stop` | 停止当前执行 | 可选 `session_id` |
| `list_approvals` | 列出待审批请求 | — |
| `resolve_approval` | 批准/拒绝一条待审批请求 | `approval_id`, `approve` |
| `get_quota` | 查看所有订阅端点当前用量、上限、来源与重置时间 | — |
| `set_quota_cap` | 修改一个订阅端点的用量上限，复用设置页保存路径 | `endpoint`；至少一个 `five_hour_percent` / `weekly_percent`（1–100 整数） |
| `focus_main_display` | 把主窗口挪回主屏（M42） | — |

## 后台订阅配额（CF-QUOTA-R2）

```bash
node scripts/dispatch-task.mjs '{"operation":"get_quota"}'
node scripts/dispatch-task.mjs '{"operation":"set_quota_cap","endpoint":"chatgpt","five_hour_percent":70,"weekly_percent":75}'
# 只改每周窗口，五小时窗口保持现值（旧配置缺值时默认 80%）
node scripts/dispatch-task.mjs '{"operation":"set_quota_cap","endpoint":"chatgpt","weekly_percent":60}'
```

`endpoint` 是设置中的端点键，不是显示名称或模型 ID；只能修改订阅（`chatgpt`）
端点。未找到端点返回 `not_found`；按量端点、空更新、非整数、null、越界或未知字段
返回 `invalid_request`，不保存。两个操作都返回订阅状态列表：`endpoint`、`label`、
`source`、`five_hour_percent` / `weekly_percent`（已用百分比）、
`five_hour_cap_percent` / `weekly_cap_percent`（上限）、
`five_hour_resets_at_ms` / `weekly_resets_at_ms`（epoch 毫秒或 null）、`over_cap`。
没有订阅端点返回空列表；没有用量记录时保留原有“尚无用量记录”标记，不冒充服务端读数。
不返回设置全文、凭据或原始服务端响应。修改复用设置页 `setQuotaCap → settings.save → save_settings`，
保存成功后重新读取 `subscription_quota_status`；原生审计记录目标端点和修改值。
应用必须运行，但无需前台或解锁；这些操作不启动任务，也不需要 `delivery_authorized`。

## 应答

一行 JSON：成功是 `{"ok":true,"result":{...}}`，失败是
`{"ok":false,"error":{"code":"...","message":"..."}}`。`code` 取值稳定：
`invalid_request`（协议/字段不合法）、`denied`（安全边界拒绝）、`not_found`
（项目或会话不存在）、`internal`（应用侧执行失败）。

## 排查

- `CodeFactory is not accepting local tasks (...)`：应用没在运行（或启动时没能
  绑定 socket，日志里会有 `local task entry unavailable`）。
- 通道由应用启动时创建，退出时随进程消失；不需要也不应该单独启动守护进程。
