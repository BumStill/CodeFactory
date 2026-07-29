# 会话执行治理 P0–P1

状态：交付中
范围：turn capability、delivery preflight/outcome、completion convergence、task segment、进度快照、sub-agent attempt

## 现场基线

- 正式版近 24 小时：31 条 root 用户输入、685 次工具调用、38 次 gate recovery、40 个 rejected candidate。
- “先出方案不修改”回合仍发生 11 次 `edit_file`。
- 13 次 `deliver_changes` 均返回业务 blocked，但 normalized tool status 均为 done。

## 执行清单

- [x] 正式版轨迹与现有代码/规格复核。
- [x] 业务、架构、UX 设计与 Requirements Traceability。
- [x] Failure-first：ReviewOnly 硬门禁。
- [x] Failure-first：delivery preflight 零副作用与 structured blocked。
- [x] Failure-first：completion recovery 最多一次。
- [x] Failure-first：segment/progress/attempt additive migration。
- [x] P0 实现与 focused verification。
- [x] P1 实现与 reload/viewport verification。
- [x] 真实 CodeFactoryDev 主路径。
- [ ] PR、CI、merge、release artifact 与正式 App 验收。

## AI Collaboration

- Context scope：正式版会话执行链与交付链，不改变普通手工 Git。
- Assumption：明确“发布/上线”构成 Deliver；普通“修复/实现”仅构成 Implement。
- Review point：权限不能扩大意图；业务 blocked 不能再记 done；P1 不删除旧审计数据。
- Validation result：前后端全量测试、Rust workspace check、治理校验通过；真实
  CodeFactoryDev 中只读回合仅调用 `read_file`，33.9 秒一次收敛；诱导
  `touch` 时框架在权限门禁前返回 `denied`，哨兵文件未创建；运行中进度原位
  更新，空数据库在 React StrictMode 下可直接进入一个内存草稿。
