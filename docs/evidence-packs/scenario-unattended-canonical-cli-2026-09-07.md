# E2E-001 canonical CLI 前置改造证据

## 范围

对应 CF-STG-R31。正式 binary 最先进入独立 unattended CLI 模块，避免其他可变 smoke wrapper 在 driver 之前截获命令；CLI 直接挂载 driver，driver 直接挂载 observation 与 process-tree 源码。`lib.rs` 保留首个无条件模块声明，产品的其余模块不冻结。

本 PR 不修改 registry、trusted judge、workflow、ruleset 或发布配置，不提升 E2E-001 完整状态；只是后续 Bootstrap-1a 可保护入口的普通前置。没有 UI 行为变化，因此不以本次 headless 证据冒充真实 WebView 验收。

## 失败优先与验证

- 独立实现角色先增加正式 main 的分发顺序验收以及 CLI 契约；旧实现的首分支/模块契约出现 3 个失败，改造后 Node 契约 7/7 通过。
- `src-tauri/tests/unattended_cli_entry.rs` 用 Rust 编译实际 `main.rs` 和记录型 facade，执行 unattended parent/worker、history、delivery、未知 flag 五条路径；检查真实执行顺序，而不是只在文本中查字符串。
- 主执行者重新运行 `pnpm test:unattended-contract` 等价 Node 入口，7/7 通过。
- `pnpm exec tsc --noEmit` 通过。仓库简介中的 `pnpm typecheck` 已不是有效脚本，使用 CI 实际入口。
- 主执行者从本 worktree 运行 `pnpm cargo:shared -- run --bin codefactory -- --unattended-long-task-smoke <临时回执>`，真实正式 binary 成功。观察值：`supervisor_hard_kill_issued=true`、`worker_reaped=true`、`replacement_process_distinct=true`、`user_message_count=1`、`human_prompt_count=0`、`side_effect_receipt_count=1`、`replay_call_link_count=2`、`objective_status=completed`、`descendant_process_count=0`、`leaked_resource_count=0`、cleanup 两个动作和 `cleanup_ok` 均为 true。
- `node scripts/verify-unattended-failure-receipt.mjs <正式 binary>` 通过：exit code 1，诊断 `unattended_smoke_failed`，`cleanup_ok=false`；本机 Unix 无效临时根场景保守记 leak 1。证明失败不能被退出成功掩盖，也不能丢失匿名失败回执。

这次本地开发 binary 的编译时 SHA 为 `unknown`，仅作为行为回归证据；新的可信 PR adapter 将要求传入 HEAD 并复核编译时 SHA。Windows Job Object 与服务端 exact-head 行为由该普通 PR 的 required CI 继续验证。未取得 CI 与 merge 证据前不称默认分支完成。

## AI Collaboration

- context scope：合成 smoke、专用 CLI、main 分发顺序与共享 Cargo 缓存；不读取生产会话或凭据。
- assumptions：只调整入口归属和顺序，不改变 parent/worker、SQLite、幂等副作用或清理语义。
- review point：主执行者审查独立实现的 diff，确认入口首分支和私有 path 委托；另一个 QA 指出后续必须保护 Cargo config，作为 Bootstrap-1a 范围处理。
- validation result：本地契约、真实成功/失败 binary 路径与类型检查通过；本 PR 自身不改变 judge。
