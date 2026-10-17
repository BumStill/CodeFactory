## Summary
复用既有 FakeForge，为 E2E-004 增加 CI 失败后跨进程恢复同一 PR 与合并 SHA 绑定的 synthetic artifact 字节回读回归。仅完成 L2 前置切片，不宣称完整 E2E-004。

Scenario-Test: CXD-002 HLT-001
README-Update: reviewed
README-Update-Reason: 仅增加合成测试与进展记录，不改变公开产品、安装或隐私承诺。

## Requirements / self-acceptance
| Req | 状态 | 证据 |
|---|---|---|
| R1 | partial | 既有真实 Git、dirty file、SQLite FakeForge；provider 与失败测试轨迹尚未组合 |
| R2 | partial | e2e004_ci_failure_recovers_same_pr_and_artifact 验证 PR→CI failure→success→merge→artifact；steer/queue 未组合 |
| R3 | partial | CI 瞬时失败、dirty file 已验证；provider reconnect 与修改后测试失败未组合 |
| R4 | partial | 分支/commit/PR identity、单次 PR/merge/release dispatch、dirty file/root index、跨进程恢复和 artifact bytes/digest；durable run/owner 等待未组合 |
| R5 | partial | 既有 nightly 全 Rust workspace 自动包含新增非 ignored test；匿名 synthetic receipt 在 fixture 内回读，结束随 fixture 清理，不作为 trusted case receipt |
| R6 | deferred | 模板禁止修改信任根，registry 保持 designed |
| R7 | pass | 原命令先 exit 101，修复后原目标通过；整组 10 passed、2 supervised worker entries ignored |

## Validation
- 全 Rust workspace: `CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-e2e004" CARGO_INCREMENTAL=0 cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast` passed。
- FakeForge 全组：10 passed、2 supervisor-only worker entries ignored；worker 由 supervisor 实际运行。
- 治理基线与 git diff --check passed。
- Spec Harness：规格原文持久化于 docs/specs/feature-specs/e2e004-fake-forge.md。
- AI Collaboration：context scope=指定任务、registry、既有 FakeForge；assumptions=L2 fixture 不是 L3/L4；review point=跨进程幂等与真实 Git 字节；validation result=以上测试。按模板禁止 delegate_tasks，未启动 sub-agent，不声称独立 QA。
- 无 UI 或前端产品修改，真实浏览器截图不适用。真机验收由编排方发版后完成：同一任务 steer 顺序、重连、CI 等待释放 owner、原 run 恢复与真实交付审计。当前 not live，不执行发布。

## Spec Amendment (pending)
R6 要求 registry 更新，与模板 §6 禁止修改 registry/validator/workflow 冲突。建议编排方通过受控信任根流程登记 target 与精确 remaining gaps，状态最多 partially_implemented。本 PR 不修改治理门禁。其余 R1–R4 未完成部分完整记录在长任务中，不能将本切片作为完整 E2E-004 验收。
