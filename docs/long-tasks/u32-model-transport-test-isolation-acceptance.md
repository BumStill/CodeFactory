# U32 PR 自验收：模型路由测试隔离

标题：`fix(test): model routing tests are isolated and stable on every platform`

修复 Windows CI 并行运行时的本地 HTTP fixture 端口竞争。原 `unreachable_base_url()` 绑定临时 loopback 端口后立即释放，导致该端口可能被并行 HTTP fixture 重用，错误响应被误认为端点可达/过载。现改为保留 listener 并返回本地合成 502；真正需要 connection-refused 语义时用单独 `closed_loopback_port()`。生产端点健康/cooldown 的进程级共享行为保留：跨 session 共享可用性观测是预期产品语义，用于自动路由短暂避开刚失败的 provider；测试通过显式注入独立 `EndpointHealthRegistry` 隔离状态。

## Requirements Traceability / Self-acceptance

| Req ID | Acceptance | Status | Evidence |
|---|---|---|---|
| CF-PFB-R7 | 模型路由测试任意顺序、任意并行度稳定 | PASS | 普通并行：`CARGO_TARGET_DIR="$HOME/.cache/codefactory-cargo-target" cargo test --manifest-path src-tauri/Cargo.toml --lib model_transport::tests:: -- --test-threads=8` 连续 20 轮，每轮 40 passed、0 failed；另一次最终并行模块运行 40 passed。随机顺序：`RUSTC_BOOTSTRAP=1 CARGO_TARGET_DIR="$HOME/.cache/codefactory-cargo-target" cargo test --manifest-path src-tauri/Cargo.toml --lib model_transport::tests:: -- -Z unstable-options --test-threads=8 --shuffle` 连续 20 轮，每轮 40 passed、0 failed；日志在 `/tmp/u32-model-transport-shuffled-{1..20}.log`。 |
| CF-PFB-R8 | 产品共享状态语义保留，测试状态可隔离 | PASS | 生产共享 registry 未修改；`endpoint_health_registries_are_isolated_between_tests` 断言一个独立 registry 标记 provider 不可用不会影响另一 registry。 |
| CF-PFB-R9 | #589/#590 所阻塞测试在当前最新 main 合并状态通过 | PASS（本地） | `prefer_mode_waits_only_after_every_endpoint_is_unavailable`：1 passed；`a_stale_remediation_after_restart_lets_the_continuation_run`：当前 main 该测试名不存在，因此按官方 #590 PR 源码对应检查未能以名称过滤；完整 `model_transport::tests::`（含全部 40 当前模块测试）并行运行通过。PR Windows CI 结果待 GitHub runner。 |

### TDD / 失败优先记录

Windows 归档失败：#589 Run `37920830238`，`check-rust` 的 `prefer_mode_waits_only_after_every_endpoint_is_unavailable` 报 `PROVIDER_RECOVERY_WAITING: provider overloaded`；#590 Run `37921018954`，`a_stale_remediation_after_restart_lets_the_continuation_run` 收到 loopback connection error。修改前本机单跑第一项通过，说明需并行压力才能触发。曾观察到错误 fixture CRLF 时失败；修正后单项、串行全模块和并行全模块恢复通过。没有删除、跳过或放宽原断言。

### Applicable Harnesses

- Spec Harness：规格文件新增 CF-PFB-R7/R8/R9；自验收表按 Req ID 映射证据。
- Compatibility Harness：原有 prefer、fixed、全端点不可用及对话中回落测试均在完整模块中执行；生产 routing 和共享 health 语义不变。
- AI Collaboration Harness：context scope 为 U32 task、task-template、quick profile、Prefer 规格和 #589/#590 CI 日志；关键假设为释放临时端口造成 fixture 交叉响应；review point 为端口是否由单一 fixture 持有、失败请求是否有合成 502、断言是否保留；结果为 20 次普通并行及 20 次随机顺序并行全通过。

Scenario-Test: 无（修改仅限 Rust 测试 fixture 与 feature spec，不映射业务场景）。
README-Update: not-required — 未改变公开产品、安装、隐私或平台承诺。
README-Update-Reason: 此项变更仅影响测试隔离和规格记录。

## 验证记录

- `pnpm test:rust:fast -- prefer_mode_waits_only_after_every_endpoint_is_unavailable`：PASS，1 passed。
- `cargo test --manifest-path src-tauri/Cargo.toml --lib model_transport::tests:: -- --test-threads=1`：PASS，40 passed。
- `cargo test --manifest-path src-tauri/Cargo.toml --lib model_transport::tests:: -- --test-threads=8`：PASS，40 passed。
- 普通 8-thread 并行重复 20 次：每轮 40 passed、0 failed。
- 随机 shuffle + 8-thread 并行重复 20 次：每轮 40 passed、0 failed。
- `python3 tools/governance/validate_scenario_test_governance.py --ci --repo . --base-ref origin/main --title "fix(test): model routing tests are isolated and stable on every platform" --body-file docs/long-tasks/u32-model-transport-test-isolation-acceptance.md`：PASS。
- `git diff --check`：PASS。
- Windows PR CI：开 PR 后由 Windows runner 验证；本地 macOS 结果不冒充 Windows 执行证据。
