## M174 交付证据

R1/R3：`m174_cross_process_merged_wait_is_closed_out` 使用独立子进程与真实 SQLite，覆盖自有/接手分支、启动存量、旧 silent closeout、合并后普通等待回答；断言 §8 唤醒、ended 与幂等。先观察“还没合并”断言失败，再修复。

R2：上述测试调用 `reconcile_waiting_on_startup`，覆盖全部持久 waiting_wake；复用限频 PR 观测及持久投递。

关闭 PR：`m174_closed_pr_final_settles_instead_of_waiting_forever` 先看到 WaitingWake != Ended，再修复。

前端：pnpm test，159 files / 1055 tests passed；pnpm build passed。
Rust：按模板独立 target 执行 workspace 全量测试，结果随后补齐。

Harness：Spec（原始任务已版本化）、Compatibility（旧 SQLite 与跨进程）、Observation（PR 字段、事实文案与投递幂等）、Release（仅合并和 §8 收尾）、AI Collaboration。
AI Collaboration：context scope 为 thin PR 观测、持久唤醒及启动恢复；assumptions 为实际交付工具结果决定 PR 归属；review point 为终态优先级及重复收尾；validation result 为先红后绿回归与全量测试。已发出两个只读独立审查任务，不允许改文件。

真机验收由编排方发版后完成；本任务 not live、不发版。安装后分别验证外部 squash/队列合并、接手他人分支合并、重启已合并/关闭存量会话；核对自动 §8、ended、事实文案、幂等及分支/工作区清理。无 UI 布局变更，不新增截图。

Spec Amendment：无。

Scenario-Test: HLT-001, HLT-002, HLT-004, HLT-005, CAP-007
README-Update: reviewed
README-Update-Reason: 内部 PR 会话恢复修复，不改变公开安装、平台或隐私承诺。

Release-Urgency: immediate
