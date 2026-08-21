# Skill 生命周期与运行时系统架构设计

> 状态：Draft for approval
>
> 对应规格：`docs/specs/feature-specs/skill-lifecycle-system.md`
>
> 安全附录：`docs/design/skill-lifecycle-threat-model.md`
>
> 设计原则：本地优先、内容不可变、状态正交、默认禁用、按需加载、失败可见、证据可关联。

本设计实现 `CF-SKL-R1..R19`；需求优先级、交付阶段、Scenario 和最低证据以规格 §6.2 RTM 为唯一权威。本文件不得把 Phase 3 的 `R13/R14/R17` 重新写成 P0 发布门槛。

## 1. 当前架构缺陷

当前实现以目录和 `manifest.json.enabled` 同时承载内容、安装、审核和启用语义，并由不同入口直接调用文件写入函数。主要结构性问题如下：

- `scan_skill_dir` 既是 catalog，又是损坏过滤器；解析失败直接消失；
- install、update、enable 和 delete 没有统一 operation、transaction、path boundary 或 receipt；
- marketplace 从前端回传完整对象后直接写入，后端没有重新解析可信 source；
- `SKILL.md` 被转换为 `system_prompt.md`，源包与运行包没有区分；
- runtime 读取所有 enabled prompt，没有 candidate selection、package pinning 或 use receipt；
- `tool_policy.json` 和 slash commands 没有权威执行器；
- UI 与 Headless 通过有无 `AppHandle` 走不同加载逻辑；
- installed、reviewed、enabled、loaded 和 verified 被压缩成 `installed/enabled` 两个布尔值。

目标架构必须先拆开这些事实，再提供统一编排层。

## 2. 高层组件

```text
Registry / URL / Git / Local / Agent tool
                    │
                    ▼
             Typed Source Adapter
                    │
                    ▼
     Fetcher ──> Staging Area ──> Package Validator
                    │                    │
                    │                    ├── quarantine / incompatible / failed
                    │                    │
                    └──────────────> Atomic Commit
                                           │
                       ┌───────────────────┴───────────────────┐
                       ▼                                       ▼
            Immutable Package Store                   SQLite Lifecycle Index
                       │                                       │
                       └────────────── Catalog Reconciler ─────┘
                                           │
                 ┌─────────────────────────┼────────────────────────┐
                 ▼                         ▼                        ▼
          Resource Center          Activation Service        Update/Rollback
                                           │
                                           ▼
                           Compact Catalog + Skill Resolver
                                           │
                                           ▼
                              skill_load / resource_read
                                           │
                                           ▼
                              Agent Loop + Turn Receipt
```

### 2.1 组件职责

| 组件 | 职责 | 不负责 |
| --- | --- | --- |
| `SkillSourceAdapter` | 将 typed source 转成可读取 artifact；处理 status、timeout、ref/subdir | 不决定是否启用 |
| `SkillPackageValidator` | 路径、manifest、资源、大小、digest、兼容和 capability 校验 | 不写最终目录 |
| `SkillInstaller` | operation state machine、staging、原子提交、receipt | 不从前端接收“已可信”的 package 对象 |
| `SkillPackageStore` | 保存不可变内容寻址包 | 不保存 enabled 布尔值 |
| `SkillLifecycleRepository` | SQLite 中保存 package、review、activation、operation、receipt | 不直接解析 package 文件 |
| `SkillCatalogReconciler` | 对账 SQLite 与 package store，暴露 orphan/corrupt/missing | 不静默删除异常 |
| `SkillActivationService` | review、scope、enable/disable、effective policy | 不修改 package 内容 |
| `SkillResolver` | 构造 compact catalog、选择候选、固定 package digest | 不把所有 Skill 正文拼进 prompt |
| `SkillRuntime` | `skill_load`、资源读取、预算和 turn receipt | 不自动执行包内脚本 |
| `SkillEvidenceService` | selection summary、load/call receipt、结果验收 receipt 与 anonymous 内存生命周期 | 不从 loaded/completed 推断 outcome passed |
| `SkillUpdateService` | 检查更新、diff、批准、切换与回滚 | 不自动接受远程更新 |

## 3. 权威模型

### 3.1 内容权威：Immutable Package Store

建议路径：

```text
<app-config>/CodeFactory/skills-v2/
  packages/
    <normalized-skill-id>/
      <content-sha256>/
        manifest.normalized.json
        SKILL.md
        slash_commands.json          # optional
        tool_policy.json             # optional restriction only
        resources/                   # declared resources only
  metadata/
    <content-sha256>.receipt.json     # 不参与 content digest
  staging/
    <operation-id>/
  quarantine/
    <operation-id>/
  legacy-backup/
    <migration-id>/
```

规则：

- `packages/<id>/<digest>` 一旦提交不可修改；更新产生新 digest；
- package manifest 不保存 enabled/reviewed 状态；
- 资源路径必须是 UTF-8 可表示的相对路径，标准化后仍位于 package root；
- 不跟随 symlink，不接受 hardlink/特殊设备文件；
- staging 与 package store 必须位于同一 filesystem，以便使用原子 rename；
- quarantine 可保存最小诊断所需内容，但默认不保留远程大 payload；用户删除时走可恢复 trash。

