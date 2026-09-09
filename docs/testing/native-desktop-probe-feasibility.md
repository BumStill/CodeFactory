# macOS 原生桌面探针：隔离前置合同

对应场景补全规格 M1 的桌面 feasibility probe。此文记录本地安全合同和只读原生权限预检；后续新增的 CI-only AX observer 见 [原生只读观察器设计](../design/native-desktop-observer-slice.md)。尚无真实 Tauri 点击、输入、重启或截图结果，不构成 L3/L4、完整 E2E 或场景门禁。没有替代产品的 AppKit 演示程序。

## 原始风险与当前边界

对 `5c25e49f` 源码的检查发现，独立 HOME 和临时 bundle identifier 不能隔离 CodeFactory 的共享 Keychain。不能直接复制或启动现有产品二进制进行测试。

本 PR 已加入 [Synthetic 产品隔离入口](../design/scenario-desktop-isolation.md)，并通过独立代码审查和本地合成测试。只有从本次源码构建且通过该入口核验的 App 才进入远端观察；旧二进制不适用。以下表格保留最初检查到的普通启动风险，不表示新增隔离入口仍无条件执行这些路径。真实桌面运行和完整 probe 的证据仍待取得。

| 路径 | 已确认的行为 |
| --- | --- |
| `src-tauri/src/lib.rs:1513` → `observe_chatgpt_auth_on_startup` → `codex_auth::load_tokens` | 原生启动无条件读取 ChatGPT credential；与 fixture 是否含 endpoint 无关 |
| `src/App.tsx` → `syncChatGptCatalog` → `codexAccount` → `current_account` | settings 加载后，前端会再次读取 credential；已登录时还会请求远程模型目录 |
| `SettingsPage` 的 `ChatGptLoginCard` → `codexAccount` | 进入相应设置页也会读取 credential |
| `ModelPicker` → `loadModels` → `commands/models.rs::list_models` | 非 ChatGPT endpoint 即使没有 `key_ref`，仍生成默认引用、读取 CredentialBroker 并请求远程 `/models` |
| `src-tauri/src/secrets.rs::get_key` | Keychain service 是编译期 `com.codefactory.app` / `com.codefactory.app.dev`；还会访问本地 fallback、legacy credential，必要时迁移 |
| `config/settings.rs::load` | debug 配置缺失时可能从 release 配置复制；legacy 配置也可能迁移。不能把“改了 bundle identifier”当成所有存储已隔离 |
| `UpdaterBanner` → updater 初始化 | 存在独立于模型请求的后台更新网络路径；无 endpoint 不等于无网络 |

这些结论来自源码检查。没有打开生产 DB、配置、凭据或运行中的 App，也没有实际触发上述命令。

## 最小隔离设计，本 PR 已实现并审核，待真实运行

1. 产品进程启动时先验证显式 probe 模式及单 run 所有权标记，再初始化 settings、DB、credential broker 或任何后台任务。缺失、非临时根、symlink、身份不匹配一律退出；不能回落到正常用户路径。
2. settings 与 Tauri app data 必须都定向到同一个专属 root。加载合成配置时禁止 release/legacy 迁移、已有 session 恢复和用户配置发现；fixture 的 endpoints、key refs、MCP、hooks、远程仓库均为空。
3. 在 secrets 的公共 get/set/delete 边界提供专属隔离 backend，进入任何 fallback/legacy/Keychain 代码前完成分流。单纯跳过 startup 一次调用不够，前端 account、模型列表及设置保存仍可再次到达 credential 路径。隔离模式不得读取生产值，写删只能拒绝或操作临时合成存储。
4. 对该本地探针明确禁止所有外部请求及自动后台副作用，包括模型目录、OAuth、更新、MCP 和恢复任务；通过独立测试证明，而非依赖没有发送聊天文字。具体隔离实现由后续真实产品切片决定。
5. 驱动记录实际创建的子进程及 OS birth token、规范化 executable path/digest、唯一 bundle identifier；每次 AX 操作和终止前重新验证。LaunchServices 返回另一个 bundle、PID 被复用或身份无法观察时，禁止向该进程发送终止信号。
6. 最终真实路径才是：原生 AX 打开主题设置并选择浅色 → 原生输入固定合成文本且不按发送 → 读取输入与主题投影 → 截取该 PID 的窗口 → 退出并确认回收 → 重开同一隔离 app、取得新 PID 和 birth token → 验证主题持久化并再次截图 → 清理并观察无残留。

## 本切片的可执行内容

### 与产品隔离分支共用的协议

已只读核对 `docs/design/scenario-desktop-isolation.md` 与 `desktop_context.rs` 的待交付设计，并统一以下约定；本探针不另定义产品存储布局：

```text
realpath("/tmp")/
  codefactory-scenario-<随机后缀>/
    manifest.json
    owner.json
    home/Library/
      Application Support/<identifier>/settings.json
      Caches/<identifier>/
    tmp/
    Probe.app/Contents/MacOS/codefactory
```

identifier 为 `com.codefactory.scenario.<run UUID 去掉连字符>`。root 必须是 canonical `/tmp` 的直接子目录，目录名不要求包含 run ID；不能用 `os.tmpdir()` 或其他环境决定的临时根替代产品协议。

