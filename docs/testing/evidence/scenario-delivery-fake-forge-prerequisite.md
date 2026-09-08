# E2E-004 后端交付链前置：持久 fake forge

## 范围与设计

- 对应 CF-STG-R14/R29、M4；只补后端交付函数的真实 Git、SQLite/WAL 和替换进程回归。
- 唯一产品委托是 `delivery.rs` 的 `cfg(test)` 子模块。测试调用生产 `deliver::<DeliveryRemote>`，不复制交付状态机，不改运行时代码。
- fake forge 把 PR、merge、release target、run ID/status/head SHA 和合成 release 元数据存入独立 SQLite/WAL。重开连接和替换测试进程必须复用已存在记录，而非依赖内存计数器；release observer 重读全部身份/状态字段，缺字段失败、head 不符返回 HeadMismatch，不硬填完成状态。副作用数从原始 mutation 行重算，另查独立 request 日志；`open_or_get_pr` 的只读复用与真正创建分别记账。
- 所有 Git remote 都是世界内 bare 仓库；子进程清空继承环境并指定临时 HOME/config/cache、禁用全局/系统 Git 配置与交互。无网络、真实账号、生产数据库或 GUI。
- 检查失败和 pending CI 不向上执行；再检查 release tag 不包含交付 SHA 时不能取得 `live_verified`。后者直接复用生产 `github_release_live_from_value`，tag ancestry 用临时 bare Git 的真实 `merge-base` 核验，而非 fake 直接返回 Failure。
- 同一分支/head 重试不产生重复 PR/merge/release。预存 dirty 文件放在真实 root checkout，交付在已创建的独立 Git worktree 中执行；断言原文件内容、原 index 字节及 root HEAD 保持不变。本片不证明产品自动创建 worktree，也不声称直接对 dirty cwd 调用 `deliver` 会自动区分用户修改。
- hard-kill 放在 fake forge 已提交 release 行、尚未返回生产交付函数的窗口。父进程复用已有 `StdProcessTree`，在子进程 group/job 归属建立后才放行其 Git 动作；45 秒执行 deadline，终止与重启均不经过桌面/CLI入口。重启前确认生产本地回执仍是 `intent_release`，然后通过生产只读对账续接。
- 正常退出和 hard-kill 后均断言 process tree 活跃数为 0；WAL 重开后检查 `PRAGMA integrity_check=ok`；父进程只清理自己创建的 `TempDir`，并断言该精确路径已不存在。
- 清理顺序经过独立审查修正：Unix 用 `waitid(... WNOWAIT)` 观察退出而不提前 reap，主 PID 保留期间清理 owned group，然后有界等待并 reap；不按已回收、可能复用的 PID/PGID 发信号。macOS 对只剩 zombie 的进程组会拒绝 SIGKILL，因此只在已确认主进程退出后，通过 `proc_listpgrppids` 的实际返回区间明确读到唯一保留主 PID 才跳过信号；空、全零、缺 leader、重复、失败或截断结果均拒绝。Windows 使用 kernel-owned Job。范围限定为本 fixture 不逃逸 group/job 的后代，不是通用 OS 沙箱。

