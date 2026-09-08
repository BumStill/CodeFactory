# Scenario World 桌面安全隔离：最小切片

## 范围与证据边界

对应 CF-STG-R8 / CF-STG-R29 / CF-STG-R30 的 `isolated_app_data` 前置能力。此次只允许真实 Tauri / WebView 启动、真实设置中的主题保存与重开、composer 输入但不发送。不提升 registry 状态，不更改 judge、workflow、ruleset、main.rs 或 canonical driver；不声称 OS 沙箱或完整 E2E。

本轮不启动桌面、不访问真实凭据。实现先交独立安全审查，审查后才能由外部 supervisor 创建临时世界并运行 L3 feasibility probe。

## 小设计

1. `lib::run` 第一阶段生成 Tauri context，随即验证并固定 `Normal / Synthetic / Rejected`；必须早于 Builder、插件和任何 WebView。重复初始化把状态锁为 Rejected。环境变量只是请求，缺失、部分请求或校验失败均不回到 Normal。
2. 首片仅支持 macOS 的合成启动。其他平台保留 Normal；Synthetic 请求明确拒绝，不能用未审查的 Windows 路径语义充数。运行时 manifest 的 schema/run/owner 必须精确匹配，identifier 固定为 `com.codefactory.scenario.<run UUID compact>`。世界为规范化 `/tmp` 下直接子目录；HOME、config/data/cache/tmp 和 app 子目录必须已经存在，属于当前用户、无组/其他写权限，不得落在 OS 用户根内。
3. 不信任单个环境 flag 或 manifest 自报路径：比较实际 HOME、dirs config/data/cache 与预定布局；逐组件拒绝软链接，记录目录、manifest、owner 的文件身份，每次设置操作重新检查，检测替换和 manifest 改写；普通/发布/旧配置绝不探测。此为同用户进程防误触，不保证抵御恶意同 UID 的检查与使用竞态。
4. Synthetic 走独立但真实 Tauri Builder，不运行普通 setup，所以不创建数据库、恢复监督器、MCP、终端、BRIDGE 或认证观察器。不注册 shell/dialog/updater/process 插件；重建空 runtime ACL，仅允许本片需要的应用主题 core 命令。普通 invoke 只接受 `get_settings` 与 theme-only `save_settings`，其他包括 catalog、模型、gh、OAuth、工具、聊天发送全部拒绝。采用 incognito WebView，禁用 asset protocol 和外部页面导航，限制 CSP；不把 macOS `data_directory` 当隔离证明。
5. Synthetic secrets 的 get 返回空、set/delete 拒绝，并且在 keyring/fallback/legacy 入口之前判断。Normal 以注入 fake 存储验证委托兼容，不运行真实 Keychain。
6. Synthetic settings 为独立合成默认值（无 endpoint/key_ref、无 hook/MCP/git remote、onboarded=true），只持久化 schema + theme。坏文件、链接或路径替换必须报错，不回退生产/旧目录或普通默认值。父 supervisor 持有世界及 owner marker，正常退出、失败和 hard-kill 的世界回收属于后续 probe；本切片不自行递归清理用户路径。

现有 CLI smoke 可能不进入 `lib::run`：尚未初始化且完全没有世界请求时保留 Normal 委托；若在完整校验前发生存储访问且任一世界环境字段出现，就锁为 Rejected，之后移除变量也不解锁。测试覆盖所有部分请求组合。正确的 Synthetic 启动必须先完成校验，再访问存储；初始化后不再按环境 flag 切换模式。Synthetic 只接受嵌入式前端资源，缺少 `index.html` 时在 Builder 前拒绝，禁止连接普通开发服务器；后续 probe 必须构建独立 identifier 并内嵌当前前端。

## 失败优先验收

