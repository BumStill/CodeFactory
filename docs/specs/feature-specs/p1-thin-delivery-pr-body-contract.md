# 阶段 1 P1：thin 交付生成的 PR 正文缺仓库必填行（README-Update / Scenario-Test 等），CI 必挂

## 证据
- #757（10-19）、#769（10-20 M126）check-frontend 均失败：`PR body must contain exactly one 'README-Update: required|reviewed' line` / `non-empty 'README-Update-Reason:' line`。legacy 交付工具会补齐这些行；thin 交付（阶段 1 R2「正文以本次输入为准直接覆盖」）不再补。
## 需求
- R1 thin `deliver_changes` 生成/更新 PR 正文时，按仓库规则（docs/governance/rules.yml、check-frontend 的 PR body 校验脚本、scenario-test 治理）确保必填行存在且唯一：`Scenario-Test:`、`README-Update:`、`README-Update-Reason:`、以及输入里的 `Release-Urgency:`（放在最后一段）。模型输入里已有则保留，缺则给出合理默认并在工具结果里提示模型核对（README-Update 默认值按是否改了 README/用户可见功能判断，拿不准用 reviewed + 原因）。
- R2 本地预检：交付前用与 CI 相同的校验脚本检查正文，不通过则在工具结果里直接说缺什么，不推送一个注定失败的 PR 正文。
- R3 测试（先失败）：输入正文缺 README-Update → 生成的正文含两行且通过校验脚本；输入已有 → 不重复；Release-Urgency 位于最后一段。
## 约束
- 阶段 1 缺口（P1）。合成 fixture。授权交付（ceiling: merged）。

## Requirements Traceability

| Req ID | requirement | 最小证据 |
| --- | --- | --- |
| R1 | thin 正文必填行存在且唯一（`Scenario-Test:` / `README-Update:` / `README-Update-Reason:` / 输入里的 `Release-Urgency:` 位于最后一段），已有则保留 | `tools::delivery_thin` 合成 forge 测试（生成正文含必填行）+ `agent::delivery` 正文装配测试（不重复、urgency 末段）+ 真实 `validate_readme_contract.py` 对生成正文返回 0 |
| R2 | 交付前用 CI 相同脚本预检正文，不通过直接在工具结果报缺什么且不推送 | `body_precheck_failure_never_pushes`：reconcile 返回错误时 `pushes == 0`、错误原文进工具结果；预检调用 `validate_readme_contract.py` 与 `validate_scenario_test_governance.py --ci` |
| R3 | 三条测试先失败后通过 | 红：新增测试在实现前编译/断言失败（提交前记录），绿：实现后同一命令通过 |

## Applicable Harnesses

- Spec Harness：本规格 + 需求追踪表。
- AI Collaboration Harness：context scope = `src-tauri/src/tools/delivery_thin.rs` 与 `src-tauri/src/agent/delivery.rs` 的正文装配/校验路径；assumptions 见 Implementation Notes；review point = 复用 legacy 既有 reconciler 而非另写一套规则；validation = 下方测试矩阵。

（本任务不涉及 UI、发布执行、上传负载或视口，故未启用 Viewport / Payload / Release Harness。）

## Implementation Notes

- thin 交付复用 legacy 的同一套正文契约实现，不复制规则：`reconcile_scenario_contract_body`（由 registry 推导 `Scenario-Test:`）、README 声明装配（`required` / `reviewed` + 非空原因）、`append_release_urgency`（幂等，放最后一段）、`validate_pr_body_intent`（shell 片段与声明合法性）。
- 新增 seam：`agent::delivery::prepare_thin_pr_body(root, title, body, urgency)`；thin `run()` 在其 `Commands` trait 上新增 `reconcile_body`（默认直通，生产实现调用上述 seam），因此合成 forge 测试可在无网络、无 python 的情况下验证顺序与失败行为。
- 顺序：stage → commit（本地）→ 正文装配 + 双 CI 校验脚本预检 → push → PR create/edit。推送是第一个远端变更，预检失败时不会推送注定失败的正文。
- 基线解析：`remote_default_branch(origin)`，缺失时 `origin/main` 兜底；`origin/<base>` 不存在时做一次 `git fetch origin <base>`，仍失败则 fail closed 并说明。
- 未改 `docs/testing/scenario-registry.json`、`tools/governance/`、`.github/`（信任根）。