`content_sha256` 使用版本化 `codefactory-skill-digest-v1`：校验并标准化 portable relative path，按 UTF-8 path byte 升序排列；normalized manifest 自身不出现在 `files[]`，其余每项编码为 length-prefixed `{path, role, required, byte_length, sha256(file_bytes)}`，再连同 canonical JSON 编码的 normalized manifest 做 SHA-256。文件正文逐 byte 保真，不改换行；manifest canonical JSON 固定 UTF-8、字段顺序与数字编码。source locator、operation/time、review/activation、receipt 和 metadata 不参与摘要。所有 adapter 对等价文件集必须得到同一 digest；algorithm 版本写入 package row，变更算法必须产生新版本而非静默重算。（`CF-SKL-R1/R19`）

### 3.2 状态权威：SQLite Lifecycle Index

SQLite 保存可变生命周期状态；不在 package manifest 中修改 enabled。

#### `skill_packages`

| 字段 | 说明 |
| --- | --- |
| `package_id` | UUID，内部稳定身份 |
| `installation_id` | 关联稳定安装对象 |
| `skill_id` | 规范化逻辑 ID |
| `version` | 作者声明版本，可为空但不得替代 digest |
| `content_sha256` | 内容权威摘要，唯一 |
| `digest_algorithm` | `codefactory-skill-digest-v1` 等版本化算法 |
| `schema_version` | normalized manifest schema |
| `source_kind` | registry/git/http_raw/http_archive/local/legacy/user_created/builtin |
| `source_locator_redacted` | 去凭据、去 query secret 的来源摘要 |
| `compatibility_state` | compatible/incompatible/unsupported |
| `integrity_state` | verified/corrupt/missing；quarantined artifact 尚未形成 package row |
| `capabilities_json` | prompt/resources/slash_commands/tool_restrictions 等 |
| `installed_at` | 安装提交时间 |
| `superseded_by` | 新 package，可为空 |
| `registry_snapshot_digest` / `registry_key_id` | 官方 registry 验证证据，可为空 |
| `registry_signature_state` | not_applicable/verified/invalid/expired |
| `resolved_source_revision` | Git commit SHA 或 immutable registry revision |

唯一约束：`(skill_id, content_sha256)`；同一版本号不同摘要必须显示冲突，不能覆盖。

#### `skill_installations`

| 字段 | 说明 |
| --- | --- |
| `installation_id` | 一个本地安装的稳定身份，不随版本变化 |
| `skill_id` | 规范化逻辑 ID |
| `origin_kind` | builtin/user_created/registry/git/http/local/legacy |
| `state` | installed/tombstoned/purged |
| `created_at` | 首次安装时间 |
| `tombstoned_at` / `restore_deadline` | 可恢复删除窗口 |
| `last_operation_id` | 最近生命周期操作 |

remove/restore 以 `installation_id` 为目标；不得通过自由字符串 ID 推导文件路径。`purged` 只保留最小审计 tombstone，不再保留 package 内容。

#### `skill_reviews`

| 字段 | 说明 |
| --- | --- |
| `package_id` | 被审核包 |
| `state` | unreviewed/approved/rejected/revoked |
| `approval_basis` | explicit_user/builtin_release/legacy_grandfathered |
| `reviewed_at` | 时间 |
| `review_surface` | resource_center/migration/agent_handoff |
| `capability_snapshot_json` | 用户批准时看到的能力摘要 |

#### `skill_activations`

| 字段 | 说明 |
| --- | --- |
| `activation_id` | UUID |
| `skill_id` | 逻辑 Skill identity |
| `package_id` | enabled/disabled record 固定到具体 digest；`blocked_in_scope` 时为空 |
| `scope_kind` | global/project；session scope 留到后续 |
| `scope_key_digest` | 项目路径摘要；不暴露原路径到外部事件 |
| `state` | enabled/disabled/blocked_in_scope |
| `effective_policy_json` | 权限交集后的只读快照 |
| `updated_at` | 时间 |

check constraint：`state in (enabled,disabled) => package_id IS NOT NULL`，`state=blocked_in_scope => scope_kind=project AND package_id IS NULL`。同一 `(skill_id, scope_kind, scope_key_digest)` 最多一个 effective record。解析优先级固定为 `project override > global`，同一 root turn 只能得到一个 effective package；用户在当前项目禁用全局版本时必须写显式 `blocked_in_scope` negative override，不能通过“没有 project activation”表达。删除 project override 前 UI 必须说明会恢复 global 版本还是继续屏蔽；activation receipt 记录 effective package、scope source 和被覆盖的 activation。（`CF-SKL-R5/R10`）

#### `skill_install_operations`

| 字段 | 说明 |
| --- | --- |
| `operation_id` | 可恢复操作身份 |
| `source_kind` / `source_summary` | typed source 和去敏摘要 |
| `state` | queued/fetching/staged/validating/committing/succeeded/succeeded_with_errors/recoverable_failed/failed/rolled_back |
| `recovery_state` | not_needed/pending/recovered/recovery_failed |
| `error_code` / `error_detail_redacted` | 稳定错误与安全摘要 |
| `bytes_received` / `files_seen` | 有界 payload 证据 |
| `package_id` | 成功后关联 |
| `created_at` / `updated_at` | 时间 |

parent operation 的总状态由 item 聚合：全部成功为 `succeeded`，成功与失败并存为 `succeeded_with_errors`，全部失败为 `failed`；parent 的 `package_id` 仅用于单项兼容读取，批量流程不得依赖它。未形成 package 的 item 可以没有 `package_id`。

`recoverable_failed` 表示 staging/intent 仍在且可重试；`rolled_back` 只在 reconciler 已确认 staging 清理且旧 active state 未变时使用。恢复过程写独立 `recovery_state`，完成时置 `recovered` 并把 operation `state` 原子更新为最终 `succeeded/failed/rolled_back`；UI 不得只凭 recovery 状态猜测“已回滚”。

