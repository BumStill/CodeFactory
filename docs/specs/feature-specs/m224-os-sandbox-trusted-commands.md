# M224：信任模式改用系统沙箱，取代「读命令文字判断范围」

## 现场
- `trusted.git.unproved`（`src-tauri/src/agent/permission_gateway.rs`）靠解析命令文字证明访问范围，误报不断：
  - 11-04 02:47：会话在自己工作区 `git stash push -u; git fetch; git merge origin/main`。
  - 11-04 02:55：不含 git 的只读 `grep`。
  - 11-04 03:55：heredoc 里的 Rust 字符串 "/tmp/{library_id}" 被当成 python 要访问的路径。
  - 11-04 20:5x：47236186 在自己工作区 `git fetch && git merge origin/main && git config core.hooksPath .githooks`。
  - 还有：自己 PR 的 `gh pr edit`。
- 反过来，文字扫描也挡不住真正的越界：脚本里拼出来的路径它看不见。
- 11-04 03:17：`git commit --no-verify` 只是碰巧因误报进了审批才被人拒掉。门禁绕过应当由网关直接拒绝。

## 用户决定（2026-11-04）
用户选择改成操作系统沙箱，不再继续补文字规则，也不改成「只拦危险清单」。分阶段上线。

## 业界做法（参考，实现前以官方文档和 openai/codex 源码核实）
- macOS：Seatbelt，通过 `/usr/bin/sandbox-exec`，系统自带。
- Linux：Landlock + seccomp，或 bubblewrap。
- Windows：受限令牌 + 文件 ACL；Codex 有独立的 windows-sandbox crate，区分提权和非提权两种模式。

## 决定
- 边界由操作系统执行，不靠文字推断。文字解析只用于判断「这是不是远端或门禁动作」，不再用于证明文件访问范围。
- 只作用于信任模式的 bash 工具。safe/standard 模式行为不变。
- 本单只做阶段 1（macOS）和跨平台接口。Windows 是阶段 2，另开单；在它上线前，Windows 行为保持现状。

## 需求
- R1 接口：抽出跨平台的 `CommandSandbox`：输入 cwd、可写路径集合、网络策略，输出受限的启动方式。bash 工具的所有启动路径（前台、后台任务）都要经过它，不得留旁路。
- R2 macOS 实现：用 Seatbelt 启动命令，策略为全盘可读、写入只允许 R3 的路径。子进程继承限制。
- R3 可写范围：本会话已登记的工作区、该工作区 git 需要写的路径（含 `git_common_dir` 下的 worktree 元数据、refs、objects）、系统临时目录、`.codefactory-cache` 编译缓存、工具链缓存（如 `~/.cargo`、pnpm store）。清单写成数据，放一处。主 checkout 的工作树文件不可写。凭据目录（`~/.ssh`、钥匙串、应用数据目录、`settings.json`）不可写；读取限制在 Implementation Notes 里写明能否做到，做不到要说清楚。
- R4 网络：阶段 1 保持可联网（git fetch、cargo、pnpm 都需要），只记录这一点是已知边界，不声称已隔离网络。
- R5 审批改动：沙箱生效时，本地命令不再因 `trusted.git.unproved` 进入审批。上面的样本全部免审批执行。远端动作继续按 M130 的两层权限判定（会话级允许交付）。
- R6 门禁绕过：`--no-verify`、`CODEFACTORY_SKIP_SYNC_GATE`、修改 `core.hooksPath` 指向 `.githooks` 以外等，由网关直接拒绝，并在工具结果里给修复指引（如先合并最新 main）。不进审批。
- R7 失败要关闭：沙箱不可用（`sandbox-exec` 缺失、策略加载失败）时退回现有审批路径，不得静默无沙箱执行。启动时探测一次，状态可查。
- R8 越界体验：命令被沙箱拒绝时，工具结果要说明是沙箱拦的、拦了哪个路径、该怎么办，模型能据此改路径；不许变成看不懂的 `Operation not permitted`，也不许自动提权重试。
- R9 回退开关：提供内部设置关闭沙箱、回到旧路径，默认开启。这是回滚手段，不是用户界面选项。
- R10 兼容：真实工作流必须在沙箱内跑通：`cargo test`、`pnpm test`、`pnpm build`、`git commit`（含 pre-commit hook）、`git merge`、worktree 操作、后台任务、Playwright 或 Chrome 类验证。不能跑通的列出来并给出处理办法，不许放宽成全盘可写。

## 测试矩阵（先看到失败）
- T1（R2/R3）：真实 `sandbox-exec`：写工作区成功；写主 checkout、`~/.ssh`、工作区外路径失败；子进程写同样失败；脚本里拼接的路径也被拦（文字扫描做不到的用例）。
- T2（R5）：四条现场样本逐字回放，不产生审批且执行成功。
- T3（R6）：绕过类命令被直接拒绝，不产生审批，结果含修复指引。
- T4（R7）：模拟沙箱不可用，走审批路径，绝不裸跑。
- T5（R1）：前台和后台启动都带沙箱（断言实际进程，不只断言构造参数）。
- T6（R8）：越界失败的工具结果包含路径和说明。
- `cfg(target_os = "macos")` 的测试 CI 上要有 macOS job 真正执行；如果当前 required checks 不跑，必须在 PR 里写明并补上，不算绿。

## Harness
Spec、AI Collaboration、Compatibility（权限行为变化）；治理改动风险按 quick-profile 判定，场景 ID 以 `validate_scenario_test_governance.py --ci` 打印为准。这是安全边界改动：规格有错或做不到，停下来报告，不许自行放宽。

## 验收（真机，必须）
- 新版信任模式真实会话：跑完一单（改代码、测试、提交、同步 main、开 PR），全程 `trusted.git.unproved` 审批为 0。
- 同一会话让它尝试写工作区外文件：被系统拦下，模型能看懂原因。
- 关掉沙箱开关后回到旧行为，再打开恢复。
