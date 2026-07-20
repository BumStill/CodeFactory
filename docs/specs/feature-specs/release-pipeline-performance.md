# CodeFactory 发布流水线性能规格

## Requirements Traceability

| Req ID | Requirement | Surface | Validation |
| --- | --- | --- | --- |
| CF-REL-PERF-R1 | `Auto Release` push 版本 commit/tag 后，从 `main` 显式 dispatch `Release` 并传入 tag | auto-release workflow | contract test + real run |
| CF-REL-PERF-R2 | Release 中所有读取源码的 job 必须 checkout 输入 tag，不得把 workflow ref SHA 当作构建 SHA | release workflow | field assertions + executable smoke |
| CF-REL-PERF-R3 | draft 必须在双平台构建前创建；已有 draft 可复用，已公开 release 禁止覆盖 | prepare-release job | contract test + rerun behavior |
| CF-REL-PERF-R4 | Windows 与 macOS 只依赖 changelog 和 draft 准备，必须可并行调度 | release DAG | contract test + job timestamps |
| CF-REL-PERF-R5 | finalize 必须同时依赖两个平台，任一失败时不得公开 release | release DAG | dependency assertion + real run |
| CF-REL-PERF-R6 | Release run 使用 `main` cache scope，连续发布可恢复上一轮 sccache | Actions runtime | sccache statistics from two releases |
| CF-REL-PERF-R7 | 安装包、签名、updater manifest 与发布后 macOS 公开下载复验保持不变 | release artifacts | existing release tests + published smoke |

## Primary User Path

`merge feat/fix -> Auto Release -> version commit/tag -> dispatch Release on main -> prepare draft -> Windows/macOS parallel build and smoke -> finalize -> public macOS artifact smoke`

## Applicable Harnesses

- Spec Harness：Req ID、主路径和失败边界随实现提交。
- Compatibility Harness：保持安装包、签名、updater 与 release 命名契约。
- Release Harness：真实 tag、draft、双平台资产和公开下载路径。
- Observation Harness：记录 job 起止时间和 sccache 命中统计。
- AI Collaboration Harness：独立审查 DAG、tag/SHA 身份和重跑边界。

## Testing Matrix

| Scenario | Expected result | Evidence |
| --- | --- | --- |
| Auto Release 有 feat/fix | push 后显式 dispatch 输入 tag | workflow log |
| Release 在 `main` 上运行 | checkout/构建内容来自输入 tag | tag SHA output + executable receipt |
| Windows/macOS runner 可用 | 两个平台构建时间区间重叠 | Actions job timestamps |
| 任一平台构建或 smoke 失败 | finalize skipped，release 保持 draft | release state + workflow DAG |
| 同一 draft tag 重跑 | 复用 draft 并重新上传 | prepare log + assets |
| 已公开 tag 重跑 | 失败且不覆盖公开资产 | prepare error |
| 下一次发布恢复缓存 | Rust sccache 命中大于 0 | sccache post-run statistics |

## Proof Tiers

- `contract-tested`：静态 workflow 契约与本地测试通过。
- `merged-live`：配置已合并到 `main`，后续 Auto Release 使用新路径。
- `release-observed`：真实发布证明双平台并行且公开产物复验通过。
- `cross-release-cache-observed`：至少第二个 main-scoped Release 证明 Rust sccache 跨版本命中。

在达到对应证据前，不得越级声称性能目标已实现。

## AI Collaboration Record

- context scope: `auto-release.yml`、`release.yml`、v1.48.0 job 时间和 sccache 统计。
- assumptions: hosted runner 分配没有异常排队；tauri-action 可以向同一已有 draft 上传不同平台资产。
- review point: workflow ref 与构建 tag 的身份必须分离，`CODEFACTORY_BUILD_GIT_SHA` 必须等于 tag commit。
- validation result: v1.48.0 基线已量化；新 DAG 与缓存作用域尚待实现和真实发布验证。
