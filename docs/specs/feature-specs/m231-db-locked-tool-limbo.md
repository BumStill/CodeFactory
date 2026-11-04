# M231：工具调用撞上 `database is locked` 后回合停住，状态是等待中，却没有任何东西会叫醒它

## 现场（2026-11-04，v1.91.7x，电脑开盖醒着）
- 会话 2ee42f66，10:58:29：最后一条工具结果是 `update_plan` 返回 `Database error ... (code: 5) database is locked`，回合就此停住。
- `thin_session_state` 是 `waiting_wake`，理由还是过期的「10:52 重置」；`wakeup_schedules` 里没有这个会话的 pending 行。没有任何人会改变这个状态，11:01 编排方手工发消息才继续。
- 这不是偶发：`tool_calls` 里 10-22 至 11-04 至少 15 个「会话+工具」组合出现过这个错误，涉及 `bash`、`update_plan`、`read_file`、`write_file`、`edit_file`；`messages` 里共 42 条 tool 消息。最近一次除上面外还有 e7d2bb48（11-04 08:42，`bash`）。
- 工具本身多半没有问题，只是应用记录或准入这一步写库时撞了锁，错误却被当成工具结果交给模型，或者让回合提前结束。

## 用户看到的问题
会话看起来「在等待」，但永远等不到；用户不发消息就一直停着。这违反 E2E-001（无人参与）和「不许造中间态」。

## 决定
- SQLite 写锁竞争是应用内部的瞬时状态，不是工具失败，也不是模型需要处理的事情。
- 只修这一族：写锁竞争的处理，以及「等待中必须有唤醒来源」这条不变量。不改模型路由和等待文案的其他逻辑。

## 需求
- R1：先查清每条路径。对 `update_plan` 和 `bash` 这两个真实样本，确认 `code: 5` 是在哪一步写库时产生的（工具执行前的准入、执行中、结果落库），以及连接是否配置了 `busy_timeout`、是否有「先读后写」的 deferred 事务升级（这种情况下 busy_timeout 不生效，会立即返回 5）。结论写进规格的 Implementation Notes。
- R2：工具路径上的写库，遇到 `SQLITE_BUSY`/`SQLITE_LOCKED` 要有界重试（带退避，总时长上限写明）。需要写的事务用 `BEGIN IMMEDIATE`，或用等价方式避开升级失败。重试不得重复执行有副作用的工具：工具已经执行过时，只重试落库这一步，不再次运行工具。
- R3：重试耗尽后，内部错误不得原样作为工具结果给模型。回合按「可恢复的内部错误」处理：登记一条真实的短延迟唤醒重试，等待理由写真实原因（例如「本地数据库忙，稍后自动重试」），次数有上限。超过上限后明确失败，并交代试了什么；不许无限等待，也不许静默。
- R4：不变量：会话进入 `waiting_wake` 时，必须在同一事务内有一条 pending 唤醒，或者有明确的外部等待对象（待批审批、PR 监视等）。写入时不满足就不许进入该状态。
- R5：启动和定期巡检时，发现「`waiting_wake` 但没有唤醒来源」的会话，要补一条立即唤醒或转为明确失败，并留一条可查记录。已结束或用户已停止的会话绝不能被复活（M226/M230 的教训）。
- R6：过期的等待理由不得留着：重置时间已过、又没有 pending 唤醒的理由，不能继续显示给用户。

## 测试矩阵（先看到失败）
- T1（R2）：一个连接持有写锁，工具落库在锁释放后成功，工具只执行一次（用副作用计数断言）。
- T2（R2）：覆盖 deferred 事务升级场景：修复前立即返回 5，修复后成功。
- T3（R3）：写锁一直不释放，模型上下文里看不到 `database is locked`，会话得到 pending 唤醒和真实理由；超过上限后进入明确失败。
- T4（R4）：构造没有唤醒来源的状态写入，断言被拒绝。
- T5（R5）：`waiting_wake` 无唤醒来源的会话被救回；`ended` 和用户已停止的会话不变。
- T6：真实文件 SQLite + 两个并发写者的集成测试，不用内存库，不用 mock。

