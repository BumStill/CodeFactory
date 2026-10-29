# M200：升级重启把在跑的会话判成结束 + 派单「失败」却已建会话导致重复

## 本机证据（10-29）
1. 会话 5ee1876f（M194）13:58 还在改代码、33 条测试刚通过；14:11 安装 v1.91.55 重启后，thin 状态变成 `ended / orphaned_by_restart`，没有任何自动续接，只能人工发消息续上（零介入试验第 2 次介入）。E2E-001 要求：重启后必须自动完成或进入真实不可恢复终态。当时模型不可用（ChatGPT 额度到顶、DeepSeek 高峰），疑似"重启续接时无模型"走了 orphan 结案分支，而不是挂 model-availability 唤醒等待。
2. `codefactory --dispatch` `create_and_send` 在无可用模型时返回 `ok:false delivery_failed "did not start a new turn in time"`，但会话和用户消息已经落库、并挂了 model-availability 唤醒（c7023ffb、82191e46）。调用方以为失败再派一次 → 同一任务 3 个会话，16:01 会同时开跑。

## 需求
- R1 重启续接：重启时被打断的会话若暂无可用模型，必须进入 waiting_wake（事实型 model-availability 唤醒），模型恢复后自动续接；不得以 orphaned_by_restart 结案。只有工作区丢失等真实不可恢复情况才结案，并写人话原因。
- R2 dispatch 语义：会话/消息已持久化时一律返回 `ok:true`，附 `status: "started" | "queued_waiting_model" (含预计时间)`；只有真的没落库才 `ok:false`。可选：`create_and_send` 支持幂等键 `client_request_id`，重复请求返回同一会话。
- R3 回归测试（先失败，合成 fixture）：无模型时重启 → 会话 waiting_wake + pending model-availability 唤醒，模型恢复后自动续跑；无模型时 dispatch → ok:true queued；同一 client_request_id 两次 → 同一会话。场景映射核对：本改动涉及的产品文件已全部被 `scenario-registry.json` 的变更模式覆盖（CXD-002 / HLT-001 / HLT-004 / HLT-005 / RTE-003 / RTE-004），`scenario-mappings.json` 无需新增条目（已用治理校验函数复核为 0 个未映射产品文件）。

## 约束
- task-template（harness + SDD + TDD）；合成 fixture；不改生产数据。授权交付（ceiling: merged）。

## Implementation Notes
- 按 R3 落实可选幂等键；只使用合成 fixture，不访问生产会话数据库。
- Applicable Harnesses：Spec、Compatibility、Observation、Release（仅合并交接，不发版）、AI Collaboration。
- Requirements Traceability：R1 对应 Rust 重启/唤醒持久化集成测试；R2 对应真实 chat store 派单回执测试及协议测试；R3 对应重复请求、无落库失败反例与场景映射。
- 已验证（本地）：`cargo test --lib m200_` 覆盖 4 条合成断言（等待唤醒不被孤儿清理、回执必须精确匹配已落库消息、同键复用同一会话与首条消息且内容冲突拒绝、幂等 UUID 稳定）；`vitest` 派单/协议 54 条通过；`tsc --noEmit` 与 `pnpm build` 通过。
- 真机验收由编排方发版安装后完成：运行中安装重启且模型不可用，核对 waiting_wake 与 model-availability 唤醒；恢复模型确认自动续接；通过本地 dispatch 连续提交相同幂等键确认只建一份会话，回执包含实际排队时间。
