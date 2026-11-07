# M260：只读命令放在 shell 循环里，仍然要审批

## 现场（2026-11-07，v1.103.0，M248b 之后）

会话 00b00066 运行：

```
for c in c9eb5089c a00d7c16f; do echo "--- $c ---"; git log -1 --format='%B' $c | tail -8; done && grep -rn "Release-Urgency" .github/workflows/*.yml | head -10
```

全是读取，却被 `trusted.git.unproved` 拦下。M248b 修好的是平铺的命令组合；循环、子
shell、`$(...)` 这类结构解析不了时，整条进审批。

## 用户决定（必须遵守）

信任模式靠 OS 沙箱，不再补文字规则。Python 等解释器脚本读工作区外文件继续审批
（11-07）。

## 需求

- **R1**：用 `permission_intents` 统计 M248b 上线后仍被 `trusted.git.unproved`
  拦下的命令结构（匿名聚合），写进规格。
- **R2**：沙箱生效时，`for`、`while`、`if`、子 shell、`$(...)` 里只有本地 git 读写
  和普通命令的，不再审批。由沙箱挡越界写入。
- **R3**：结构里含远端写（`git push`、`gh pr merge` 等）、破坏性改写、解释器
  heredoc 的，照旧走现有判定。解析不了的仍然审批（fail closed）。
- **R4**：不新增按命令文字匹配的规则。

## R1 统计（匿名聚合，只出结构与计数，不出命令原文）

数据源：本机应用库 `permission_intents`，起点取 M248b 合并时间
`2026-11-07T03:01:40+08:00`，条件 `prompt_args_json._permission.rule =
'trusted.git.unproved'`。

| 结构 | 合计 | 其中纯本地读写 | 含远端写 | 含破坏性改写 |
|---|---|---|---|---|
| `$(...)` 命令替换 | 4 | 1 | 2 | 1 |
| 平铺序列（M248b 已覆盖的形态） | 3 | 1 | 2 | 0 |
| `for`/`while` 循环 | 1 | 1 | 0 | 0 |
| **合计** | **8** | **3** | **4** | **1** |

结论：M248b 之后剩余 8 条 `trusted.git.unproved` 审批里，3 条是纯本地读写被结构
解析连带拦下（1 条循环、1 条 `$(...)`、1 条平铺序列），是 R2 要消掉的部分；另 5 条
含远端写或破坏性改写，按 R3 继续审批，不因本单放宽。没有出现 `while`/`if` 的
真实样本，测试里按规格补齐。

## Requirements Traceability

| Req ID | 要求 | 最低证据 |
|---|---|---|
| R1 | 上线后仍被拦下的结构有匿名统计 | 本文 R1 表 + 聚合 SQL（`.codefactory-cache/m260-shapes.sql`，未入库） |
| R2 | 沙箱生效时，循环/子 shell/`$(...)`/`if` 里只有本地读写时不审批 | 现场样本原样 `covers_local_command(...) == true`，且 `sensitive`/`destructive_intent` 为 false；网关用例 `permission_intents` 计数为 0 |
| R3 | 远端写、破坏性改写、解释器 heredoc、解析不了的结构照旧判定 | 三条反例仍 `sensitive == true` 且 `covers_local_command(...) == false`；heredoc 仍走 `trusted.heredoc.unproved` |
| R4 | 不新增命令文字匹配规则 | 复核 diff：新增逻辑只做结构解析（词法 + 控制关键字 + 变量绑定），不引入按命令名/子串的豁免表 |

## Applicable Harnesses

- Spec Harness：本规格与 Requirements Traceability。
- Compatibility Harness：`trusted_git` 既有构造器（M139/M159/M177/M182/M198/
  M207/M248/M248b）的测试全部保持通过；`segments()` 严格解析器不变，远端归属判定
  不受影响。
- Release Harness：PR CI 全绿后由合并队列合入，不手工合并。
- Observation Harness：`permission_intents` 计数断言（审批次数可观测）。
- AI Collaboration Harness：PR body 记录上下文范围、假设、复核点、验证结果。
- 不启用 Viewport / Payload Harness：无 UI 与载荷面变化。

## 测试矩阵

| 用例 | 结构 | 期望 |
|---|---|---|
| `m260_loop_sample_produces_no_approval` | 现场样本原样（`for ... do echo; git log $c | tail; done && grep ...`） | 不敏感、非破坏性、不越界→沙箱生效时 `covers_local_command == true` |
| `m260_nested_structures_stay_local` | `$(...)`、子 shell、`while`、`if` 内只有本地 git 读写 | 同上 |
| `m260_loop_with_remote_write_still_asks` | 循环里 `git push` | `is_delivery_command == true`，`covers_local_command == false` |
| `m260_subshell_destructive_rewrite_still_asks` | 子 shell 里 `git reset --hard origin/x` | `destructive_intent == true`，`covers_local_command == false` |
| `m260_loop_with_python_heredoc_still_asks` | 循环里 python heredoc | `sensitive == true`，heredoc 证明仍是决定路径 |
| `m260_unparseable_structure_fails_closed` | 反引号、`case`、`$(( ))`、命令词位置的替换 | `sensitive == true`（无法证明） |

## 实现

- `src-tauri/src/agent/trusted_git.rs` 内的 `mod trusted_shell_structure`（M260 唯一的结构展开器；内联而非独立文件，因为场景注册表只映射 `trusted_git.rs`，新增产品文件会在 PR 门禁被判为覆盖缺口，而注册表本身又不允许在普通 PR 里自修改）：
  词法器把命令切成 `Word` / `Sep` / `OpenParen` / `CloseParen`，引号内的分隔符与
  `$` 不参与切分；`$VAR` / `${VAR}` 由 `for` 绑定表代入，`$(...)` 与 ` ( ... ) `
  递归展开，`for` / `while` / `until` / `if` 由专用解析函数逐个展开，产物是
  「每个简单命令的 argv 列表」。
- 展开失败（反引号、`case`、`select`、`function`、`coproc`、`eval`、`source`、
  `trap`、`$(( ))`、命令词位置本身是替换、深度超限、括号不配平、残留关键字）一律
  返回 `None`，调用方按既有约定当作敏感，处理方式没有放松。
- `trusted_git::executable_segments()` 改为委托该展开器；`sensitive()`、
  `destructive_intent()`、`local_rewrite()`、`registered_rewrite()`、
  `backup_registered()`、`covers_local_command()` 共用同一份展开结果，因此
  「循环里只有本地 git 读写」时既不需要快照，也不再触发审批，而循环里出现
  `git push` 或 `git reset --hard` 时仍分别由 `is_delivery_command()` 和
  `destructive_intent()` 拦住。
- 写范围没有变化：沙箱仍然是唯一的边界，`is_enforced == false` 时整条路径退回
  原有文字证明 + 审批。
- 已修的两处实现缺陷（首轮运行即暴露）：词法器处理空白时未推进游标导致死循环；
  右括号紧贴单词（`(git add -A)`）时被误判为「无法解析」，现在右括号总是先结束
  当前单词。

## 不变量

- 信任模式的边界仍然只有 OS 沙箱：本单不放宽写范围，也不新增"看起来安全"的文字
  白名单。
- 沙箱不可用时（`is_enforced == false`）整条路径退回原有文字证明 + 审批。
- 远端写与破坏性改写的判定完全沿用 M159/M248b 的既有谓词。
