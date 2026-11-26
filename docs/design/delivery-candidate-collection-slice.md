# 交付归属候选收集切片与场景登记提案

## 当前切片 / R2-COLLECT
仅新增独立 host 协议模块，不接入 Rust 生产派发。候选始终未准入；内存冻结值不是可信磁盘封存或来源认证。冲突计划不包含 apply，不能修改 workspace、index 或签发 applied。已有正常 shell/hooks 产品路径保持不变。

先红：缺模块导入失败，collector-red.log。新单元/真实 POSIX 子进程验收由 tests/test_delivery_candidate_collector.py 执行。Python/Node/C 正例只使用临时合成目录，无真实用户资料。不能替代生产 handler、Docker 控制端独占、全部逃逸后代、GUI 或 Windows 证明。

## 提案（尚未登记）
窄 ID `DLV-COL-001`，Req R2-COLLECT：change_patterns 精确为 tools/delivery_candidate/collector.py、tests/test_delivery_candidate_collector.py；目标 `pnpm:test:delivery-candidate`，package.json 已有真实命令。建议受支持的 macos-14 runner 执行 POSIX 路径（本轮 Mac 未跑）。Linux 本地运行另留回执。证据为真实进程+纯协议单元，不宣称 L4 E2E；pull_request hard gate，不能用 Windows skipped 当通过。

原14文件需按职责登记，以下是待独立治理审核的精确路径分组，不是已注册或完整覆盖声明。每组需逐个列实际测试函数，不用目录通配：

- `src-tauri/migrations/0027_workspace_content_provenance.sql` → DLV-OWN-001 / rust:r2_content_epoch_aba_invalidates_inflight_but_preserves_completed_history; rust:r2_content_stop_and_replacement_run_cannot_settle_old_dispatch; rust:r2_content_previous_schema_migrates_and_reopens_without_adopting_history
- `src-tauri/src/agent/content_provenance.rs` → DLV-OWN-001 / rust:r2_content_epoch_aba_invalidates_inflight_but_preserves_completed_history; rust:r2_content_stop_and_replacement_run_cannot_settle_old_dispatch; rust:r2_content_previous_schema_migrates_and_reopens_without_adopting_history
- `src-tauri/src/agent/content_provenance_tests.rs` → DLV-OWN-001 / rust:r2_content_epoch_aba_invalidates_inflight_but_preserves_completed_history; rust:r2_content_stop_and_replacement_run_cannot_settle_old_dispatch; rust:r2_content_previous_schema_migrates_and_reopens_without_adopting_history
- `src-tauri/src/agent/delivery_hook_commit.rs` → DLV-OWN-003 / rust:r2_scope_native_host_receipts_deliver_exact_tree_to_local_origin; rust:r2_scope_valid_host_receipts_do_not_cover_foreign_staged_content; rust:r2_scope_sqlite_reopen_rederives_proof_and_rejects_stale_branch
- `src-tauri/src/agent/delivery_legacy_backend_fixture.rs` → DLV-OWN-003 / rust:r2_scope_native_host_receipts_deliver_exact_tree_to_local_origin; rust:r2_scope_valid_host_receipts_do_not_cover_foreign_staged_content; rust:r2_scope_sqlite_reopen_rederives_proof_and_rejects_stale_branch
- `src-tauri/src/agent/delivery_legacy_fixture.rs` → DLV-OWN-003 / rust:r2_scope_native_host_receipts_deliver_exact_tree_to_local_origin; rust:r2_scope_valid_host_receipts_do_not_cover_foreign_staged_content; rust:r2_scope_sqlite_reopen_rederives_proof_and_rejects_stale_branch
- `src-tauri/src/agent/delivery_scope.rs` → DLV-OWN-003 / rust:r2_scope_native_host_receipts_deliver_exact_tree_to_local_origin; rust:r2_scope_valid_host_receipts_do_not_cover_foreign_staged_content; rust:r2_scope_sqlite_reopen_rederives_proof_and_rejects_stale_branch
- `src-tauri/src/agent/delivery_scope_acceptance.rs` → DLV-OWN-003 / rust:r2_scope_native_host_receipts_deliver_exact_tree_to_local_origin; rust:r2_scope_valid_host_receipts_do_not_cover_foreign_staged_content; rust:r2_scope_sqlite_reopen_rederives_proof_and_rejects_stale_branch
- `src-tauri/src/agent/delivery_scope_native_fixture.rs` → DLV-OWN-003 / rust:r2_scope_native_host_receipts_deliver_exact_tree_to_local_origin; rust:r2_scope_valid_host_receipts_do_not_cover_foreign_staged_content; rust:r2_scope_sqlite_reopen_rederives_proof_and_rejects_stale_branch
- `src-tauri/src/agent/shell_intent.rs` → DLV-OWN-002 / rust:r2_shell_intent_matches_real_posix_shell_bytes; rust:r2_shell_intent_never_partially_proves_dynamic_or_unknown_tail
- `src-tauri/src/agent/shell_intent_recovery_tests.rs` → DLV-OWN-002 / rust:r2_shell_recovery_unchanged_files_never_redispatch; rust:r2_shell_recovery_expected_files_do_not_claim_whole_command_done
- `src-tauri/src/tools/delivery_hook_commit_tests.rs` → DLV-OWN-003 / rust:r2_scope_native_host_receipts_deliver_exact_tree_to_local_origin; rust:r2_scope_valid_host_receipts_do_not_cover_foreign_staged_content; rust:r2_scope_sqlite_reopen_rederives_proof_and_rejects_stale_branch
- `src-tauri/src/tools/delivery_thin_commit.rs` → DLV-OWN-003 / rust:r2_scope_native_host_receipts_deliver_exact_tree_to_local_origin; rust:r2_scope_valid_host_receipts_do_not_cover_foreign_staged_content; rust:r2_scope_sqlite_reopen_rederives_proof_and_rejects_stale_branch
- `src-tauri/src/tools/delivery_thin_stop.rs` → DLV-OWN-003 / rust:r2_scope_native_host_receipts_deliver_exact_tree_to_local_origin; rust:r2_scope_valid_host_receipts_do_not_cover_foreign_staged_content; rust:r2_scope_sqlite_reopen_rederives_proof_and_rejects_stale_branch

