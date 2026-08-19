# 浏览器接入零手工准备规格

CodeFactory 有两条读取网页的路径：**用户自己的浏览器**（扩展）和 **CodeFactory 自带浏览器**（受管 Chrome for Testing 下载）。
本规格约束的是这两条路径的 *准备过程*——即用户从「想让 agent 读网页」到「真的能读」之间必须付出的动作，
以及 Windows 上安装到最后一步因权限失败时系统必须如何自愈。

页面读取语义、注入脚本、权限提示与会话隔离仍归 `on-demand-embedded-browser-pane.md` 与 `src-tauri/src/browser/policy.rs`、
`profile.rs`；本规格不重定义它们。

## 背景问题

| 现象 | 根因 |
| --- | --- |
| Windows 上浏览器下载「最后因为权限失败」 | 安装只有一个固定根目录、下载前不验证可写；最终把解压目录 rename 到位这一步在 Windows 上会被杀软句柄、Controlled Folder Access、重定向的 AppData 直接拒绝（`os error 5`），且旧目录删除失败被静默忽略 |
| 扩展让用户操作太多 | 需要仓库 checkout 跑 `pnpm ext:build`、手动 load unpacked、再把端口和配对码抄进扩展；而 token 每次进程启动重新生成、端口是随机端口、bridge 只在打开设置页时才启动——等于每次重启都要重新配对一次 |
| 已配对好的扩展「装过但 App 不再认识」 | 配对文件是全机唯一、后写覆盖先写。第二个实例（开发构建与安装版并存是常态）发现固定端口被占用后回退随机端口，并把这个端口写进 `bridge.json` 与扩展目录；该进程退出后配对指向无人监听的端口，而扩展只会拨这一个地址，于是固定端口上的安装版再也不会被找到 |

## Requirements Traceability

| Req ID | 要求 | Surface | 验证 |
| --- | --- | --- | --- |
| CF-BP-R1 | 受管浏览器安装根目录是有序候选列表（Windows: LocalAppData → 用户目录 → TEMP），不是单一路径 | `browser::install` | unit |
| CF-BP-R2 | 下载开始前必须真实探测目标目录可写，且探测包含**目录 rename**（真正会失败的那个操作），不可写要在下载前给出列出所有尝试路径的可执行说明 | `browser::install` | unit |
| CF-BP-R3 | 安装状态检测必须扫描所有候选根目录；落在回退目录的完整安装必须被认成「已安装」，不得再次索要 150 MB 下载 | `browser::install` + `browser_chromium_status` | unit + e2e |
| CF-BP-R4 | 归档文件写在已验证可写的安装根目录内（不用系统 `%TEMP%`），并清理超过 6 小时的遗留下载，但不得删除其它窗口正在进行的下载 | `browser::download` | unit |
| CF-BP-R5 | 最终移动到位必须：退避重试、先把旧安装 rename 挪走再删、清 read-only、rename 持续被拒时降级为递归复制 | `browser::download` | unit + e2e |
| CF-BP-R6 | 目录级失败（权限形态）必须用**已下载的归档**在下一个候选根目录重试安装，不得让用户重新下载 | `browser::download` | e2e |
| CF-BP-R7 | 权限类失败的错误文案必须点明杀软/受控文件夹访问/AppData 重定向/受管浏览器仍在运行，并带上具体路径；非权限失败（坏 zip、缺可执行文件）不得跨目录重试 | `browser::download` | unit |
| CF-BP-R8 | 解压与移动在阻塞线程上执行，不占用 async worker | `browser::download` | 代码结构 |
| CF-BP-R9 | 扩展全部文件编译进二进制（`include_str!`），安装版用户无需仓库 checkout 或构建命令即可获得可加载扩展；`page.js` 仍是桌面端注入的同一份 | `browser::extension_package` | unit |
| CF-BP-R10 | 扩展落盘目录按用户固定（不随版本变化），使 Chrome 推导出的 unpacked extension ID 跨重启、跨升级稳定 | `browser::extension_package` | unit |
| CF-BP-R11 | 配对信息由 App 写入扩展自身目录的 `pairing.json`，扩展自行读取；正常路径下用户不需要复制端口或配对码 | `extension/background.js` + `browser::extension_package` | unit + 真实浏览器 e2e |
| CF-BP-R12 | 配对 token 跨重启持久化；损坏或被手改的配对文件必须重新生成 token，不得把空/短值当成期望 token | `browser::extension` | unit |
| CF-BP-R13 | bridge 优先绑定固定端口，被占用时回退随机端口；持有固定端口的实例是已发布配对的唯一所有者，回退到随机端口的实例只有在固定端口与已发布端口都无人应答时才写 `bridge.json` 与 `pairing.json` | `browser::extension` | unit + e2e |
| CF-BP-R18 | 扩展对同一份配对至少尝试两个地址：记录端口优先、固定端口兜底，失败后交替。单一记录端口失效不得成为「必须重新配对」的终点 | `extension/background.js` | unit |
| CF-BP-R14 | bridge 在 App 启动时就监听，不依赖用户打开设置页 | `lib.rs` setup | 代码结构 |
| CF-BP-R15 | 扩展 service worker 每次连接都重读 `pairing.json`（`cache: no-store`），不使用缓存值；文件不可用时回退到手工填写的值，手工值不得覆盖 App 写入的实时值 | `extension/background.js` | unit |
| CF-BP-R16 | 设置页一键完成「准备扩展」，并提供打开 Chrome 扩展页、打开扩展目录、复制目录路径；已连接后不再展示安装步骤 | `SettingsPage` | component |
| CF-BP-R17 | 手动配对入口保留但折叠，用于 App 无法写入的扩展副本（商店安装） | `SettingsPage` + `extension/options.*` | component |

