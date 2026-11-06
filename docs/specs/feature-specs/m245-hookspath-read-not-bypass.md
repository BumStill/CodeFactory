# M245：只读取 core.hooksPath 的命令不算绕过门禁

## 背景与用户决策

信任模式把 `git config --get core.hooksPath` 等只读命令误判为 `trusted.sandbox.bypass`，中断无人值守工作。授权交付至 merged，不发版；真实安装版验收由编排方发版后完成。

## 规格（原文）

- R1：只读取或只提到 `core.hooksPath` 的命令不算绕过。至少包括：`git config --get core.hooksPath`、`git config --get-all core.hooksPath`、`git config core.hooksPath`（不带值就是读取）、`git config --list`、`grep`/`sed` 等命令的参数或文件内容里出现这个词。
- R2：修改仍然拦下。至少包括：`git config core.hooksPath /tmp/x`、`git config --unset core.hooksPath`、`git config --unset-all core.hooksPath`、`git config --global core.hooksPath /tmp/x`、`git config --local --replace-all core.hooksPath /tmp/x`、`git -c core.hooksPath=/dev/null commit`、`git -c core.hooksPath= commit`、`GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_*` 环境变量注入同一配置。
- R3：一行里有多条命令时逐条判断：`git config --get core.hooksPath; git config --unset core.hooksPath` 必须拦下。分号、`&&`、`||`、管道、换行、子 shell 都要覆盖。无法解析的命令按拦下处理（fail closed）。
- R4：查清 `gate_bypass` 命中后到底是直接拒绝还是进审批，让代码和 M224 规格一致。如果两者冲突，写 Spec Amendment，不要自行改变含义。
- R5：`--no-verify` 和 `CODEFACTORY_SKIP_SYNC_GATE` 的现有判定不放宽。这是安全边界，本单只修 `core.hooksPath` 的误判。

## Requirements Traceability / 测试矩阵

| Req ID | 最低证据 |
| --- | --- |
| R1 | 表驱动只读/文本命令测试先红后绿 |
| R2 | 表驱动写入、删除、运行时覆盖与环境注入拒绝测试 |
| R3 | 各控制操作符、子 shell、解析失败、混合命令回归测试 |
| R4 | 网关集成测试断言直接 Deny、零审批事件、含修复指引 |
| R5 | 原测试全部保留；追加既有绕过标志回归 |

## Applicable Harnesses

Spec、Compatibility、Payload、Observation、AI Collaboration；Release Harness 仅覆盖合并交接，本单不发版。无 UI 变更，不触发 Viewport Harness。

## Implementation Notes

- 现场代码与 M224 R6 冲突：`permission_gateway.rs` 注释写直接拒绝，实际调用 `request_permission_with_reason`。按既有 M224 R6 修复为直接拒绝，不改变规格含义；PR 以 Spec Amendment 记录冲突及处理建议。
- hooksPath 判定按命令/参数边界解析，不再把任意文本出现当成写配置；解析不明确时 fail closed。
- 未启动并行角色：任务模板明确禁止 `delegate_tasks`，本单按单执行流完成。
- 后台操作入口继续为 bash 工具，不新增 GUI 依赖。

## 真实安装版验收（编排方发版后执行）

1. 信任模式运行 `git config --get core.hooksPath`：不产生审批，输出当前配置。
2. 记录配置，运行 `git config --unset core.hooksPath`：直接拒绝，结果含修复指引，零审批，配置前后相同。

本单仅合并，状态为 not live。
