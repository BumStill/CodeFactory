# CodeFactory 锁屏无关 Agent Runtime 证据

## 结论

- status: `passed`
- proof tier: `agent-runtime-no-gui`
- macOS runtime state: `CGSSessionScreenIsLocked=Yes`
- provider: `deepseek`
- model: `deepseek-v4-pro`
- fixture: `/private/tmp/codefactory-product-eval-member-scan-v1`
- fixture type: 普通 Python 源码包，不是 benchmark task

这次运行证明锁屏不会再阻断 CodeFactory 的真实 provider 请求、共享 Agent core、真实工作目录命令和完成门禁。Tauri 可见性由独立的远端 macOS App 窗口/截图门禁证明，不再等待本机解锁。

## 闭环

第一次锁屏运行完成两处源码修复、零残留和 `4 / 4` 测试，但最终 evidence 为 `failed`。原因是共享门禁只接受 `exit "$grep_rc"` 形式，误拒绝了“保存搜索状态、`>1` 时明确 `exit 2`、最后 `test ! -s`”的等价可靠扫描，导致 30 个模型请求耗尽。

加入等价退出语义的失败回归并修复后，第二次仍在锁屏状态运行：

- 两处旧 API 修改完成。
- `last_source_mutation_sequence=11`。
- `last_source_scan_sequence=13`。
- `last_successful_project_test_sequence=13`。
- completion blockers: `[]`。
- tool calls: `11`。
- model requests: `12`。
- duration: `42437 ms`。
- project tests: `4 passed`。

完成证据满足后，Rust runtime 使用无工具 finalization round 收口，没有继续修改工作区。

## 独立复核

- 独立状态保留型 `grep` + `test ! -s` 零残留检查通过。
- 独立 `.venv/bin/python -m pytest -q` 为 `4 passed`。
- 复核结束时 `CGSSessionScreenIsLocked=Yes`。
- 证据目录扫描未发现 `api_key`、`CODEFACTORY_AGENT_API_KEY` 或 secret-shaped value。

## 完整性

- execution contract SHA-256: `67f7c4be4c2913e51189b9aea01055f6d091330638671f6826446abc31cd0ef3`
- result SHA-256: `fa6b221661d6e2d3648f3b258c2b2676709bf790c46f427e2676c6af6812ae47`
- trajectory SHA-256: `b74eb040c6fe4a716f9b43ff5f189c933f74e456381f96a0fe2b31b9689e5953`
- trajectory: `22` JSONL rows，`14042` bytes

## 剩余边界

- OS credential 第一次授权仍可能要求用户在解锁状态确认；已授权的 credential 和运行中的 Runtime 不依赖屏幕。
- 用户主动锁屏不会被绕过。
- 本地锁屏时不执行辅助功能点击；PR 的精确候选 App 和发布 DMG 改由 GitHub macOS 可见会话生成 PID 绑定的窗口元数据和非空截图。
- 新远端门禁只有在 PR workflow 和 release workflow 实际通过后，才能作为 `remote-real-app-gui` / `released-artifact-gui` 证据；本地脚本通过不等于远端已通过。

## 当前候选锁屏复跑

独立审查后，当前候选加入三项通用加固并再次在真实锁屏状态运行：

- 产品 Runtime 使用独立 `ProductPolicy`，不再继承 benchmark 对 `/tests/`、`/solution/` 的专用限制。
- macOS 工具命令使用工作区写入 sandbox；独立协议测试中的 `touch ../escaped.txt` 实测返回 `Operation not permitted`，工作区外文件未创建。
- 子进程环境改为白名单，证据在回传与落盘前统一脱敏；自定义 endpoint 的合法 `owner/model` ID 不再被错误截断。

当前候选的 DeepSeek canary 在开始与结束时均记录 `CGSSessionScreenIsLocked=Yes`：

- status: `passed`
- completion blockers: `[]`
- tool calls: `10`
- model requests: `12`
- duration: `64777 ms`
- independent residual scan: zero matches
- independent project tests: `4 passed`
- workspace write isolation: `macos-sandbox-exec`
- result SHA-256: `59d3203c94b747d9876608c7d8aab46309474955542ddf04a14992a4e9c69525`
- trajectory SHA-256: `e8a51db26699ebc6e97ee88c270934a33b800cc1df6f782c9513ea5ff8cd0d17`
- trajectory: `20` JSONL rows，`12034` bytes

