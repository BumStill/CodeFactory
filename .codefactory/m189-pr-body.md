## M189 修复
R1 根因：心跳每 12 poll 复用重启恢复，control.status<>'cancelled' 接受 completed，提示无条件取最后工具；去重键含 turn.updated_at，每次续跑变化导致无限恢复。
R2：已完成/最终回答过滤、同进程隔离、启动清理前冻结旧 active 事实、工具 ID 持久 claim 和根回合唯一索引。

| Req | 自验收证据 | 结果 |
| --- | --- | --- |
| R1/R2/R4 | m189_final_answer_survives_three_recovery_sweeps | 先失败（completed 回合入选），修复后通过，三轮零候选/notice、状态 ended |
| R2/R4 | m189_restart_claim_survives_real_process_restart | 三个真实子进程首次 claim=1、后续=0，notice=1，同进程 claim=0 |
| R3 | m189 规格家族审查表 | 有待裁决缺口，未声称完全满足 |

## Spec Amendment（待裁决）
周期 wake 成功投递无总次数上限、enqueue 在 CAS 之前是至少一次语义、PR repair 原因变化无总 claim 限制。建议明确周期次数/截止时间、独立 durable claim ledger、耗尽时系统事故停泊。未擅改持续提醒语义。

## Harness 与验收
Spec：原任务规格和 Req 映射。Compatibility：真实 SQLite、启动清理与历史 smoke。Observation：候选/claim/notice/状态断言。Release：只合并，另行发版，not live。AI Collaboration：context scope=thin/startup/history smoke；assumption=旧进程 active 是中断事实；review point=失败回归、启动清理顺序；validation result=针对测试通过，完整检查见 CI。模板禁止 delegate_tasks，采用单执行流。
真机验收由编排方发版后完成：同一进程完成任务后观察三个 sweep，无新增恢复消息/模型调用，ended/idle；真实工具中断重启仅恢复一次。仅合成数据。无 UI 布局修改。

Scenario-Test: HLT-001, HLT-003, HLT-004, HLT-005, CXD-001, CXD-002, RTE-003, RTE-004
README-Update: reviewed
README-Update-Reason: 内部恢复正确性修复，不改变公开产品、安装、隐私及平台承诺。

Release-Urgency: immediate
