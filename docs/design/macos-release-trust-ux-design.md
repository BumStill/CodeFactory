# macOS 正式版发布信任：UX 设计

## 用户可见路径

- GitHub 下载：双击 DMG、拖入 Applications、正常首次启动；不要求右键打开或修改“隐私与安全”。
- 程序内更新：旧版发现新版本，下载安装后重启到新版本；更新载荷与 DMG 内 App 具有同一 Developer ID 与公证状态。

## 失败体验

签名或公证平台故障时不发布版本，因此终端用户不会看到缺 Windows 或缺 macOS 资产的半成品 release。维护者在 workflow summary 和 artifact 中看到缺失 secret 名称、失败组件、安全处置与 owner；不显示凭据内容，也不生成要求用户回复“继续”的消息。

## 验收

- Given 浏览器下载的正式 DMG，When 用户首次打开 App，Then `spctl` 接受且来源为 Developer ID/notarized 路径。
- Given 上一正式版与新公开 `latest.json`，When 检查 darwin-aarch64 更新，Then 版本递增、URL/签名匹配、解包 App 通过 strict codesign/staple/Gatekeeper。
- Given 任一 Apple secret 缺失，When 定时或手动发版，Then 在任何版本 PR/tag/draft 产生前停止并写 `platform_incident`，不请求用户继续。
