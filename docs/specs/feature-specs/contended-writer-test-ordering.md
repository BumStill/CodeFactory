# 争用写锁用例改为锁序断言（不再依赖固定睡眠）

## 背景

`storage::db::tests::begin_write_waits_for_a_contended_writer_instead_of_failing`
用于证明"写锁被占用时，第二个写者应当等待而不是报错"。原实现有三个时序假设：

1. `tokio::spawn` 出来的持锁方在 40ms 内一定已被调度并取到写锁；
2. 因此第二个写者一定在持锁期间才开始 `begin_write`；
3. 于是它至少会等待 150ms。

在满载的测试进程里（`--lib` 同时跑 1762 个用例，多个 `multi_thread` runtime 抢 CPU），
假设 1 会失效：持锁方被推迟调度，第二个写者反而先拿到写锁，`waited` 只有几百微秒，
断言倒挂。同一断言在 `docs/long-tasks/m116-ceiling-upgrade-pr.md` 里记录过 428µs 的偶发失败，
本次修复期间的本地全量重跑又出现 318µs 的同类失败；CI 与单跑均通过。

这类失败与被测行为无关，属于"测试用计时窗口猜测调度"的缺陷：修补方向不是放宽阈值
（放宽只会把真实回归也一起放过），而是把断言改成不依赖调度的锁序性质。

## 需求

- **Req 1｜不猜调度**：持锁方在真正取得写锁后显式通知，第二个写者等到该通知再开始；不得用固定睡眠推断持锁状态。
- **Req 2｜锁序断言**：断言第二个写者取得写锁的时刻不早于持锁方提交并释放锁的时刻；持锁期间被抢占必须失败。
- **Req 3｜语义不变**：争用写锁不得以错误返回，且两笔写入都必须落库（计数为 2）。
- **Req 4｜可证伪**：断言必须能被负向对照证伪——持锁方不取锁时用例必须失败，并且失败信息指向锁序。

## Requirements Traceability

| Req | 实现 | 验证 |
|---|---|---|
| Req 1 | `db.rs` 中 `tokio::sync::oneshot` 通知 + `holds_lock.await` | `storage::db::tests::begin_write_waits_for_a_contended_writer_instead_of_failing` 通过 |
| Req 2 | 记录 `acquired` 与持锁方提交后的 `released`，断言 `acquired >= released` | 同上用例；负向对照（移除持锁方的 `UPDATE`）必须以 `the second writer acquired the write lock while the holder still held it` 失败 |
| Req 3 | 保留 `.expect(...)` 与末尾 `assert_eq!(counter(&pool).await, 2)` | 同上用例 |
| Req 4 | 锁序断言而非时间阈值 | 负向对照实测 `exit 101` + 上述失败信息 |

## 场景登记

`src-tauri/src/storage/db.rs` 此前没有任何场景映射，任何 storage 改动都会撞上 registry 覆盖缺口。
本规格同时登记 `DAT-001`（`runtime_lifecycle`，L1，`pull_request` 硬门禁），
自动化目标就是上面的用例，因此该路径的后续改动必须带这条证据。

## 证据

- 负向对照：移除持锁方写锁获取 → `test result: FAILED`，`exit=101`，失败信息为锁序断言；
  恢复文件后 `cmp` 确认与修改版逐字节一致。
- 占满负载的全量 `--lib`（1762 passed / 10 ignored）连续两轮通过，用时 94.94s、99.15s——
  与出现 318µs 抖动时相同的负载条件。

## 不做什么

- 不放宽 `waited` 的下限，也不把该用例加入 `docs/testing/flaky-quarantine.json`：
  根因已消除，登记跳过会掩盖真实的锁序回归。
