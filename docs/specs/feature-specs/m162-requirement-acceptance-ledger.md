# M162：规格逐条验收账本——每个 R 项都要有对应证据才算完成

## 背景
- 10-22：M145 规格的 R3（会话诊断）被漏做却声称完成；M155 R5（跨进程 smoke）被推迟；用户问「能确保修改不破坏已经 OK 的功能吗」。
## 需求
- R1 交付前生成 `docs/acceptance/<task-id>.md`（或 PR body 段落）：规格中每个 `R<n>` 一行——状态（done/deferred/n/a）、证据（测试名/命令输出/文件:行）。deferred 必须写原因与后续任务。
- R2 governance-baseline 校验：PR 引用的规格（docs/specs 下或 PR body 声明的 spec 路径）里出现的每个 R 编号都在账本中出现；状态为 done 的必须有测试名或证据路径；缺项 fail closed。
- R3 交付工具开 PR 前本地预检同一校验（复用 M156 的预检通道），缺项先补而不是推注定失败的 PR。
- R4 测试（先失败）：缺一项 R → 失败；deferred 无原因 → 失败；完整账本 → 通过。
## 约束
- task-template；改 tools/governance 属信任根，PR 里注明需要 gate-dance。授权交付（ceiling: merged）。

Release-Urgency: default

## Implementation Notes（M162 实施后补写）

### 唯一权威：需求编号来自规格本身

`tools/governance/validate_acceptance_ledger.py` 只从规格的「需求」章节条目里读
`R<n>`（`- R1 …` / `- **R1 …**`），不维护任何 R 编号清单。正文、实现对照表、
「约束」章节里的 `R` 引用都不会被当成新需求，也不会被要求解释——这一条由
`test_parse_requirements_reads_需求_section_only` 固定。

账本路径同样由规格文件名推导：`docs/specs/**/m162-xxx.md` →
`docs/acceptance/m162.md`；PR body 显式写 `Acceptance-Ledger: <path>` 时以声明为准。

### 账本契约

| 状态 | 必须写什么 | 缺了会怎样 |
| --- | --- | --- |
| `done` | 测试名（`path::test`）、`file:line`、证据路径或一条测试命令 | fail |
| `deferred` | **同时**写原因与后续任务（`原因：… \| 后续：m170`） | fail（这正是 M155 R5 的形态：只说"推迟"） |
| `n/a` | 不适用的原因 | fail |
| 其它状态 / 缺行 | — | fail |

表格行与列表行等价（`| R1 | done | … |` 与 `- R1 | done | …`），解析器不依赖列宽。

### 三个入口共用同一份判断

1. **CLI（R2 的基线侧）** `validate_acceptance_ledger.py --repo . [--body-file p] [--base-ref main]`：
   - 有 body/base-ref：校验 PR 引用到的规格（正文声明 + `base...HEAD` 变更的
     `docs/specs/**`）都有完整账本，`checked = pr_referenced_specs`；
   - 都没有（push 到 main）：自检已存在的 `docs/acceptance/*.md` 与规格自洽，
     `checked = existing_ledgers`；
   - 任何问题 → 退出码 1 + JSON 报告，fail closed。
2. **governance-baseline**（R2）：`tools/governance/validate_repo_governance_baseline.py`
   在原有 manifest/marker 检查之外调用同一函数；body 取 `--body-file`，缺省时取
   `GITHUB_EVENT_PATH` 里的 `pull_request.body`。新增失败码 `ACCEPTANCE_LEDGER_INCOMPLETE`。
3. **交付预检**（R3）：`precheck_thin_pr_body` 的 `checks` 表里加一行，脚本以
   `--body-file / --repo / --base-ref` 被调用——与 `validate_readme_contract.py`、
   `validate_scenario_test_governance.py` 完全同一条通道，因此"缺项先补而不是推
   注定失败的 PR"不需要第二套实现。脚本不存在时该仓库自动跳过（`governance`
   不必空转在没接这个契约的仓库上）。

### 为什么不门禁所有历史规格

`_self_check_existing_ledgers` 只校验**已经存在**的账本，`check_pr_acceptance_ledger`
只校验**这次 PR 引用或改动**的规格。M156 等历史规格没有账本不会让任何一次无关
提交变红；但只要有人改动那份规格或引用它，缺项立刻 fail closed——即"新账从这次
开始记，旧账只在被触碰时结清"。这是能立刻生效又不制造大面积红的最低线。

### 信任根说明（gate-dance）

本 PR 改动了 `tools/governance/validate_acceptance_ledger.py`（新增）与
`tools/governance/validate_repo_governance_baseline.py`（接线），属于治理信任根
变更：必需检查需由默认分支 trusted runner 跑完，普通 `force` 不得绕过。

### 证据

见 `docs/acceptance/m162.md`（本规格 R1–R4 的逐条证据）。
