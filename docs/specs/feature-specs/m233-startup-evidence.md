# M233：实现与验证记录

## 排查范围

对 `v1.91.70..v1.91.80` 的完整提交和前端差异进行审查，不能将 `v1.91.75` 之后前端无变更推导为整个范围无前端风险。

| 候选 | 观察 | 处理 |
| --- | --- | --- |
| M217 | setup 用 block_on 重建每日账本，历史用量查询在窗口事件循环上等待 | 后台恢复链先重建账本，再开放自动恢复；联网余额刷新仍是带 5 秒超时的后台任务 |
| M226/M230 | setup 用 block_on 执行历史结算；git status/worktree/ls-remote 原来无超时 | 移到专用 blocking worker；复用 30 秒有界进程执行器；文件系统探测最多两条，35 秒后按未知/有在途工作处理；closeout 单并发、120 秒等待上界，保留后台 worker 所有权 |
| M192 | connect → ensure_schema → ensure_search_index 首次升级在事务中全量回填 | 启动连接只建知识库基本 schema；后台事务升级 FTS；早到的搜索按需完成升级，不暴露缺表错误 |
| M191 | start 本身已 spawn，但 notify 注册和 WalkDir 使用同步 OS API | 监听注册单 blocking lane、10 秒上界；目录遍历双 blocking lane、30 秒上界；超时后不按空目录删除旧索引 |
| 前端 | tauri.ts 主要新增类型与调用包装；knowledge store 在资源页加载失败明细；desktopDispatch 新增 usage_status route | 首屏会话/设置 effect 无需等待联网才能渲染；完整前端测试与既有真实浏览器启动验收 |
| 其他 | 每日备份在 setup 内同步 copy；已有 usage-v1 仍重复扫描历史 | 备份移到 blocking worker；启动连接已有迁移收据时跳过重复历史用量回填 |

这些是已确认的阻塞路径，不是已在用户 Windows 上复现的唯一根因。当前环境为 macOS，Windows 真机回报仍由编排方发版安装后验收。

## 需求与证据

- R1/R2：`scripts/startup-responsiveness.test.mjs` 修复前两项失败，修复后通过；Rust 合成 320 历史会话、320 工作目录及每次调用延迟的独立 git 替身，后台结束前要求查询在 2 秒内完成。
- R3/R6：保留 M226/M230 的准入判断及 ended 查询排除；沿用真实工作区/合并/历史续接回归；探测错误按有在途工作处理，不授权清理；技术超时由系统自动重试/自愈，不变成用户决策。
- R4：`startup.log` 持久记录 setup/database/spend-rebuild/merged-settlement/recovery/daily-backup 的步骤、状态和耗时；原 panic hook 继续写 crash 日志。Windows 默认路径 `%APPDATA%\com.codefactory.app\startup.log`，panic 文件在同目录 `crash\panic-*.log`。
- R5：新增 Windows 专属 `m233_windows_webview_paints_and_invokes_while_history_is_slow`，通过现有 Windows Cargo test gate 执行；要求真实 WebView 两帧布局之后的 IPC 在 2 秒内返回，且慢恢复尚未完成。此测试不伪装成正式安装版验收，也不修改 workflow。
- R7：启动恢复链与知识库路径探测脱离 setup；同步文件系统调用不能被安全强杀，超时只能停止等待，permit 由仍存活的 worker 持有，避免无限启动重复 worker。网络余额读取已有 5 秒 timeout；远程 git 新增 30 秒上界并禁止交互凭据提示。

## 已知边界

数据库基本 schema 修复仍需在发布 AppState 之前完成；本任务没有重构所有历史 schema 同步。首次使用量旧格式迁移和创建索引也仍属于连接阶段。日志单列 database 耗时以便后续现场定位，不将“移出历史结算”声明成所有启动 I/O 已消失。

真实 Windows GUI 测试须以 CI 结果为准。合成 worker/查询测试不是正常 setup 的端到端代替品：接线测试防止恢复链回到前台，Windows WebView 测试补窗口级证据；用户数据库的性能及真正主界面仍需安装后核对。

## Applicable Harnesses

Spec、Compatibility、Release、Observation、Payload、AI Collaboration。未改产品 UI 布局；沿用启动会话真实浏览器测试，不宣称其等同于 Windows 安装版。

AI Collaboration：context scope 为指定版本范围、启动/结算/知识库链路；假设为阻塞风险而非唯一已证实根因；review point 为 M188 顺序、M226/M230 在途保护与取消扫描兼容；validation result 记录于 PR。请求过独立只读审查，未将未返回的审查当通过。

## Windows CI 加载期修复

首轮 `check-rust` 在任何测试执行前退出 `0xc0000139 / STATUS_ENTRYPOINT_NOT_FOUND`。新增真实 WebView 测试令 lib test 链入 Tao/Wry 的 `SetWindowSubclass`、`DefSubclassProc` 等 comctl32 v6 导入；Tauri 默认 manifest 用于正式 exe，Cargo 测试 exe 未携带此依赖。`src-tauri/build.rs` 仅在 Windows MSVC 对 tests 添加 `/MANIFEST:EMBED` 及 Common-Controls 6.0 依赖，不取消 GUI 验收，不修改 workflow。实际 Windows 结果以同一 PR 新 head 的 CI 为准，不把 macOS 构建通过视为已证明该加载错误关闭。

## 真机验收交接（模板 §8）

编排方发版后在 Windows 安装新版本，保留现有本地数据库和知识库：
1. 启动几秒内看到真正主界面，能打开历史会话并输入，不只确认进程存在。
2. 核对 startup.log 中 setup 与 background recovery 分离；若仍白屏，提供最后一个 started 未 completed 的步骤、耗时和 crash 日志。
3. 已 ended 历史会话不自行恢复；有未提交/在途工作的工作区不被删除。
4. 外网断开、不可达项目路径下首屏仍可交互；知识库失败不清空已有索引。
5. Mac 安装版同样检查首屏、历史会话和启动日志。

本任务交付上限为合并，不启动发版；真实安装版验收由编排方发版后完成（not live）。


## R8 补充（编排方接手）

`fts_tokens_sql` 的 SQL 递归切词耗时随文本长度平方增长（合成基准：每 chunk 4000 字 36.7ms），且全量回填在一个写事务里，被杀后回滚重来。现改为：

- 触发器只把变动的 chunk 写入持久队列 `knowledge_fts_pending`，不切词；
- `sync_search_index` 用 Rust 线性切词，每批 100 个 chunk，短写事务，批间释放写锁；队列持久，重启后继续；
- 升级时只写入 rowid，不切词；已完整索引的 v1.91.80/81 不重建；
- 搜索每次最多追赶一批，不再同步全量回填。回填完成前搜索只覆盖已索引部分。

新增测试：`m233_tokens_are_linear_and_match_the_index_format`、`m233_upgrade_backfill_is_batched_resumable_and_keeps_writes_moving`。

本 PR 撤回了新增的 Windows WebView 测试和 `src-tauri/build.rs` 改动：该测试让 Windows 测试程序因 Common Controls v6 无法加载，修复又造成链接失败并触发信任根门禁。窗口级证据因此缺失，由用户在 Windows 上验收，后续单独补回。这条取代上文 R5 中对该测试的描述。
