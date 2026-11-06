# M256：Windows CI 偶发失败家族审查

## 现场（2026-11-05 到 11-06）
Windows 的 `check-rust` 接连偶发失败，每次都拖住合并或发版：
- `browser_open_reads_the_same_content_when_repeated`（M242，已修）
- `m164_reserve_capped_primary_waits_when_every_alternative_is_blocked`（M249，已修）
- `m202_pending_failure_subscription_survives_restart`：`left: Running, right: WaitingWake`，`thin_derive.rs:1980`
- `delivery::scenario_tests` 5 个用例同时超过 45 秒 worker deadline，日志为空（`delivery_scenario_tests.rs:944`）

## 规格（放到 `docs/specs/feature-specs/m256-windows-ci-flake-family.md`）
- R1：统计最近 30 天 `main` 和 PR 上 `check-rust` 的失败：测试名、次数、耗时，以及重跑后是否通过。写进规格，按共同原因分组。
- R2：给每组找根因。优先看共享全局状态、固定时长等待、并行进程数和 Windows 启动成本。
- R3：修排名靠前的组。至少修上面的 m202 和 delivery worker deadline。如果是产品缺陷，修产品。
- R4：worker 超时时，失败信息必须说清卡在哪一步，不能是空日志。
- R5：不隔离或跳过测试，不放宽断言，不靠单纯加大超时来变绿。确实要调整等待时要有测量依据。不改信任根。

## 测试
- 每个修复都有确定性复现：修复前失败，修复后通过。
- PR 写明本机连跑的命令和结果。Windows 以 PR 的 `check-rust` 为准，本机不能代替。

## 交付
授权交付。PR body 写 `Scenario-Test`，按任务模板收尾。

## Implementation Notes：R1/R2 审计口径

- 窗口：2026-10-08 至采集时可见的 2026-11-07（用户时区 Asia/Singapore）；按 10 日分段查询，绕过 GitHub Actions 单查询最多 1,000 条限制。
- CI workflow 共 1,533 次运行；230 次当前结论 failure，其中 175 次当前 `check-rust` failure；另补查 36 个曾重跑的 run 的 74 个 job attempt，去重后共有 **194 个失败 check-rust job attempt**。
- 每个 `(run_id, attempt, job_id, test_name)` 仅计一次，stdout failure header 与 quiet/full libtest 输出去重。子 worker 自身失败与父测试分别列出，不相加冒充独立事故。
- 表中耗时是 GitHub job 的 `completed_at-started_at`，**不是单测试耗时**：quiet libtest 不提供每个用例耗时，不能反推出精确数值。worker deadline 明确为 45 秒；示例 run 42233630287 整个 job 1,515 秒。已核验日志中的单套测试耗时：42081561452 为 634.31 秒，39979585434 为 530.54 秒，37916977834 为 230.96 秒。
- 恢复列为同一 run 后续 attempt 的 check-rust success 或同 SHA 后续 workflow success 的次数；0 不等于重跑仍失败，也可能没有重跑。未把新 SHA 修复后绿灯计作重跑成功。
- 26 个 job 未出现 libtest 失败名：包括 Windows API/导入/可见性编译错误、构建/跨进程 smoke 失败，单列为非测试失败，不伪造测试名。
- 可复核 URL 形状：`https://github.com/BumStill/CodeFactory/actions/runs/<run_id>`。采集原始数据和完整逐测试表保留在当前工作区 `.codefactory-evidence/m256-audit/`；关键结果及来源如下，不提交 runner 原始路径或其他原始生产数据。

### 共同原因分组与修复边界

