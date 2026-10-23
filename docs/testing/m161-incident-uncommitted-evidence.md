# M161 交付验收

修复规格：docs/specs/feature-specs/m161-incident-uncommitted.md。R1–R4：身份检查后持久保全 tracked/staged/unstaged/untracked 到 rescue ref，再恢复登记分支；冲突保留引用并回到可继续的登记分支；不改写远端历史。

## Req ↔ 测试
- R1/R2/R4：reattach_never_discards_uncommitted_work_on_a_switched_branch，先复现原始 uncommitted changes 错误，后转绿；断言 rescue 未跟踪父提交内容。
- R1/R2/R4：m161_conflict_preserves_snapshot_and_session_can_continue 通过；真实 Git 冲突、救援内容、登记 HEAD、远端历史、无 unmerged 索引、会话消息位置和重新 attach。扩展用例在实现后加入，不冒称 red-first。
- R3/R4：m161_startup_heals_seven_incidents_once_with_index_and_files_intact 通过；七个合成 incident 批次恢复，暂存与未暂存内容分别保留，重复启动幂等。未访问生产数据库。
- R4：既有 external_path 测试覆盖 Windows verbatim/UNC，Git 入口统一归一化路径；Windows 原生执行由 CI 验证。

## Harness / AI Collaboration
Spec：任务原文入库。Compatibility/Payload：合成 Git/SQLite、登记身份守卫、checkout --no-overwrite-ignore。Observation：execution_workspace_rescues 持久快照与结果、rescue Git ref、tracing、assistant 会话说明。Release：只合并，不发版，not live。
context scope：execution_workspace 故障及合成夹具；assumptions：救援引用不自动推送；review point：独立只读 Git 数据安全审查；validation result：以最终验证结果为准。

## 验证
pnpm test：158 files / 1053 tests passed（含 lightModeAudit）；pnpm build 成功。最终 Rust `cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast` 退出码 0：主库 1971 passed / 12 ignored，其余 workspace 和集成测试通过；M161 两项扩展验收通过。模块并行回归中一次既有 m114b_user_waiting_deliveries_stop_polling_concluded_prs 时钟累计断言 saw 3，原断言不变，隔离重跑及最终全量均通过。统一 local scenario harness、治理基线、scenario --ci 与 README 合约通过。

独立只读审查已尝试通过 dispatch 发起，但该入口返回需任务面板启动，未获得已执行审查结果；未把它计作通过的独立验收。

## Windows CI 夹具修复
Windows check-rust 两项 M161 测试失败于实际 CRLF 与预期 LF 的逐字节比较（registered / unstaged），不是保全引用丢失。合成根仓库显式设置 core.autocrlf=false，保留所有内容断言；不改变真实用户仓库配置。使用隔离 GIT_CONFIG_GLOBAL=...（core.autocrlf=true）复验继承隔离，两项 M161 用例均通过；随后最终 workspace 全量退出码 0。

## §8 验收分工与收尾
真机验收由编排方发版后完成：安装新版本，切换登记工作区分支并制造 tracked/untracked 改动，重启核对会话继续和内容；冲突核对中文说明、rescue 位置、持久审计、远端历史。无 UI 布局变更，不需要截图。
合并后执行 merged=true/远端 main 核对、精确远端分支清理、本地状态确认、专用构建缓存及本任务后台任务清理。不删除应用管理的目录。
Spec Amendment：无需求语义调整；任务“七类”按证据中七个同类 incident 实例验证。

Scenario-Test: RTE-004
README-Update: reviewed
README-Update-Reason: 修复既有工作区自愈行为，不改变公开安装、平台或隐私承诺。
