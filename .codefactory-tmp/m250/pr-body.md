## Summary
修复 M250：非零退出不再自动被归因于沙箱。只在原始命令输出含 macOS 实测 EPERM 文本 `Operation not permitted` 时生成提示，不修改 Seatbelt 策略。

## Requirements / TDD
| Req | 测试 / 证据 | 状态 |
| --- | --- | --- |
| R1 | m250_observed_denials_keep_hint_and_path；实际 Git 拒绝和 sandbox_apply 输出格式 | pass |
| R2 | m250_ordinary_failures_have_no_sandbox_hint：false、grep 无匹配、git config 缺键、测试失败、编译失败，真实进程退出码及原始输出断言 | 旧实现先失败，修复后通过 |
| R3 | m250_observed_denials_keep_hint_and_path / m250_real_seatbelt_acceptance | 格式回归通过；新建 Seatbelt 被当前嵌套沙箱拒绝，真实用例 NOT COVERED，不计通过证据 |
| R4 | 无策略生成变更；M224 默认拒绝测试 | 待全量测试 |

## Applicable Harnesses
- Spec：版本化 M250 规格，首个提交仅规格。
- Compatibility / Payload：保留退出码、stdout、stderr；只调整提示判定，不放宽权限。
- Observation：实际命令错误格式与字段断言；嵌套沙箱限制明确记录。
- Release：本次只授权合并，不发版，not live。
- AI Collaboration：context scope 为沙箱模块、bash 提示分支、M224/M250 测试和任务模板；assumptions 为实测 macOS EPERM 可作为拒绝证据，而普通非零状态不可以；review point 为判定与原始输出、策略不变；validation result 在 CI 和下述验证记录中补齐。独立只读代码/验收审查已提交并行入口。
- Viewport 不适用：没有 UI 修改，不要求浏览器截图。

## Validation
- `rustc --edition=2021 --test src-tauri/tests/m250_sandbox_failure_hint.rs -o .codefactory-tmp/m250/hint-tests && .codefactory-tmp/m250/hint-tests --nocapture`：普通失败先红后绿；真实新建 Seatbelt 明确 NOT COVERED。
- `pnpm test`：166 个文件、1,117 项测试通过（含 lightModeAudit）；`pnpm build` 通过。
- 源码目录外 `/private/tmp` 运行编译后的回归产物：普通失败及拒绝路径断言均通过。
- `python3.11 tools/governance/run_scenario_harness_gate.py --stage local --repo . --policy-repo .` 与治理基线通过；系统 Python 3.9 缺少 tomllib，改用现有 3.11 后通过，未修改治理脚本。
- Rust workspace 全量：后台进行中，交付前更新。

## 真实应用验收分工
真机验收由编排方发版后完成：安装新版本，在信任模式会话运行 `git config --get` 不存在的键，核对退出码 1、无沙箱提示；再写工作区外 synthetic 文件，核对未创建、带拒绝提示及路径。本次未发布，not live。

## §8 收尾
合并后确认 merged=true、squash commit 位于 origin/main，核对远端分支删除、工作区干净，清理任务独立缓存及临时文件。不删除受管工作区。不提交 Release-Urgency trailer，无待决 Spec Amendment。

Scenario-Test: RTE-005
README-Update: reviewed
README-Update-Reason: 仅修正命令失败诊断，不改变公开安装、隐私、平台或沙箱边界承诺。
