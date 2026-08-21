# DeliveryRun 中断恢复收敛

- 日期：2026-08-21
- 状态：实现与本地验收中
- 目标：一次授权的交付任务在本地 Git 提交窗口掉线后自动续接；无法证明归属的分叉安全且有界停泊，不要求用户发送“继续”
- Primary User Path：用户授权修改、测试与发布 → CodeFactory 在同一 Objective/DeliveryRun/PR 上推进 → 进程或 App 中断 → 系统自行完成或形成稳定 system incident → 不出现无限 claim、重复远端写或假运行 UI

## 现场根因

正式 v1.81.23 的最新 DeliveryRun 已经安全拒绝未知远端写入，但没有收敛：同一 `takeover_reconciliation/platform_incident` 跨数百次 lease expiry 重复 claim，`claim_epoch` 持续增长，而 linked Objective 已进入 `technical_recovery_exhausted`。本地 worktree 的 persisted head 与当前 head/dirty change-set 互不相等，当前证据不足以自动采纳；正确结果应是稳定停泊，不是继续空转。

代码上有四个相互放大的缺口：

1. `git commit` 是本地不可重放副作用，但此前只有提交后的 DeliveryRun identity revision，没有提交前 write-ahead receipt；commit 成功到 outcome 落盘之间掉线时，新 owner 无法区分“本系统刚提交”与“外来提交”。
2. takeover identity mismatch 被无类型地记录成可恢复 `platform_incident/external_state_uncertain`；相同失败不会递增 `stage_attempt`，也没有 claim ceiling。
3. Objective 的恢复预算与 DeliveryRun 的 startup planner 分属两套 owner；Objective 已停泊时，DeliveryRun 仍被 planner 认领。
4. `plan_startup_recovery` 只看非终态和稳定身份，未把 `next_action_authorized` 与 linked Objective 生命期作为 claim 前置条件。
5. 只修“commit 后已经 push”的恢复仍不完整：若 commit 后、push 前中断，当前 head 是 receipted child，但既有 canonical PR/remote 合法地仍在 parent；旧 takeover 只接受两边完全相等，会再次把同一系统提交误判为外来分叉。

## 为什么原有测试没有拦住

- 既有 takeover 测试只断言一次 identity mismatch 时“零远端 mutation”，因此把一次性的 fail-closed 当成完整安全；没有断言第二、第三次 lease expiry 后必须完成或停泊。
- 既有 lost-heartbeat 测试手工调用 `mark_claim_reconciled`/permit 校验，没有执行真实 `resume_claimed_delivery`，更没有跨 `git commit -> process kill -> persist_durable_outcome` 的崩溃窗口。
- Objective、DeliveryRun planner 和 delivery adapter 分层单测各自为绿，没有跨表 oracle 断言“Objective parked ⇒ linked DeliveryRun unclaimable”。
- `should_resume_claimed_delivery` 明确把所有 `platform_incident` 视为可恢复，但没有相同 failure signature 的累计预算测试。
- 场景注册表 CXD-002 使用不存在的 `src-tauri/src/delivery*.rs` 路径，真实 `src-tauri/src/agent/delivery_run.rs` 与 `src-tauri/src/tools/delivery.rs` 改动不会触发该 P0 场景。
- E2E-001 的 exact executable smoke 覆盖普通文件工具与 provider 恢复，不执行 Git/DeliveryRun/fake forge；release workflow 因而可能全绿却完全没有进入本次失败面。
- 第一版新增 smoke 仍险些重复同一错误：它手工调用 identity helper、手工 SQL 写 `completed`，没有穿过生产 `resume_claimed_delivery`/Completion Arbiter；并且 receipt 只核对 parent/tree/message，没有核对预先记录的 exact child SHA。独立 QA 因此拒绝把结构性绿灯当作行为证据，推动验收升级为三进程生产入口。
- 第一版重复预算按 callback 次数计数，同一 lease 的两次回调会提前停泊；这说明“有上限”本身不是正确 oracle，预算单位必须是 durable `claim_epoch`。

## 需求与不变量

