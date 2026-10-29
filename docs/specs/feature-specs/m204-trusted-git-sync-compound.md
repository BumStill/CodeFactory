# M204：信任模式下「同步主分支 + 后续命令」的复合命令仍弹审批（M198 遗漏）

Release-Urgency: immediate

## 现象（10-29 21:33，v1.91.59，会话 63368532，信任模式）
M198 上线后 30 分钟内信任模式审批为 0，但随后立刻出现：
```
git fetch --prune origin main && git merge origin/main; /opt/homebrew/bin/python3 tools/governance/validate_scenario_test_governance.py --ci --base-ref origin/main --title '...'
```
被 `trusted.git.unproved`（「无法证明 Git 操作仅影响本会话已登记工作区，或无法完成可恢复备份」）拦下。这是仓库 AGENTS.md 强制要求的并行分支同步动作，每单都会做，属于每单必弹。

## 要求
1. 先复现：用 M198 的表驱动夹具补这条用例（`&&` 串 fetch+merge，再用 `;` 接一个只读/工作区内命令），确认当前判红。
2. 根因要落在「为什么证明失败」：是复合命令切分后 `;` 段让整体无法证明？是 cwd 推断不到登记工作区？还是 merge 前备份步骤失败？写清楚，不要只把这串字面量加白名单。
3. 规则：在本会话登记工作区内的 `git fetch`（任意 remote/refspec，含 --prune）、`git merge origin/<默认分支>`（含 --autostash、--no-edit）与其它已放行段任意 `&&`/`;`/`||` 组合，信任模式下不弹审批；merge 前的可恢复备份失败时应自动降级为「先 stash 再 merge」或报明确错误，而不是转成用户审批。
4. 不放宽：push 到默认分支以外的远端动作、reset --hard 到非本会话产物、删除他人分支等仍按原规则。
5. 家族回归：把 M198 夹具里的所有命令再与 `; <只读命令>`、`&& <工作区内 python/pnpm 命令>` 组合各跑一遍，全部不弹。

## 验收
- 新夹具全部通过；真机信任模式会话执行上面原样命令不产生 permission_intents 记录。

## Implementation Notes

### 根因
`segments` 已支持三种分隔符，cwd 登记身份验证不受影响。`prove` 对非 Git 段只接受有限的只读 filter；Python 文件执行和 pnpm 工作区脚本均落入 `_ => return None`。因此整串证明在调用 snapshot 前失败，gateway 把证明失败与备份失败都合并为审批。

### Requirements Traceability
| Req ID | 对应要求 | 最低证据 |
| --- | --- | --- |
| M204-R1 | 原始复现与根因 | M198 gateway 表先红后绿 |
| M204-R2 | 组合、remote/refspec 和工作区脚本 | 分隔符交叉表、真实本地 bare remote 执行、permission_intents 增量为零 |
| M204-R3 | 备份失败明确报错、不审批 | 冲突 index 触发真实 write-tree 失败，检查 Deny 内容与 intent 零增量 |
| M204-R4 | 安全边界不放宽 | 非本会话目录、脚本逃逸、远端写入回归 |
| M204-R5 | M198 家族后缀组合 | 原表全部追加只读和 Python/pnpm 后缀 |

### Applicable Harnesses
Spec、Compatibility、Release（仅合并交接，不发版）、Observation、Payload、AI Collaboration。无 UI 变更，不新增 Viewport 场景。入口仍为 bash/本地 dispatch，不依赖 GUI。

### 验证与限制
- 原始事故命令已先红：审批超时 `TimedOut`，不是 Git snapshot 命令报错。
- 实现后同名 gateway 测试通过；覆盖原始事故形状、M198 JSON 正/负向家族后缀、3×3 分隔符、通配 fetch refspec、真实冲突 index 备份失败。
- 真实 bare remote + shell + Python：校验 `checked.txt=verified`、`origin/main=HEAD`、permission_intents 无增量。
- 全工作区测试曾发现新增测试进程遗漏 `.no_window()`，已补上后重跑，未弱化测试。
- 沿用 trusted 模式已允许的工作区脚本入口，不声称对任意 Python/npm 代码做语义证明或提供 OS 沙箱。脚本和 manifest 必须在工作区，pnpm 任务须在 scripts 声明，inline interpreter code 与工作区外参数仍不进入组合证明。
- 只扩展 fetch 的 remote/refspec 参数域；执行型 remote helper、upload-pack override、push、删分支与现有 reset 规则不变。

### 验收分工
真机验收由编排方发版后完成：安装包含此修复的版本，启用信任模式，在登记工作区执行任务原始命令，核对实际 cwd、merge 结果及 permission_intents 无新增。会话内采用真实 Git/解释器进程、合成登记工作区和 SQLite 集成测试；不宣称已上线。