## Primary User Path

**扩展路径（用户自己的浏览器）**
1. 用户打开「设置 → 浏览器」。App 立即把扩展写到固定用户目录，并把当前端口与配对码写进该目录的 `pairing.json`。
2. 用户点「打开 Chrome 扩展页」，开启开发者模式，点「加载已解压的扩展程序」，选中面板上显示（可一键复制/一键打开）的目录。
3. 扩展 service worker 读取 `pairing.json`，自行连上 loopback bridge。面板状态变为「已连接」。**全程不需要输入任何值。**
4. 之后每次重启 CodeFactory：App 启动即监听并刷新 `pairing.json`，扩展在 keepalive 周期内自动重连。用户无动作。

**受管浏览器路径**
1. 用户在同一面板点「下载浏览器」。
2. App 先选出真正可写的目录（含目录 rename 探测），再下载约 150 MB，解压到暂存目录，移动到位，最后写版本标记。
3. 如果某个目录在移动阶段被拒（Windows 杀软/权限），App 用已下载的归档换下一个候选目录继续安装，不重新下载。
4. 完全没有可写目录时，报错列出所有尝试过的路径与解除办法。

## Applicable Harnesses

- Spec Harness：本文件 + 单元测试。
- Compatibility Harness：安装根目录顺序改变后，旧位置（`~/.codefactory/browser/chromium`）的既有安装必须仍被识别（CF-BP-R3）。
- Payload Harness：约 150 MB 归档的落盘位置、断点遗留清理（CF-BP-R4）。
- Observation Harness：权限失败文案必须可执行；跨目录重试要留 `tracing::warn` 记录。
- AI Collaboration Harness：见下方 collaboration 记录。

## 测试矩阵

| 场景 | 层级 | 位置 |
| --- | --- | --- |
| 候选根目录顺序、去重、回退存在 | unit | `browser::install::tests` |
| 不可写目录在下载前被跳过；全部不可写时列出路径 | unit | `browser::install::tests` |
| 探测不留残留文件 | unit | `browser::install::tests` |
| 回退目录里的安装被识别；完整安装优先于损坏安装 | unit | `browser::install::tests` |
| 旧安装先挪走再删；rename 被拒时复制降级 | unit | `browser::download::tests` |
| 权限形态错误标记为可换目录重试，坏 zip / 缺可执行文件不换 | unit | `browser::download::tests` |
| 归档落在安装根目录；陈旧下载被清、进行中的不动 | unit | `browser::download::tests` |
| 缺可执行文件不写版本标记（杀软删 chrome.exe 形态） | unit | `browser::download::tests` |
| 完整安装链路（index → 下载 → 解压 → 移动 → 标记 → detect），本地服务器，无网络 | e2e | `browser::download::end_to_end` |
| 第一个目录不可用时改用回退目录且只下载一次 | e2e | `browser::download::end_to_end` |
| 已安装不再重复下载 | e2e | `browser::download::end_to_end` |
| 上次中断的残留不与新安装合并 | e2e | `browser::download::end_to_end` |
| 扩展文件齐全、`page.js` 与桌面端同源、重复准备不改动未变文件 | unit | `browser::extension_package::tests` |
| token 持久化；损坏配对文件被拒 | unit | `browser::extension::tests` |
| 扩展从 `pairing.json` 取配对、不用缓存、无文件时回退手工值、手工值不覆盖实时值、坏文件不拨号 | unit | `src/lib/browserExtension.test.ts` |
| 设置页调用 prepare、不再出现 `pnpm ext:build`、复制路径、打开扩展页/目录、无 Chrome 时的说明、已连接后收起步骤 | component | `SettingsPage.browser.test.tsx` |
| 真实 Chrome 里仅靠 `pairing.json` 完成握手并读到真实页面 | 真实浏览器 e2e | `scripts/verify-extension-bridge.mjs` |

## Evidence Pack Requirements

- 不接受「UI 显示已连接」「命令返回 200」作为完成证据。扩展路径必须有真实浏览器握手记录（`chrome-extension://…` origin + 真实页面抽取内容）。
- 受管浏览器路径必须有安装后 `install::detect` 认可、且二进制可执行的记录。
- Windows 权限行为若未在 Windows 实机验证，必须在交付说明中显式写出未验证场景与原因，不得默认通过。

## 兼容性与发布边界

- `~/.codefactory/browser/chromium` 仍是非 Windows 首选、Windows 次选，旧安装不需要迁移。
- `bridge.json` 保留原有字段，新增读取语义（token 复用）；旧文件可直接沿用。
- 扩展 manifest 版本升到 `0.2.0`；协议版本仍为 1，旧扩展仍能用手工配对连上。
- `pairing.json` 只写入 App 自己落盘的扩展目录；商店安装路径不受影响。

## AI Collaboration 记录

- Context scope：`src-tauri/src/browser/{install,download,extension,extension_package}.rs`、`extension/*`、`SettingsPage.tsx` 浏览器面板、`scripts/verify-extension-bridge.mjs`。
- Assumptions：Windows 侧失败为权限/句柄形态（`os error 5/32`），依据是「下载走完、最后一步失败」这一现象；Chrome 不提供无管理员权限的程序化安装 unpacked 扩展途径，因此「加载已解压」这一步只能压缩到一次点击，不能消除。
- Review point：安装根目录改成多候选后，`detect` 必须同时扫描全部候选，否则会出现「装好了却说没装」；`pairing.json` 内含 capability token，必须限制为 owner-only 权限。
- Validation result：Rust `browser::` 103 passed / 1 ignored；前端 61 passed；真实浏览器 `verify-extension-bridge.mjs` 通过（`chrome-extension://…` 握手 + 抽取真实页面）。Windows 实机未验证，原因见交付说明。
