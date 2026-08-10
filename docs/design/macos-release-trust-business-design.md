# macOS 正式版发布信任：业务设计

## 用户结果

用户从 GitHub 或程序内更新拿到的 CodeFactory，应直接通过 macOS 标准安全检查，不再依赖右键打开、手工白名单或忽略安全告警。

## 业务门禁

“Windows 包已生成”或“macOS 在 CI 能启动”都不等于可发布。只要 macOS Developer ID、公证、staple、Gatekeeper 或 updater 载荷任一失败，整个跨平台 release 保持未发布。

签名凭据缺失属于 release platform incident，不是产品范围选择，也不是要求用户回复“继续”的业务决策。系统安全默认是保留目标、停止发版、产出可审计诊断并由平台维护路径补齐凭据。
