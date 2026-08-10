# macOS 正式版发布信任规格

## 问题

历史正式版只有 Tauri updater 签名，App 本体使用 ad-hoc 签名，浏览器下载后无法通过标准 Gatekeeper 路径。发布 smoke 能证明进程启动，却不能证明用户拿到的是 Developer ID 签名、公证并 stapled 的正式包。

## Requirements

- **CF-MRT-01**：Auto Release 在创建版本 PR、tag 或 draft 前，必须确认 Apple Developer ID 与 notarization 凭据契约完整。缺失时写结构化 `platform_incident`，`requires_user_continue=false`，并 fail closed。
- **CF-MRT-02**：macOS App 必须使用 `Developer ID Application` 签名，启用 hardened runtime、secure timestamp，不得包含 `get-task-allow`。
- **CF-MRT-03**：Apple notarization 和 stapling 必须在上传产物前成功；任何失败均不得发布 draft。
- **CF-MRT-04**：构建期 DMG 与公开重新下载 DMG 都必须通过 `codesign --deep --strict`、`stapler validate`、`spctl --assess`，再执行既有真实窗口和隔离数据库 smoke。
- **CF-MRT-05**：公开 `latest.json` 的 `darwin-aarch64` 必须选择当前 `.app.tar.gz`，内嵌签名必须与 `.sig` 一致，当前版本必须高于上一正式版，解压后的 updater App 必须通过同一 Developer ID、公证和 Gatekeeper 门禁。
- **CF-MRT-06**：Apple 与 updater 私钥、密码、API key 内容不得写入日志、artifact 或仓库；artifact 只记录缺失的 secret 名称与系统处置。

## 当前验收边界

CF-MRT-05 证明“上一版本可发现的公开元数据与更新载荷”在结构、版本和 macOS 平台信任上有效；它不冒充一次真实 UI 点击后的替换和重启。真实上一版本安装升级仍需在首个具备 Apple 凭据的正式版上补充 installed-App 自动化证据。
