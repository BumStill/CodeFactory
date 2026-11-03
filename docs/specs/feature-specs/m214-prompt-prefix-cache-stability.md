# M214：提示词前缀稳定（让 DeepSeek 前缀缓存真正生效）

- 状态：已实现，待真实应用验收（DeepSeek ≥30 次调用前后对比）
- 关联：M210（成本观测）、M212（按端点每日花费硬上限）
- 上游任务单：`task-m214-prompt-prefix-cache-stability.md`

## 1. 问题

DeepSeek 的前缀缓存（prompt prefix cache）按「请求的最长公共前缀」命中。10-30 一次
25 分钟会话花费 ¥7，`model_usage_events` 里 cached tokens 长期为 0 或极低，说明
**每一轮请求的前缀都在第 0 个 token 附近就断裂**，缓存从未命中。

用户可见后果：同样的工作，输入 token 以全价计费，成本约为应有值的 5–10 倍。

## 2. 断点清单（本次定位）

| # | 断点 | 后果 | 处理 |
|---|------|------|------|
| B1 | `user_visible_time_context()` 把「当前本地时间 `%Y-%m-%d %H:%M:%S`」写进 **base system prompt 开头** | 每条请求前缀从第一个段落就不同，缓存必然 0 命中 | 精度降到「日期 + 时区 + 偏移」，并移到 system prompt **末尾**；精确时刻由工具获取 |
| B2 | 易变块（时间、交付链就绪提示）夹在稳定块（人格/契约/项目知识/技能）之间或末尾之前 | 任一易变块的插入会顶掉后续稳定内容 | 装配顺序固定为「稳定 → 易变」，易变块严格置于末尾 |
| B3 | 回合内改写已发送历史（工具输出截断、通知插入、压缩） | 前缀在被改写位置断裂，且不可逆 | 只在显式压缩点一次性改写；压缩后重新建立稳定基线 |
| B4 | 缺少可回归断言与命中率诊断 | 回归不可见，只能靠账单发现 | 新增前缀指纹断言 + 每次调用缓存命中率诊断 + 健康检查阈值 |

实现位置（全部落在已注册的产品文件里，不新增独立产品文件）：
- `src-tauri/src/agent/mod.rs`：内联 `pub mod prefix_stability`（易变块渲染、纯追加断言、
  诊断聚合与 `CACHE_BREAK_ALERT_RATIO`）+ 系统提示词装配顺序；
- `src-tauri/src/agent/objective_diagnostics.rs`：把聚合结果接进会话诊断 JSON。

## 3. 需求与实现映射

- **R1 稳定前缀排序**
  - `base_system_prompt` 只保留**字节稳定**的契约文本（含「用户可见时间」的行为约定，
    但**不含**实时时钟）。
  - 项目知识（memory/README/config）→ 技能 → 用户偏好，顺序固定（沿用
    `context_budget::assemble` 的 supplied-order 渲染，优先级只决定预算分配，不改变顺序）。
  - `volatile_time_block()`（日期 + 时区 + 偏移 + 遇到精确时刻用工具）追加到
    system prompt 的**最后**。
- **R2 不改写已发送历史**
  - `prefix_stability::shared_message_prefix` 给出「上一次请求有多少条 wire message
    被下一次逐字节复用」；等于上次条数即纯追加；
  - 回合内新增内容只能追加；唯一允许的改写是显式压缩点
    （`agent/context_recovery.rs` 的 `authorize_compaction`，需显式授权，
    这是本设计里唯一合法的前缀例外）；
  - 改写不会被静默当作可缓存：诊断会把它计为断裂（见 `rewritten_history_is_visible_as_a_prefix_break`）。
- **R3 可回归断言**
  - `prefix_stability` 单测：同会话连续两次请求，第二次请求 = 上一次请求的完整前缀
    + 新的尾部；system prompt 与历史前缀**字节一致**（用字符串相等断言，不是长度断言）；
  - 覆盖跨回合与自动恢复回合；
  - 压缩点作为显式例外：压缩后新 epoch 的基线以压缩结果为准，但仍要求「其后每轮
    只在尾部增长」。
- **R4 诊断**
  - 每次模型调用记录 `cache_hit_ratio = cached_tokens / input_tokens`；
  - 聚合「缓存断裂率」= 断裂调用数 / 总调用数，断裂定义：`cached < 上次 input × 0.8`
    （与任务验收口径一致）；
  - 健康检查在断裂率 > 20% 时报错（阈值常量 `CACHE_BREAK_ALERT_RATIO = 0.20`）。

## 4. 验收

- `cargo test` 覆盖 R1–R3（含跨回合、自动恢复回合）：`prefix_stability` 与
  `context_budget`/`agent::tests` 的断言。
- 真实应用：用 DeepSeek 跑 ≥30 次调用的任务，统计 `model_usage_events`，
  缓存断裂调用占比 ≤ 10%，PR 中贴出前后对比。

## 5. 已收集证据

- **改前（红）**：`prefix_stability` 首轮测试在 `src-tauri/src/agent/mod.rs` 的
  prompt 头部检出 `11:26:13` —— 秒级时钟让每一次调用共享 0 缓存前缀。
- **改后（绿）**：`cargo test --lib prefix_stability` 10/10；`m214_` 1/1
  （含 DB 级健康检查：4 次调用、3 次断裂、断裂率 0.75 → `alert=true`）；
  merge `origin/main` 后 `cargo test --lib agent::` 1207 passed / 0 failed；
  合并前全量 `cargo test --lib` 2168 passed / 0 failed。
- **尚未完成（真实应用）**：DeepSeek ≥30 次调用的前后对比数字，需要带真实
  API key 运行应用；本 PR 只交付代码与单测，未宣称 live 验证完成。
  运行后看 `session_diagnostics.prompt_cache`（`breaks` / `break_ratio` /
  `alert`）以及日志里的 `[prompt-cache] ... cache_hit_ratio=`。

## 6. Applicable Harnesses

- Spec Harness：本文件 + 单测断言。
- Observation Harness：`model_usage_events` 前后对比（真实应用验收）。
- Payload Harness：不适用（无上传/体积路径变更）。
- Viewport Harness：不适用（无布局变更）。
