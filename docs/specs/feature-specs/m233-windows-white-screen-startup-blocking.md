# M233（紧急）：Windows 打开新版本直接白屏

## 现场（2026-11-04，用户报告，Windows）
- 用户：「新版本 windows 打开直接白屏了」。具体版本和白屏时长待用户回复；编排方没有 Windows 机器。
- v1.91.75 → v1.91.81 之间前端没有改动（`src/` 无 diff），所以页面代码本身不是原因，优先怀疑启动时后端阻塞。

## 主要嫌疑（从代码推断，未在 Windows 上证实）
- `src-tauri/src/lib.rs` 的 Tauri setup 里用 `tauri::async_runtime::block_on` 同步执行 `agent::thin_derive::settle_merged_sessions`（以及 `reconcile_orphan_turns` 等）。
- M230（v1.91.78）在 `settle_merged_sessions` 里新增了 `delivery_closeout_may_end` 门禁，并扫描全部历史会话（注释：「Scan durable IDs, including history outside dispatch's 200-row list」）。M226 的 `pending_work_in` 对每个工作区跑 `git status --untracked-files=all`、`git worktree list`，以及每个嵌套 worktree 的 git status。之后还有 `closeout_all`。
- Windows 上启动进程很慢，加上杀毒软件扫描，几百个会话可能要几分钟甚至更久。这段时间 setup 不返回，窗口是白的。
- M192（v1.91.80）的 `knowledge::ensure_schema` 首次升级会在事务里全量重建 FTS。资料多时也要先排查它是否在启动路径上阻塞。

## 要求
- R1：先证实：写一个可执行复现，构造几百个历史会话和工作区，测量 setup 阻塞时长，确认慢在哪一步。如果嫌疑不成立，继续找真实原因（包括 WebView2 加载、前端首屏 IPC 等待、panic）。结论和数据写进 PR。
- R2：setup 不得等待逐会话的 git、文件系统或全表扫描工作。把这些移到后台任务，窗口和前端首屏不依赖它们。M188 的顺序约束要保留：合并会话结算完成前，不得准入重启恢复（可以让恢复排在后台结算之后，而不是让窗口等）。
- R3：后台结算要有界：限制并发、单条 git 超时，失败只跳过该会话并记录，不阻塞其余。已 ended 的会话不再重复扫描。
- R4：启动阶段任何一步超过阈值（例如 2 秒）都要写一条带步骤名和耗时的日志，以后能直接定位。如果日志没有落到用户能找到的文件，一并补上，并在 PR 写明 Windows 路径。
- R5：回归测试：合成几百个历史会话 + 慢 git（测试替身，每次调用延迟），断言 setup 路径在阈值内返回、窗口命令可响应，后台结算之后完成。不得只测 hook 内部状态。CI 里的 Windows 真实应用检查要能发现这个回归。
- R6：不得削弱 M226、M230 的保护：历史会话不被复活，在途工作不被清理。

## 验收（真机）
- Windows（用户）：装新版后打开，几秒内出现界面。
- Mac：`pnpm tauri dev` 或正式版启动耗时不变差，历史会话状态不变。

Release-Urgency: immediate

## 用户补充（11-04 15:47）
- Windows 上 v1.91.70 正常，v1.91.80 和 v1.91.81 白屏；v1.91.71–79 未知（用户未必都装过）。
- 所以嫌疑范围是 v1.91.70..v1.91.80，不只是 M230。这段范围内与启动相关的改动都要排查：
  - M191 `knowledge_watch::start`（lib.rs setup 新增：崩溃修复 + 全库对账 + 文件监听 + 定时兜底；Cargo.toml 有依赖变化）；
  - M217 `daily_spend_live::rebuild`（setup 里 block_on）和 `install_database`；
  - M226/M230 `settle_merged_sessions`；
  - M192 FTS 重建；
  - 前端 `src/lib/tauri.ts`、`src/stores/knowledge.ts`、`ResourcesPage.tsx`、`desktopDispatch.ts` 的改动（v1.91.75 之前）。
- 「前端没改」只对 v1.91.75 之后成立，上面规格里的这句话要按这里修正。

## 用户补充 2（11-04）：「我是在局域网里用」
- 含义待用户确认：可能是没有外网，也可能是项目或知识库放在局域网共享盘上。
- 编排方已查：index.html 无外链；`daily_spend_live::refresh` 是后台任务且带超时，不阻塞 setup。还没查完的是前端首屏是否等待联网结果。
- R7：测试必须覆盖这两种环境：
  - 断外网或 DNS 黑洞：启动不得等待任何网络调用，所有启动期网络请求都要后台执行并有超时；
  - 工作区或知识库路径在慢速或不可达的网络路径（UNC、映射盘）：`git status`、`canonicalize`、`Path::exists`、notify 监听、全库对账都不得阻塞 setup 或首屏，且要有超时。注意这些同步文件系统调用本身可能长时间不返回。

## 用户补充 3（11-04）：能访问外网、知识库在本地、直接在这台 Windows 上打开
- 网络不通、共享盘、远程访问基本排除（项目目录位置用户未说）。R7 保留为防护要求，但不是主要排查方向。
- 回到本地原因：启动阻塞（M191 全库对账与监听、M217 rebuild、M226/M230 逐会话 git、M192 FTS 重建）或 Windows 上的崩溃、前端报错。
