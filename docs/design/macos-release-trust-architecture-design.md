# macOS 正式版发布信任：架构设计

## 基线通道（当前生效）

```text
Auto Release
  -> version PR / required checks / tag
  -> Tauri build + updater signature
  -> build DMG real-app smoke
  -> publish latest.json
  -> anonymous public DMG download
  -> public DMG real-app smoke
```

基线通道只依赖现有 `RELEASE_PAT`、`TAURI_SIGNING_PRIVATE_KEY` 与 `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`。它不得读取或检查 Apple Developer ID/notarization 凭据。

## 可选 Apple 信任增强（尚未启用）

Developer ID、公证与 stapling 若未来被业务明确采纳，应作为独立 capability 实现：

1. 先在不影响基线发布的验证 workflow 中证明证书导入、hardened runtime、notary、staple 与 `spctl`。
2. 对构建 DMG、公开回下载 DMG 和 updater App 验证同一身份与公证状态。
3. 在凭据和真实产物证据稳定后，再通过显式配置切换发布策略。
4. 配置缺失或增强验证失败时保持基线通道可用；只有产品契约明确要求增强通道时才 fail closed。

## 失败语义

- 现有 updater 签名或真实产物验证失败：`platform_incident`，自动修复/重试，不能伪造发布。
- Apple 可选能力未配置：`capability_unavailable`，非终态、非 blocker、不得请求用户继续。
- 需要改变发布信任等级：`needs_business_decision`，仅在确实要采用该业务目标时提出一次完整决策。