- CF-DR-R21：本地 commit write-ahead identity receipt；精确子提交只采纳一次；既有 PR 精确停在 receipted parent 时只读续接同一 PR；外来变化零 mutation。
- CF-ORC-R41：Objective/DeliveryRun 恢复生命期一致；最终只能完成或形成一个稳定 incident。
- 相同 `delivery_identity_conflict` 最多消耗两个不同 `claim_epoch`；同一 lease 内重复 callback 只算一次。第二个 epoch 以精确 permit 在同一事务停泊 Objective/DeliveryRun/turn/run-control，第三次 poll 的 claim/event 数不再增长。
- 用户 core input/business decision/显式拒绝/取消不能被后台 delivery supervisor 重新认领。
- 不削弱原有 fail-closed：没有精确 receipt 时绝不自动 rebase、force push、创建新 PR 或覆盖本地变化。

## Failure-first 与验证证据

- 新增失败测试先确认：相同 takeover 失败无限 claim、parked Objective 的 DeliveryRun 仍 claimable、commit 前没有可恢复 receipt、既有 PR 停在 receipted parent 时仍被误拒、typed Delivery failure 被错误路由到通用 Tool domain。
- 定点回归已通过：同一 claim epoch 两次 callback 仍为 attempt 1，第二个 epoch 才为 attempt 2；同 parent/tree/message 但不同 exact child SHA 被拒绝。
- `--delivery-recovery-smoke` 使用真实 Git/bare remote/生产 SQLite：精确 commit object 与 intent 落盘后、branch ref CAS 前 hard kill；替代 owner 在当前 lease/epoch/Objective 写锁 permit 下完成 exact CAS；identity revision 后再次 hard kill；第三 owner 走生产 resume；identity revision=1、single push=1、canonical PR=1、Completion Arbiter 一致完成、foreign park=true、claim epoch plateau=true、duplicate remote write=0、cleanup=true。
- PR/nightly/release 将运行同一 exact-executable E2E-011；发布后还必须用公开安装的 macOS binary 重跑，并校验 build SHA 与 tag 一致。

## 2026-08-21 交接快照

- 工作树：`/Users/leo/Projects/CodeFactory/.claude/worktrees/codex-fix-delivery-recovery-convergence`
- 分支：`codex/fix-delivery-recovery-convergence`
- 基线：`255b054c`；当前仍未 fetch/merge 最新 `origin/main`，提交前必须执行同步门禁并解决冲突。
- 写入本交接段前的未提交 diff SHA-256：`05a6c9e58b06fd86949b834f58aadfcd1e49cf18ef05ffa932b3cc52ffdecab4`；以接管时现场 `git diff | shasum -a 256` 为准。
- 不要在主 checkout 修改；主 checkout 含用户自己的旧分支/未提交改动。

### 已实现并有定点证据

- 相同 takeover failure 按不同 `claim_epoch` 有界计数；Objective 与 DeliveryRun 一致停泊，后续 claim/event plateau。
- local commit 使用 exact child SHA write-ahead receipt、隔离 index、owner/epoch/Objective writer fence 与 owned index-lock CAS；外来 lock、外来提交、取消或 stale owner 均零 ref mutation。
- push 返回成功后必须 `ls-remote` 精确等于授权 SHA，才允许 committed/PR；真实 bare-remote `post-receive` 外来推进测试先红后绿。
- PR create/body 在 mutation 后 fresh observe number/URL/head/base/body；committed receipt 重入只读复用，foreign drift 停止且不重放。
- Hook/GitLab merge 不再信任 `ok`/HTTP 2xx 或空 merge SHA；必须 fresh observe 非空 merge SHA或同 head auto-merge queued。
- committed PR/merge 投影拒绝 committed result A 与 fresh observation B 的身份冲突。
- branch-update 正常路径与恢复路径使用 exact provider head、临时 ref、writer-fenced local FF；恢复后 intent 结算为 `reconciled_committed`，避免 generic reconcile 二次处理。
- E2E-011 已扩展为四进程：pre-ref hard kill、identity revision 后 hard kill、push committed/pre-outcome hard kill、最终生产 resume/Completion Arbiter；single push/canonical PR/foreign park/claim plateau/零重复远端写。
- release/auto-release 已开始冻结 exact SHA；tag-only 手动 release input 保持兼容；prepare/build/finalize/公开验证 checkout frozen SHA。

