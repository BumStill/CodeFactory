# M161：工作区 incident 且有未提交改动时自动保全并恢复，不再让用户处理

## 证据（v1.91.26 启动日志 10-22 17:49）
- `incident repair: branch is not restorable yet … error=managed workspace identity mismatch: branch changed with uncommitted changes; start a new session or commit/stash them, then continue`
- `startup: managed workspaces still in incident after self-heal count=7`。这些会话「继续」仍会失败，提示把 git 操作推给用户（用户在 Windows 上不会做）。
## 需求
- R1 自愈时若有未提交改动：先把改动完整保全（提交到 refs/codefactory/rescue/<objective>/<ts>，含未跟踪文件），再切回登记分支并把保全的改动重新应用；冲突时保留在 rescue ref、工作区回到登记分支，会话里给出一句人话说明和保全位置，会话可继续。
- R2 不丢任何文件；不改写已推送历史；全过程留审计。
- R3 启动时对现存 7 类 incident 执行一次（合成 fixture 验证，不对生产库做测试写入）。
- R4 测试（先失败）：切分支+未提交改动+未跟踪文件 → 自愈后继续且改动仍在；冲突路径 → rescue ref 存在、会话可继续。Windows 路径（verbatim 前缀）覆盖。
## 约束
- task-template；合成 fixture；授权交付（ceiling: merged）。

Release-Urgency: default

## Implementation Notes
- 验收入口为 attach_existing / heal_incident_workspaces，均不依赖 GUI，保留启动一次性修复机制。
- Spec / Compatibility / Payload / Observation / Release / AI Collaboration Harness；真实安装版由编排方独立发版后验收，本任务只交付到合并。
- R1/R2/R4：真实 Git 合成夹具验证 tracked、staged、untracked、冲突保全及登记分支历史不变；R3：七个合成 incident 批次恢复及重复运行幂等。
- 独立审查范围为 Git 数据安全与审计接口，不接触生产会话数据库。