| 家族 | 根因证据 | 本次处理 |
| --- | --- | --- |
| 确定性平台/结构门禁 | `production_background_commands_use_no_window` 明确指出新增 Command 缺 `.no_window()`；trusted heredoc/sync 的 proof assertion、Windows `AsRawHandle`/私有 tokenize 等编译错误。无同 SHA 恢复证据，不归类为偶发等待 | 审计记录，当前全量回归复核；不改信任根或放松结构断言 |
| 共享进程状态 | m202 的 Running/WaitingWake 不一致；m203 虽使用独立 SQLite，仍在全局 `CHAT_SESSION_EXECUTORS` 注册同一 `derive`，可使无 executor 的另一个 fixture 返回 Running | R3：m203 在创建依赖行前使用 UUID session 键；不修改 m202 断言，不串行化测试。新回归同时持有 live control 证明不再误占 `derive` |
| worker 资源放大与启动成本 | 42233630287 首 attempt 的五个父场景同时失败，四个明确报 45 秒 worker deadline，hardkill 场景报 marker 等待超时；worker 日志为空。每个并发子进程 `Runtime::new()` 又建立主机 CPU 数量线程，本机实测每进程 10 个，改后 1 个；Windows 启动延迟是负载放大的表现，历史空日志不足以证明卡在 runtime 内 | R3/R4/R5：current-thread scheduler，保留并发进程、45 秒 deadline 和所有业务断言；父进程报告 phase/PID/elapsed，子进程立即输出 worker-enter、runtime-build、git-world-init、forge-open、delivery-run；无阶段时明确 startup，不能给出虚假的更精确定位。Windows 结论以本 PR CI 为准 |
| 固定时间/异步就绪 | permission gateway 的 `durable permission intent was admitted: Elapsed(())`；browser 导航在 60 秒内未完成；supervisor 低租期在负载下被回收；数据库 writer、进程树就绪/退出和历史重启扫描时序 | 本次优先修复上面两组；browser/M164 按任务现场已由 M242/M249 修复，不重复调整等待。其他历史用例只复核当前全量结果，不以历史次数断言当前产品仍有缺陷 |

### 失败测试排名（完整限定名前缀按模块缩写；耗时秒；恢复次数/失败次数）

下表为失败 job attempt 次数，不是失败 workflow 总数。多个测试可同属一个 job。

