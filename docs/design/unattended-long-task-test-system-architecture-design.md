# 长任务无人参与测试系统：架构设计

## 总体结构

测试采用“真实控制平面 + 确定性边界替身”：AgentLoop、Objective、权限、工具、SQLite migrations、receipt、reconcile 和正式可执行文件必须使用生产实现；只有外部 provider、时间、网络断点和远端平台使用可编程替身。

```text
scenario catalog
      |
      v
formal executable parent -------- deterministic provider
      |                                  |
      +-- worker process A -- real AgentLoop/tool/SQLite
      |        |
      |        +-- mutation committed + receipt durable
      |        X  hard kill
      |
      +-- worker process B -- startup reconcile/claim/resume
               |
               +-- same DB/objective + receipt replay + verify + settle
      |
      +-- receipt oracle: identity / trajectory / outcome / cleanup
```

## 两层系统测试

### PR hermetic system test

正式 binary 的 `--unattended-long-task-smoke <receipt>` 作为稳定入口。父进程创建隔离临时项目和真实 SQLite，启动同一 executable 的内部 worker：

1. 合成用户输入通过正式 admission 写入一次。
2. 确定性 OpenAI-compatible SSE provider 要求一次真实 `write_file`。
3. receipt 写入后，provider 在下一轮阻塞；父进程 hard kill worker A。
4. worker B 打开同一 DB，执行 startup reconcile、claim 和 resume。
5. provider 重发相同 mutation；tool backend 必须从 receipt replay，不能再次写入。
6. 执行真实验证工具，Completion Arbiter 将相同 objective 置为 completed。
7. 父进程查询 SQLite 和文件系统，输出机器可读 receipt。

这个入口不能编译为 test-only Rust binary，因为 release gate 必须对正式安装包中的同一个 executable 运行。

### Nightly / release artifact canary

Nightly 扩大到 provider 429/503/reset、partial output、permission channel drop、tool unknown result、CI/release reconcile、stop race 和 update safe point。Release job 在签名后的 Windows executable 上至少重复 `HLT-001`，并验证 receipt 内的 build SHA 与 tag SHA 一致。

## 四类 oracle

| Oracle | 断言 |
| --- | --- |
| Identity | session/root/objective/delivery identity 前后一致 |
| Trajectory | active → waiting_system → claimed → completed；没有伪 user turn |
| Outcome | 目标文件和验证证据正确，交付边界满足 |
| Safety | receipt 唯一、无 live lease、无 claimable remediation、临时进程回收 |

receipt 最低字段：`ok`、`scenario_id`、`build_git_sha`、`process_restart_observed`、`same_objective`、`user_message_count`、`human_prompt_count`、`side_effect_receipt_count`、`objective_status`、`live_owner_count`、`claimable_remediation_count`、`cleanup_ok`。

## 故障模型

- `crash_before_effect`：允许按原 identity 重试。
- `crash_after_effect_before_receipt`：只允许观察/对账；证明确未执行后才能获得新 permit。
- `crash_after_receipt`：相同 fingerprint 只能 replay receipt。
- `provider_zero_output`：可按 durable recovery policy 路由或退避。
- `provider_partial_output`：禁止盲目 replay root turn。
- `repeated_claim`：`attempt_index` 每次 claim 增长；上限查询按 claim 总数而不是 row count。
- `cancel_vs_claim`：session cancel intent 是持久 fence，取消完成前不能报告成功。

## 测试数据和兼容性

- fresh fixture 由正式 migrations 创建。
- 旧版本 fixture 只包含最小 schema/typed state，不包含用户内容；每个 fixture 标记 source version 和预期迁移。
- 每个场景使用固定 seed、虚拟时钟或有界 deadline；禁止随机 sleep。
- provider script 保存请求序号、request digest 和已发送 output 类型，便于判定重放是否合法。

## CI 分层

| Gate | 时间预算 | 场景 |
| --- | ---: | --- |
| PR required | 5 分钟内 | HLT-001/002/003/004，fresh + 最近 release DB |
| Nightly | 30 分钟内 | 全 fault matrix、增量 steer、delivery/update |
| Release exact binary | 10 分钟内 | HLT-001 + build identity + cleanup |
| 生产只读 | 24h 窗口 | ownerless、重复 receipt、CTA、恢复上限、false complete |

## 防止测试自身撒谎

- 场景先验证 fault 确实触发；未触发即失败。
- 每个 smoke 输出完整 receipt 并由工作流字段级解析，不能只看 exit code。
- CI 不配置 flaky retry；偶发失败保留 artifacts 和 seed。
- test double 只能替代系统边界，不能替代待验证的状态机、持久化或工具 backend。
- structural contract test 只确保门禁没有被删除，不替代行为 smoke。
