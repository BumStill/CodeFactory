# M195：web_fetch 遵循系统代理/PAC，并识别「被网络安全策略拦截」页面

## 证据（10-29 用户 Windows 会话反馈 + 编排方核对 origin/main）
- 公司网络下 `web_fetch` 读公网页面（METR 官网）返回的是公司网关的「风险提示/拦截页」，模型只能绕道找别的来源。
- `src-tauri/src/tools/web_fetch.rs` `default_client()` 只用 reqwest 默认代理行为（环境变量；Windows 上不解析 PAC/WPAD 自动配置脚本）。企业 Windows 普遍用 PAC，浏览器能走的路由 web_fetch 走不了。

## 需求
- R1 遵循系统代理配置：Windows 读取 Internet Settings（含 PAC/WPAD 自动配置，按 URL 求值）、macOS 读取系统代理设置；环境变量仍优先。代理认证（NTLM/Kerberos）若暂不支持，返回明确原因。调研现成 crate（如 PAC 解析/系统代理库）并写进规格取舍。
- R2 识别拦截/插页：响应是已知网关拦截页特征（例如 200 但标题/正文为安全风险提示、跳转到公司网关域名、典型 block page 模式）时，不把它当正文返回，而是返回结构化错误 `BLOCKED_BY_NETWORK_POLICY`（附网关 host 与原 URL），人话说明「被你所在网络的安全策略拦截」。
- R3 不做绕过：不得尝试绕开企业安全策略（不换 DNS、不直连、不伪装 UA 去规避拦截）。拦截时给模型的建议仅限：换用其他公开来源，或若用户已在自己浏览器中能打开，可经附着浏览器只读读取（同样受公司策略约束）。
- R4 现有安全检查不变（本机/元数据地址拒绝、fake-IP 环境不误判）。

## Requirements Traceability

| Req ID | requirement | minimum evidence |
|---|---|---|
| R1 | web_fetch 按「环境变量 → 系统代理（含 PAC 按 URL 求值）→ 直连」选路；不支持的代理形态（SOCKS-only、需 NTLM/Kerberos 的 407）返回明确原因 | `m195_*` 代理选路/解析单元测试（合成 PAC、合成 `scutil --proxy` 输出、合成 `reg query` 输出）+ 实机 macOS `scutil` 解析冒烟测试 |
| R2 | 网关拦截页识别为 `BLOCKED_BY_NETWORK_POLICY`，错误里带网关 host 与原 URL | `m195_*block*` 测试：200+风险提示页、302→网关域、正常页面不误判 |
| R3 | 拦截时不绕过；建议仅限换来源 / 附着浏览器只读 | 错误文案断言（不含换 DNS、直连、伪装 UA 之类建议）+ 代码中无绕过分支 |
| R4 | 本机/元数据地址拒绝、fake-IP 不误判保持原样 | 既有 `literal_local_hosts_are_rejected_without_any_dns_lookup` / `ordinary_domains_are_not_ruled_out_by_address_guessing` / `a_rejected_host_never_reaches_the_network` 全绿 |

## Applicable Harnesses
- **Spec Harness**：本文件（Req ID ↔ 测试映射见上表与 PR body）。
- **Compatibility Harness**：Windows（`reg query` 文本解析）与 macOS（`scutil --proxy` 文本解析）两条平台路径都由平台无关的纯解析函数覆盖，CI 两个平台都能编到；Linux 无系统代理读取，保持 reqwest 环境变量行为。
- **Release Harness**：`deliver_changes`（ceiling: merged）→ 合并队列。
- **Observation Harness**：新增结构化错误码 `BLOCKED_BY_NETWORK_POLICY` 与代理不可用原因出现在工具结果文本中，属既有工具结果观测面。
- **AI Collaboration Harness**：见 PR body 的 AI Collaboration 记录。
- 不涉及 UI，故不启用 Viewport Harness；不涉及上传/对象存储，Payload Harness 不适用。

## 测试矩阵