CLI 边界补充：`lib.rs` 的八个非保护 smoke（包括 worker 分支）匹配自己的 flag 后、runtime/IO 前集中拒绝世界请求，防止 Chrome attach 在进入桌面前初始化 BRIDGE。受保护的 `unattended_smoke_cli` 仍由原 canonical driver 执行其独立 hermetic smoke，不改变它的入口合同，也不授予其 Synthetic 桌面身份；本切片不宣称拦截所有 canonical CLI。若后续要求所有 CLI 都拒绝世界请求，需要另行批准受保护入口变更。

| 层 | 失败场景 | 通过要求 |
| --- | --- | --- |
| 源委托合同 | 校验晚于 Builder；普通 setup/插件进入 Synthetic；凭据护栏不在真实存储之前 | 现有代码先失败，修复后通过 |
| 隔离单元 | 错 identifier/schema/run/owner、部分请求、HOME/config/cache 不符、链接/替换、重复初始化 | 拒绝且不能回退 Normal |
| 存储单元 | Synthetic 读写密钥、Normal fake 委托、坏设置文件和未知字段 | 不触真实存储；主题可保存/重载；坏数据拒绝 |
| 权限单元 | plugin 命令和未审查 IPC、远程导航 | 默认拒绝，只有明确审查的主题能力开放 |
| 静态/构建 | 本地编译、合同、typecheck、治理门禁 | 不启动真实 App；保留后续 L3/L4 缺口 |

## 审查点

原生能力边界：插件 ACL 不是 WebKit/OS 权限沙箱。此片仅允许固定合成文本键入，不粘贴、不拖放、不选文件、不使用媒体设备。WebView 显式拒绝新窗口和下载，初始化脚本在捕获阶段拦截 DOM paste/drop、相关 beforeinput 和 file input 点击；不读取剪贴板内容。`disable_drag_drop_handler` 实际会允许 HTML5 drop，故不把该 API 当作拒绝开关。后续若验收附件、剪贴板或媒体，必须额外审查原生与前端入口，当前检查不计为这些能力已经隔离。

构建边界：本仓库没有根 `custom-protocol` feature。后续构建应先通过锁定版本验证 `tauri/custom-protocol` 或命令级 `TAURI_CONFIG` 的 `build.devUrl=null` 与独立 identifier；不得改 Cargo/tauri.conf 可信全局输入来便利测试。本轮只核对代码中的资产条件，不创建或启动真实桌面。

上下文范围只有本工作树；normal 行为不扩大，测试均使用随机临时目录或 fake 存储。重点审查 Tauri WebView 创建顺序、runtime ACL 与普通 invoke 两条路径、设置和凭据调用前的不可回退分支。实际验证结果在代码评审交接中逐项记录。

## 本轮局部验证（2026-09-08，未启动桌面）

- 失败优先：最初启动/凭据委托合同为 2 个断言失败与缺模块错误；独立审查新增 CLI 旁路合同先得到 8 个入口失败；未知 IPC 字段 Rust 测试先捕获原始 payload 被忽略的断言失败，再严格化处理。
- 通过：`cargo test --lib desktop_context` 11 项；`settings_persistence_tests` 3 项；`secrets::tests` 3 项（仅 fake 与临时文件，不调用 Keychain）；Python 委托及 DOM 输入拦截合同 6 项；unattended 入口合同 7 项。
- 通过：`cargo check --bin codefactory`、`pnpm exec tsc --noEmit`、governance baseline、本地 scenario gate、`git diff --check`。全部 Rust 命令经 `pnpm cargo:shared --`，不建立独占 target。
- 按当前 base 与 7 个实际文件的普通 PR 计划为 19 个 target（Mac 2、Windows 17），无 blocker、无 trust-root 文件交集；`Scenario-Test: ALL` change contract 无错误。此为计划，不是这些远程 target 已经执行。
- 独立安全审查推动修正了 CLI 旁路、严格原始 IPC JSON、原生能力边界文案。真实窗口启动、截图、主题 UI 保存/重开、composer 输入与进程清理尚无 L3 证据；等待安全复核和允许的桌面条件，不请求解锁或绕过系统权限。也不作 L4 安装包声明。
