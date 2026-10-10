# 任务：查清并修好 Windows CI 上的偶发失败（M64）

## 问题与证据（2026-10-10，同一天 4 种，相关 PR 都没碰对应代码，重跑即过）
- `scripts/verify-permission-mode-headless.mjs`：`ArrowDown should focus trusted mode`（#606、#622 的 check-frontend，Windows）
- composer overlap 真浏览器验收：`locator.waitFor: Timeout 10000ms exceeded`（#609）
- reconnect banner 真浏览器验收：`locator.waitFor: Timeout 10000ms exceeded`（#612 的 execute-windows）
- Rust：`agent::objective_supervisor::tests::short_claim_lease_stays_live_until_adapter_settlement` 在 objective_supervisor.rs:4119 panic（#624 的 check-rust，Windows）
- 影响：串行合并因此跳过这些 PR，每次失败都要人工重跑，多等 15–20 分钟 CI；当天发版也被拖慢。

## 规格

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| CF-FLK-R1 | 对上面 4 个用例各自找出根因：焦点、动画、端口、时钟、进程或文件句柄等，写进 PR，并附上能稳定复现的方法（例如 CPU 压力、慢速时钟、Windows runner 上循环跑 N 次） | 每个用例一份根因说明，附复现记录 |
| CF-FLK-R2 | 按根因修复，不允许只加重试或只延长超时来掩盖；等待改成等真实的前置条件（元素可交互、焦点确实到位、锁状态可观测） | 修复后在相同压力下循环 N 次（自定，说明理由）全部通过 |
| CF-FLK-R3 | 修复不能削弱用例本身：断言的用户行为保持不变 | 对照改前改后的断言 diff |

### Applicable Harnesses
Spec Harness；Compatibility Harness；Viewport Harness（3 个真浏览器验收）；AI Collaboration Harness。

## 约束
- 不改 `.github/` 下的工作流与场景注册表；不靠在 CI 里加重试次数来过。
- 其余约束同模板。

## 标题
`fix(test): Windows CI acceptance and timing tests wait on real preconditions instead of flaking`

## Implementation Notes（不改变任何需求或验收标准）

### 根因（CF-FLK-R1）
1. **Rust：`short_claim_lease_stays_live_until_adapter_settlement`。** 旧测试体用 `tokio::time::sleep(4500ms)` 表示“adapter 比租约（2000ms）慢”，隐含假设这 4.5s 内心跳任务一定被调度过。CI 版 `objective_supervisor.rs:4119` 是 `run_with_claim_lease_timing(...).await.unwrap()`：续租被拒（`Ok(false)`）后返回 `Err("objective remediation ownership changed; adapter cancelled")`。续租 SQL 要求 `lease_expires_at > now`（`objective.rs` 的 `renew_claimed_remediation`），runner 变慢时心跳晚于租约到期执行，续租被正确拒绝，于是把仍在执行的 adapter 取消掉。仓库内 `objective_supervisor.rs` 的 `provider_attempt_sweep_stays_off_the_claim_hot_path` 文档注释已记录同一现象。
2. **Rust：续租被拒的第二种来源（真实产品缺陷）。** 负载下追踪到被拒时 remediation 行是 `status='superseded', lease_owner=NULL, attempt_index` 不变——即 adapter 自己已经结算了该 claim（结算是一个 await 点），随后的心跳续租必然失败，却把已提交的结算当作“所有权被抢占”丢弃。修前在 10 路 CPU 占满下 20 次里 8 次失败，失败点与 CI 完全一致。
3. **三个真浏览器验收**：固定端口被占用（同一 runner 上并行 job/前一次未回收的 Vite）时脚本连不上目标页；`waitForServer` 只要求 HTTP 可访问，任何页面（包括上一次的旧夹具）都会让它通过，于是随后 `locator.waitFor` 超时；动画/布局尚未稳定就取布局；菜单已提交但焦点尚未落到条目时按下 `ArrowDown`，按键落在触发器上。
4. **前端焦点用例**：`PermissionModePicker` 的菜单在 portal 中提交后才移动焦点，`ArrowDown` 若在“菜单可见但无条目持有焦点”的窗口内到达，roving focus 的 `indexOf(document.activeElement)` 为 -1，落到第一个条目（标准）而不是当前模式的下一个（信任）。

### 修复方式（CF-FLK-R2）
- Rust：adapter 的等待窗口改为由**可观测的租约续期**驱动（观察到 2 次 `lease_expires_at` 前进才结算），claim 租约取 30s（与 `objective.rs` 其它租约测试一致），使 runner 调度抖动不再能让“假定的睡眠时长”等价于“心跳一定跑过”。
- Rust 生产代码：`renew_claimed_remediation` 返回 `ClaimRenewal{Renewed, ClaimClosed, OwnershipLost}`；心跳在 `ClaimClosed` 时停止续租并抽干 adapter（不丢弃已提交的结算），在 `OwnershipLost`（他人持有 / epoch 被提升 / 租约过期）时维持原取消语义。判定在续租自身的同一事务内完成，不额外占连接。
- 真浏览器脚本：新增 `scripts/headless-acceptance-support.mjs`，回环端口先探测再用；等 Vite 真正返回包含目标夹具标记的文档；等元素真正拿到焦点；等到稳定帧后再取布局。
- 前端：在 layout effect 中同步聚焦已选条目，并在“无条目持有焦点”时把当前索引按已选项计算。

### 复现与验证
- 冻结复现：`SIGSTOP` 冻结测试进程 3s（等价于 runner 不给心跳 CPU）：修前 `objective_supervisor.rs:4119:10` panic（`objective remediation ownership changed; adapter cancelled`），修后通过。
- 负载复现：10 路 busy worker 占满 10 核，`objective_supervisor.rs` 两个租约用例各循环 20 次，修后 20/20；修前同一命令 8/20 失败。
- 三个真浏览器验收在 6 路 busy worker 下各循环 5 次，全部通过。
