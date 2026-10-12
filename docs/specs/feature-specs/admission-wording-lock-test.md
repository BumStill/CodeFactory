# 准入 wording 审计的确定性锁回归测试

## 背景与用户决定
修复 `admission_records_a_wording_constraint_without_waiting_on_the_write_lock` 在 #580 CI 的偶发失败；当前 1500ms 墙钟断言把 CPU 调度、文件系统与数据库操作耗时误判为 SQLite 写锁等待。只改测试，不更改产品准入语义；不放大超时、不重试。在当前受管分支、最新 main 上工作，禁止委派，交付至 PR 合并队列。

## 决策
使用真实 SQLite 前置条件：所有测试连接的 `busy_timeout=0`，在已持有写锁的事务下用另一连接尝试写入，必须立即得到 SQLITE_BUSY。释放探测事务后执行真实准入；其 wording 审计必须与新建 Objective 同事务持久化。若回归为池外写入，审计丢失，字段断言失败。取消墙钟阈值，保留持久事件和 capability_effect 字段断言。

## Requirements Traceability
| Req ID | 要求 | 最低证据 |
| --- | --- | --- |
| ADM-LOCK-R1 | 准入审计不跨连接等待自己持有的写锁 | 真实双连接 SQLITE_BUSY 前置断言 + 准入事件字段断言；池外写入负向变异应失败 |
| ADM-LOCK-R2 | 不依赖 CPU 调度墙钟阈值、不重试 | 移除 elapsed 阈值；CPU 压力下原测试循环 N=100 全过 |
| ADM-LOCK-R3 | 维持审计语义 | event_type=turn_wording_constraint，capability_effect=none |

## Applicable Harnesses
Spec Harness：本规格与 Req/test 映射。AI Collaboration Harness：单执行流（用户禁止 delegate_tasks），独立负向变异与自审。无产品行为、UI、发布或接口兼容性变更，不启用 Viewport / Release Harness。

## 测试矩阵
1. 双连接持锁前置断言：SQLITE_BUSY（code=5）。
2. 池外审计负向变异：同一回归测试必须失败，恢复生产实现后通过。
3. CPU 压力：N=100，逐次执行 exact test，零失败、无失败重试。
4. Rust workspace 全量、前端全量测试与 build、场景治理与 README 正文格式验证。
