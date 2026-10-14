# M83：本地派单接收端常驻与有界应答

## 背景与用户决策

v1.88.1 真机在设置 → 关于页面时，get_quota / create_and_send 已写入审计却无法应答。用户授权按任务模板实施，交付止于合并队列合并，不在本任务发版。

## 已批准任务原文

1. 派单前端接收端挂到应用最外层（App 级别或独立 provider），不随页面切换而卸载。需要工作区状态的操作（例如 switch_session）在接收端里自行切回工作区页面，或直接调用不依赖页面的服务。
2. 后端等待前端回复必须有超时（例如 30 秒）。超时就返回明确的错误码和原因，例如「前端未响应」；客户端进程不能无限挂起。
3. 先写失败测试：模拟「当前停在设置页时发来 create_and_send / get_quota」，断言请求得到正确处理；再模拟「前端完全无响应」，断言请求在超时后返回错误。之后再修。
4. 在真实应用里验证：停在设置页时用入口派单成功，并附上审计日志和截图。

约束：不改 `.github/` 工作流、场景注册表与校验器；只用合成数据。

## Requirements Traceability

本节仅为上述四条编号，不改变要求。

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| CF-M83-R1 | App 级唯一接收端，所有页面有效，switch_session 切回工作区 | App 页面切换测试；真实设置页派单 |
| CF-M83-R2 | 前端无响应有界失败，清理请求并拒绝迟到回复 | Rust 无响应计时测试与错误字段断言 |
| CF-M83-R3 | 测试先红后绿 | App listener 缺失失败；Rust 等待函数缺失编译失败；原命令重跑通过 |
| CF-M83-R4 | 设置页真实应用派单、审计与截图 | 隔离实例验收记录 |

## Applicable Harnesses

Spec Harness；Compatibility Harness（沿用闭协议及同源会话服务）；Observation Harness（审计、错误字段）；Release Harness（本次只合并，不发布）；AI Collaboration Harness；Viewport Harness（设置页实地截图）。

## 测试矩阵

- 设置页 get_quota，断言结果字段且页面不切换。
- 设置页 create_and_send，不依赖已卸载 composer；按新会话 ID 发送，不能静默跳过。
- switch_session，先等待 selectSession 成功，再切回工作区。
- 前端沉默，返回 frontend_timeout；迟到回复发送失败。
- 正常应答/执行失败，结果和错误保持不变。
- 全量前端、Rust 工作区、生产构建及治理门禁。

## Implementation Notes

- 新增 `useAppDispatchBridge` 作为 App 常驻调用，移除 Workspace 注册。
- create_and_send 复用按 session_id 寻址的 dispatchSend 与权限更新服务；只在真正送达后回成功。
- 后端默认上限 30 秒，环境覆盖仅可缩短，0/无效值回默认；超时返回 frontend_timeout，并提醒操作可能已经开始，重试前先查状态。超时不等于撤销任务。
- 不增加监听端口，不放宽 UID、权限、审批或结构化交付授权。

## 验收分工补充（用户已批准，2026-10-14）

真机验收由编排方发版后完成。本 PR 使用 headless 验收页（真实 Chromium、生产 App/store/handlers、合成 IPC）与集成测试作为证据，不把 mock IPC 描述为原生 socket 或安装版验收。R4 的最终安装版验收随发版由编排方执行，不阻断本 PR 合并。

编排方核对步骤：
1. 安装包含本 PR 的发布版，记录版本与 build identity；使用临时合成项目、信任模式和 DeepSeek v4 Flash，停在设置 → 关于。
2. 运行 `node scripts/dispatch-task.mjs '{"operation":"get_quota"}'`，断言单行应答 `ok=true`、配额字段与设置同源，页面保持关于；无订阅端点时允许空数组。
3. 运行 `node scripts/dispatch-task.mjs '{"operation":"create_and_send","project":"<合成项目绝对路径>","message":"只回复 M83 合成验收成功，不改文件，不交付","model":"deepseek/v4-flash","permission_mode":"trusted","delivery_authorized":false}'`；断言返回非空 session_id、delivery.status=delivered，消息只出现在该会话一次，页面保持关于。
4. 核对应用数据目录 `dispatch-audit.log`：上述两条 operation、source、project、message、delivery_authorized 与请求一致；保存合成条目的日志摘录及关于页截图，不复制凭据或真实会话内容。
5. 发送 switch_session 到返回的 session_id，确认切回工作区并显示目标消息；查询 status 确认实际处理状态。
6. 在隔离验收实例中模拟前端无响应，确认 30 秒内返回 frontend_timeout，客户端退出；错误不表示已开始任务被取消，重试前查询状态。恢复前端后再次查询成功。

## AI Collaboration

- context scope：M83、task-template、App/Workspace、dispatchActions、headless_dispatch、既有派单测试。
- assumptions：同源 store 服务不需要工作区组件存在；待真实桌面验证。
- review point：唯一事件接收端、session_id 寻址、超时 pending 清理、迟到应答。
- validation result：在随 PR 的验收记录中据实记录。
- 未启动子 agent：任务模板明确禁止 delegate_tasks，本次保持单执行流。
