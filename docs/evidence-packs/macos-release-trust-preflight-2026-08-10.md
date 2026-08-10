# macOS 正式版发布信任门禁证据（2026-08-10）

## 结论

实现候选已把 macOS 发布从 ad-hoc 签名改为 Developer ID + hardened runtime + notarization + staple 的 fail-closed 流程，但当前仍为 **not live / platform incident**：GitHub 仓库和本机均没有可用 Apple 核心身份，不能真实签名或发布，且不得降级伪造完成。

## 已验证事实

- GitHub repository secrets 只有 `RELEASE_PAT`、`TAURI_SIGNING_PRIVATE_KEY`、`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`；未配置五个 Apple secret。
- 本机 codesigning keychain 中 `Developer ID Application` 有效身份数量为 0。
- 在约定的本地受管/下载目录中，没有发现 `.p12` 或 `AuthKey_*.p8` 候选文件。
- 公开 v1.78.4 DMG 交给新 strict gate 后 exit 1，失败原因为 `code has no resources but signature indicates they must be present`；这证明新门禁会拒绝历史 ad-hoc 包。
- `python3 -m unittest tests.test_release_workflow tests.test_github_main_gate tests.test_readme_contract`：37 项通过。
- `python3 tools/governance/validate_repo_governance_baseline.py`：pass。
- 三个 release shell 脚本通过 `bash -n`，两个 workflow 通过本机 YAML parser，`git diff --check` 通过。

## 最小外部身份输入（一次性完整集）

1. `APPLE_CERTIFICATE`：Developer ID Application `.p12` 的 base64。
2. `APPLE_CERTIFICATE_PASSWORD`：该 `.p12` 的导出密码。
3. `APPLE_API_ISSUER`：App Store Connect issuer ID。
4. `APPLE_API_KEY`：App Store Connect key ID。
5. `APPLE_API_PRIVATE_KEY`：对应 `.p8` 私钥内容。

CI keychain 密码由 workflow 自动生成，不需要新增长期 secret。上述身份必须来自 Apple Developer Account Holder / App Store Connect 官方路径；在它们进入 GitHub Secrets 前，安全动作固定为停止发版并记录 `platform_incident`，`requires_user_continue=false`。

## 真实发布验收命令

```bash
python3 -m unittest tests.test_release_workflow tests.test_github_main_gate tests.test_readme_contract
python3 tools/governance/validate_repo_governance_baseline.py
gh workflow run auto-release.yml --ref main
```

Release runner 必须随后在构建 DMG 和公开回下载 DMG 上执行：

```bash
scripts/verify-macos-release-artifact.sh <CodeFactory.dmg> <version>
scripts/verify-macos-updater-artifact.sh <latest.json> <app.tar.gz> <app.tar.gz.sig> <version> <previous-version>
```

只有 PR/CI/merge、正式 release、匿名公开下载、Developer ID/notary/staple/Gatekeeper、真实窗口与 updater 载荷门禁全部通过，才允许将状态改为 live。