本次 trajectory 中 `cd /home/user` 的失败原因是路径不存在，不能作为 sandbox 拒绝证据；sandbox 写边界只引用上面的独立真实命令测试。该结果仍是 `agent-runtime-no-gui`，不是 release artifact 或 `real-app-gui` 证据。

## 最新锁屏收敛复跑

同一天的真实 App 旧会话重试先暴露最终 provider payload 仍可能在上下文压缩后形成缺失工具结果。候选现在在每次模型请求前执行最终协议修复：补齐缺失结果、保留匹配结果、丢弃孤立结果；三类 provider payload 回归测试通过。

随后锁屏 Runtime 复跑又依次暴露三个通用产品缺陷，并均以失败回归修复：

- Python 3.14 的 `TimeoutExpired.stdout/stderr` 可能是 bytes，脱敏函数此前会抛 `TypeError`；现在先统一安全解码，超时作为工具失败回传。
- 安装、外部运行和项目测试塞进同一命令时，结构化门禁无法证明阶段顺序；源码交付任务现在始终要求三个独立工具调用。
- `.venv/bin/python -m pip install --no-index --no-build-isolation --no-deps -e .` 此类正常参数顺序此前不被识别为源码安装；现在按 pip 调用结构和本地目标识别，而不是依赖固定字面量。

干净失败基线为 `4 failed`。修复后的真实 DeepSeek canary 在开始和结束时均为 `CGSSessionScreenIsLocked=Yes`，并得到：

- status: `passed`
- completion blockers: `[]`
- source mutation / scan / install / external runtime / project tests sequence: `8 / 10 / 17 / 18 / 19`
- tool calls: `17`
- model requests: `20`
- duration: `114922 ms`
- independent residual scan: zero matches
- independent source install: passed
- independent outside-source result: `9`
- independent project tests: `4 passed`
- execution contract SHA-256: `43f85a2669c4abca93cb7a4e381b9be99250faed0b81b9d9d477f929f61a9686`
- result SHA-256: `8ac43b40b7c5d134bc3b34e63cc3a85bfee51898077f1528b423d1d3a5a400da`
- trajectory SHA-256: `8f8bad5161f0236be6cb483f44bf4cbd219d0fc46fd0ae6707cf79acbda11d49`
- trajectory: `34` JSONL rows, `22690` bytes

本地远端门禁脚本预演启动了精确 debug App，绑定实际 PID 观察到 `1200x800`、layer `0`、alpha `1` 的稳定窗口，并生成 `680 KB` 非空截图。候选随后合入最新 `v1.44.0` 主干并再次完成可追踪的 debug App 构建，进程退出码为 `0`，当前 App 可执行文件 SHA-256 为 `f216040c205acfc32fa5031456a0c4ff16f81a6d84189ba3c0acb85600426ecd`。本地预演证明脚本可执行；精确当前候选的 GitHub-hosted macOS 窗口与截图结果仍必须由 PR CI 给出。

PR `#106` 的第一次 GitHub-hosted canary `29395835212` 在结构断言上返回成功，但下载并目视复核的 `window.png` 显示 WebView 为白屏。原因是窗口截图包含每边约 `56` 像素的黑色阴影区，旧内容采样仍被阴影颜色干扰。该次绿色 check 不作为 GUI 通过证据。门禁随后改为根据逻辑窗口尺寸推导 scale 与阴影，只采样真实窗口中央内容，并在最多 `20` 秒内等待 WebView 绘制。旧远端白屏按新算法为 `1` 个颜色桶、`0 / 625` 非主色采样，必定失败；本地真实 onboarding 截图为 `10` 个颜色桶、`208 / 576` 非主色采样。修复后的 hosted canary 才能决定 `remote-real-app-gui` 是否成立。

第二次 hosted canary `29396893121` 正确阻止了候选，但在 WebView 等待期间一次 `screencapture` 返回非零后提前退出，未跑满有界重试且没有上传诊断文件。脚本现在把单次捕获失败视为可重试观察错误，最多继续 `20` 次；最终仍失败时写入 `failure.json` 并保留 `window-last.png`，CI 继续失败但证据 artifact 可用于定位。该基础设施修复后的第三次 canary 仍需通过。