| 测试名 | 次数 | job 耗时 | 恢复 | 来源 run（代表性；全部来源见采集逐测试表） |
| --- | ---: | --- | --- | --- |
| `production_background_commands_use_no_window` | 29 | 570–1412 | 0/29 | 40403875214, 41947837870 |
| `permission_gateway::m159_gateway_session_identity_backup_and_remote_fence` | 11 | 896–1101 | 0/11 | 39979585434, 40861692501 |
| `browser_session::browser_open_waits_for_the_navigation_before_reading_it` | 11 | 610–932 | 0/11 | 39201470829, 39204689089 |
| `historical_schema_matrix_preserves_identity` | 9 | 863–1138 | 3/9 | 40487721903, 41161230629 |
| `delivery::scenario_tests::e2e004_steer_reconnect_repair_resumes_same_durable_run` | 8 | 572–1515 | 2/8 | 39368178638, 42081561452, 42233630287 |
| `delivery::scenario_tests::delivery_fake_forge_worker` | 7 | 572–799 | 0/7 | 38076013646, 39051401515 |
| `background_tasks::stop_kills_the_whole_process_tree_not_just_the_shell` | 6 | 796–946 | 2/6 | 39092341420, 39464949787 |
| `browser_session::browser_open_reads_the_same_content_when_repeated` | 6 | 786–1269 | 1/6 | 39561956977, 42089585821 |
| `process_tree::std_process_tree_sweep_reaps_a_detached_descendant` | 6 | 707–872 | 4/6 | 39633582513, 40316252193 |
| `model_transport::prefer_mode_waits_only_after_every_endpoint_is_unavailable` | 5 | 666–763 | 2/5 | 37920830238, 38212077879 |
| `objective_supervisor::short_claim_lease_stays_live_until_adapter_settlement` | 5 | 623–709 | 2/5 | 37916977834, 38063209113 |
| `tool_backend::a_download_actually_lands_its_file_and_commits_the_receipt` | 5 | 744–932 | 0/5 | 39630597461, 39644494692 |
| `trusted_git::m215_trusted_heredoc_and_read_only_git::field_shapes_are_proved` | 5 | 1138–1220 | 0/5 | 41666849291, 41671098922 |
| `chat::admission_records_a_wording_constraint_without_waiting_on_the_write_lock` | 5 | 659–797 | 3/5 | 37877003938, 38915714786 |
| `browser_session::thin_browser_open_reaches_driver_without_receipt` | 5 | 716–868 | 3/5 | 39189448369, 39759890139 |
| `execution_workspace::reattach_rebuilds_a_deleted_worktree_from_its_branch` | 4 | 604–1014 | 0/4 | 39730462930, 39730880848 |
| `thin_derive::m202_pending_failure_subscription_survives_restart` | 4 | 1175–1325 | 1/4 | 41645750389, 41808064484, 42196362165, 42222903009 |
| `trusted_git::m215_trusted_heredoc_and_read_only_git::read_only_prefix_before_heredoc_is_proved` | 4 | 1138–1220 | 0/4 | 41668556637, 41671098922 |
| `chat::m144_admission_retries_a_real_concurrent_writer` | 4 | 641–1021 | 0/4 | 39665855822, 40377332130 |
| `delivery::scenario_tests::hardkill_after_forge_release_commit_reconciles_before_any_redispatch` | 3 | 670–1515 | 1/3 | 38076013646, 42233630287 |
| `delivery::scenario_tests::same_identity_retry_reopens_persistent_forge_without_duplicate_mutations` | 3 | 670–1515 | 1/3 | 38076013646, 42233630287 |
| `execution_workspace::m163_split_recovery_preserves_both_dirty_workspaces` | 3 | 780–1038 | 0/3 | 39980575283, 39981889391 |
| `failover::m164_reserve_capped_primary_waits_when_every_alternative_is_blocked` | 3 | 1120–1238 | 0/3 | 41747318983, 42111293606, 42202790088 |
| `objective_supervisor::a_reclaimed_claim_epoch_still_cancels_the_stale_adapter` | 3 | 842–971 | 0/3 | 39063954586, 39745122908 |
| `thin_derive::m211_current_workspace_merge_still_gets_closeout` | 3 | 1046–1263 | 0/3 | 41160216453, 41160699663 |
| `thin_derive::m211_previous_objective_merge_never_ends_continuation` | 3 | 1046–1263 | 0/3 | 41160216453, 41160699663 |
| `thin_derive::m211_reconcile_reopens_session_mis_ended_by_previous_merge` | 3 | 1046–1263 | 0/3 | 41160216453, 41160699663 |
| `browser::extension::lifecycle_smoke::lifecycle_loopback_observes_generations_timeout_and_cleanup` | 3 | 580–820 | 0/3 | 39058036339, 39058221317 |
| `m177_sync_compositions_from_the_incident_family_are_provable` | 3 | 1046–1422 | 1/3 | 41772917030, 41864708458, 41866291589 |
| `delivery::local_release_intent_with_proven_absence_resumes_and_dispatches_exactly_once` | 2 | 670–761 | 0/2 | 38076013646, 38076391653 |
| `delivery::merge_intent_reconciles_open_same_head_and_resumes_safely` | 2 | 670–761 | 0/2 | 38076013646, 38076391653 |
| `execution_workspace::m161_conflict_preserves_snapshot_and_session_can_continue` | 2 | 724–792 | 0/2 | 39884039509, 39884129806 |
| `execution_workspace::m161_startup_heals_seven_incidents_once_with_index_and_files_intact` | 2 | 724–792 | 0/2 | 39884039509, 39884129806 |
| `model_transport::a_stale_remediation_after_restart_lets_the_continuation_run` | 2 | 719–734 | 0/2 | 37921018954, 37957913699 |
| `thin_derive::m187_cross_process_active_facts_prevent_terminal_projection` | 2 | 946–967 | 0/2 | 40419901229, 40423215579 |
| `browser::extension::m136_silent_peer_is_closed_and_diagnostics_are_redacted` | 2 | 822–859 | 0/2 | 39576981427, 39671699429 |
| `formal_main_dispatches_unattended_first_and_preserves_other_entrypoint_order` | 2 | 674–771 | 0/2 | 38014843194, 38014992038 |
| `knowledge::aborted_scan_releases_the_library_and_leaves_no_partial_rows` | 2 | 992–1080 | 0/2 | 40863827465, 41743967025 |
| `m159_r1_four_incident_shapes_and_backup_restore` | 2 | 802–997 | 0/2 | 39884743726, 39886433864 |
| `background_tasks::m168_a_blocked_sink_cannot_stall_pending_wakes` | 1 | 949 | 0/1 | 40375127689 |
| `background_tasks::reconcile_resumes_a_live_task_finishes_a_dead_one_and_marks_unknown_lost` | 1 | 796 | 0/1 | 39092341420 |
| `delivery::scenario_tests::preexisting_dirty_file_survives_delivery` | 1 | 1515 | 1/1 | 42233630287 |
| `delivery::scenario_tests::wrong_release_tag_head_never_claims_live` | 1 | 1515 | 1/1 | 42233630287 |
| `delivery::pending_ci_is_handed_back_to_the_agent_with_a_background_watch_instead_of_looping` | 1 | 891 | 0/1 | 39126423394 |
| `execution_workspace::m161_conflict_preserves_rescue_and_registered_branch` | 1 | 937 | 0/1 | 39885419255 |
| `execution_workspace::m161_dirty_incident_is_rescued_and_continues` | 1 | 937 | 0/1 | 39885419255 |
| `execution_workspace::m161_interrupted_saved_rescue_resumes_without_new_snapshot` | 1 | 937 | 0/1 | 39885419255 |
| `execution_workspace::m226_cleanup_snapshot_preserves_nested_worktree_work` | 1 | 1169 | 0/1 | 41742184071 |
| `permission_gateway::m130_real_local_commit_then_approved_push_updates_synthetic_remote` | 1 | 734 | 0/1 | 40419236617 |
| `permission_gateway::m130_remote_approval_persists_and_resumes_same_call` | 1 | 734 | 0/1 | 40419236617 |
| `thin_derive::m174_merge_during_wait_crosses_process_and_closes_out` | 1 | 1250 | 0/1 | 42252968346 |
| `thin_derive::uncertain_running_projection_is_not_ended_or_retimestamped` | 1 | 735 | 0/1 | 40816340436 |
| `m177_incident_composition_is_provable_without_approval` | 1 | 772 | 0/1 | 40429488393 |
| `m215_unproved_heredoc_names_the_line_and_stays_closed` | 1 | 918 | 0/1 | 41171403522 |
| `storage::db::begin_write_waits_for_a_contended_writer_instead_of_failing` | 1 | 774 | 1/1 | 39676076048 |
| `tools::bash::guard_tests::m147b_body_file_is_copied_not_edited` | 1 | 769 | 0/1 | 39672213852 |
| `tools::pr_body_guard::m147b_body_file_is_copied_not_edited` | 1 | 822 | 0/1 | 39671699429 |

