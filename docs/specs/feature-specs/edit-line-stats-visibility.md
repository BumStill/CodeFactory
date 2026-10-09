# 文件编辑行数与回合改动总和可读性规格

## 问题与目标

用户对当前会话里编辑行展示的原始反馈是：

> 「编辑文件行最后写 123b-169b 是指文件大小吗，这有啥用啊，肯定是 +多少行 -多少行，更符合常识吧。最好有个地方显示当前这次修改总和多少。」

两个问题：

1. 折叠的工具行把 `edit_file` 的 `old_string` / `new_string` **字符数**当成摘要（`src/foo.rs (985b → 4133b)`）。这既不是文件大小，也不是用户能据以判断“改了多少”的信息；git、GitHub、编辑器一律用 `+X −Y` 行数表达同一件事。
2. 回合结束时没有任何地方告诉用户“这一次总共改了多少”。结果卡只有改动的文件数量，没有量级。

目标：编辑行用行数表达改动量，回合结果卡用一行给出本回合的总和，两者都遵循既有颜色约定且不引入内部术语。

## Requirements Traceability

| Req ID | 要求 | Surface | Scenario IDs | 验证 |
| --- | --- | --- | --- | --- |
| CF-ELS-R1 | 折叠的 `edit_file` / `edit` 行必须显示该次改动的新增与删除行数 `+X −Y`；不得再显示参数字符数、字节数或 `→` 形式的长度对比。一次调用里有多个编辑段时必须求和，不能只报第一段 | 会话工具行（`src/components/ToolCallCard.tsx`） | UI-013 | `src/components/ToolCallCard.lineStats.test.tsx`；真机验收 `verify-edit-line-stats-headless.mjs` |
| CF-ELS-R2 | 行数必须按行级 diff 计算：未改动的上下文行不得同时计入新增与删除（40 行文件改 1 行 = `+1 −1`，不是 `+40 −40`）；CRLF、裸 CR、结尾换行、空文件、纯追加、纯删除必须给出一致结果 | `src/lib/editLineStats.ts` | UI-013 | `src/lib/editLineStats.test.ts`（12 例） |
| CF-ELS-R3 | `write_file` / `write`：确认新建文件时显示 `+N`；覆盖已有文件且旧内容不在参数里时只显示新写入行数（`写入 N 行`），不得编造删除数，也不得显示 `+0` | 会话工具行 + `src/lib/editLineStats.ts` | UI-013 | `ToolCallCard.lineStats.test.tsx`（新建 / 覆盖两个用例） |
| CF-ELS-R4 | 未真正落地的调用（失败、被拒、取消、阻断）不得显示行数统计，也不得计入回合总和 | 工具行 + `summarizeTurnEvidence` | UI-013 | `ToolCallCard.lineStats.test.tsx`、`TurnResultSnapshot.lineStats.test.tsx`；真机验收的 denied 用例 |
| CF-ELS-R5 | 回合结果卡必须以一行显示“本次改了 N 个文件”与本回合总计 `+X −Y`；点击该行打开既有改动视图（文件列表）。本回合没有行数改动时不显示该行，不产生噪音 | `src/components/TurnResultSnapshot.tsx` | UI-013 | `TurnResultSnapshot.lineStats.test.tsx`；真机验收点击用例 |
| CF-ELS-R6 | `+` 必须为绿色、`−` 必须为红色，颜色取自主题 token；明暗两种主题下文字与卡片底色对比度不低于 3:1；编辑行与结果卡在窄视口均不得横向溢出 | `src/components/LineChangeStats.tsx` | UI-013 | 真机验收：双主题取计算色 + 对比度断言 + 溢出断言 + 截图 |
| CF-ELS-R7 | 只读工具行（read / grep / glob / web / kb / skill 等）展示与行为不变；不修改工具结果 JSON、后端工具实现与 provider 请求 | 会话工具行、`src-tauri/src/tools/*` | UI-013 | `ToolCallCard.lineStats.test.tsx` 只读用例；改动集只有前端口径 |

## Primary User Path

用户在会话里让 agent 改代码。每完成一次编辑，折叠的工具行显示 `编辑 · path +X −Y`，用户不必展开即可判断改动量级；被拒绝或失败的编辑保持可识别但不谎报行数。本回合结束时，结果卡出现一行「本次改了 N 个文件 +X −Y」，只统计真正落地的编辑；用户点击它即打开既有的改动视图查看文件列表。用中文表达，不出现内部术语。

## Applicable Harnesses

