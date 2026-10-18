## M116 修复

同一 objective/worktree/repo/branch 的交付上限升级复用原 delivery run，持久化会话声明并在工具入口校验授权；降低调用按 R3 拒绝。全局配置仍为硬上限。

## Req ID 与验收
- R1：已确认原消息 ceiling 只生成 bool，未落库；缺少生产首调用参数/当时设置快照，无法断言首次 pr_only 究竟来自显式参数还是配置。未读写生产 DB。
- R2：消息声明、会话 SQLite round-trip、默认 resolver 测试覆盖。
- R3：同身份 pr_only→through_ci_green→through_merge；历史 immutable trigger 幂等替换；降低调用拒绝且记录不变；真实 execute 越权请求拒绝且无文件副作用。
- R4：升级测试先红，输出原 identity collision；旧 trigger 测试先红。其余解析/默认测试缺少独立 red-before-implementation 证据，不宣称全量严格 TDD。

## 验证
- M116 focused：8 条已通过，新增历史迁移验收后纳入最终 workspace 回归。
- pnpm test：151 文件、1013 测试通过（含 lightModeAudit）。
- pnpm build：通过；仅已有 chunk 大小警告。
- Rust workspace：最终 `RUST_TEST_THREADS=2 CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-m116" cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast` 通过；日志 `/tmp/m116-workspace-bounded.log`。此前默认并发时既有 `begin_write_waits_for_a_contended_writer_instead_of_failing` 计时断言偶发失败（428µs），原用例单独重跑通过，未修改测试；限制并发后全量通过。
- governance baseline：pass。

## Harness / AI Collaboration
Spec、Compatibility、Observation、Release、AI Collaboration；无 UI/载荷修改。Context scope：授权解析、聊天事务、SQLite schema、交付工具。Assumptions：全局配置是硬上限。Review point：同身份升级不得扩大授权，生产幂等升级必须移除旧触发器。Validation：合成 SQLite + 真工具入口 + 项目回归。按模板不使用 delegate_tasks；此变更路径紧密耦合，单执行流。

## 真实应用验收与收尾
not live；本任务仅合并、不发版。真机验收由编排方发版安装后完成：授权 merged 后先 pr_only 再 merged，核对同一 run/PR；只授权 pr_only 时请求 merged，核对无提交/推送；省略 ceiling 核对会话默认；重启续接；降低请求核对拒绝。合并后确认 merged=true、main 包含 squash SHA、精确远程分支清理、本地状态与专属缓存清理。

Spec Amendment：无；R3 明确覆盖旧 M69 允许降低调用的行为，未自行放宽需求。

Scenario-Test: HLT-001, HLT-002, HLT-003, HLT-004, HLT-005, CXD-001, CXD-002, RTE-004
README-Update: reviewed
README-Update-Reason: 修复内部交付授权与持久恢复，不改变 README 的公开产品、安装或隐私承诺。