### Requirements Traceability / 验收矩阵

| Req | 验收 | 证据 |
| --- | --- | --- |
| R1 | 完整窗口 + 重跑 attempt，不将 job 时长冒充单测试时长 | 1,533 runs、194 失败 job attempts、逐测试统计及 26 个非 libtest 失败 |
| R2 | 共享状态、固定等待、并发资源、平台成本分别审查 | 上述家族根因表及代表 run |
| R3 | 确定性红绿 + 原有 m202/m203/真实 forge 测试 | `m256_startup_fixture_cannot_claim_another_fixtures_executor`、`m256_worker_runtime_does_not_multiply_host_cpu_threads`；原重启/同 PR/工件/进程树断言保留 |
| R4 | 空日志、仅 libtest banner、已进入 Git 阶段均可诊断 | `m256_empty_worker_log_still_identifies_startup_phase` |
| R5 | 不跳过、不串行、不放宽、不加超时、不改信任根 | diff 审阅 + 全量 Rust/前端与 PR Windows CI |

Applicable Harnesses：Spec、AI Collaboration、Compatibility、Observation；交付启用 Release Harness 的 PR/CI/merge 部分，本次不发版。无 UI/Viewport 改动。context scope 为任务、模板、相关测试与 CI 聚合证据；假设是共享执行者键与线程放大；review point 是确定性红绿和全量并发；validation result 以 PR 验收表为准。

真机验收由编排方发版后完成；会话内使用真实 Git/SQLite/受管子进程集成测试，不把本机 macOS 结果代替 Windows `check-rust`。无 Spec Amendment。
