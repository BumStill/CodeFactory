# 测试检查程序升级与恢复后复测

## 先说结论

测试结果格式前后不一致、只看“通过”字样可能误判的问题，已通过 [PR #508](https://github.com/BumStill/CodeFactory/pull/508) 合入默认分支。原来的全部合并保护已经恢复。

这不表示所有场景已经补齐。接下来由本记录所在的普通 PR，在完整保护下真正运行 Windows 和 Mac 检查；只有服务器回执与独立复算都通过，才能确认新版检查程序正常工作。界面操作、安装升级等尚缺的完整场景仍继续保留，不能提前记为完成。

## 已完成的切换

- 授权范围：本轮批准的 #508 共 13 文件差异；仅临时解除 `scenario-gate-pr` 一项限制，保留其他五项检查及所有其他保护。
- 并行同步：保留 #510 的恢复提示测试和 #509 新增的模型目录场景 `UI-015`，没有覆盖它们。最终测试 head 为 `7bbc4f2dc96551ea01d286ac3ef80922e851db2b`，base 为 `04a92442bc4449349ce6fb58c16bee5443cf9ea2`。
- 合并前五项检查全通过：[CI 34181882558](https://github.com/BumStill/CodeFactory/actions/runs/34181882558)、[界面验收 34181882550](https://github.com/BumStill/CodeFactory/actions/runs/34181882550)、[治理基线 34181882545](https://github.com/BumStill/CodeFactory/actions/runs/34181882545)。Linux Python 共 317 项，其中 307 通过、10 项仅适用 macOS 而跳过；本机可运行集 290 项通过，同步最后一项目录更新后场景相关 117 项再次通过。
- 旧检查程序按预期拒绝四个受保护文件变化及新版回执声明；这不是将失败测试强行改绿，而是经外部审查后更新检查程序本身。
- 合并提交：`fc2432658b87993b8af10df0cc416f594218e6ac`。读回确认唯一父提交为上述 base、完整文件树与测试 head 一致、`Release-Urgency: hold` 保留。测试基础设施变更不单独发产品版本。
- 成功窗口（UTC）：`03:14:59.420755` 至 `03:15:11.565191`，约 12.1 秒。规则集 `20222077` 已与原始快照逐字段对账；六项检查仍绑定 GitHub Actions App 15368，active、strict、空 bypass actors、PR、禁止删除和强推等保护保持不变。仓库标准 `manage_main_branch_ruleset.py verify` 返回 `converged`。

## 网络异常没有绕过保护

第一次在读取检查结果时断开，尚未改动规则；第二次在临时配置读回时断开，`03:11:22.281952` 至 `03:11:32.507458` 的窗口内自动恢复原始规则，未发送合并请求。

调整仅限单次命令的连接方式：GET 读取最多重试一次，第二次使用既有环境路由，不改全局代理。PUT 不自动重试；如果合并结果不确定，先恢复保护，再只读核查 PR、合并父提交、完整文件树和 release trailer，不重复发送合并。遇到其他人同时改规则，只补回本次解除的那一项，保留对方改动并停止。

这些处理有失败优先模拟测试：最终 8 项本地测试通过；独立审查另验了合并结果正反例、GET 超时回退、环境不变和 PUT 仅调用一次。实际第二次失败也证明了自动恢复路径有效。

## 恢复后的实际复测要求

本 PR 在 `package.json` 增加 `test:scenario-platform` 命令，调用既有共享 Cargo 入口运行 `scenario_target_platform`。增加前命令缺失而失败，增加后真实 Rust 测试为 `1 passed; 0 failed; 0 ignored`。它是可复用的早期检查入口，不替代服务器真实执行。

因为 `package.json` 影响全局运行配置，新的默认分支检查程序会生成非空计划。当前基线为 28 个逻辑场景、113 个执行目标（Windows 104、macOS 9），其中 E2E-001 回执必须恰好一份。目标及 alias 不等于新的逻辑场景，不能重复计数。执行时须按本 PR 的实际 base/head 重新生成计划，不硬编码历史的 110 或 111 个目标。

验收必须同时满足：

1. 六项 required checks 全部通过，线上规则仍完整。
2. plan、两个平台 runner receipt、aggregate 的 base/head 一致，目标完整唯一，无缺项、多余、跳过或错平台。
3. Windows E2E-001 由原始观测重新计算 case receipt，driver/verifier/fixture、源码与 binary build SHA 匹配，不能只相信候选返回的 `passed`。
4. 单用户消息、零人工 prompt、真实中断与进程替换、持久完成状态、幂等副作用及零泄漏清理均成立。
5. UI 仍明确为本阶段不要求；Mac 九个目标不冒充 Mac E2E-001，PR 二进制不冒充正式安装包验收。

实际 run、回执与独立复核结果由本 PR 的服务器检查和验收记录提供。纯文档修改形成的零目标绿灯不能替代这轮复测。

首轮运行期间，#511 已合入默认分支；本 PR 保留该修复并重新运行最新 base/head 的检查，旧运行只作为历史证据，不复用为新提交的通过结果。

独立复查还发现一个既有报告标签偏差：Mac 的 `browser-chrome-attach-smoke` 在调试二进制上运行，却输出 `evidence_level=exact_release_artifact`。实际连接、tab 观测、租约回收和 detach 断言可作为本轮 PR smoke 证据，但该标签不能证明正式安装包。此问题单独跟进修正，不提升本轮证据等级。

## 仍未完成的部分

复杂场景仍是 10 个部分实现、1 个设计中，剩余 26 项缺口没有改成完成。后续继续补真实界面、浏览器异常恢复、完整交付过程、并发工作区和 Windows 安装升级；具体顺序见 `docs/long-tasks/scenario-test-completion.md`。

## AI Collaboration

- context scope：合成夹具、源码、公开 CI、匿名回执及规则快照，不读取生产聊天或凭据。
- assumptions：同一测试入口；本阶段通过不等于所有完整场景完成。
- review point：独立复核 13 文件范围、自动恢复与网络异常、合并身份、恢复后真实回执。
- validation result：#508 合入及原始保护恢复已验证；本 PR 的非空复测必须满足上述条件后才能收尾。
