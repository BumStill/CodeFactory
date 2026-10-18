# M119：受管浏览器启动的 Chrome 不再把 CodeFactory 当「责任进程」

## 原始任务规格（逐字保留）

### 证据（编排方 10-18 用系统日志实测）
- 23:10:37 与 23:15:23，tccd 记录：`requesting=com.google.Chrome (/Applications/Google Chrome.app)`，`responsible=com.codefactory.app`，服务 `kTCCServiceSystemPolicyAppBundles`，结果 `does not allow prompting for unentitled binaries; returning denied`，随即系统通知「已阻止 CodeFactory 修改 Mac 上的 App」。
- 原因：本机没装受管 Chrome for Testing，`src-tauri/src/browser/chromium.rs::executable()` 回落到 `/Applications/Google Chrome.app`；Chrome 由 CodeFactory 直接 fork/spawn（chromiumoxide 内部启动），macOS 把 Chrome 启动时对自身 app 包的写入归到「责任进程」CodeFactory 头上。CodeFactory 本身并不想改任何 App。
- 另外：TCC 里 CodeFactory 的旧授权记录 cdhash 与当前版本不匹配（ad-hoc 签名每次发版都变），所以即使用户手动开了「App 管理」，下次发版也会失效——「引导用户去开权限」不是根治办法。

### 需求
- R1（根治）macOS 上启动 Chrome/Chromium/Edge（受管浏览器会话、`browser_open_extensions_page` 等所有启动点）时**放弃责任归属**：用 `posix_spawn` + `responsibility_spawnattrs_setdisclaim(attr, 1)`（libSystem 私有符号，Terminal/VS Code/Chromium 自身都在用；用 `dlsym` 取，取不到就退回普通 spawn 并记一条日志，不失败）。chromiumoxide 若不能定制 spawn：改为自己启动 Chrome（带 `--remote-debugging-port`、`--user-data-dir` 等现有参数），再用 `Browser::connect` 连 DevTools 端点；保留现有超时、清理（会话结束必须杀掉进程、回收临时 profile）与「受管会话持有和回收」约束。
- R2 Windows/Linux 行为不变。
- R3 测试（先失败）：单测覆盖「macOS 启动参数构造包含 disclaim 路径 / dlsym 失败时回退」；真机验收由编排方做：开一个浏览器会话 → `log show --predicate 'process=="tccd" AND eventMessage CONTAINS "SystemPolicyAppBundles"'` 里不再出现 `responsible=com.codefactory.app` 的拒绝，且不再弹系统通知；浏览器会话照常可用、结束后无残留 Chrome 进程。
- R4（兜底，小）若今后 CodeFactory 自己确实需要改别的 App（目前没有）：写入 `/Applications/*.app` 遇到 EPERM 时，界面给一张卡片「需要在 系统设置 › 隐私与安全性 › App 管理 里允许 CodeFactory」+ 打开 `x-apple.systempreferences:com.apple.preference.security?Privacy_AppBundles` 的按钮；用户回到应用时自动重试一次，成功后按需提示重启。不自动改系统设置（macOS 也不允许）。本条可以只写规格+最小实现，不要扩大范围。

### 约束
- 合成 fixture，不写生产 DB。在规格文档（docs/specs/）补一节说明。授权交付（ceiling: merged）。

## Requirements Traceability
| Req ID | 实现边界 | 最小证据 |
| --- | --- | --- |
| R1 | macOS 独立 spawn seam + CDP connect；扩展页共用 seam；显式进程 owner | 真进程 spawn/回收测试、参数与 fallback 测试、生命周期 smoke |
| R2 | cfg 隔离，其他系统保留 Browser::launch 与现有扩展页行为 | 平台编译 / CI |
| R3 | 合成参数、隔离临时 profile；真机 TCC 检查由编排方发布后执行 | 失败优先记录、测试结果、编排方步骤 |
| R4 | 当前无修改第三方 App 的业务路径；只提供精确匹配的 EPERM 分类与系统设置 URL 常量作为最小后端接缝 | 分类单测；未来写入调用点必须接入卡片/一次重试，不能把普通浏览器启动错误当作需授权 |

## Applicable Harnesses
Spec、Compatibility、Release、Observation、Payload、AI Collaboration。无现有前端行为修改，不启用 Viewport；R4 完整卡片仅在未来实际 App 写入入口启用。

## Primary User Path / 测试矩阵
1. 开受管浏览器 → 读取页面内容/快照 → 结束 → 精确 Chrome PID 已退出、临时 profile 回收。
2. 启动失败 / CDP 连接超时 / 所有权检查失败 → owner drop 杀进程并回收，错误保留 binary 与超时语义。
3. 打开扩展管理页 → 通过相同 macOS disclaim spawn；不关闭用户自有浏览器。
4. dlsym 缺失或 setter 失败 → 日志 + 普通 posix_spawn，不因私有 API 缺失拒绝启动。
5. Windows/Linux 保持原有启动与清理路径。
6. R4 匹配 EPERM + `/Applications/<name>.app/...`，非此路径/其他错误不提示 App 管理；未来 UI 只自动重试一次、不修改系统权限。

## 合成 fixture 与生产隔离
所有新增测试只使用临时目录、合成命令/路径及本地合成页面。不访问或写入生产会话 DB，不拷贝真实用户会话、凭据或浏览器 profile。真实安装版的 TCC 日志、通知消失和残留进程验收由编排方发版安装后完成；本 PR 合并不声称已经 live。

## Implementation Notes
chromiumoxide 0.9.1 的进程参数和启动 hook 未公开，macOS 改为独立构造受管参数并连接既有 loopback CDP。进程 owner 必须在所有中途失败和 drop 时清理；连接成功仍保留 owner 而非把连接等同于进程所有权。私有符号只在 macOS 动态查找，无链接期依赖。
