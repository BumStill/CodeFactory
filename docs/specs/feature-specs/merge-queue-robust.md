# 合并队列健壮性修复（M94 / M78 / M68 同族）

## 用户授权与信任边界

用户 2026-10-17 明确批准修改 `scripts/merge-queue.mjs`、`.github/workflows/merge-queue.yml` 以及为此需要的场景注册表条目。候选 PR 不能自证：若 `scenario-gate-pr` 因 cannot self-modify 失败，交给编排方按治理流程处理，不绕过、不改门禁；其它检查必须全绿。本任务交付止于合并，不发版。

## 原始任务规格（原文）

### 真机证据
- **M94**（2026-10-17 00:24–00:42）：merge-queue.yml 连续失败，报错为 `gh pr edit 662 --add-label "merge-queue: blocked: required check check-rust fa…"` → `'merge-queue: blocked: …' not found`。脚本在第一个被阻断的 PR 上崩溃，后面已就绪的 #665/#666 也全部停滞，持续 2 小时。
- **M78**：`schedule */10` 经常一个多小时都不触发；PR 在 CI 结束后才打上 armed 标签时，没有任何事件能唤醒队列，编排方只能每几分钟手动 dispatch 一次。
- **M68 同族**：squash 合并时 `Release-Urgency: immediate` trailer 丢失（#648、#654 合并后 plan_release 都显示 immediate=0），紧急修复因此不会自动发版。

### 要做的
1. **M94**：单个 PR 的阻断或失败只能影响这个 PR。阻断原因改用已存在的固定标签（例如 `merge-queue: blocked`，不存在就先创建；名称固定，不拼接动态文本），具体原因写进 PR 评论。任何单 PR 错误都要被捕获、记录，然后继续处理下一个 PR，整次运行不得因此退出非零。用合成的 gh 客户端写测试：第一个 PR 打标签失败时，后面已就绪的 PR 仍然会被合并。
2. **M78**：merge-queue.yml 增加 `pull_request_target: types: [labeled]` 触发（只在加的是 `merge-queue: armed` 时运行），保留 schedule 和 workflow_run 作为兜底。注意安全：pull_request_target 下不得检出或运行 PR 的代码，只运行 main 上的脚本。
3. **squash trailer**：合并时 squash 提交信息的最后一段必须保留 PR 分支提交里的 `Release-Urgency:`（以及 `Co-Authored-By:`）trailer，参考记忆 `project_squash_trailer_footer_paragraph`：trailer 必须在最后一个段落。合并后用 `tools/release/plan_release.py` 能读到它。写测试覆盖：单提交、多提交、trailer 不在最后一段这三种情形。
4. 每一项都先写失败测试，再修。

### 约束
只改上面列出的文件和相应测试或注册表条目；只用合成数据；其余同模板。

## Requirements Traceability / 验收矩阵

| Req ID | 要求 | 可执行验收 |
| --- | --- | --- |
| CF-MQR-R1 | M94 单 PR 错误隔离、固定标签、评论原因 | synthetic gh label / merge / checks 故障后第二个 PR 仍合并；标签名称及评论字段断言 |
| CF-MQR-R2 | M78 armed 事件触发且只运行受信任默认分支 | workflow trigger、job 条件、checkout ref 和兜底断言 |
| CF-MQR-R3 | squash 保留 urgency 与作者 footer | 单提交、多提交、非末段 trailer；临时合成 git 仓库提交后真实 plan_release.py 断言 immediate=1、slot=patch、skip=false |
| CF-MQR-R4 | 保持严格门禁与现有行为 | 既有 CF-MQ-R1…R6 回归；治理及场景门禁 |

## Applicable Harnesses

Spec Harness：本文件与需求测试对应；Compatibility Harness：现有队列及严格 required checks 不变；Release Harness：真实发布规划器解析合成 squash 提交；Observation Harness：逐 PR error/dropped/merged 结果与阻断评论；AI Collaboration Harness：按任务要求单执行流，不调用 delegate_tasks，审查范围限于本任务文件，合成输入无生产数据。

## Implementation Notes

- 单 PR 失败在本次运行去重，避免持续重试使其它 PR 饥饿；仓库级列表或 ruleset 错误仍 fail closed。
- GitHub 客户端仅作为合成适配器注入；不执行 PR 中的代码或提交信息。
- 先运行新增测试：原有 16 项通过、新增 8 项失败；实现后 24 项通过。新增测试路径使用 URL 解码以兼容本机含空格工作目录。
- 本任务不改变桌面 UI 或 Rust。真机验收由编排方发版后完成：检查 armed 标签事件启动默认分支 workflow、一个阻断 PR 不影响其它就绪 PR、squash 最后一段保留 trailer 且发布规划器识别。信任根自修改门禁必须保留，由编排方处理。
