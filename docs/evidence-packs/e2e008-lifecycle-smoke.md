# E2E-008 L2 验收记录

## 实现与验收

新增正式 binary loopback 生命周期 smoke；不启动真实浏览器、不访问用户 profile/生产 DB。

| Req | 实现与证据 | 结果 |
| --- | --- | --- |
| R1 | 源码目录外运行正式二进制；真实 WebSocket 握手、idle heartbeat、旧代际取消、旧连接关闭、重连、半开超时、同一 bridge/tab、精确 6 条命令 ledger | PASS |
| R2 | 注入失败先写固定匿名 receipt，再 exit 1；成功 receipt 字段逐一 assert | PASS |
| R3 | Windows PR CI 同阶段运行正式 binary 并校验字段；四项 Node 合约 | PASS local，待 CI |
| R4 | registry 增加 binary 与 PR required target；保留 partially_implemented、L3/L4 | PASS contract，待治理校验 |
| R5 | 首次 Node 0/4，实施后 4/4；长任务记录进展 | PASS |

完整 Rust workspace 测试通过；前端 151 files / 1013 tests 通过，包含 lightModeAudit；pnpm build 通过。首次完整 Rust 测试因 unattended_cli_entry 录制 facade 缺少新函数编译失败；补齐 facade，原顺序断言未放宽，原命令重跑通过。

## Harness 与协作

Spec：规格原文落盘及 R1–R5 映射。Compatibility：原 smoke 分派顺序与完整回归。Observation：匿名字段级 receipt 和失败路径。Payload：本地 synthetic WebSocket。AI Collaboration：context scope 为任务/备份 diff/transport/registry；assumptions 为 L2 不替代 L3/L4；review point 为备份清理竞态及测试 facade；validation result 为正式 binary 成功/失败、全量测试及构建。任务要求不调用 delegate_tasks，采用单执行流。

## 真实应用验收

无 UI 改动，无布局截图。真机验收由编排方发版后完成：安装精确版本、同一会话选择 tab、触发扩展空闲/瞬断/交错重连/升级，核对自动续接、无人工 prompt、操作不重复及审计日志。真实桌面和 exact release artifact 尚未验证（not live）。

无待决 Spec Amendment。L3/L4 为后续建议，不在本单旁做。

Scenario-Test: ALL
README-Update: reviewed
README-Update-Reason: 仅增加内部 synthetic smoke 与 CI 门禁，不改变公开产品、安装或隐私承诺。
