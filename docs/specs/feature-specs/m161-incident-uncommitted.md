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
- 原任务未指定规格文件名，本文件按任务号归档；不改变 R1–R4 的含义。
- Spec / Compatibility / Payload / Observation / Release / AI Collaboration Harness：真实 Git 临时仓库、内存 SQLite、启动入口与重附着断言；发版及安装版验收由编排方另行执行。
- 测试矩阵：无冲突的 staged/unstaged/binary/untracked/deleted 文件；登记分支有冲突；同名未跟踪文件冲突；七条历史 incident 的启动批处理；同分支 dirty 幂等；Windows verbatim/UNC 路径；仓库或 worktree 身份不符时不得执行恢复。
- Git stash 对象（含 index、原 HEAD、未跟踪文件 parent）先持久写入 rescue ref，再切换；不 push、不改写分支历史。refs/stash 在恢复完成前也保留作额外保护。
- 会话通知使用 execution_workspaces 的 failure_detail 投影，审计记录保全引用和每阶段结果；系统技术失败由系统自动重试/自愈，不指示用户执行 Git 操作。
- 自验使用合成 fixture，不访问生产会话数据库。真机验收由编排方发版后完成：安装新版本、触发历史 incident 启动恢复、确认会话可继续、核对通知位置与审计引用、核对文件内容及原分支历史。
