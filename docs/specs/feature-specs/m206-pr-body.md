## M206：未处理工作不能被旧终态覆盖

Scenario-Test: HLT-001, HLT-004, RTE-003, RTE-005
README-Update: reviewed
README-Update-Reason: 修复后台会话状态与派单续接，不改变公开功能、安装、隐私或平台承诺。

### 根因 / Req 映射
- R1：三例匿名 SQLite 夹具均先红：m206_output_numbers_never_claim_pr（Some(188)）、m206_new_user_outranks_old_merge_and_settlement（Ended）、m206_same_process_unanswered_turn_never_orphaned（retired=1）。修复后同命令三例通过。
- R2：统一 unanswered_user 判断；保护状态派生、thin_state::save、直接收尾事务、预算拒绝和工作区结算；OS 进程出生时间是 orphan 回收必要条件，无法证明则不回收。系统唤醒根使用持久身份，不按文本猜测。PR 发现只认交付记录或登记工作区 head 分支 gh 查询。
- R3：真实新用户 admission 原子重开 ended 状态；dispatch 回执不再将只有消息无回合/模型唤醒的持久行当 started。m206_receipt_rejects_persisted_message_without_started_turn 已先红。
- R4：旧 pr-N 文本关联精准隔离，原引用与观测归档，消息与唤醒历史保留；只取消同会话同 PR 自动唤醒。m206_quarantines_only_text_inferred_observation_and_preserves_history 验证幂等与字段行数。

### Harness / AI Collaboration
- Spec：docs/specs/feature-specs/m206-session-ends-over-pending-work.md 原任务 + Implementation Notes。
- Compatibility：历史 thin SQLite、系统唤醒、交付引用、旧收尾路径回归。
- Observation：真实 SQL 状态、回执字段、孤儿回收计数、历史归档断言。
- Release：只交付合并，保留 Release-Urgency trailer；另行发版安装验收。
- AI Collaboration：context scope 为 thin 家族、派单和准入、工作区与持久存储；assumptions 不读取生产消息、不按文本猜测身份；review point 为所有直接终态写入、并发准入、旧 PR 关联；validation result 以最终命令证据更新。任务模板禁止 delegate_tasks，采用单执行流。
- 无 UI 布局改动，不适用 Viewport / 截图。后台沿用 dispatch send 与持久唤醒入口。

### 验证与自验
- M206 八项测试通过：三例核心夹具先红后绿；回执缺少 started root 检查先红后绿；历史归档幂等与真实 ended admission 集成通过；8 个终态入口共享矩阵、同进程已回复但仍有活动的独立孤儿约束通过。源码目录外执行已编译测试产物，thin 家族 88 项通过。
- pnpm test && pnpm build：163 个测试文件、1086 项测试通过，生产构建通过。
- M151 四项、M158 六项、m173_missing_credentials_is_visible_user_wait_not_orphan 均通过。M151 旧终态断言与 M206 未回应不变式冲突，改为持久等待；M158 改用真实交付工具记录而非文字 PR 夹具；诊断读取先快照旧投影，重置不再取消未回应任务唤醒。
- 统一 local scenario harness、scenario governance --ci、README contract、governance baseline、git diff --check 均通过。系统 Python 3.9 缺 tomllib，使用现场存在的 Python 3.11 执行同一门禁，未修改 validator。
- Rust workspace 最终全量通过：`CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-m206" CARGO_INCREMENTAL=0 cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast` 退出码 0；主库 2116 项通过、12 项既有隔离 worker 忽略，其余 workspace/integration/doc tests 全通过。
- Spec Amendment：无。

### §8 / 真机验收分工
not live：本任务不发版安装，不直接读写生产数据库。真机验收由编排方发版后完成：
1. dispatch send mode=queue 到 ended 会话，1 分钟内核对新 assistant 回合及非旧合并终态；无启动证据必须明确错误。
2. 新建会话至少观察 5 分钟，不出现 orphaned_by_restart；核对活动时间晚于 OS 进程出生时间。
3. 工具输出含 #188/M188，不建立 PR #188 监视；核对旧文本引用归档且其它会话观测不变。
4. 合并后本会话负责 merged=true、squash SHA 位于 origin/main、release plan trailer、精确远端分支状态、工作区无未提交/未推送、本任务 target 清理；不删除托管工作区。

Release-Urgency: immediate
