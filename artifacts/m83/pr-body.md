## M83 修复

App 常驻唯一派单接收端；新建消息复用按 session_id 寻址的发送服务，不依赖 composer；switch_session 等待选择成功再回工作区。后端等待最多 30 秒，返回 frontend_timeout，清理 pending 并丢弃迟到回复。

Scenario-Test: HLT-003, HLT-004, HLT-005, UI-001, UI-002, UI-003, UI-004, UI-005, UI-006, UI-013, UI-014
README-Update: reviewed
README-Update-Reason: 安装、安全与公开功能承诺不变；超时细节和验收分工记录在派单规格。

## Requirements 与自验收

规格：docs/specs/feature-specs/dispatch-bridge-always-on.md

| Req | 验收 | 状态与证据 |
| --- | --- | --- |
| CF-M83-R1 | 设置页 get_quota / create_and_send；显式切会话 | PASS：App.dispatch-lifetime.test.tsx；真实 Chromium light/dark 生产 App + store + handlers，quota 字段断言、send_message 目标 session_id 与内容断言、发送次数恰为 1、返回 delivered、关于页保持、switch_session 返回工作区 |
| CF-M83-R2 | 前端沉默有界应答、迟到回复、结果保真 | PASS：headless_dispatch::bridge::timeout_tests 两项 Rust 测试；计时与 frontend_timeout 字段断言；正常结果和错误不变 |
| CF-M83-R3 | 测试先红后绿 | PASS：App listener 未定义失败；Rust wait_for_interface 不存在编译失败；相同命令重跑通过 |
| CF-M83-R4 | 安装版设置页派单与审计、截图 | 按用户批准分工移交：真机验收由编排方发版后完成，本 PR 不声明已在安装版验证 |

## 测试证据

- pnpm test：147 文件、984 测试全通过，包含 lightModeAudit。
- pnpm build：TypeScript 与 Vite 生产构建通过。
- CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-m83" cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast：全工作区通过，主库 1587 通过、9 ignored。
- node scripts/verify-dispatch-bridge-headless.mjs：真实 Chromium light/dark 验收通过；合成 IPC，不冒充原生 socket。
- 截图：artifacts/m83/headless/settings-light.png、artifacts/m83/headless/settings-dark.png；字段回执：artifacts/m83/headless/receipt.json。
- 原配额/审批结构测试仅更正迁移后的文件定位，保留权限与配额断言；未改工作流、场景注册表、校验器。

## Harnesses

Spec：编号与测试矩阵；Compatibility：同源 store/action 和既有全量回归；Observation：错误码、迟到应答和字段回执；Viewport：真浏览器 light/dark 截图；Release：仅交付至合并，发版另行授权；AI Collaboration：下述记录。

## Threat model

仍为仅本机当前 OS 用户的 0600 Unix socket，闭 schema，审批一次一条，交付授权必须来自结构化字段。不新增网络端口、不放宽权限。超时不撤销可能已启动的操作；错误提示查询 status 后再重试，避免重复派单。

## 真机验收交接

**真机验收由编排方发版后完成。**
1. 安装包含此 PR 的版本并记录 build identity，使用临时合成项目、信任模式和配置中的 DeepSeek v4 Flash 精确模型 ID，停在设置 → 关于。
2. 运行 `node scripts/dispatch-task.mjs '{"operation":"get_quota"}'`，断言 ok=true、配额与设置同源（无订阅允许空数组），关于页保持。
3. 运行 `node scripts/dispatch-task.mjs '{"operation":"create_and_send","project":"<合成项目绝对路径>","message":"只回复 M83 合成验收成功，不改文件，不交付","permission_mode":"trusted","delivery_authorized":false}'`（预先设置 Flash 默认模型或附配置中的精确 model ID）。断言 session_id 非空、delivery.status=delivered，消息只落入返回的会话一次，关于页保持。
4. 核对应用数据目录 dispatch-audit.log 对应的两条 operation/source/project/message/delivery_authorized 与请求一致，保存合成条目摘录与截图，不复制凭据或真实会话内容。
5. switch_session 到返回会话，确认切回工作区并显示目标消息；status 核对真实处理状态。
6. 隔离实例模拟前端无响应，确认 30 秒内 frontend_timeout 且客户端退出；恢复前端后查询成功。超时不代表动作撤销，重试前核对状态。

## AI Collaboration

context scope：M83、task-template、App/Workspace、dispatchActions、headless_dispatch 与 headless 夹具。assumptions：业务服务不依赖页面生命周期（已由真实 App/store 浏览器验收）；review point：唯一监听器、目标寻址、有界等待、pending 清理；validation result：上述本地与 CI 证据。按模板使用单执行流，未启用子 agent。

Spec Amendment：无待确认变更；验收分工为用户本轮明确批准，已记录到版本化规格。
本次不发版，安装版 not live；无额外产品范围扩展。
