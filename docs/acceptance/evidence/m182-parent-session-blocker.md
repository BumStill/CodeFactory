# M182 候选应用独立阻断：新会话缺少 parent_session_id

## 复现范围

2026-10-29，M182 分支先合入最新 `origin/main`（含 v1.91.51 修复），重新编译候选应用后复现。此问题不是 M182 Git 危险清单的通过证据，也未在本 PR 修改数据库绕过。

## 可复现步骤

1. `pnpm build` 后以 `TAURI_CONFIG='{"identifier":"com.codefactory.m182.acceptance","build":{"devUrl":null}}' CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-m182" CARGO_INCREMENTAL=0 cargo build --manifest-path src-tauri/Cargo.toml --bin codefactory` 构建。
2. 在任务专属临时目录准备独立 HOME、0700 dispatch 目录和本地 synthetic Git 仓库，初始提交只含合成内容。
3. 设置 `HOME=<任务临时目录>/home`、`CODEFACTORY_DISPATCH_DIR=<任务临时目录>/dispatch` 启动候选二进制；不读取、复制或修改生产配置/凭据。
4. 用 `CODEFACTORY_DISPATCH_DIR=... node scripts/dispatch-task.mjs '<JSON>'` 发送：

```json
{"operation":"create_and_send","project":"<合成 Git 仓库绝对路径>","message":"仅执行 git status --short；合成验收，不推送远端。","permission_mode":"trusted","delivery_authorized":true}
```

5. 实际返回：

```json
{"ok":false,"error":{"code":"internal","message":"Database error: no column found for name: parent_session_id"}}
```

6. 真实 dispatch `{"operation":"list_approvals"}` 返回 `{"ok":true,"result":[]}`，使用 Python JSON 断言检验。此空列表不代表 R5 已执行。
7. 停止任务自己启动的候选进程树，保留日志。原始日志：`/tmp/cf-m182-acceptance.l7vyIl/application-resume.log`。

## 处理边界与用户批准的替代

用户在本轮明确批准：若此错误仍复现，独立记录阻断后以合成 fixture 的等价证据完成 R6，并在 PR 如实标注。

采用 `permission_gateway.rs::m159_gateway_session_identity_backup_and_remote_fence`：SQLite trusted + delivery_authorized 会话、真实登记/身份校验、生产 `authorize`、真实本地 bare remote 和 Git shell 命令。六条 R5 集合逐条实际执行，断言退出状态、每次执行前新增 backup ref、文件/提交/工作区删除效果、快照内容和 pending_permissions 为空。第六条登记第二 Objective/工作区，并验证删除后脏内容可从快照读回。不是模型会话端到端成功声明，也不是以 mock Allow 替代生产授权。

独立阻断建议后续修复新数据库 schema 与 `Session` FromRow 一致性；本 PR 不擅自扩大数据库修改范围。真实安装版仍由编排方发版安装后验收；本任务仅交付到合并。