这些最小目标可执行，但还不足以独立覆盖所有 hooks/Stop/恢复与兼容分支；正式登记前须将既有435项回执中的对应精确用例补入审核批次，不以本表自证全面覆盖。真实生产链只到隔离本地 bare origin，不含真实远端/UI/provider。原14路径诊断另保留，不把新Python模块误称已被该诊断覆盖。

## 治理与权限
核实 origin=BumStill/CodeFactory；main/HEAD=989c8da9。当前无对应新登记。统一 gate 原始 local-head-gate.log 为 OK，但 `_changed_files` 固定 base...HEAD，未覆盖未提交候选。不得作为当前tree认证。现有工具无 tree 输入；不制造 bypass commit 来骗取检查。

已读 docs/evidence-packs/scenario-runner-bootstrap-2026-09-07.md 的真实先例：独立治理 PR，其余 required checks 通过、外部执行者审定固定head/main和规则快照后，曾短暂移除scenario-gate-pr再合并并在finally恢复，读回verify+canary。该历史授权不适用于本轮。当前 registry 是 TRUST_ROOT_FILES，不存在本候选自改自证的合规方式。

最小待维护者操作：独立审核固定代码与目标批次、批准可信 registry 登记；若仍需该bootstrap窗口，另行批准精确规则20222077/required context操作、风险窗口、恢复快照及canary。此轮没有远端/保护规则授权，不执行上述动作。先完整登记存在的targets，不能提前登记不存在目标；恢复后用可信main的runner验证精确候选。当前提交/远端正式gate仍待该流程，保留本地tree与测试。

## 独立角色
开发collector_slice，安全复核collector_review；主协调只接入测试命令和记录门禁。Spec、Compatibility、Payload、AI Collaboration适用。全局Windows绝对路径skill不可读，未声称完全继承Mac harness。PPT与安全/PM工作树未修改。

## 最终本轮回执
10/10 Python测试通过，独立QA同样10/10；py_compile与diff检查通过。修复独立审查发现的父子/大小写路径冲突、模式静默转换、可伪造候选构造、快照预算和wait/reap后killpg的PID复用时序。使用waitid WNOWAIT保留leader直到killpg；既测超时路径也测成功EOF后的延迟子进程，观察超过延迟窗口。仍不证明主动setsid逃逸后代结束。

正式pnpm入口本机失败：运行测试前触发依赖自动安装，ENOENT mkdir /home/agent/.local/share/pnpm。未安装或改HOME/安全配置。直接命令 `python3 -m unittest discover -s tests -p test_delivery_candidate_collector.py -v` 实际10/10通过；target发现成功不等于正式runner执行成功。原始pnpm-final.log保留。

内存冻结候选、摘要一致性与纯冲突计划已实现；持久不可变对象存储、宿主run/epoch绑定、真实受控执行器、生产handler接线、CAS落地、正常hooks联合新路径仍未实现/未验收。旧Rust正常hooks代码不变，原435+2证据保留而未重跑。新切片Linux原生进程已验证，Mac原生/容器、Windows、GUI/provider均not run。