- **Spec Harness**：CF-ELS-R1..R7 全部有对应测试或真机断言。
- **Compatibility Harness**：工具别名（`edit`/`edit_file`、`write`/`write_file`）、light/dark 主题 token、以及历史会话中缺少 `result` 或旧参数形状的工具调用（降级为不显示统计，绝不抛错）。
- **Viewport Harness**：编辑行与结果卡在 1100×900 与 430×900 窄视口下的布局、横向溢出与点击区域；明暗两种主题截图。目标视口与溢出规则见下方测试矩阵。
- **Observation Harness**：真实浏览器（Chrome）渲染生产组件并读取计算样式，而不是 jsdom；jsdom 不排版、不算色。
- **AI Collaboration Harness**：记录 context scope、假设、审查点与验证结果（见本规格末尾与 PR 正文）。
- **Release Harness**：本切片不适用——交付边界为 `pr_only`，未发布、未安装产物验证，最终状态必须记为 `not live`。

## 测试矩阵

| Path type | Scenario ID | 用户路径 / 故障 | 期望结果与 oracle | Gate / evidence level | 自动化或证据 |
| --- | --- | --- | --- | --- | --- |
| Primary path | UI-013 | 一次改一行的编辑 | 行显示 `+1 −1`，且不含 `→` / `N b` | unit + real browser | `ToolCallCard.lineStats.test.tsx`、`verify-edit-line-stats-headless.mjs` |
| Primary path | UI-013 | 只加 3 行 / 只删 2 行的编辑 | `+3 −0` / `+0 −2` | unit + real browser | 同上 |
| Primary path | UI-013 | `write_file` 新建文件 | 只显示 `+4`，无删除数 | unit + real browser | 同上 |
| Primary path | UI-013 | 回合结束看总和 | 一行「本次改了 4 个文件 +8 −3」 | unit + real browser | `TurnResultSnapshot.lineStats.test.tsx`、真机验收 |
| Primary path | UI-013 | 点击总和行 | 打开既有改动视图（文件列表出现） | real browser | 真机验收点击断言 + 截图 |
| Failure path | UI-013 | 用户拒绝 / 调用失败 / 取消的编辑 | 不显示任何行数，也不计入总和 | unit + real browser | `ToolCallCard.lineStats.test.tsx` denied 用例、真机 denied 断言 |
| Compatibility path | UI-013 | 明暗主题、窄视口、只读工具行、旧历史调用 | 颜色与布局不变形、不溢出；只读行无统计；旧调用不报错 | real browser + unit | 双主题截图、溢出断言、只读用例 |
| Complex E2E | — | 不适用 | 本切片不涉及持久状态、真实进程或交付副作用 | — | — |
| Release path | — | 不适用（`pr_only`，未发布） | 必须写明 `not live` | — | — |

### Viewport Harness 细节

- Target viewport：1100×900（桌面）与 430×900（窄）。
- Overflow rule：`scrollWidth <= clientWidth + 1`，编辑行与结果卡各自断言。
- 颜色规则：`+` 绿通道占优、`−` 红通道占优，与卡片底色对比度 ≥ 3。
- Screenshot：`docs/testing/evidence/m36-edit-line-stats/edit-line-stats-dark.png`、`edit-line-stats-light.png`、`edit-line-stats-open-changes-light.png`。

## Evidence Pack Requirements

- 单元/组件测试输出（`pnpm test` 全量绿，含 `lightModeAudit`）。
- `pnpm build` 通过输出。
- 真机浏览器验收命令与通过 JSON（`node scripts/verify-edit-line-stats-headless.mjs`；不新增 `package.json` 脚本别名，避免把全局清单文件带进改动集）。
- 明暗双主题截图 3 张（上面路径）。
- 场景声明：`Scenario-Test: UI-013`。

## 兼容性和发布边界

- 只改前端展示层；工具结果文本、后端工具实现、provider 请求与持久化格式均不变，历史会话可直接渲染。
- `write_file` 覆盖场景无法从参数得知旧内容，因此永不显示删除数——这是刻意的诚实降级，不是缺陷。
- 本切片不发布：交付边界 `pr_only`；发布与真实安装产物验证由 release task 负责。

## Implementation Notes

- 行数统计集中在 `src/lib/editLineStats.ts`：先做公共前缀/后缀裁剪，再对剩余区域求 LCS；极端大文本（裁剪后仍超过 4,000,000 个 DP 单元）退化为“改动区域全部计新增/删除”，避免卡住 UI。
- 结尾换行不算独立一行（`"a\nb\n"` 是 2 行），CRLF 与裸 CR 一律按行边界处理。
- `write_file` 的新建判定读取工具结果里的 `@@ -0,0 +N,M @@` hunk 头或显式新建措辞；判不出来就退回 `写入 N 行`。
- `ToolCallCard` 与 `TurnResultSnapshot` 共用 `toolCallLineStats`，因此行与总和不会出现口径分叉。
