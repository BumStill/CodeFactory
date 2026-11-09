# M267 订阅额度读数新鲜度与判定留痕

## 背景（真实故障）

一次长任务里，ChatGPT 订阅端点在 07:28 读到周窗口 48%，随后 52 分钟内发起
131 次 Codex 调用。后台实时刷新一直失败，没有新读数到达；周窗口用量实际涨到
89%，直到 08:20 新读数通过响应头出现，应用才第一次落回 DeepSeek。护城河在
两段调用里形同不存在。

这不是"没有上限"，而是"上限只看一个冻结的数字"。

## 根因

`QuotaLedger::snapshot` 一旦存在服务读数就无条件返回它：

```rust
if let Some(server) = entry.server {
    return Some(server);   // 之后的调用全部无效
}
```

`record_local_usage` 仍然在累加，但结果永远不被读取。于是：

- 读数只在"被观测到的那一刻"有效，之后无论多少调用、多久时间，判定都不变；
- 读数的年龄没有任何传递，判定处无从知道它是 1 分钟前还是 1 小时前；
- 判定本身不留痕，事后只能靠复现那一小时的调用才能解释为什么放行。

## 需求

### R1 读数边界

只有 `x-codex-primary-*` / `x-codex-secondary-*` 响应头携带用量（窗口长度为 0
或剩余时间为 0 的整组头按 M244 丢弃，不计入读数）。没有头的端点不产生读数，
继续按本地估算处理。报告时本项属于"该端点这次也没带用量头"，不作为根因；根因
是 R2。

### R2 读数新鲜度（固化，避免回退）

1. 任何服务读数都只描述**它被观测到的那一刻**。
2. 读数之后本地继续观测到的加权用量，按窗口分别累加到同一读数上：
   `effective = clamp(reading + since_reading / budget)`。
3. 新读数到达时 `since_reading` 清零——它已经包含此前全部调用。
4. 同样以 300 秒（`SERVER_READING_MAX_AGE_MS`，与实时刷新周期同量级）作为
   "陈旧"界线；超过界线的判定在记录里标 `stale`。
5. 服务窗口的重置点被读数采纳为本地窗口锚点，估算与真实窗口对齐。
6. M164 语义不变：窗口已过重置点但读数未刷新时，仍按 100% 保守处理。

### R3 判定留痕

每次判定（放行 / 拦截 / 无读数）都记录：时间、端点、窗口、来源、读数观测时间、
读数年龄、用量百分比、上限百分比、结论、是否陈旧。进程内保留最近 200 条，
并在应用数据目录追加 `quota-decisions.jsonl`。

### R4 不破坏既有约束

M244（空窗口头不得阻断）、M164（过期读数不授权）、本地估算回退与硬上限语义
全部保留，旧测试保持通过。

### R5 文案

到达上限后给用户的文字必须写清：哪个窗口、已用多少、上限多少、什么时候重置。
交接提示、路由阻断原因两处都要满足。

## 实现

- `src-tauri/src/agent/quota_cap.rs`：`EndpointLedger.since_reading`、
  `snapshot` 叠加增量、`QuotaDecision` 与 `QuotaLedger::install_audit_log` /
  `recent_decisions`、`CapBreach::window_label`、`handover_notice` 补上限。
- `src-tauri/src/agent/failover.rs`：阻断原因带上窗口/用量/上限。
- `src-tauri/src/lib.rs`：启动时把判定日志装到应用数据目录。

## 验收证据

`cargo test --lib quota_cap::`，其中：

- `m267_calls_after_the_last_reading_still_cross_the_cap`：48% 读数 + 20 次调用
  → 第一次越线即拦截，最终 88%（旧实现全程放行）。
- `m267_a_stale_reading_still_binds_through_local_accrual`：读数超过新鲜度界线
  30 分钟，仍然按累加拦截，记录标 `stale` 并带读数年龄。
- `m267_every_decision_is_recorded_with_its_evidence`：判定记录与
  `quota-decisions.jsonl` 行内容断言。
- `m267_handover_notice_states_the_cap`：文案含窗口、用量、上限、重置。