| 场景 | 类型 | 期望 |
|---|---|---|
| PAC 按 URL 选 PROXY / DIRECT | 单元（合成 PAC） | 匹配的 host 走 PAC 选出的代理，内网 host 直连 |
| 环境变量优先 | 单元 | `HTTPS_PROXY` 存在时交给 reqwest 环境代理（含 `NO_PROXY` 语义），不覆盖系统 PAC |
| `NO_PROXY` 命中 | 单元 | 直连 |
| PAC 不可读/语法不支持 | 单元 | 回落到固定系统代理，没有固定代理则直连，且记录原因 |
| PAC 只给 SOCKS | 单元 | `Unsupported` + 明确原因 |
| macOS `scutil --proxy` 解析 | 单元（合成输出）+ 实机解析 | 提取 HTTP/HTTPS 代理、PAC URL、例外列表 |
| Windows `reg query` 解析 | 单元（合成输出） | 提取 AutoConfigURL / ProxyEnable / ProxyServer / ProxyOverride |
| 200 + 风险提示页 | 单元（合成 fixture） | `BLOCKED_BY_NETWORK_POLICY`，含网关 host 与原 URL |
| 302 → 网关域 | 单元（合成 fixture） | 同上，原 URL 保留请求地址 |
| 正常长文章含 security 字样 | 单元 | 不误判 |
| 本机/元数据地址 | 既有测试 | 拒绝且不发网络请求 |

## Implementation Notes

### 现成 crate 调研与取舍（R1）
- `boa_engine` / `rquickjs`（完整 JS 引擎跑 PAC）：PAC 是任意 JS，最通用；但等于给进程再塞一个 JS 运行时（数 MB + 编译时间 + 新的安全面），而 `web_fetch` 只需要一个只读的选路结果。**不选**。
- `proxycfg` / `system-proxy` / `proxy-pac` 一类 crate：多数只做「读系统代理」，PAC 求值要么缺失、要么长期无人维护，且对 `NO_PROXY`/例外列表的语义与 reqwest 不一致。**不选**，改为自己读同样的两处系统配置。
- `pac-v1`（PAC 解析器）：只解析脚本结构，不提供求值环境；仍需自写求值器。**不选**。
- 结论：**reqwest 保持环境变量语义（含 `NO_PROXY`），系统代理自己读，PAC 用一个只覆盖真实脚本子集的求值器**（`web_fetch.rs` 内的 `pac_eval` 模块）。

### PAC 求值器覆盖范围
支持：`if / else / return / {} / ;`、`|| && !`、`== != < > <= >=`、字符串/数字/布尔字面量、`shExpMatch`、`dnsDomainIs`、`isPlainHostName`、`localHostOrDomainIs`、`isInNet`（字面 IP 与系统解析）、`dnsResolve`、`isResolvable`、`host.toLowerCase()` / `.toUpperCase()` / `.indexOf()` / `.substring()` / `.substr()`。
不支持（遇到即 `Err`，走「PAC 不可用」回落并记录原因）：时间/日期类函数（`weekdayRange`、`dateRange`、`timeRange`）、`myIpAddress`、数组/循环/自定义函数。

### 代码落位

选路 / PAC 求值 / 拦截页识别都在 `src-tauri/src/tools/web_fetch.rs` 的私有模块 `net_policy`、`pac_eval`、`block_page` 内：`docs/testing/scenario-registry.json` 的 CAP-001 已登记该文件，而注册表属场景门禁信任根，普通 PR 不得自改（门禁会 fail closed），因此不新增独立文件。

### 平台读取方式
- macOS：`scutil --proxy`（系统当前生效配置，含 `ProxyAutoConfigURLString` 与 `ExceptionsList`），文本解析成结构体。
- Windows：`reg query "HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings"`（`AutoConfigURL`、`ProxyEnable`、`ProxyServer`、`ProxyOverride`），文本解析成同一个结构体；不引入新的 FFI 依赖。
- 两处都是「读取系统既有配置」，不写配置、不改 DNS、不做 WPAD 广播发现（`AutoConfigURL` 未设置时不猜 `wpad`）。
- 两个探测进程都走 `crate::util::no_window::NoWindow`：Windows 上带 `CREATE_NO_WINDOW`，不会在 GUI 里闪控制台窗口（由 `tests/no_window_regression.rs` 的全仓扫描守住）。

### 代理不可用与认证
- `Unsupported`：PAC 只给出 SOCKS（本构建未启用 `socks` feature）或代理 URL 形态无法交给 reqwest 时，`web_fetch` 直接返回原因，不静默直连。
- HTTP 407：返回明确文案，说明代理要求认证（NTLM/Kerberos 暂不支持），不做凭据猜测。

### 拦截页识别
打分制，避免把正常页面误判：标题命中强特征（3 分）、正文命中特征（每项 1 分，最多 3 分）、跨 host 跳转（2 分）、网关型 host 名（1 分）；分数 ≥ 3 判为拦截页，正文超过 5000 字符且分数 < 5 时不判。

### R4 保持不变
`validate_target()` 与「域名不做预解析」的判断保持原样：fake-IP DNS 环境下公网域名会解析到 `198.18.0.0/16`，一旦改成「解析后再判私网」就会拒绝整个公网。