### 当前已通过

- `pnpm cargo:shared -- check --manifest-path src-tauri/Cargo.toml`：通过（仅既有 dead-code warning）。
- 新增 failure-first：push foreign post-receive、PR create foreign head、PR body post-mutation drift、merge ok但无positive observation、committed PR/merge冲突：均先红后绿。
- `pnpm cargo:shared -- test --manifest-path src-tauri/Cargo.toml committed_provider_receipts_are_freshly_observed_and_never_replayed -- --nocapture`：1/1。
- `node --test scripts/delivery-recovery-smoke-contract.test.mjs`：3/3。
- `python3 -m unittest tests.test_release_workflow tests.test_github_main_gate`：35/35。
- `git diff --check`：通过。
- 之前的 exact executable `--delivery-recovery-smoke` 在三进程版本通过；扩展为四进程后尚未重新运行正式 binary，不能引用旧结果作为当前证明。

### 仍是 P0 / 不得开 PR 或发布

1. `release.yml` 的 build-windows/build-macos 只在 job 开始复核 tag，随后长时间编译，`tauri-action` 上传前没有相邻复核。tag 若在编译期间移动，冻结 SHA 构建的资产仍可能上传到已移动的 tag 名下。应拆分 build/upload，或在上传动作紧前与紧后精确复核 remote tag；post-check 失败时 draft 必须保持不发布。prepare 的 `gh release create`、finalize 的 upload/edit 也应把复核紧贴每个 mutation。新增可执行/结构 contract 模拟 `authorized=A, tag_at_upload=B`，断言 publish/tag/build/upload count 为 0（已产生 draft 时只能保留为 incident）。
2. branch-update committed 后 canonical PR 已 merged 且 provider 自动删除 feature branch 时，当前恢复 fetch 仍依赖 `repo.branch`；需要从 provider PR ref 或 exact SHA 可寻址 ref 获取 receipted `next_head`。先写 production-connected 测试：merged+deleted head branch、observer count=1、intent=`reconciled_committed`、第二次恢复 observer count 不增长、update actuator=0、继续 merge/release/completion。
3. `pre_intent_isolated_stage_leaves_real_git_state_bit_identical` 只是同进程正常返回，不是 SIGKILL。新增子进程 hard-kill：HEAD/ref、真实 index bytes/staged set、worktree bytes/status、mutation intents、remote writes 不变；明确临时 index/unreachable objects 的允许与 GC 契约。目前测试名比证据强。

### 接管执行顺序

1. 先完成上述两个 P0 failure-first 与实现；将第 3 项至少收窄规格/测试名称，最好补 hard-kill。
2. 更新旧 Hook happy-path fixtures，使 create/body/merge 都提供 exact observe 响应；跑相关 Hook/Stub 定点。
3. 重新跑当前四进程 exact executable：`pnpm cargo:shared -- run --manifest-path src-tauri/Cargo.toml -- --delivery-recovery-smoke <receipt>`，逐字段核对四 owner、post-push receipt、Completion Arbiter。
4. 跑全量 Rust、frontend、build、治理与 workflow tests；请求独立 QA 固定新 diff hash 复审，P0/P1 清零。
5. `git fetch --prune origin main`，确认查重；merge 最新 `origin/main` 并重跑验证。最终 commit 使用 `fix:` 且保留 `Release-Urgency: immediate`。
6. PR body 声明 `Scenario-Test: HLT-001, HLT-002, HLT-005, CXD-002` 与 `Complex-E2E: E2E-011`；等待五项 required checks，squash merge。
7. 扫描 `<latest-tag>..main` 无 hold/非法 urgency 后 exact-head dispatch Auto Release；预期 patch 为 v1.81.24（以届时最新 tag 为准）。核验公开 6 类资产、`latest.json` 与 tag SHA。
8. 匿名下载公开 DMG，安装后用 exact installed executable 重跑 DeliveryRun recovery smoke，并在正式 App 验证旧会话 claim/event plateau、无假 spinner/Stop、真正 active root 仍可停止。通过前只能报 `released_not_live_verified`。
