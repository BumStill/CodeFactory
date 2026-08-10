# macOS 发布信任门禁纠偏证据（2026-08-10）

## 结论

本次一度把 Apple Developer ID/notarization 从可选增强误设为强制发布前置，并导致 Auto Release 在创建版本前失败。该判断超出了既有产品契约，制造了新的非业务阻断；现已撤销强制门禁，恢复历史上工作的 Tauri updater 签名发布通道。

## 已验证事实

- 历史正式版使用 `RELEASE_PAT` 与 Tauri updater 签名 secrets 完成版本 PR、tag、Windows/macOS 资产和 `latest.json` 发布。
- GitHub 未配置 Apple Developer ID/notarization secrets；在错误门禁出现前，这不影响现有正式发布。
- 错误门禁在任何版本 mutation 前失败，因此没有产生半成品版本 PR、tag 或 draft release。
- 失败优先测试已复现“缺 Apple 凭据阻断现有发布”，修复后契约要求 Auto Release 与 Release 都不读取 Apple secret。
- 基线产物验证继续覆盖构建 DMG、匿名公开下载 DMG、版本、arm64、Evolution、真实窗口和隔离数据库。

## 机制修正

1. 发布门禁只校验现行发布契约真正依赖的输入，不能把建议中的增强能力自动升级为必需条件。
2. 未采纳的 Apple 信任增强记为 `capability_unavailable`，不阻断、不索取凭据、不要求用户继续。
3. 若未来要改变分发信任等级，必须先形成明确业务决策与可回滚迁移，再把增强门禁接入正式发布。
4. 证据层必须区分 Tauri updater 签名与 Apple 平台签名，不能互相替代或混称。

## 验收命令

```bash
python3 -m unittest tests.test_release_workflow tests.test_github_main_gate tests.test_readme_contract
python3 tools/governance/validate_repo_governance_baseline.py
gh workflow run auto-release.yml --ref main
```

最终 live 证据仍必须包括 PR/CI/merge、正式 release、完整跨平台资产、公开 `latest.json`、匿名 macOS DMG 下载以及真实 App smoke；但不再包含未经业务采纳的 Developer ID/notary/staple 门禁。
