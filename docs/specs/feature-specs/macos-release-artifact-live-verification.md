# macOS 发布产物实地验收规格

## 范围

本规格定义 CodeFactory macOS 发布产物在公开发布前的最低实地验收。目标是阻止“构建、上传和签名均成功，但用户安装后没有可见主窗口”的坏版本进入正式 release。

该 smoke 只证明安装包可挂载、版本正确、应用能启动并创建可见主窗口。它不能替代 CodeFactory AI 编程主路径、模型调用、工具审批、diff、命令输出和任务修复闭环的发布后验收。

## Requirements Traceability

| Req ID | Normalized requirement | Surface | Validation method | Owner |
| --- | --- | --- | --- | --- |
| CF-MAC-REL-R1 | 从本次构建生成的 DMG 挂载并复制 `CodeFactory.app` 到临时安装目录，验收对象不得是开发二进制或机器上既有安装 | release artifact | release workflow log + local DMG smoke | release |
| CF-MAC-REL-R2 | 安装包内 `CFBundleIdentifier` 必须为 `com.codefactory.app`，`CFBundleShortVersionString` 必须等于当前 tag | app bundle metadata | field assertions | release |
| CF-MAC-REL-R3 | 必须通过 LaunchServices 启动被复制的精确 app bundle，并观察到属于该进程的 layer 0、alpha 大于 0、至少 800x600 且连续稳定 2 秒的 onscreen window | packaged runtime | CoreGraphics window observation | QA |
| CF-MAC-REL-R4 | 验收不得依赖 Accessibility 授权；远端可见会话必须先通过 Screen Recording preflight，再保存绑定该窗口 ID 的截图；检查应排除标题栏/边框，验证至少 800x600 的内容区域具有足够颜色与非主色采样；成功元数据只能在全部截图断言后写入 | observation harness | screenshot + metadata artifact | QA |
| CF-MAC-REL-R5 | smoke 失败时 `build-macos` 必须失败，`finalize` 不得发布 draft release | release workflow | dependency and failure propagation review | release |
| CF-MAC-REL-R6 | 复制后必须先卸载 DMG再启动临时副本；启动时注入隔离 HOME 并确认数据库写入隔离目录；smoke 完成后必须终止测试进程，不污染 runner、本机已有安装或用户数据 | runtime cleanup | process, mount and temporary-home cleanup assertions | release |
| CF-MAC-REL-R7 | 支持对指定已发布 tag 手动运行同一 smoke，用于验证 GitHub 托管 macOS runner 或复查历史产物，不必切出新版本 | observation workflow | workflow_dispatch run | release |
| CF-MAC-REL-R8 | PR 阶段必须在 GitHub macOS 可见会话构建并启动本次精确 debug App；本机锁屏不得把 GUI 证据变成待解锁 blocker | pull request candidate | lock-independent desktop workflow | QA |

## Primary Release Path

`PR -> remote debug App window/screenshot -> merge -> tag -> macOS build -> DMG -> mount -> copy to temporary install directory -> detach -> assert bundle metadata/arm64 -> LaunchServices launch exact app with isolated HOME -> observe stable onscreen main window + screenshot -> terminate -> upload evidence -> finalize release`

只有上述链路通过，macOS build job 才能成功，后续 `finalize` 才允许把 draft release 公开。

## Applicable Harnesses

- Spec Harness：Req ID、主路径、成功与失败边界随实现提交。
- Compatibility Harness：不改变现有 DMG、updater tarball、Windows installer 和 `latest.json` 契约。
- Release Harness：检查本次构建产物，不读取机器上既有 app 作为发布证据。
- Observation Harness：用进程 PID、window layer、alpha 和 bounds 形成字段级断言。
- AI Collaboration Harness：独立 QA 角色审查观察方式和失败边界。

## Testing Matrix

| Scenario | Expected result | Evidence |
| --- | --- | --- |
| DMG 不存在或无法挂载 | 立即失败，不进入 finalize | non-zero exit + error log |
| bundle id 或版本与 tag 不一致 | 立即失败 | actual/expected metadata log |
| app 进程启动失败或提前退出 | 立即失败 | executable path + exit status |
| app 只有小窗、瞬时窗口或没有 onscreen layer 0 window | 超时失败 | PID + observed window summary |
| runner 没有 WindowServer / GUI security session | 明确报告基础设施阻塞，不冒充应用无窗口 | CoreGraphics nil result |
| Screen Recording 不可用、窗口截图失败或内容区域近似纯色 | 明确失败，阻止候选或发布；部分 artifact 不得包含 `status=ok` | screenshot error + uploaded partial evidence |
| app 创建正常主窗口 | 输出 PID、window id/title/bounds 后成功 | Quartz observation log |
| smoke 结束 | 测试进程已终止，DMG 已卸载，临时目录已删除 | cleanup completion |
| 指定历史 release tag 手动复查 | 下载该 tag 唯一 arm64 DMG 并执行同一脚本 | `macOS Release Artifact Smoke` run |

## Product Boundary

这项机制提升的是发布可靠性：每轮 CodeFactory 产品能力改进不会因为“包能构建但用户打不开窗口”而被误判为已经上线，也不会因开发机锁屏停在本地等待。它不提升模型推理或 agent 规划分数，也不能单独证明 Terminal-Bench 2.1 能力；后续仍需安装版真实任务或发布版同口径评测。

## AI Collaboration Record

- context scope: macOS release workflow、Tauri DMG、安装版窗口观察和上一轮 v1.42.6 实地验证。
- assumptions: GitHub macOS runner 提供 WindowServer；CoreGraphics 可在无需 Accessibility 授权时读取 onscreen window metadata；截图仍需 runner 的 Screen Recording preflight 实际返回成功，不能从本地结果推断。
- review point: Quartz 断言必须绑定本次启动进程 PID，不能只按 app 名称匹配机器上已有窗口。
- validation result: 本机 v1.42.6 DMG smoke 已通过，精确临时副本产生 1200x800、layer 0、alpha 1 的窗口并稳定 2 秒；真实用户数据库时间戳未变化，进程、挂载点和临时目录已清理。版本错配和 420x132 小窗口 fixture 均按预期失败。PR CI 和下一次真实 release workflow 仍待合并后验证。
