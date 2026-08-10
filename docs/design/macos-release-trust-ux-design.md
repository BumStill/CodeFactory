# macOS 正式版发布信任：UX 设计

## 当前用户路径

- GitHub 下载：用户下载 DMG、安装并启动；release 证据验证真实窗口与核心 Evolution 路径。
- 程序内更新：旧版读取公开 `latest.json`，下载由 Tauri updater 私钥签名的载荷并校验后升级。
- 当前通道不承诺 Apple Developer ID/notarization；相关限制必须在发布说明中如实表达。

## 非阻断原则

Apple 信任增强未配置时，用户不应看到缺凭据、回复“继续”或等待人工补 secret 的提示，现有发布照常进行。只有当用户明确选择将 Apple 标准安全路径设为必需业务目标，才进入一次性完整配置和切换流程。

## 验收

- Given 仓库只有既有 Tauri updater secrets，When 手动或定时切版，Then 版本 PR、tag、跨平台资产和公开 release 正常完成。
- Given 构建和公开下载的 macOS DMG，When 执行 release smoke，Then 版本、架构、Evolution、真实窗口与隔离数据库均有证据。
- Given Apple secret 全部缺失，When 运行 Auto Release，Then 不检查、不报错、不要求用户输入。
- Given 未来显式启用 Apple 增强，When 验证失败，Then 不冒充已公证；是否阻断由已批准的发布策略决定。