#### `skill_install_operation_items`

批量 Git/local import 中每个发现包必须有独立 item，防止总成功数掩盖失败项：

| 字段 | 说明 |
| --- | --- |
| `item_id` / `operation_id` | 批次内稳定身份 |
| `source_member` | 去敏的相对成员摘要 |
| `discovered_skill_id` | 解析后 ID，可为空 |
| `state` | discovered/validating/committed/quarantined/failed/skipped |
| `package_id` / `installation_id` | 成功后关联 |
| `error_code` / `error_detail_redacted` | 逐项错误 |

#### `skill_turn_selection_summaries`

每个 root turn 恰好一个 summary，即使没有 Skill 被选择：

| 字段 | 说明 |
| --- | --- |
| `summary_id` / `session_id` / `root_turn_id` / `run_id` | 回合身份 |
| `activation_snapshot_digest` / `catalog_revision` | 固定解析输入 |
| `matched_count` / `selected_count` / `load_attempted_count` | 允许全部为 0 |
| `catalog_truncated` | catalog 是否因预算缩减 |
| `decision_codes_json` | matched-but-not-selected/conflict/none 的稳定原因 |
| `created_at` | 时间 |

anonymous turn 使用同结构内存对象但不写表。

#### `skill_turn_receipts`

| 字段 | 说明 |
| --- | --- |
| `receipt_id` | UUID |
| `session_id` / `root_turn_id` / `run_id` | 与真实回合绑定 |
| `skill_id` / `package_id` / `content_sha256` | 固定加载内容 |
| `match_state` | not_evaluated/not_matched/matched |
| `selection_state` | not_selected/selected_explicit/selected_auto/conflict |
| `invocation_kind` | none/explicit_user/slash_command/model_catalog_call |
| `selection_rule_id` | 稳定规则 ID；不持久化用户派生自由文本 |
| `loaded_resources_json` | 路径摘要、digest、字符数 |
| `budget_json` | requested/loaded/truncated/dropped |
| `load_state` | not_requested/loaded_full/loaded_partial/dropped/failed |
| `error_code` | 加载失败时稳定错误 |

anonymous turn 的同结构 receipt 只存在进程内存，随 turn 生命周期释放；不写 SQLite、遥测或诊断导出。覆盖率分别统计 persistent 与 ephemeral receipt，不把 anonymous 流量伪装成缺失。（`CF-SKL-R18`）

#### `skill_outcome_verification_receipts`

| 字段 | 说明 |
| --- | --- |
| `verification_id` / `turn_receipt_id` | 与真实 load/call 证据关联 |
| `criteria_id` / `evidence_id` | 可执行验收与证据对象 |
| `verifier` | harness/user/agent-with-evidence |
| `status` | not_evaluated/passed/failed/inconclusive/not_applicable |

它证明“任务结果按某标准被验证”，不证明 Skill 与结果之间的因果；回合完成、模型自述或 `loaded_full` 不自动生成 `passed`。

#### `skill_removal_operations`

保存 `removal_id`、`installation_id`、previous/next installation state、tombstone deadline、actor、open-objective reference count、result/error 和时间。restore/purge 只能引用该稳定 identity；历史 receipt 保留但不保留已 purge 内容。

### 3.3 Catalog Projection

`SkillCatalogProjection` 是 Resource Center 与 runtime 共用的只读聚合，但两者使用不同视图：

```text
SkillCatalogProjection {
  packages: InstalledPackageRow[],          # 已提交 package；含 missing/corrupt/incompatible
  legacy_diagnostics: LegacyDiagnosticRow[],# 尚未迁移但确实存在的 legacy 目录
  operations: InstallOperationRow[],        # 含无 package identity 的 fetch/validate failure
  attention_count: packages needing attention + failed operation items
}
```

运行时只读取 approved/eligible activation 的 package 视图；资源中心显示完整投影。“我的技能”不为 URL timeout 等操作失败伪造 package 行，“安装记录”也不能因 catalog 为空而被空状态吞掉。（`CF-SKL-R4`）

## 4. 状态正交模型

必须分别回答以下事实：

```text
Storage:     absent | staging | installed | missing | corrupt | quarantined
Installation lifecycle: present | tombstoned | purged
Review:      unreviewed | approved | rejected | revoked
Basis:       explicit_user | builtin_release | legacy_grandfathered(reserved, initial rollout ineligible)
Activation:  disabled | enabled(scope, package_digest) | blocked_in_scope
Match:       not_evaluated | not_matched | matched
Selection:   not_selected | selected_explicit | selected_auto | conflict
Load:        not_requested | loaded_full | loaded_partial | dropped | failed
Invocation:  none | explicit_user | slash_command | model_catalog_call
Outcome:     not_evaluated | passed | failed | inconclusive | not_applicable
Compatibility: compatible | incompatible | unsupported
Update:      current | update_available | superseded | rollback_available
Operation:   queued | fetching | staged | validating | committing | succeeded | succeeded_with_errors | recoverable_failed | failed | rolled_back
Recovery:    not_needed | pending | recovered | recovery_failed
```

这些名称与规格 §6.1 完全一致。SQLite `integrity_state` 映射 Storage 的 installed/missing/corrupt；operation/staging 映射 staging，隔离的 operation item 映射 quarantined，未发现 identity 映射 absent。quarantined artifact 不创建 `skill_packages` row；若已安装 package 后来校验失败则是 corrupt，不改称 quarantined。`skill_installations.state` 映射 Installation lifecycle；`compatibility_state` 与 `superseded_by` 分别映射 Compatibility/Update。显式选择可以是 `not_matched + selected_explicit`；matched-but-not-selected 必须有稳定 rule reason；`Invocation` 不是 `Load` 之后的状态。UI 可以组合展示，但 API 必须保留原始字段。