## Applicable Harnesses
Spec、AI Collaboration；影响回合恢复，追加 E2E-001 相关场景映射，以 `validate_scenario_test_governance.py --ci` 打印的 ID 为准。

## 验收（真机）
- 真实 app 上用一个外部进程短暂持有写锁，同时会话在跑工具：会话不报 `database is locked`，自动继续，不需要人工发消息。
- 发版后一周查 `tool_calls`：不再有新的 `database is locked` 工具结果；`waiting_wake` 且无唤醒来源的会话为 0。

## Implementation Notes

### R1 路径核查
- 生产连接统一经过 `storage::db::pool_connect_options`，每条连接显式配置 WAL、FULL、foreign_keys 和 10 秒 busy_timeout，池获取上限 30 秒。
- `update_plan`：工具执行中 `tools/update_plan.rs::execute` 使用 deferred `db.begin()`，先读最新 revision 再插入事件；并发工具会在读后写升级时直接返回 code 5，busy_timeout 无法保护这次升级。新增 8 个真实文件并发执行测试已先复现该错误；改用共享 `begin_write`（BEGIN IMMEDIATE）后通过。
- `bash`：shell 进程本身不使用应用 SQLite；写库边界在执行前的 `DesktopToolBackend::mutation_preflight`（已有 BEGIN IMMEDIATE）、`SqlitePersistence::tool_execution_started_at_ms`，以及执行后的 `trajectory::record_terminal_tool_outcome` / metadata。后者此前绕过共享写锁重试，现统一进入 BEGIN IMMEDIATE；执行起始时钟和 started 轨迹也同样保护。历史样本只有工具结果，无法仅从 code 5 断定某次 bash 命中了哪个边界；上述结论来自生产调用链与并发回归，不虚构原始现场堆栈。

### R2 预算与副作用边界
- `begin_write`：最多 3 次获取，50/100ms 退避；生产 busy_timeout 每次 10 秒。若发现池中泄漏事务，仅回滚闲置连接，再给一次获取机会；最多 4 次，busy 等待 40 秒 + 150ms 退避 + 最多 800ms 闲置巡检，另受每次池获取 30 秒上限约束，保守总预算不超过 161 秒（不含已获得锁后的正常事务执行）。
- 单条 `retry_write` 同样 3 次、50/100ms；生产 busy 等待总计 30.15 秒。测试连接使用 20ms 以快速验证耗尽。
- 仅重试写锁获取或纯数据库操作，不包裹工具执行。真实工具轨迹测试在副作用计数递增后持有独立连接的写锁，断言落库等待、释放后成功、模型 replay 内容正确且计数为 1。已派发的 mutation 仍由现有持久收据与防重放机制保护。

### R3–R6 状态与恢复
- 内部锁错误在 desktop persistence 边界返回 `local_store_contended`，不生成原始 `database is locked` 工具消息。薄会话错误结算原子写入真实 5 秒唤醒与本地数据库忙理由；第三次明确失败，不沿用模型恢复时间。
- `thin_state::save` 在 BEGIN IMMEDIATE 内核对 pending 唤醒、运行后台任务或活跃 PR 观测。无源且不可识别外部等待直接拒绝；外部等待在同事务补建系统恢复唤醒才允许写入。用户停止状态在补建前再次检查。
- 启动与定期巡检复用现有入口；数据库孤儿等待补立即唤醒并写审计，补建时同事务替换旧理由；结束与停止会话不在巡检范围内。
- 旧测试需要直接注入历史坏数据，而不能再通过生产 save 制造 R4 明确禁止的新状态；未改变这些测试的终态、来源、计数、恢复或防复活断言。

### 验收分工
真实安装版验收与发版后一周审计由编排方发版安装后完成；本任务负责文件 SQLite 并发集成测试、完整回归与合并后 §8 收尾，不自行发版。
