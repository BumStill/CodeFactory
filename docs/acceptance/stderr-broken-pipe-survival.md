# stderr 管道关闭导致进程 abort：逐条验收账本

规格：docs/specs/feature-specs/stderr-broken-pipe-survival.md

| 需求 | 状态 | 证据 |
| --- | --- | --- |
| R1 | done | `src-tauri/src/lib.rs` 内联 `mod std_stream_guard` 提供启动期修复，通过加载期钩子在任何 `main` 之前安装（`lib.rs::run` 另有一次幂等调用）；`std_stream_guard::tests::a_pipe_whose_reader_closed_is_detected` 与 `a_repaired_stream_accepts_writes_again` 覆盖「识别死管道 + 修复后可写」 |
| R2 | done | `std_stream_guard::tests::a_live_pipe_is_not_reported_broken` 证明有读者的管道不被改写；`a_regular_descriptor_is_never_repaired` 证明普通描述符不动；端到端健康管道用例仍完整收到 `usage:` 文本，CLI stdout/stdout JSON 契约未被静默丢弃 |
| R3 | done | 守卫改为加载期安装，覆盖全部入口；本次 diff 未触碰任何信任根文件（`src-tauri/src/main.rs`、`src-tauri/src/unattended_smoke_cli.rs`、`docs/testing/scenario-registry.json` 与 `origin/main` 逐字节一致），库内唯一显式安装点在 `lib.rs::run` |
| R4 | done | 守卫在加载期把无读者的 stderr 改指 null 设备，钩子的 stderr 副本因此不再失败，`panic_log.rs` 保持与 `origin/main` 逐字节一致（本次 diff 未修改它）；T5 死管道运行未产生新的崩溃记录、未 abort |
| R5 | done | 边界在规格 `docs/specs/feature-specs/stderr-broken-pipe-survival.md:27` 显式声明：只覆盖「启动时读端已不在」、仅 Unix；实现侧由 `src-tauri/src/lib.rs` 内 `mod std_stream_guard` 的 CFG 门控与 `std_stream_guard::tests::a_regular_descriptor_is_never_repaired` 证明未扩大作用面；本单未声称覆盖运行期被关闭的管道或 Windows 句柄，也未改动任何打印宏调用点 |

## 验证命令

- `node scripts/cargo-shared.mjs test --manifest-path src-tauri/Cargo.toml --lib std_stream_guard -- --test-threads=1` → 4 passed
- `node scripts/cargo-shared.mjs test --manifest-path src-tauri/Cargo.toml --lib panic_log -- --test-threads=1` → 5 passed
- `node scripts/cargo-shared.mjs build --manifest-path src-tauri/Cargo.toml --bin codefactory` → 成功
- T5 端到端：二进制 stderr 接到读端在启动前关闭的管道 → 退出码 2；健康管道 → 退出码 2 且 `usage:` 完整
- `python3 tools/governance/validate_repo_governance_baseline.py` → pass

真实安装版验收由发版后完成；本次只交付源码与 CI，not live。