## 5. SkillPackage v2

### 5.1 Normalized manifest

```jsonc
{
  "schema_version": 2,
  "id": "release-pr-writer",
  "name": "Release PR Writer",
  "description": "在需要整理发布 PR 时加载",
  "version": "1.2.0",
  "entrypoint": "SKILL.md",
  "triggers": {
    "explicit_aliases": ["release-pr"],
    "task_kinds": ["git_delivery"],
    "keywords": ["release PR", "发布说明"],
    "automatic": true
  },
  "capabilities": {
    "resources": {"present": true, "required": true},
    "slash_commands": {"present": true, "required": false},
    "tool_restrictions": {"present": true, "required": true},
    "scripts": {"present": false, "required_execution": false}
  },
  "compatibility": {
    "min_codefactory_version": "1.82.0",
    "platforms": ["windows", "macos", "linux"]
  },
  "files": [
    {"path": "references/release-checklist.md", "role": "reference", "required": true, "size": 1234, "sha256": "..."}
  ]
}
```

normalized manifest 必须列出参与包的全部文件及 `path/role/required/size/sha256`。未支持的 required capability 阻止批准/启用；只附带且未声明 required execution 的 scripts/templates/assets 必须 byte-for-byte 保存，可作为只读附件但不能自动执行。`loaded_partial` 只表示可选内容因预算被截断；required 内容缺失或超预算一律 `failed`。（`CF-SKL-R1/R9`）

### 5.2 输入格式适配

- CodeFactory v2 package：严格读取 normalized manifest 或受支持 archive；
- 标准 `SKILL.md`：保留原文件，解析受支持 frontmatter，缺省字段由 adapter 补齐并标记 inferred；
- legacy CodeFactory：读取 `manifest.json + system_prompt.md`，生成 `SKILL.md` 兼容包装和 migration receipt；
- Git repo：typed source 必须指定 ref 和可选 subdir；不得把任意 GitHub 文件 URL当 repo clone；
- raw `SKILL.md` URL：显式 source kind，不能与 JSON manifest URL 模糊猜测；
- marketplace：后端只接收 registry ID/version，自己读取 registry 元数据并校验 digest；不接受前端回传完整 package 作为信任依据。

## 6. Typed Source API

```rust
enum SkillSource {
    Registry { registry_id: String, skill_id: String, version: Option<String> },
    Git { url: String, r#ref: String, subdir: Option<String> },
    RawSkillMd { url: String, expected_sha256: Option<String> },
    HttpArchive { url: String, expected_sha256: Option<String> },
    LocalSelection { source_handle: SkillSourceHandle },
    UserCreated,
    LegacyMigration { trusted_internal_locator: LegacySkillLocator },
}
```

`SkillSourceHandle` 由 Tauri 后端通过原生目录选择器或精确用户授权创建，是有过期时间、单次使用、绑定 canonical root identity 的 opaque capability；renderer/model 不能提交任意 `PathBuf` 让后端读取。Agent 若要导入本地目录，必须引用用户已选择的 handle，或对用户明确提供且位于当前授权 project root 内的路径重新走一次 permission gate。Legacy locator 只由 migration service 内部生成，不暴露为 Tauri/Agent 参数。

Tauri / tool API：

```text
preview_skill_resolution(ephemeral_draft_text, cwd_scope, explicit_selection) -> EphemeralResolutionPreview
start_skill_install(source) -> InstallOperation
get_skill_install(operation_id) -> InstallOperation
review_skill_package(package_id, decision) -> SkillReviewReceipt
list_skill_catalog(filter) -> SkillCatalog
get_skill_package(package_id) -> SkillPackageDetail
set_skill_activation(skill_id, package_id?, scope, desired_state) -> ActivationReceipt
create_skill_draft(content_bundle) -> InstallOperation
revise_skill(package_id, content_bundle) -> InstallOperation
check_skill_updates(skill_id) -> UpdateSummary
apply_skill_update(skill_id, package_id, decision) -> UpdateReceipt
rollback_skill(skill_id, target_package_id) -> ActivationReceipt
remove_skill(installation_id, recoverable=true) -> RemoveReceipt
restore_skill(removal_id) -> RestoreReceipt
get_skill_turn_evidence(root_turn_id) -> SelectionSummary + SkillTurnReceipt[] + OutcomeVerificationReceipt[]
record_skill_outcome_verification(turn_receipt_id, criteria_id, evidence_id, verifier, status) -> OutcomeVerificationReceipt
```

`preview_skill_resolution` 把 composer draft 作为单次调用内存参数交给 backend，以便执行 keyword/task rule；该参数不得进入日志、SQLite、遥测或后台缓存，响应后立即释放。发送时 backend 重算并把 activation/package snapshot 固定到 root turn，前端不得自行匹配。`desired_state` 是 `enabled/disabled/blocked_in_scope` typed enum，并受 §3.2 check constraint 约束，不能再退化为布尔值。

`create_skill_draft`/`revise_skill` 也进入 staging/validator/immutable commit；每次保存都生成新 digest/package，previous draft 标记 superseded，UI 可用 draft lineage 分组但不存在可变 package。修改已启用 Skill 同样生成新 package。UI 的“保存草稿”仅提交 `unreviewed+disabled`，即使提供“审核并启用”的连续旅程，也必须顺序产生 install、review、activation 三份 receipt，不能绕过状态机。（`CF-SKL-R2/R5/R8`）

