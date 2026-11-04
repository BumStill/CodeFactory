# fix: unblock Windows startup recovery (M233)

修复 Windows v1.91.80/81 启动白屏的已确认同步阻塞路径；未将其声明为用户机器上已证明的唯一根因。

## 规格与排查
- 原始规格：docs/specs/feature-specs/m233-windows-white-screen-startup-blocking.md（首个提交原文）。
- 完整版本范围 v1.91.70..v1.91.80：M191、M217、M226/M230、M192 及前端变更，详见 docs/specs/feature-specs/m233-startup-evidence.md。
- Spec Amendment: 无；已知边界不作为降低验收标准的批准。基本数据库 schema 连接仍同步，详见证据文档；首次历史格式迁移仍需现场耗时核对。

## Req ↔ 测试与验收
| Req | 实现与测试 | 状态 |
| --- | --- | --- |
| R1/R2 | startup-responsiveness.test.mjs 两项先红后绿；m233_slow_history_worker_leaves_command_queries_responsive_and_finishes，320 历史会话/320 目录/真实慢 git 替身，后台结束前查询限时 2 秒 | 本地通过，最终数据补充于验证结果 |
| R3 | 有界 git 30 秒；探测最多两条、35 秒等待；closeout 单并发、120 秒等待；技术超时保留工作由系统自动重试/自愈 | 针对性及完整 Rust 测试 |
| R4 | startup.log 中 setup/database/spend-rebuild/merged-settlement/recovery/daily-backup 步骤、状态和耗时 | 日志读取断言通过 |
| R5 | Windows 专属 m233_windows_webview_paints_and_invokes_while_history_is_slow，真实 WebView 两帧布局后 IPC 必须在 2 秒内完成且恢复未完成；接入现有 Cargo test Windows gate | Windows CI 验证，本地 macOS 不冒充通过 |
| R6 | M188 结算后恢复顺序；M226/M230 既有防复活/在途工作回归不削弱，ended 不进入结算查询 | 完整 Rust 回归 |
| R7 | 余额刷新 5 秒网络超时；git 禁交互/30 秒；UNC/慢目录的探测、遍历、监听移入有界 blocking lane，超时不清空索引 | 代码路径与自动化验证，真实不可达网络盘仍需编排方核对 |

## 验证
- pnpm test：163 个文件、1089 项通过（含 lightModeAudit）。
- pnpm build：通过。
- pnpm test:startup-session:headless：通过；截图在系统临时目录 codefactory-startup-session-headless/with-history.png、history-continue.png、empty.png；未改 UI 布局。
- Rust：CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-m233" cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast；完整运行通过，包含防闪窗、历史恢复和知识库回归。
- 320 会话/工作目录 + 真实慢 git 替身独立测量：setup_return_and_query_ms=0（毫秒精度），background_total_ms=10474；只证明共享后台启动器与查询响应，不冒充完整正常 setup 性能。
- 实际应用 cargo build 成功后从 /tmp 执行 --history-session-smoke，机器断言 ok、same_objective、second_restart_stayed_cancelled 全为 true。
- node --test scripts/startup-responsiveness.test.mjs：2/2 通过。
- 治理基线通过；最终 PR body 将经 scenario --ci 和 README contract 双检查。

## Applicable Harnesses / AI Collaboration
Spec：原文规格及 Req 映射；Compatibility：旧 schema、ended、在途工作保护；Release：交付只到 merged、not live；Observation：步骤持久日志与 panic hook；Payload：历史 SQLite 与路径探测安全；AI Collaboration：context scope 为指定版本范围和启动链，assumptions 为阻塞风险并非唯一已证实根因，review point 为恢复顺序与取消扫描兼容，validation result 为上述测试。独立只读审查曾请求，未将未返回的结果视为已通过；当前主要验证为单执行流。

## 真实应用验收与 §8 收尾
真机验收由编排方发版后完成：Windows 保留本地数据安装新版，几秒内看到真正主界面、打开历史会话并输入；核对 ended 不复活、在途工作区不删除；断外网/不可达路径下仍可交互；Mac 同步检查。Windows 日志：%APPDATA%\com.codefactory.app\startup.log，panic：同目录 crash\panic-*.log。
本任务仅授权合并，不发版（not live）。CI 绿后标记 merge-queue: armed，由队列合并，不手工 merge。合并后核对 API merged=true、squash SHA、Release-Urgency trailer；检查远端分支、干净状态、清理本任务构建及临时进程，不删除 managed workspace。

Scenario-Test: DAT-001, HLT-001, HLT-004, KB-001, RTE-003, RTE-004
README-Update: reviewed
README-Update-Reason: 修复现有启动实现与诊断日志，不改变 README 的公开功能、平台或隐私承诺。

Release-Urgency: immediate
