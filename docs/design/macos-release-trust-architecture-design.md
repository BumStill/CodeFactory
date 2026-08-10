# macOS 正式版发布信任：架构设计

## 流程

```text
Auto Release preflight
  -> version PR / tag
  -> Release preflight
  -> ephemeral keychain imports Developer ID Application .p12
  -> App Store Connect API key enables Tauri notarytool path
  -> Tauri build signs + notarizes + staples App/DMG and signs updater archive
  -> build DMG trust + GUI smoke
  -> finalize publishes latest.json
  -> anonymous re-download of DMG/latest.json/archive/.sig
  -> public DMG trust + GUI smoke + updater payload trust
```

## Secret contract

- `APPLE_CERTIFICATE`：Developer ID Application `.p12` 的 base64。
- `APPLE_CERTIFICATE_PASSWORD`：`.p12` 导出密码。
- `APPLE_API_ISSUER`：App Store Connect API issuer ID。
- `APPLE_API_KEY`：App Store Connect key ID。
- `APPLE_API_PRIVATE_KEY`：一次下载的 `.p8` 私钥内容。
- 既有 `TAURI_SIGNING_PRIVATE_KEY` 与 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` 继续只负责 updater 签名，不能替代 Apple 平台签名。

凭据只进入 GitHub secret env；CI 自动生成一次性 keychain 密码，证书导入一次性 keychain，notary key 写入 runner temp，job 结束 always 清理。preflight artifact 只含状态和缺失名称。

## 失败语义

凭据缺失或无效、notary rejection、staple/codesign/spctl 失败均 fail closed。Auto Release 在版本 mutation 前阻断；Release 在 draft 创建前阻断。状态统一为 `platform_incident`，不转换为 `needs_user`。