`record_skill_outcome_verification` 只接受注册的 criteria、可解析 evidence identity 与授权 verifier，由 `SkillEvidenceService` 所有；anonymous turn 只更新内存 evidence bundle，`get_skill_turn_evidence` 也只在该回合存活期间返回，不能落库。

Agent tools 只包装这些服务，不保留第二套文件实现：

```text
skill_search
skill_install_start
skill_install_status
skill_list
skill_get
skill_load
skill_resource_read
skill_create_draft
skill_revise_draft
```

canonical public names 是 `skill_install_start/status` 与 `skill_create_draft`。Phase 0 保留现有 `skill_fetch`、`skill_create` 作为 deprecated compatibility alias，但 alias 必须路由到同一 service、使用相同 permission 分类、返回 canonical operation/receipt 并带 replacement metadata；不得保留旧文件实现。L0/L1 兼容测试固定 alias 行为，下一 major 才可删除。（`CF-SKL-R2`）

Agent 可以安装或创建 `unreviewed + disabled` package，不能代替用户 review/enable。`skill_load`/`skill_resource_read` 分类为 `RuntimeContextRead`：它们会把不可信包内容带入模型上下文，必须同时满足固定 activation snapshot、package eligibility、预算与审计，不得仅因“不写文件”就按普通 read-only 放行。

## 7. 安装事务

### 7.1 流程

1. 创建 `operation_id` 并持久化 `queued`；
2. source adapter 将 payload 拉入 `staging/<operation_id>`；
3. 在读取/解压每个 entry 前完成 path 与配额检查；
4. 严格解析 manifest/`SKILL.md`，生成 normalized manifest；
5. 校验所有 declared resources、兼容性、capability 和 digest；
6. 对每个发现包写 operation item，并生成 validation/capability report；
7. 在 SQLite 预写 committing intent；
8. staging 目录 fsync 后原子 rename 到 `packages/<id>/<digest>`；
9. SQLite transaction 写 package、`unreviewed` review state 和 succeeded operation receipt；
10. emit `skill://catalog-changed` 和 `skill://operation-updated`；
11. 资源中心显示“已安装，等待审核”；后续 `review_skill_package` 与 activation 是独立动作。

用户点击“安装”之前的来源/能力摘要属于 pre-install preview；安装后的正式审核固定到已提交 package digest。两者都不能隐式合并 enable。

启用的硬前置条件：storage=`installed`、integrity=`verified`、compatibility=`compatible`、review=`approved` 且 approval basis=`explicit_user` 或 `builtin_release`、required capabilities 全部受支持、effective policy 可计算且非冲突、scope identity 可解析。initial rollout 的 `legacy_grandfathered` 不满足 eligibility。任何条件在启用后变为 `revoked/rejected/corrupt/missing/incompatible` 时，activation service 必须在同一事务中撤销该 package 的全部 activation，并发出稳定事件；不能继续让旧 catalog 加载。

### 7.2 崩溃恢复

启动 reconciler 检查：

- DB 为 `committing` 且 final package 存在：验证 digest 后补齐 succeeded；
- DB 为 `committing` 且只有 staging：恢复提交或标记 `recoverable_failed`；只有确认清理且旧 active state 未变后才能标记 `rolled_back`；
- final package 存在但 DB 无 package：登记 orphan，显示“需要恢复”，不静默采用；
- DB 有 package 但文件缺失/摘要不符：标记 missing/corrupt，立即从 resolver 排除；
- 同一 operation 重放必须幂等；不得产生第二个 package 或重复 activation。

### 7.3 稳定错误码

| 错误码 | 条件 |
| --- | --- |
| `SKILL_SOURCE_UNSUPPORTED` | source kind/URL 类型不支持 |
| `SKILL_FETCH_TIMEOUT` | 远程拉取超时 |
| `SKILL_FETCH_HTTP_STATUS` | 非成功 HTTP status |
| `SKILL_PACKAGE_TOO_LARGE` | 压缩/解压/单文件/文件数超限 |
| `SKILL_PATH_ESCAPE` | traversal、absolute path、separator/normalization escape |
| `SKILL_LINK_FORBIDDEN` | symlink/hardlink/device entry |
| `SKILL_MANIFEST_INVALID` | schema 或必填字段无效 |
| `SKILL_ID_COLLISION` | 归一化 ID 冲突 |
| `SKILL_RESOURCE_MISSING` | 声明资源缺失或 digest 不符 |
| `SKILL_DIGEST_MISMATCH` | expected 与实际摘要不符 |
| `SKILL_INCOMPATIBLE` | app/platform/capability 不兼容 |
| `SKILL_CAPABILITY_UNSUPPORTED` | 包要求未实现能力 |
| `SKILL_ATOMIC_COMMIT_FAILED` | final commit 失败 |
| `SKILL_LEGACY_CORRUPT` | legacy package 不可迁移 |

错误详情只包含安全摘要，不回显凭据、完整 query、原始敏感文件内容。

## 8. 安全与信任边界

### 8.1 Path boundary