独立 QA 核对的 API 依据：[Apple libproc 实现](https://github.com/apple-oss-distributions/xnu/blob/main/libsyscall/wrappers/libproc/libproc.c#L70-L78) 将 `proc_listpids` 字节数除以 `sizeof(int)`，所以 `proc_listpgrppids` 返回 PID 个数。测试按这个返回区间解析，不能遍历预填零的整个缓冲区证明无后代。

## 失败优先验收

首个执行测试：`agent::delivery::scenario_tests::forge_mutations_survive_a_fresh_connection`。
先用明确不持久化的 fixture scaffold 观察重开连接读取失败，再实现 SQLite 持久化。
这是新测试能力缺失的红灯，不声称发现生产交付缺陷。

实际行为测试：

- `ci_not_green_never_merges_or_releases`（failure 与 pending 两个临时世界）；
- `wrong_release_tag_head_never_claims_live`；
- `same_identity_retry_reopens_persistent_forge_without_duplicate_mutations`；
- `preexisting_dirty_file_survives_delivery`；
- `hardkill_after_forge_release_commit_reconciles_before_any_redispatch`。
- `supervisor_reclaims_owned_descendant_after_main_worker_exits`（真实后代存活负例）。
- `owned_group_observation_rejects_empty_missing_leader_and_truncation`（macOS 严格成员列表负例）。
- `release_observation_reloads_all_persisted_dispatch_fields`（不同 run ID/status、错误 head 与缺字段负例）。

`delivery_fake_forge_worker` 与 `owned_survivor_descendant` 是父 supervisor 指定 exact 名称启动的 ignored 入口，不独立计为通过用例；没有世界 owner/隔离环境会失败。

## 证据边界

本片不证明 UI 追加约束、Objective/DeliveryRun 持久绑定、lease/CAS 全链恢复、真实发行包、L3/L4 或完整 E2E-004。调用的是生产 `deliver` 及它的 Git 本地回执，`mutation_permit=None`，不冒充 durable DeliveryRun 数据库恢复。

**仍缺 artifact 内容身份验证：** 当前生产 `github_release_live_from_value` 只检查 published/non-draft/non-prerelease、assets 非空及 tag 包含交付 SHA，不校验 asset 内容摘要或 artifact manifest SHA。测试只证明错误 tag 被生产验证器拒绝；合成 asset 元数据不是安装包字节，更不是正式安装包验收。本片不改该运行时验证器，下一片若修复需独立评审兼容性与外部 exact-artifact 边界。

测试不接入 registry、可信 case runner、required workflows 或 ruleset；后续将其提升为可信 PR slice 需单独批准信任根迁移。

## 验证记录

- 基线：`origin/main=52bd3657ed15b75ea916cc5c1a271a5d85b2bacc`；分支 `codex/scenario-delivery-fake-forge`，使用 canonical `pnpm worktree:start` 和共享 Cargo 缓存。
- 红灯：`pnpm cargo:shared -- test --lib --locked agent::delivery::scenario_tests::forge_mutations_survive_a_fresh_connection -- --exact --nocapture`，实际执行 **0 passed / 1 failed**，exit 101；重开 SQLite 得到 `None`，预期为已确认 PR payload。编译 39.91 秒。
- 第二个红灯：`pnpm cargo:shared -- test --lib --locked agent::delivery::scenario_tests::supervisor_reclaims_owned_descendant_after_main_worker_exits -- --exact --nocapture`，**0 passed / 1 failed**，exit 101，6.06 秒；旧实现主进程已退出但 group 仍有后代。负例后代有 6 秒自退出安全网，红灯等其退出且精确清理临时世界后才最终失败，未按已 reap 的 PGID 发送信号。
- 第三个红灯：`pnpm cargo:shared -- test --lib --locked agent::delivery::scenario_tests::release_observation_reloads_all_persisted_dispatch_fields -- --exact --nocapture`，**0 passed / 1 failed**，exit 101，0.02 秒；重开数据库后旧 observer 硬填 `synthetic-run-1`，与原始记录 `persisted-run-29` 不符，再改为读取原始字段。
- 绿灯：`pnpm cargo:shared -- test --lib --locked agent::delivery::scenario_tests -- --nocapture`，**9 passed / 0 failed / 2 ignored**，最终复跑执行 2.99 秒。包括不同 PID 的成功重试与 hard-kill 替换进程、原始持久计数 `(PR=1, merge=1, release=1)`、错误 tag 不取 `live_verified`、dirty/index 保留、SQLite 完整性及清理断言。
- `python3 tools/governance/run_scenario_harness_gate.py --stage local --repo . --policy-repo .`：通过。
- `python3 tools/governance/validate_repo_governance_baseline.py`：通过。
- `python3 tools/governance/validate_scenario_test_governance.py`：通过（28 scenarios / 11 complex cases），只是结构/治理通过，不代表 E2E-004 已补全。
- `pnpm cargo:shared -- check --lib --locked`：通过，20.54 秒；现有 unused/dead-code warnings 未扩大到产品修改。
- `git diff --check`：通过。最终 fresh fetch 后 `origin/main` 仍为上述基线且已被当前分支包含。独立 QA 已复核持久字段、错误 head/缺字段拒绝和进程回收顺序；主执行者再次实际运行本模块，9 passed / 0 failed / 2 ignored，2.91 秒。

## 后续门禁

本地提交使用正常 pre-commit 同步和 canonical scenario hook；提交前 fresh fetch，必要时 merge 最新 main 后重新验证。独立复核通过后，由主执行者继续普通 PR、CI 和合并；不启动 GUI，不绕过场景门禁。测试改动不单独触发产品发版。
实际 `delivery.rs` 的 change_patterns 命中 HLT-001、HLT-002、HLT-005、CXD-002。普通 PR 应显式声明 `Scenario-Test: E2E-004, CXD-001, CXD-002, HLT-001, HLT-002, HLT-005`，并再次用可信 base 的计划核对实际影响。Windows 进程 job/临时 Git 路径分支尚未在本机验证；现有 `check-rust` 的全包 `cargo test` 会执行本模块，应交 Windows runner，不把 Mac 通过视为跨平台完成。
测试策略采用分层验收：基础持久化红灯、生产交付函数行为、独立进程恢复；不把夹具自检替代业务完成证据。
