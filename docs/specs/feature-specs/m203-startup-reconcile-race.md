# M203：应用刚启动时新建的会话被启动对账判成 orphaned_by_restart

## 本机证据（v1.91.58，10-29 20:05–20:06）
- 20:05:30 应用启动（安装 v1.91.58）。20:06:03 通过 `--dispatch create_and_send` 新建只读验证会话 86579030（用户消息已落库），20:06:06 被插入一条「定时唤醒 thin-source-repair-…：存量等待仍无推进：系统自动重试/自愈：接续中断回合」，20:06:12 thin 状态变 `ended / orphaned_by_restart`。会话没有任何助手回复，任务未执行。
- 同次重启前就在跑的 M198/M191/M195 会话都正确保留（M200 对存量会话生效），问题只在「启动对账窗口内新建的会话」。

## 需求
- R1 启动对账只处理「本进程启动之前」就存在的回合/会话（按 created_process_instance 或启动时间戳划界）；本进程内新建的会话、回合一律不碰。
- R2 「存量等待仍无推进」的 source-repair 不得对刚创建、尚未开始第一回合的会话下手。
- R3 回归测试（先失败，合成 fixture）：启动对账进行中并发 create_and_send → 会话正常开始第一回合、不被结案；重启前的存量中断回合仍按 M200 续接。登记 `docs/testing/scenario-mappings.json`。

## 约束
- task-template（harness + SDD + TDD）；合成 fixture；不改生产数据。授权交付（ceiling: merged）。

Release-Urgency: immediate

## Implementation Notes
- Applicable Harnesses：Spec、Compatibility、Observation、Release（合并交接，不发版）、AI Collaboration。
- R1：固定进程启动边界，按根回合时间和 transport 所属进程排除新回合；启动等待扫描排除新会话与当前进程 transport。
- R2：新会话首回合尚未入库时不补 source-repair。
- R3：SQLite 合成 fixture 覆盖 admission 间隙、首回合 transport 入库、旧进程中断续接和重复对账；不读取生产数据。
- 真机验收由编排方发版后完成：安装启动后立即通过 dispatch create_and_send 派单，核对第一回合有助手回复、无 thin-source-repair、无 orphaned_by_restart；同时核对重启前中断会话仍自动续接。
- 无 UI 变更，不需要 Viewport Harness。按任务模板不调用子代理。