- 所有 id 先做 Unicode NFKC、大小写策略和 slug collision 检查；
- 所有相对路径逐 component 校验，reject `.`、`..`、absolute、drive prefix、UNC、NUL 和平台替代分隔符；
- 文件系统 mutation 必须以已打开的 staging/package root directory handle 为锚点逐级 no-follow 打开；Unix 使用 `openat`/`O_NOFOLLOW` 等价语义，Windows 使用拒绝 reparse point 的 handle-relative 等价实现；禁止“先 canonicalize 再按路径写入”的 TOCTOU 模式；
- archive entry 在写入前检查，不能“先解压再扫描”；
- update/delete 只接收数据库查得的 `package_id`，不接收自由路径或自由 id 作为删除目标；
- trash/remove 同样通过 package locator，禁止 `remove_dir_all(root.join(user_input))`。

### 8.2 远程供应链

- registry 响应有 timeout、status、content-type、最大字节数和 schema 校验；
- registry item 必须引用 immutable version/digest；后端按 ID 获取并校验，不信任 UI payload；
- HTTP redirect 次数和协议受限；默认只允许 HTTPS，开发环境例外需显式开关；
- Git 必须固定 commit SHA 或 tag resolved SHA，receipt 保存 resolved commit；
- Phase 1 使用随 App 发布的 registry public key 验证官方 registry envelope，并固定 package digest；receipt 持久化 snapshot digest、key ID、signature state/expiry 与 resolved source revision；第三方作者签名与团队 key trust 进入 Phase 3；
- remote package 永不自动启用或自动更新。

网络额外门禁：

- 官方 marketplace 只访问随 App 配置并显示给用户的 HTTPS registry allowlist，且必须验签；用户显式发起的 public HTTPS raw/archive 或 Git source 可以不在官方 allowlist，但必须经过来源确认、HTTPS、DNS/redirect 私网拒绝、digest 固定与同一 payload 配额。renderer/model 不能提交隐藏的任意 registry URL；任何 private/internal source 需要独立企业策略，P0 默认拒绝；
- DNS 解析结果与每次 redirect 都拒绝 loopback、link-local、RFC1918、ULA 和 metadata endpoint；
- 采用有界 streaming 读取，不能先无限 `bytes()` / `text()` 再检查大小；
- registry preview 与 install 必须绑定同一 signed snapshot digest，阻断 search/install TOCTOU。

### 8.3 Permission ceiling

```text
effective_tool_policy = user_permission_ceiling
                      ∩ project_permission_ceiling
                      ∩ turn_capability_ceiling
                      ∩ every_loaded_skill_restriction
```

- Skill 不能新增未暴露工具；
- Skill 不能扩大文件根目录、网络域或 confirmation policy；
- policy 缺失代表“不附加 Skill 限制”，不是完全权限；policy 文件存在但解析失败、schema/字段未知或声明越过上层 ceiling 时必须 fail-closed，阻止 review、activation 和 load；
- UI 只展示实际交集，不展示作者声明后却忽略；
- package scripts 不自动执行；如果模型读取脚本并请求执行，仍由正常 tool gateway 重新分类和审批。

首版 `tool_policy v1` 只允许声明限制：

```jsonc
{
  "schema_version": 1,
  "allowed_tools": ["read_file", "grep"],
  "denied_tools": ["bash", "write_file"],
  "file_scopes": ["project"],
  "network_domains": []
}
```

- `allowed_tools` 缺省表示不增加额外 allowlist；`denied_tools` 取并集；多个 loaded Skill 的 allowlist/file/network scope 取交集，任一 deny 生效；
- 交集为空或两个 policy 不能同时满足时，resolver 标记 conflict 并要求用户减少本回合 Skill，不偷偷选择较宽策略；
- 无效 schema、未知可扩权字段或声明超出上层 ceiling 时阻止 review/activation/load；已激活 package 后来被判定为无效时，在同一事务中撤销全部 activation；
- policy 解析结果、effective grants 和 denied tool 进入 turn receipt。

`skill_*` 工具不得使用前缀级无条件 Allow：

- `skill_search`、`skill_list`、`skill_get` 为 plain read-only；`skill_load`、`skill_resource_read` 为 `RuntimeContextRead`；
- fetch/install/update/rollback/remove 属于持久 mutation，必须经过正常权限分类和精确用户意图门禁；
- enable、扩大 scope、purge 和接受更新必须保留用户 UI 审核动作，Agent 不能代办；
- destructive action 只接受数据库解析出的 package/revision identity，不接受模型提供的自由文件路径。

### 8.4 Prompt injection

- compact catalog 只包含经过转义和长度限制的 name/description/capability 摘要；
- 未 review package 的正文永不进入模型上下文；
- `skill_load` 只允许当前 scope 已启用的固定 package；
- Skill 内容作为独立、带来源/边界的 context block 注入，不与 repository authority 混淆；该 wrapper 只是可观测性和 defense-in-depth，**不是安全边界**；
- repository `AGENTS.md`、用户本轮指令和平台权限始终高于 Skill；
- receipt 记录截断，关键安全尾注由 runtime 包装；真正的越权阻断必须由确定性 permission/tool gateway 完成。

## 9. Runtime 设计

### 9.1 Compact catalog

每个 root turn 构造紧凑目录：

```text
available_skill {
  skill_id, name, short_description,
  active_package_digest, scope,
  explicit_aliases, task_kinds,
  capabilities, compatibility
}
```

约束：

- 只包含当前 scope effective activation 对应且 eligibility 正常的 package；`project override > global`，`blocked_in_scope` 阻断 global fallback；
- 总 catalog 预算建议 2,000 字符，按显式 scope、项目 scope、全局 scope 排序；
- 超限时不静默丢失：输出 `catalog_truncated=true`，模型可通过 `skill_list` 查询；
- 不包含完整 Skill 正文。

### 9.2 选择与加载

