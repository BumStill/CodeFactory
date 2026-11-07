# M263：`edit_file` 支持同一文件多处改动，一次调用一次审批

## 问题（用户现场，2026-11-07 至 11-08）

用户 11-07 明确决定：Python 等解释器脚本继续要审批。但 `edit_file` 一次只能改一处，
改一个文件里的 4 处就要 4 次调用、4 次审批；模型于是改用
`python3 - <<'PY' … s.replace(old, new) … PY` 这种一次性的整段脚本，每次都要审批，
而且改动内容对用户不可读（只看到一段脚本，看不到 diff）。任务级别的成本是 3-8 次审批。

问题不在"脚本要不要审批"，而在主路径上没有"一次调用改多处"的工具形态，
所以模型只能绕道。

## 目标

同一个文件的多处精确替换，用**一次 `edit_file` 调用**完成，一次审批，输出里能看到完整的 diff。
单处调用的老写法（`old_string` / `new_string` / `replace_all`）保持完全不变。

## 需求

| ID | 需求 | 验收 |
|---|---|---|
| R1 | `edit_file` 新增可选参数 `edits`：`[{old_string, new_string, replace_all?}]`，顺序应用，**整次原子**：任一处找不到或有多处匹配则整次都不写入 | `edit_file_applies_multiple_edits_in_one_call`、`edit_file_multi_edit_is_atomic_and_names_the_missing_entry`、`edit_file_multi_edit_reports_an_ambiguous_entry_and_writes_nothing` |
| R2 | 老的 `old_string`/`new_string` 单处调用语义、错误文案、`replace_all` 零匹配容忍度都不变；混用 `edits` 与 `old_string` 直接报参数错误 | 既有 `edit_file_result_includes_unified_diff_for_changed_file` 等用例继续通过；`edit_file_rejects_mixing_single_and_batch_arguments` |
| R3 | 工具描述与系统提示告诉模型"同一文件多处改动放进一次 `edits` 调用，不要写脚本" | `edit.rs` 的 `definition()` 描述与 `SYSTEM_PROMPT` 的文件编辑段落 |
| R4 | 调研：脚本里还有什么主路径缺口（不能靠"加一个参数"解决，留作后续任务，不在本任务实现） | 见下节 |

## R4 调研结果（不实现，只记录）

脚本真正承担过的工作分四类，`edits` 只覆盖第一类：

1. **同一文件多处文本替换** — 本任务覆盖。
2. **跨多个文件同一替换**（例如一次改 5 个文件的同一行）— `edits` 只作用于一个文件，
   仍需 5 次调用。候选后续：`apply_edits` 批量接受 `{path, old_string, new_string}` 列表。
   但它把多文件写变成一个不可观察的大事务，观测契约（postcondition digest per file）需要重新设计，
   所以不在本任务里顺手做。
3. **结构化改写**（JSON/CSV 按字段改、正则替换 N 处）— 纯文本匹配做不到"按结构改"，
   而且 `jq` / `sed -i` 属于脚本，仍要审批。这是合理保留的审批点，不设法绕开。
4. **生成式重写**（模型算出一份新内容整体写入）— 已由 `write_file` 覆盖，
   提示词已要求优先 `write_file` / `edit_file`。

结论：本次只补第 1 类缺口；第 2 类需要单独的观测契约设计。

## 实现要点

- `src-tauri/src/tools/edit.rs`
  - `plan_from_args(&Value) -> Result<EditPlan, String>`：解析并校验两种形态，参数错误直接回给模型。
  - `apply_plan(&str, &EditPlan) -> Result<String, String>`：纯内存应用，失败即返回，磁盘不被触碰。
  - `execute` 用同一个 plan 计算新内容，写盘后仍保留读回字节校验。
  - 批量成功时输出 `Applied N changes to <path>` + 完整 unified diff。
- `src-tauri/src/agent/tool_backend.rs`
  - `prepare_file_observation` 的 `edit_file` 期望值改由 `edit_file_expected` 计算；
    批量形态调用**同一个** `plan_from_args` / `apply_plan`，因此准入阶段的
    postcondition digest 与真正写入的字节不可能不一致（旧实现在这里各算一遍，容易漂移）。
  - 未提供 `edits` 且缺 `old_string` 时，拒绝文案补一句"同一文件多处改动请用 `edits` 一次提交"。
- `src-tauri/src/agent/mod.rs`：`SYSTEM_PROMPT` 文件编辑段落写明"多处改动 → 一次 `edits` 调用"。

## 证据

- 单元/集成：`cargo test --lib tools::edit`（6 个用例，其中 4 个为 M263 新增）
- 准入链路：`batch_edits_are_admissible_in_one_call_and_the_executor_agrees`
  （准入判 Dispatch，随后真实执行同一份参数，断言文件内容与输出）
- 全量：`cargo test --manifest-path src-tauri/Cargo.toml --lib`
- 前端：`pnpm test`、`pnpm build`（工具 schema 变化不涉及 TS 改动，仍需回归）
- Scenario：`HLT-001`（一次授权后跨进程自动完成）、`HLT-005`（`tool_backend.rs` 命中）

## 不做什么

- 不改 `docs/testing/scenario-registry.json`（信任根）。
- 不降低脚本审批门槛：解释器脚本仍然需要审批，本次只是让主路径不再需要它。
- 不新增跨文件批量改写（见 R4 第 2 类）。