- `manifest.json` 精确字段：`schema_version=1`、`run_id`、`owner_token`、`identifier`、`capabilities=["isolated_app_data"]`。
- `owner.json` 精确字段：`schema_version=1`、`run_id`、`owner_token`，必须与 manifest 及私有运行租约一致。
- HOME 为 `root/home`；macOS config/data 均为 `home/Library/Application Support`，cache 为 `home/Library/Caches`，TMPDIR 为 `root/tmp`；config/data 和 cache 下的 identifier 目录须预先存在。产品会核验实际目录解析结果，而非只信任声明。
- 请求字段为 `CODEFACTORY_SCENARIO_MANIFEST`、`CODEFACTORY_SCENARIO_RUN_ID`、`CODEFACTORY_SCENARIO_OWNER_TOKEN`，必须一起提供，并配合实际 HOME/TMPDIR。新增 CI-only supervisor 在未来启动的独立 child 环境中设置这些字段，不修改主机环境；本地尚未启动任何 App。

本探针额外要求 root 权限 0700、生成的 run/owner 使用 UUID v4，并固定 `Probe.app/Contents/MacOS/codefactory` 为 supervisor 自有 bundle 位置。这些是本探针的较严格约束；产品不要求目录名包含 run，也未把此 bundle 文件名定义成产品 API。每层目录以及 manifest、owner、executable 都检查类型、uid 和权限；文件还拒绝 symlink/hardlink。manifest/owner 以 no-follow 文件句柄限量读取并核对 device/inode。

私有运行身份包括 run ID、owner token、identifier、PID/birth token 与 executable digest；公开观测只保留 run/identifier/进程身份和摘要，`owner_token` 在任意层级出现都会被拒绝。未来 supervisor 必须实际观察子进程、AX 操作、持久状态、截图与清理，不能把进程自报的计数填进合同就当作证据。

- `scripts/native-desktop-probe-contract.mjs`：本地纯身份/观测合同与只读路径检查。不会 launch、signal、删除文件或接入 registry。`canStopOwnedProcess` 和 `cleanupEligible` 只表达必要条件，不是实际清理能力；未来任何删目录或发进程信号的适配器，必须在操作前重新核对完整进程身份与持有目录的 device/inode，并防止核验后路径被替换。历史 predicate 不能缓存为授权。
- `scripts/native-desktop-probe-contract.test.mjs`：合成 identity、临时目录和观测对象；覆盖 PID 复用、非 owned 进程、symlink/hardlink、越界路径、错误 owner、UUID/SHA 强制类型转换、PID 与计数边界、缺点击/输入/重启、跨 run 截图、隐私和清理失败。
- `scripts/probe-macos-native-preflight.swift`：只读查询 Accessibility、Screen Capture 和当前 GUI session 的布尔状态，不弹权限申请、不启动 App、不读取 AX 树、不抓屏、不改环境。该单独的历史预检始终输出 `blocked` 并退出 3，因为它本身不执行产品隔离和真实交互验收。GUI 解锁状态无法证明时同样保持 blocked。

纯合同允许的观测只含固定 synthetic input digest、run/executable 身份、计数、主题枚举、截图摘要和清理状态；不接受原始文本、绝对路径、endpoint、key reference 或未知字段。所有观测的真实性仍须由之后的原生驱动证明；合成合同测试通过不能证明真实桌面体验。

owner marker 和 executable 必须为当前用户持有的普通文件，`nlink == 1`；不接受目录、symlink 或 hardlink。UUID/SHA 只接受 primitive string。PID 必须是大于 1、至多 `2^31−1` 的 safe integer，匹配 Darwin SDK 的有符号 32 位 `pid_t` 表示范围；这不证明该 PID 实际存在，仍须做现场身份核验。所有计数与图片尺寸都要求非负 safe integer。

验证命令：

```sh
node --test scripts/native-desktop-probe-contract.test.mjs
xcrun swiftc -typecheck scripts/probe-macos-native-preflight.swift
```

审核后可单独执行只读权限预检；它不启动产品，退出 3 是预期的未完成状态：

```sh
swift scripts/probe-macos-native-preflight.swift --preflight
```

初始失败优先证据是测试先于实现新增，执行因合同模块尚不存在而退出 1；实现后原 9 项测试通过。独立审查后，新增 hardlink、严格字符串和安全整数边界测试，先观察 4 组失败，再修复为 15 项全部通过。协议同步先将合成 fixture 切换到产品布局，观察 5 组失败，再对齐合同并补 manifest/schema/目录替换测试，最终 19 项通过。新模块的初始 red/green 不宣称已经复现真实桌面故障。

主执行者于 2026-09-08 实际执行上述只读 preflight：退出码 3，`status=blocked`、`accessibility=true`、`screen_capture=true`、`gui_session=false`、`app_launch_count=0`。这里的 `gui_session=false` 表示本探针没有证实可用 GUI session，不能据此断言用户锁屏；原因仍包括真实交互驱动未启用与产品启动隔离未核验。没有启动真实 App，也没有权限变更。

## AI Collaboration

- context scope：M1 规格、原生窗口脚本、产品 startup/settings/credential 调用链。
- assumptions：OS birth token 和 executable digest 将由将来的原生进程观察器采集，不接受候选自报替代。
- review point：父执行者先审核隔离和清理设计；产品安全入口解决前不启用真实交互。
- validation result：以本切片合同测试和 Swift 编译结果为准；真实 Tauri feasibility 仍为 blocked。
