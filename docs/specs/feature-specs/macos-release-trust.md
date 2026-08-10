# macOS 正式版发布信任规格

## 现行发布契约

CodeFactory 的既有 macOS 正式发布通道使用 Tauri updater 私钥签署更新载荷，并在构建产物和匿名公开下载产物上验证版本、架构、Evolution 闭环、真实窗口和隔离数据库。该通道已经用于历史正式版，不依赖 Apple Developer ID 或 notarization 凭据。

Apple Developer ID、公证和 stapling 是可选的分发增强。除非产品所有者明确把“无需任何 Gatekeeper 绕行”提升为业务发布要求，否则缺少这些凭据不得阻断版本 PR、tag、draft 或公开 release。

## Requirements

- **CF-MRT-01**：基线发布必须使用 `TAURI_SIGNING_PRIVATE_KEY` 与 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 生成 updater 签名；缺失或无效时才属于现行发布通道的真实平台阻断。
- **CF-MRT-02**：构建期 DMG 与匿名公开重新下载 DMG 必须验证版本、arm64 架构、Evolution 闭环、真实窗口和隔离数据库。
- **CF-MRT-03**：`latest.json` 必须指向当前公开版本并携带由发布流程产生的 macOS updater 签名。
- **CF-MRT-04**：Apple Developer ID/notarization 未配置时，发布流程必须继续基线通道，且不得要求用户补凭据或回复“继续”。
- **CF-MRT-05**：若未来显式启用 Apple 信任增强，必须作为独立、可回滚的能力上线；只有业务契约已明确采用后，它的失败才可阻断发布。
- **CF-MRT-06**：所有签名私钥、密码、证书和 API key 内容不得写入日志、artifact 或仓库。

## 诚实边界

基线通道可证明发布包可安装、可启动且程序内更新载荷有 Tauri 签名；它不声称 App 已经 Developer ID 签名或经过 Apple 公证。发布证据必须明确区分这两层信任。
