# M167（紧急）：「命令修复」门禁死锁——要求先诊断，却拒绝所有诊断

## 证据（v1.91.28，10-24 01:09 派 M160 → 会话 a0ad3153，01:13 ended）
- 首条 `cd "<含空格路径>" && sed …` 失败（resource_not_found）。
- 之后：`pwd; test -f …`、`command -v git/node/cargo`、`read_file src-tauri/src/agent/execution_workspace.rs`、`glob` **全部** 返回
  `Command repair denied: run one related bounded diagnostic before retrying.`（metadata: code=resource_not_found, command_repair_required=true, stage=diagnostic_required, system_owned=true）
- 会话无路可走，以「工具执行通道被运行时自愈门禁全局拦截」结束；授权交付任务零产出。
- 相关历史：rust 测试 `typed_command_failure_requires_diagnosis_then_changed_retry_in_same_turn`、`persisted_command_repair_envelope_forces_diagnosis_after_restart`（HLT-001）。
## 需求
- R1 进入 diagnostic_required 后，只读诊断（read_file/glob/grep/list、只读 bash 如 pwd/ls/test/command -v/git status|log|diff）**必须放行**并被计为「已诊断」；诊断后允许改过的重试。
- R2 门禁只能约束「同一条失败命令原样重试」，不得拦截其它工具；任何情况下不得出现所有工具都被拒的状态（加不变量断言）。
- R3 有上限：若诊断判定反复不满足，最多 2 次后自动解除门禁并记录（不能让会话死锁结束）。
- R4 测试（先失败）：复现上述序列（含空格路径的失败命令 → pwd/command -v/read_file/glob）→ 断言均放行、诊断后重试成功；跨重启（持久化 envelope）同样不死锁。
## 约束
- task-template；合成 fixture；授权交付（ceiling: merged）。

Release-Urgency: immediate

## Requirements Traceability / 测试矩阵

| Req ID | 最低证据 |
| --- | --- |
| M167-R1 | `m167_read_only_diagnostics_are_code_independent`：各类只读诊断与错误码交叉；含显式 test 的有界只读查询 |
| M167-R2 | `m167_resource_failure_and_restart_never_block_other_tools`：含空格路径、四类诊断、第三项及后续只读调用、更正命令；原样重试仍受约束 |
| M167-R3 | `m167_unsatisfied_recovery_releases_after_two_attempts`：两次未满足后自动解除且有结构化记录，重启保留计数 |
| M167-R4 | 上述测试先红后绿；现有命令恢复测试全量回归 |

## Applicable Harnesses

Spec Harness；Compatibility Harness（旧持久化 envelope 缺省计数为零）；Payload Harness（工具参数和含空格路径）；Observation Harness（自动解除结构化 metadata）；Release Harness（本次仅合并，不发版）；AI Collaboration Harness。

## Implementation Notes

复用共享命令结构分类，不按错误码/路径关键词筛选只读诊断；原权限与完成门禁不变。原样失败命令使用已有 cwd/timeout fingerprint 判断。自动解除仅解除命令恢复门禁，不解除权限、执行安全或完成证据门禁。

真实安装版验收由编排方发版后完成：在含空格工作目录触发不存在资源的命令，再依次执行 pwd、command -v、read_file、glob；核对工具结果均可执行及自动恢复审计 metadata，最后执行更正命令。此任务无 UI 变更，不做桌面点按或截图。