发送前 UI 调用 `preview_skill_resolution(ephemeral_draft_text, scope, explicit_selection)` 得到 ephemeral matched/selected 建议，用户可移除自动项；draft 只在调用内存中存在，不记录/持久化。提交时 resolver 用同一 rule set 重算，若 catalog revision 改变则返回 diff 并要求重新固定，不能接受前端伪造 snapshot。

候选优先级：

1. 用户显式选择或 `/skill <alias>`；
2. 当前任务已固定的恢复 receipt；
3. 确定性 trigger rule（task kind、exact alias、关键词）；
4. 模型从 compact catalog 调用 `skill_load`，必须提供稳定 catalog/rule reference；
5. 没有可靠候选则不加载。

首版每回合自动候选上限建议 3，实际加载上限建议 2；用户显式选择不受候选排序影响，但仍受总 context budget。

`skill_load`：

- 以 `package_id`/digest 加载，不能只按可变 skill ID；
- 先写 match/selection/invocation 轴，再读取 entrypoint/required resources；
- 每个 Skill 默认正文上限 8,000 字符，总 Skill 上限 16,000 字符，可按模型 context profile 调整；
- 可选内容截断时为 `loaded_partial` 且 receipt/UI 都显示；required entrypoint/resource/capability 无法加载时为 `failed`；预算淘汰且尚未读正文为 `dropped`；
- 加载后的 context block 标记来源、版本、digest、优先级和不可越权声明。

### 9.3 UI / Headless / Recovery 一致性

- catalog/resolver/runtime 是 AppHandle-independent service；builtin 也先进入 package store/index；
- UI 和 Headless 只提供不同 observer，不提供不同 loader；
- root turn 创建时固定 activation snapshot；回合中更新/禁用只影响下一 root turn；
- Objective 恢复优先使用原 `skill_turn_receipt.content_sha256`；package 缺失时进入显式 `SKILL_PACKAGE_MISSING_ON_RECOVERY`，不得换成新版本继续；
- 同一 turn 的重复 `skill_load` 幂等返回既有 receipt。
- 每个 root turn 保存 selection summary，允许 `selected_count=0`；仅 selected 或 load-attempted Skill 产生完整 load receipt，matched-but-not-selected 保存稳定理由，不为所有 enabled Skill 逐项造 receipt。

## 10. Slash command 与资源

### 10.1 Slash command

- command registry 由 activation snapshot 构造；禁用 Skill 的命令不出现；
- builtin command 保留最高优先级，Skill command 冲突必须在 enable 时阻断或要求改 alias；
- Workspace 建议、键盘提交、command router 和 Agent template expansion 使用同一个 command definition；
- command 执行先展开为普通用户输入或 typed action，并记录 Skill package digest；
- 未实现前，资源中心不得把 slash commands 标记为“可用”。

### 10.2 Resources

- `skill_resource_read(package_id, relative_path)` 只允许 normalized manifest 声明的资源；
- 文本资源按字符预算读取，二进制资源返回 metadata 或经现有 payload 工具显式处理；
- `SKILL.md` 中相对引用在安装时解析并校验；
- scripts/templates 必须完整保存；required execution 未支持时阻止 activation，optional scripts 可作为只读附件；P0 不自动执行；
- 资源访问写入 turn receipt，便于证明实际使用。

## 11. 更新、回滚与删除

- 更新先安装成新 immutable package，展示 manifest/content/policy/resource diff；
- 用户批准后只切换 activation 指针，不覆盖旧目录；
- 切换失败时 transaction 回滚到旧 package；
- 保留最近 2 个已批准版本或按磁盘策略回收；正在被 open objective receipt 引用的版本不得回收；
- rollback 是一次新的 activation receipt；
- uninstall 默认移入 app trash，先禁止新 activation，再等待当前 turn 结束；
- purge 属于不可恢复删除，需要明确确认；
- builtin Skill 可 disable，不直接 purge；升级由 app artifact 管理。

## 12. Legacy 迁移

输入：现有 `<config>/CodeFactory/skills/<id>/manifest.json + system_prompt.md`。

迁移步骤：

1. 复制原目录到 `legacy-backup/<migration-id>`，不修改源目录；
2. 每个目录独立生成 migration operation；
3. 合法 manifest/prompt 转为 v2 package；只有与当前正式版签名 builtin package digest 精确匹配且旧状态为 enabled 的项，可以写 `review=approved, approval_basis=builtin_release` 并保留 activation；所有其他 builtin、外部和本地 legacy 均 `unreviewed + disabled`；
4. `legacy_grandfathered` 作为未来兼容枚举保留，但 initial rollout 不视为 runtime eligible；若未来启用，只可延续原 prompt-only 能力，UI 必须显示“兼容启用，待复核”，新增 resources/commands/policy 在显式复核前不可加载；首次升级提供“待重新审核”队列，每个 activation 都有 migration/activation receipt；
5. 缺失/损坏 manifest 显示 `legacy_corrupt`，不加载；
6. 同一 migration 可跨 crash 幂等重放；
7. 全部成功并经过一个稳定版本后才允许清理 legacy backup；
8. rollback 只恢复 legacy 数据备份供重新迁移或旧版本 App 人工处理；candidate App 不得恢复 pre-containment loader、旧默认启用、前缀 permission allow、旧 registry 或任何已封堵的安全旁路。

不得以“兼容”为由恢复来源不可证明的 enabled prompt；任何放开 `legacy_grandfathered` 的后续变更都需要独立 threat review、Scenario 与用户迁移说明。

迁移 release 必须使用上一正式版本生成的合成数据目录执行 L4 升级验收。

