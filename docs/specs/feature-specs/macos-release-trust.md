# macOS 正式版发布信任规格

## 现行发布契约

CodeFactory 的 macOS 基线发布通道使用完整的 ad-hoc app-bundle 签名封存 `Info.plist` 与资源，并使用 Tauri updater 私钥签署更新载荷；构建产物和匿名公开下载产物都要验证签名结构、版本、架构、构建提交、Evolution 闭环、真实窗口和隔离数据库。该通道不依赖 Apple Developer ID 或 notarization 凭据。

Apple Developer ID、公证和 stapling 是可选的分发增强。除非产品所有者明确把“无需任何 Gatekeeper 绕行”提升为业务发布要求，否则缺少这些凭据不得阻断版本 PR、tag、draft 或公开 release。

## Requirements

- **CF-MRT-01**：基线发布必须使用 `TAURI_SIGNING_PRIVATE_KEY` 与 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 生成 updater 签名；缺失或无效时才属于现行发布通道的真实平台阻断。
- **CF-MRT-02**：Tauri 必须用 `signingIdentity: "-"` 在打包前生成完整的 ad-hoc app-bundle 签名；构建期 DMG 与匿名公开重新下载 DMG 都必须在执行任何发布二进制前通过 `codesign --deep --strict`，并验证版本、arm64 架构、构建提交、Evolution 闭环、真实窗口和隔离数据库。只有 linker-generated Mach-O 签名、未绑定 `Info.plist` 或缺少 sealed resources 均不得发布。
- **CF-MRT-03**：`latest.json` 必须指向当前公开版本并携带由发布流程产生的 macOS updater 签名；构建期与公开重新下载的 `.app.tar.gz` 解包后必须通过同一版本、架构和 `codesign --deep --strict` 门禁。
- **CF-MRT-04**：Apple Developer ID/notarization 未配置时，发布流程必须继续基线通道，且不得要求用户补凭据或回复“继续”。
- **CF-MRT-05**：若未来显式启用 Apple 信任增强，必须作为独立、可回滚的能力上线；只有业务契约已明确采用后，它的失败才可阻断发布。
- **CF-MRT-06**：所有签名私钥、密码、证书和 API key 内容不得写入日志、artifact 或仓库。

## 诚实边界

完整 ad-hoc 签名可证明当前 app bundle 的代码与资源封套内部一致，能发现未重新签署的传输或打包损坏，并避免把 linker-only 签名误当成可分发 bundle；它不能阻止攻击者修改后重新做 ad-hoc 签名。Tauri updater 签名负责程序内更新载荷的发布者认证。两者都不证明 Apple 认可的发布者身份，也不消除用户首次打开时可能需要在“隐私与安全性”中放行的步骤。发布证据不得把该基线通道表述为 Developer ID 签名或已公证。
