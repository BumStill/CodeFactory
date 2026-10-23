# M159 交付与自验记录

## 需求映射
- R1/R4：`m159_r1_four_incident_shapes_and_backup_restore` 四条合成真实 Git 命令，先失败后通过；执行后备份仍保留 HEAD 父链、暂存、未暂存、未跟踪内容。
- R2：`m159_r2_escapes_remote_unknown_syntax_require_approval` 先失败后通过，越界、符号链接、force push、远端删除、未知语法不自动放行。
- R2 集成：`m159_gateway_session_identity_backup_and_remote_fence` 验证 SQLite 工作区身份及交付授权不能绕过危险操作。
- R3：`m159_standard_reset_still_requests_approval` 标准模式仍审批。
- R1 卡片：`ToolCallCard.workspace.test.tsx` 先失败后通过，参数不能伪造后端 scope。

## Applicable Harnesses
Spec：原始需求已版本化。Compatibility：标准模式、无需 schema 迁移。Payload：白名单和独立 index 备份。Observation：结果 metadata 的 workspace_scope/backup_ref。Viewport：permission-mode headless 真浏览器，light/dark、1366/390 宽度，截图位于临时产物目录 codefactory-permission-mode-headless/m159-{light,dark}-{1366,390}.png。Release：仅合并，不发版，not live。

## AI Collaboration
Context scope：permission_gateway、trusted_git、tool_backend、ToolCallCard。
Assumptions：仅本会话当前 Objective 的有效工作区；未知语法保留审批。
Review point：危险 Git 不复用 remote grant；备份失败不执行。
Validation result：独立真实 Git fixture、网关集成与标准模式回归通过；最终 pnpm test、pnpm build、permission-mode headless 全部通过；统一 scenario harness local gate 通过。最终 Rust 工作区全量测试通过；no_window_regression 与普通 refspec push 两个原失败项单独重跑通过，再由完整 workspace 测试确认。
按任务模板单执行流，未使用 delegate_tasks。

## 真机验收与后台入口
命令工具和权限网关即无 GUI 入口。真机验收由编排方发版后完成：四条命令无审批且卡片有标注；backup ref 可恢复工作；跨工作区及 force push 仍审批；标准模式不变。

Spec Amendment：无。

README-Update: reviewed
README-Update-Reason: 修复既有工作区权限边界，不改变公开安装、平台或隐私承诺。
Scenario-Test: UI-009, HLT-001, HLT-005, SKL-002, SKL-003, UI-013