## 13. Payload 与性能边界

初始默认值（实现时放入集中配置并测试）：

| 限制 | 默认值 |
| --- | ---: |
| registry response | 2 MiB |
| raw `SKILL.md` | 256 KiB |
| remote archive compressed | 20 MiB |
| archive expanded | 100 MiB |
| package files | 1,000 |
| directory depth | 10 |
| single resource | 20 MiB |
| HTTP connect/total timeout | 5 s / 30 s |
| Git fetch/clone timeout | 60 s |
| redirect count | 3 |
| catalog local load P95 | 200 ms / 500 packages |

超限必须返回稳定错误，不得降级为部分包成功。

## 14. 事件与可观测性

内部事件：

```text
skill://operation-updated
skill://catalog-changed
skill://review-changed
skill://activation-changed
skill://runtime-selected
skill://runtime-loaded
skill://runtime-failed
skill://update-available
```

每个事件包含 `operation_id` 或 `receipt_id`、skill/package 身份、状态、时间和稳定错误码；不包含完整 prompt、用户任务、凭据或本机原始路径。

`selection_reason` 对持久事件只保存 enum/rule ID；发送前 preview 与 anonymous turn 的自由文本只存在内存。匿名回合不得触发本地持久 receipt、外部遥测或诊断导出。

资源中心订阅 catalog/operation 事件；Agent tool、UI button 和 migration 产生的变更都能即时投影，不再依赖某个 store 自己调用 `reload()`。

## 15. 测试架构

### L0 契约与纯逻辑

- source union、manifest schema、path normalization、Unicode/Windows/Unix corpus；
- state machine 非法转换；
- permission intersection；
- resolver precedence、command collision、budget/truncation；
- error code stability。

### L1 集成

- 临时 package store + 真实 SQLite/WAL；
- fake HTTP/registry/Git/local adapter；
- partial failure、digest mismatch、archive bomb、symlink、duplicate ID；
- atomic commit 前后 fault injection；
- catalog reconciliation 与 orphan/corrupt 投影；
- install→review→activate→load→disable 完整 receipt。

### L2 真实进程

- fetching、validating、committing、migration、update activation 各 checkpoint hard kill；
- App 重启后幂等恢复；
- Headless 与 UI 进程读取同一 lifecycle index；
- Objective 恢复固定旧 digest。

### L3 真实桌面

- 资源中心安装抽屉、失败项、审核、scope enable、更新 diff 和回滚；
- Workspace 显式选择、自动候选、已加载/失败 chip；
- 正常窗口与窄窗口；
- slash command 只有真实接入后才验收。

### L4 正式产物

- 上一正式版本数据目录 → candidate artifact migration；
- 安装、升级、首次启动 reconciliation、真实 Skill 主路径、回滚；
- receipt 中 build SHA/package digest 与 exact artifact 一致。

## 16. 关键权衡

| 决策 | 收益 | 代价 |
| --- | --- | --- |
| immutable package + SQLite state | 更新/回滚/审计清晰，避免包内 enabled 漂移 | 增加 schema 和 reconciler |
| typed source adapter | 消除 URL 猜测和多入口分叉 | API 更显式 |
| compact catalog + `skill_load` | 减少上下文污染，产生使用证据 | 多一次工具回合，需 resolver 设计 |
| 默认禁用 | 阻断远程 prompt 自动生效 | 安装到价值多一步审核 |
| P0 不执行脚本 | 大幅降低供应链风险 | 部分生态 Skill 只能只读使用或被标不兼容 |
| 官方 registry envelope 验签 + package digest | 同时固定目录真实性与包内容 | 需要随 App 管理 registry key rotation；第三方作者身份仍待后续 |
| 异常包始终可见 | 用户能诊断与恢复 | 资源中心状态更多，需要清晰 UX |

## 17. 实现切片建议

1. `security-containment`：path boundary、默认禁用、稳定错误；
2. `package-schema-store`：v2 schema、store、SQLite、reconciler；
3. `installer-adapters`：typed adapters、staging、atomic commit、events；
4. `legacy-migration-catalog`：迁移与资源中心真实状态；
5. `runtime-resolver`：catalog、activation snapshot、`skill_load`、receipt；
6. `workspace-evidence`：Skill chip、详情、Headless/恢复一致；
7. `update-rollback-commands`：更新、回滚、trash、slash commands；
8. `release-canary`：scenario registry、跨进程与 exact artifact 验收。

每个切片都必须基于 Req ID 和 `SKL-*` Scenario，不能再次形成旁路安装器或旁路 loader。

### 17.1 Phase 0 临时合同

Phase 0 安全止血允许暂时继续读 legacy 目录，但只能通过一个 `LegacyCatalogAdapter` 投影到 §3.3 相同的 `SkillCatalogProjection`、错误码和安全 ID 类型。adapter 在投影时立即执行安全 eligibility：只有与当前正式版签名 builtin digest 精确匹配且旧状态为 enabled 的项可进入 runtime；其他 legacy 一律 `unreviewed + disabled`，损坏包可见但不可加载。UI、Headless、Objective recovery 都只能读取该投影，禁止旁路 adapter 调用旧 loader。mutation 必须立即切到统一 path/permission gate，外部 import 默认 disabled，官方 registry 使用签名 snapshot。该 adapter 是带删除条件的迁移桥，Phase 1 v2 package commit 后复用同一 installation identity，禁止再增加第二套 scanner/store/API。上一版本 fixture 必须证明外部 legacy `enabled=true` 在 UI、Headless 和恢复入口均零加载。
