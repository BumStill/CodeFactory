## M96：前台新 claim 首次写入前对账

规格：docs/specs/feature-specs/delivery-fence-single-ownership.md

R1/R5：生产 DB 只读取证表明前台 epoch 119 失败并释放后，后台才签发 epoch 120；直接原因是前台重新准入时新 claim 未 reconciled，随即执行 git_stage。不能将同 OS 进程视为同一 invocation。保留 UUID、TTL 与 epoch 围栏规则。

R2/R3：前台首次写入前执行真实 local identity、remote branch/canonical PR、未决 mutation intent 和 release receipt 对账；心跳覆盖整个对账，成功后仅 CAS 晋升当前 owner/epoch，失败仍禁止写且 lease guard 释放。没有通过 owner 元数据直接晋升权限。

R4：新增 foreground_readmission_reconciles_before_first_mutation，真实 bare origin + Git checkout + 内存 SQLite，复现新 epoch permit=false，然后真实对账后 permit=true，并断言 HEAD 不变和 mutation intent=0。故障注入跳过对账时，同一测试在权限断言失败；恢复后同一命令通过。这是后补的故障注入红绿证据，不声称是实施前 TDD。重启回收由真实 plan_startup_recovery 驱动。既有 stale heartbeat 与 takeover intent 测试保留。

## 验证

- cargo test --manifest-path src-tauri/Cargo.toml --lib foreground_readmission_reconciles_before_first_mutation：1 passed（pipefail 保留退出码）。
- 最终修订 cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast：成功。
- pnpm test：151 files，1013 tests passed。
- pnpm build：成功；仅既有 chunk size 警告。

## Harness / AI Collaboration

Spec、Compatibility、Observation、Release、AI Collaboration。无 schema、UI 或授权上限变化。Context scope=任务文件、真实只读 DB、前台/后台交付和 mutation ledger；assumptions=OS process 不等于 invocation；review point=禁止元数据直接晋升与旧 permit 跟随新 epoch；validation result=上述真实 Git/SQLite 集成及完整回归。用户模板禁止 delegate_tasks，因此本次单执行流。

## 自验收与限制

R1 pass：根因时间线入规格；R2/R3 pass：首次写入前真实对账，仍 fail closed；R4 partial：新集成回归覆盖重启回收和未对账 permit，完整 stage/commit/push 真实安装路径留待编排方；R5 pass：全部所有权路径已盘点。

真机验收由编排方发版后完成：安装新版，在已有 PR 返修会话调用交付，检查 claim_reconciled 后继续 stage/commit/push 且复用同一 PR；断网或外来 head 时禁止写且释放 lease。本 PR 不发版，not live。

Spec Amendment：无需求语义修改；取证纠正了最初“后台在失败前抬 epoch”的假设。原任务 R4 指定的并发抬 epoch 故障测试与 R3 更完整的有界重取终态仍需进一步补强，不能以现有测试代替全部验收。

Scenario-Test: HLT-001, HLT-002, HLT-005, CXD-002, RTE-004
README-Update: reviewed
README-Update-Reason: 内部交付围栏修复，不改变公开安装、隐私或产品承诺。
