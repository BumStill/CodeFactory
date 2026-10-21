# M136：浏览器扩展（CodeFactory Browser Bridge）在 Windows 上很不稳定（用户 10-21 报告）

## 现状
- 扩展：MV3 service worker（extension/background.js，344 行），以 `ws://127.0.0.1:<port>` 连 CodeFactory；端口/令牌从扩展包内配对文件读取（失败回落到手填与固定端口 STABLE_PORT）；有指数退避重连与 30s keepalive 闹钟。
- 用户只说「Windows 下很不稳定」，无日志。真机会话里也多次出现 `browser_pairing_required`（「还没有连上你的浏览器…」）。
## 需求
- R1 先可观测：扩展与 App 两端记录连接事件（连上/断开及 close code/reason、重连尝试与端口、配对来源与令牌指纹（不记明文）、SW 被唤醒/挂起、App 端口变更）；App 的「设置 → 浏览器会话」显示当前连接状态与最近 50 条事件，并提供「复制诊断信息」按钮（不含令牌明文），方便用户把 Windows 上的情况发给我们。
- R2 加固（按业界 MV3 实践，逐项说明取舍）：
  - WebSocket 应用层心跳 ≤20s（Chrome 116+ 中 WebSocket 活动可让 SW 保活），App 侧对心跳超时主动关闭以触发重连；
  - SW 每次启动（onStartup/onInstalled/闹钟唤醒/任意事件）立即检查并重连；
  - App 重启/升级导致端口或令牌变化：扩展重读配对文件 + 固定端口回落；App 侧尽量保持固定端口与令牌跨重启稳定（令牌持久化），避免升级后扩展失配；
  - Windows 上解包加载的扩展目录：升级后配对文件是否更新——若扩展目录不随 App 升级，改为扩展通过固定端口的握手端点取配对（仅本机回环、需用户在 App 里确认一次），不依赖文件。
- R3 App 侧：扩展断开时，browser_session 的 attach 不应让会话卡住（已有 M131b 默认走受管通道）；断开时给出一句人话状态与「重新连接」按钮。
- R4 测试：扩展连接状态机单测（断开→退避→重连、端口切换、令牌变化、心跳超时）；App 端 WebSocket 服务端心跳/超时单测；在 Windows CI（execute-windows）上跑一次真实 Chrome + 扩展的连接-断开-重连 smoke（若 CI 不可行，PR 写明替代证据与需要用户在 Windows 上回填的场景）。
## 约束
- 安全边界不变（扩展只读用户已打开的页、令牌不外泄、仅回环地址）。授权交付（ceiling: merged），PR 标题写明内容，正文带 `Release-Urgency: immediate`。不要超出规格自加范围。

## Implementation Notes
- Applicable Harnesses：Spec、Compatibility、Release、Observation、Payload、Viewport、AI Collaboration。
- R1：两端各自有界 50 条结构化事件；只允许固定字段，不采集页面内容或原始 wire frame。扩展重连后上传其事件快照，App 合并展示；断线期间事件保存在扩展 storage。
- R2：20 秒心跳与 45 秒服务端无活动期限；闹钟仅作冷启动兜底，不承诺精确唤醒。保留旧协议兼容和多 profile standby。
- R2：现有安装目录固定且 App 启动会刷新 pairing.json；继续使用文件机制，不新增未认证令牌分发端点。复制到其他目录的安装仍需要手动配对。
- R3：重新连接由 App 主动关闭旧连接触发；后台入口沿用 dispatch 能力的约束，若无法接入必须在 PR 列明。
- R4：优先复用现有真实 Chrome 脚本，本任务未授权修改 workflow/registry 信任根；Windows 实际安装版验收由编排方发版后完成，不以本机测试替代。

## 间歇断连根因（用户 10-21 补充症状）

- 症状：连上后能用一阵，然后「自己断、过一会儿又自己回来」，期间工具调用偶发失败。
- 根因（两处叠加）：
  1. MV3 keepalive 闹钟周期为 `periodInMinutes: 1`，**大于** Chrome 对空闲 service worker 的 30s 挂起阈值。worker 被回收后，socket 早已失效却无人察觉；下一次闹钟唤醒最长要等 60s，这段空窗正是「间歇断连」。
  2. App 侧对 45s 无活动连接主动关闭（`m136_silent_peer_is_closed_and_diagnostics_are_redacted`）。worker 处于挂起态时 20s 心跳不再产生，超过 45s 即被服务端回收，扩展要等闹钟唤醒才重连。
- 修复：闹钟降到 Chrome 允许的最小周期 `0.5` 分钟，并对已存在的旧 1 分钟闹钟重建；Chrome 120 以前拒绝低于 1 分钟的周期，`try/catch` 回落而不是丢掉闹钟。20s 心跳（Chrome 116+ 中 WebSocket 活动可延长 worker 生命）仍是主保活手段，闹钟只做冷启动兜底。回归测试：`src/lib/browserExtension.test.ts`「keeps the keepalive alarm inside Chrome's 30s idle window」。

## 30 分钟不掉线验证（可复现）

- 入口：`pnpm verify:extension:soak`（等价 `node scripts/verify-extension-bridge.mjs --soak-minutes=30`）。
- 环境：真实 Chromium（managed/系统 Chrome）+ 未打包的真实扩展 + 真实 MV3 service worker，对接同一握手/Origin 规则的本机桥；不使用 mock。
- 判定（任一不满足即非零退出）：
  - 每 20s 至少一次心跳被服务端收到（整段心跳数 ≥ 时长期望的 80%）；
  - 每 5s 发一条 `list_tabs` 命令并必须成功（证明 socket 不只是「开着」，而是仍可服务）；
  - 最坏心跳间隔 < 45s（App 空闲回收阈值），超过即判定「worker 被挂起且未及时唤醒」；
  - 结束时输出 `SOAK PASS: <分钟数> continuous (...)`。
- 短时回归：`node scripts/verify-extension-bridge.mjs --soak-minutes=2` 在 PR gate 级别即可发现挂起（2 分钟内应 ≥5 次心跳）。

