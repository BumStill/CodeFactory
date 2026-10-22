# M156：交付工具生成的 PR 元数据自动正确

## 问题（2026-10-22 证据）

同一天内出现第 4 次“PR 已开，但元数据不对”，每次都靠人工补一行后才能过门禁：

| PR | 症状 | 根因 |
| --- | --- | --- |
| #834 | PR 描述缺 `README-Update:` / `README-Update-Reason:` 两行 → `check-frontend` 的 README 契约失败 | 只在作者手工写全声明时才生成这两行 |
| #834 | 追加正文后，`Release-Urgency` / `Scenario-Test` 被挤出最后一个段落，footer 读不到 | 追加逻辑只做“末尾拼接”，不做“footer 归位” |
| #836 | 同上 README 声明缺失（第二次） | 同 #834 |
| #842 | 正文写了 `Release-Urgency: immediate —— 说明`，`plan_release` 判定 `invalid_urgency`，整批发布被跳过 | trailer 值不是纯值，也没有规范化 |

共同点：这些字段**全部是机器读取的**，但生成端把它们当成“作者随便写的一段文本”。

## 需求

- **R1 唯一实现权威**：`Scenario-Test` 的值只来自仓库内的 `tools/governance/validate_scenario_test_governance.py`（registry 的 `change_patterns` 匹配 base..HEAD 的变更文件），交付工具不另造映射表；扫描/失败原因必须回显。
- **R2 README 声明自动生成**：交付时自动算 `required`/`reviewed` 与一句具体原因，作者已写的有效值保留且不重复。
- **R3 trailer 规范化**：`Release-Urgency` 只允许纯值 `immediate` / `hold`（或不写）；多行 trailer 合并到最后一个段落并去重；被追加正文挤出 footer 时归位；作者写的说明文字保留为正文，不丢。
- **R4 本地预检**：开 PR 前用与 CI 相同的 validator 校验 PR body（README 契约、`Scenario-Test`、trailer 形状），不推送注定失败的 PR；修不了时给人话原因。
- **R5 回归**：R1–R4 各有对应真实案例 fixture，能在不联网的情况下被机器断言。

## 实现

| 需求 | 落点 |
| --- | --- |
| R1 | `scripts/delivery-scenario-body.py`（Python 适配层）调用 `validate_scenario_test_governance.py --diff-json`，Rust 侧 `reconcile_scenario_contract_body` 只消费结果 |
| R2 | `apply_readme_declaration`（`src-tauri/src/agent/delivery.rs`） |
| R3 | `normalize_pr_body_trailers` + `pr_body_trailer_problems`（本轮新增），thin 路径与 durable 路径都调用，位于 README 声明之后、`plan_release` 之前 |
| R4 | `precheck_thin_pr_body`（thin）+ `prepare_release_policy`（durable）复用仓库 validator；trailer 形状由 `pr_body_trailer_problems` fail-closed 兜底 |
| R5 | `mod m156_trailer_tests`（`src-tauri/src/agent/delivery.rs`）+ `tests/test_delivery_scenario_body.py` |

关键行为：`Release-Urgency: immediate —— 说明` 规范化为

```
<正文>

说明

Release-Urgency: immediate
```

即值变纯、说明进正文、trailer 独占最后一段；重复项去重，冲突项以调用声明为准，非法值（如 `明天发布`）直接删除并给出人话说明。

## Primary User Path

用户在 CodeFactory 里授权交付（ceiling: merged）→ 模型调用 `deliver_changes` → 工具自动生成 PR 正文（summary + README 声明 + `Scenario-Test` + trailer）→ 本地预检通过后推送开 PR → CI 全绿 → `merge-queue: armed` → 合并队列合并。用户不需要再手工补任何一行元数据。

## Applicable Harnesses

- Spec Harness：本文件。
- Observation Harness：`pr_trailers` 步骤回显规范化动作与人话说明。
- 不涉及：Payload / Viewport / Compatibility（无上传、无布局、无对外 API 兼容面）。

## 验证

- `python3 -m unittest tests.test_delivery_scenario_body -v`
- `cargo test --manifest-path src-tauri/Cargo.toml m156_trailer_tests`
- 代码库级：`python3 tools/governance/validate_repo_governance_baseline.py`
