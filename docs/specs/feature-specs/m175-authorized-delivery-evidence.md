# M175 交付验收记录

Scenario-Test: CXD-002
README-Update: reviewed
README-Update-Reason: 修复已有授权交付语义，不改变公开产品或安装承诺。

## Req ID 与先失败证据
- R1/R3：m175_authorized_inline_template_cannot_remove_write_authority，修复前 Interactive != Execute；结构化交付授权优先，冲突日志 authorized_delivery_posture_conflict，不含原始消息。结构化只读子任务保持边界。
- R2：m175_only_the_first_instruction_paragraph_selects_posture，修复前 Interactive != Execute；只取首段指令，忽略后续规格、引用和围栏。首段明确撤销仍生效。
- R4：m175_authorized_dispatch_writes_only_in_managed_workspace，以真实合成 Git 仓库和 SQLite 执行分配、写入、读回，断言源 checkout 文件与 git status 不变。路径断言已修正 macOS /var 规范路径差异，不放宽隔离要求。
- 无授权「只读审查一下」与结构化只读仍为 ReviewOnly；恢复入口复用授权解析。

## Harness 与协作
Spec：docs/specs/feature-specs/m175-authorized-delivery-posture.md 保留原始规格。
Compatibility / Payload：中英文、模板上下文、显式撤销、结构化只读与真实 Git 隔离写入。
Observation：冲突日志字段断言对应实现；无用户原文。
Release：仅合并，不发版；保留规格 Release-Urgency trailer。
AI Collaboration：context scope 为共享授权解析、恢复入口与工作区；assumptions 为结构化子任务只读仍优先；review point 为授权优先级、主 checkout 零写入；validation result 见测试记录。
未调用 delegate_tasks：模板要求单执行流。无 UI 布局改动，不适用截图。

## 验证记录
- pnpm test：159 文件、1055 测试通过，含 lightModeAudit。
- pnpm build：通过。
- 治理基线：通过。
- Rust 全工作区：`CARGO_INCREMENTAL=0 CARGO_TARGET_DIR="$PWD/.codefactory-cache/cargo-target-m175" cargo test --manifest-path src-tauri/Cargo.toml --workspace --no-fail-fast` 退出码 0，含 M175 三项回归。
- 统一场景门禁：`python3 tools/governance/run_scenario_harness_gate.py --stage local --repo . --policy-repo .` 通过。
- CI 场景声明与 README validator 均通过。

## 真机分工
not live：本任务不发版。真机验收由编排方发版安装后完成：内联合成规格与模板 create_and_send，delivery_authorized=true，确认实施姿态、托管 cwd、写入与交付可用；关闭授权后只读审查仍拒绝产品写入。后台 dispatch 入口沿用，无新增 GUI 依赖。
Spec Amendment: 无。

Release-Urgency: immediate
