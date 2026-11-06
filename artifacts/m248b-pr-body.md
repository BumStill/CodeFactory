## 概要
修复 M248b：真实本地 Git 命令组合与只读外部 heredoc 不再依赖命令/路径词表证明；仅在已验证工作区与 OS 沙箱实际生效时豁免范围审批。

Scenario-Test: UI-009, RTE-005
README-Update: reviewed
README-Update-Reason: 不改变 README 已声明的系统沙箱读写边界、平台支持或安装契约，仅修复可信命令范围审批误判。

## Req ID ↔ 测试与先红证据
- R1：`m248b_real_samples_are_not_destructive_or_outside_sandbox_authority` 三条原样样本；修复前 36 passed/1 failed（outside_scope=true）。`m248b_reports_legacy_proof_predicates` 记录 rewrite/remote/outside/destructive：样本三 false/false/false/true，定位为本地 add/commit 与严格组合证明，不是远端。
- R2：只读 `permission_intents` 匿名聚合，134 条 bash，Git unproved 37、heredoc unproved 82；命令、路径、会话 ID 未写入仓库。详见规格。
- R3：`m248b_sandboxed_external_database_read_has_no_path_proof` 新增先红再修；现有网关注册身份测试增加三原样样本及外部读，断言 Allow 且 permission_intents=0；macOS 内核测试实际读外部并拒绝外写。
- R4：删除 `outside_write_sandbox_segment` 词表；保留既有 remote/Shell effects 分类；AST 在沙箱实际生效时不检查路径范围。
- R5：`m248b_history_and_remote_fences` 覆盖 push、force、删远端分支、reset、混合查询+写；AST socket/subprocess/rw SQLite 反例；m224 hooks/no-verify 禁止绕过测试；false sandbox 不得获得豁免。

## Applicable Harnesses
Spec：首提交原文规格与 Req 映射。Compatibility：保留无沙箱旧证明/审批与配置语义。Payload：bash/AST 输入与真实内核写边界。Observation：权限意图计数、匿名统计。Release：仅交付合并，真实安装版验收交接。AI Collaboration：独立只读安全与归因审查。无 UI 变更，Viewport/浏览器截图不适用。

## AI Collaboration
- context scope：permission_gateway / trusted_git / trusted_sandbox / Python AST / 相关测试。
- assumptions：OS 文件沙箱不限制网络，因此任意解释器不能直接豁免；AST 仍拒绝未建模网络/子进程能力。
- review point：独立安全审查、样本三根因与测试覆盖。
- validation result：新增回归 38 passed；全量 Rust workspace 通过（主库 2334 passed、12 ignored；包括其余 integration/crate/doc tests）；前端 166 files / 1122 tests passed，pnpm build 通过；scenario/readme/baseline/local harness 校验通过。

## 自验收与发布交接
本地 Rust 全量、前端全量/构建和治理校验通过。受影响 headless 入口已分别尝试：系统 Chrome 被 macOS bootstrap MachPort 权限拒绝，缓存 headless shell 启动 SIGSEGV，未产生浏览器通过证据；由 CI required scenario gate 重跑，不削弱门禁。真机验收由编排方发版后完成：安装新版本，下一单 trusted 模式任务核对 `trusted.git.unproved=0`；运行本地 add/commit、fetch/merge+gh 查询、外部只读 DB；确认远端写、破坏性改写、hooks 绕过和无沙箱审批仍保持。无 UI 变更，不需要会话内截图。

not live：本次授权上限为合并，不包含发版；无待决 Spec Amendment。
