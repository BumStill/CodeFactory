# M271：一条过期登记不能撤销整个工作区的 heredoc 证明

## 现场（2026-11-09）

- 会话 `38c27637`（v1.113.0，trusted 模式）里，下面这条命令停下来要审批：

  ```
  git add -A && git commit -q -F - <<'EOF' 2>&1 | tail -20
  M271: …
  EOF
  ```

  M266 已经证明这条拼写在判定层就是数据（`m266_message_heredoc_survives_output_pipeline`），
  所以问题不在 heredoc 判定，而在**网关拿不到「已登记且身份一致的工作区根目录」时静默跳过证明**。
- 具体链路：`managed_workspace_root()` 按当前活动 scope 查登记失败（该会话里有一条兄弟登记，
  其分支已被改名，`created_at/updated_at` 更新），随后 `registered_git_roots()` 又因为
  `verify_objective_workspace()` 用 `?` 提前返回，把**任何一条校验不过的登记**当成「整个集合不可信」，
  于是 `roots = None`、`root = None`，`heredoc_proved` 保持 false，命令落进通用
  `trusted.heredoc.unproved`。
- 用户看到的是「无法证明」，而真实情况是「这一步根本没执行」——文案把「没查到」说成了「被证伪」。

## 决策

1. **逐条校验，不整体放弃**（`registered_git_roots`）：只把 `verify_objective_workspace()` 通过的
   登记放进根目录集合；一条因分支改名、worktree 移除、目标已结束而失效的登记，不得撤销同会话
   其他仍然完好的 worktree 的证明力。
2. **集合为空仍然什么都不证明**：`(!roots.is_empty()).then_some(roots)` 保持不变——没有一条通过校验
   时，heredoc 边界依旧未经验证，依旧要审批。
3. **根目录回退只在无歧义时发生**：`managed_workspace_root()` 取不到时，若本会话已登记且通过校验的
   根目录**恰好一个**，用它；多于一个不猜。
4. **文案说清是「没查到」而不是「被证伪」**：跳过证明时用
   `request_permission_with_reason`，`rule` 仍为 `trusted.heredoc.unproved`，说明写
   「无法定位本会话已登记且身份一致的工作区根目录，因此边界没有经过证明（不是内容被证伪）」。
5. **不放宽门禁**：解释器 heredoc、真实 Git 写、`rm -rf`、无法解析的结构照旧审批；没有可验证根目录时
   消息 heredoc 也照旧审批（R3）。

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
| --- | --- | --- |
| R1 | 现场拼写（含 `cd "$(pwd)" &&`、`2>&1 \| tail -20`、无重定向管道）在真实网关下必须放行，且存在过期兄弟登记时仍放行 | `m271_message_heredoc_behind_output_pipeline_is_allowed`（R1 两段现场拼写 + scope 移动后的第三段） |
| R2 | 被放行的命令确实执行：`HEAD` 必须变化 | 同一用例的 `assert_ne!(rev-parse HEAD, head_before)`（放行前后各一次） |
| R3 | 没有任何可验证登记时，消息 heredoc 仍要审批；整轮零审批记录 | 同一用例尾部把唯一登记的 `branch_name` 改掉后断言 `!= Allow`，并断言 `permission_intents == 0` |
| R4 | 会话内每个已登记且身份一致的 worktree 都能证明；过期登记只跳过自己 | `registered_git_roots` 逐条 `is_ok()` + 用例中的 `synthetic-stale` 兄弟登记 |
| R5 | 文案区分「没查到根目录」与「内容被证伪」 | `request_permission_with_reason` 的新说明文本；`rule` 未变 |

## Applicable Harnesses

- **Spec Harness**：本文件 + PR body 的 Req↔测试映射，`Scenario-Test: UI-009`。
- **Compatibility Harness**：`registered_git_roots` 的签名与「空集合 = 不证明」语义不变；
  `m159_*`、`m160_*`、`m166_*`、`m221b_*`、`m266_*` 全部保持通过。
- **Observation Harness**：审批卡片 `rule` 仍为 `trusted.heredoc.unproved`，但 reason 现在能区分
  「没有根目录」与「边界被证伪」，可据此统计这两类误判。

## 测试矩阵

| 场景 | 层级 | 断言 |
| --- | --- | --- |
| `git commit -F - <<'EOF' 2>&1 \| tail -20` | Rust（真实网关 + 真实 git） | `Allow`，`HEAD` 变化 |
| 同上但无重定向 `\| tail -20` | Rust | `Allow`，`HEAD` 变化 |
| 存在过期兄弟登记（分支已改名）且 scope 移到它 | Rust | 仍 `Allow`，`HEAD` 变化，`permission_intents == 0` |
| 唯一登记也失效 | Rust | `!= Allow`（不放宽） |
| 解释器 heredoc / 真实 Git 写 / `rm -rf` | Rust（M266 回归） | 仍然审批 |

## 证据（2026-11-09）

- 修复前用真实网关跑 `m271_message_heredoc_behind_output_pipeline_is_allowed` → 失败，末条审批
  `outcome: Deny(... TimedOut)`，提示原因是旧的无原因文案。
- 分别回退「逐条校验」与「单根回退」两处改动，各自都能单独把该用例打回失败 → 两处都是必要条件。
- `cargo test --lib agent::permission_gateway::tests` → `34 passed; 0 failed`。
- `cargo test --lib agent::trusted_git::tests` → `2 passed; 0 failed`。
- 完成声明边界：本文件记录的是网关层（真实 git 仓库 + 真实 `permission_intents` 表）的机器证据；
  「用户桌面会话里同一命令不再弹窗」需要在主路径复核，本轮未声明为已验证。
