# M250：沙箱提示只在拒绝证据存在时出现

## 背景
信任模式下，普通命令非零退出被错误追加「沙箱拦截」。

## 规格（原文）
- R1：只有输出里有沙箱拒绝的证据时才追加沙箱提示。证据的具体判定由实现决定，要用真实 macOS 沙箱的拒绝输出来定，不靠猜。
- R2：普通失败（`grep` 无匹配、`git config --get` 配置项不存在、`false`、测试失败）保留原始输出和退出码，不出现「沙箱拦截」。
- R3：真实的沙箱拒绝仍然带提示，有路径时写明路径。M224 的 R8 不退化。
- R4：只改提示文字的判定，不改沙箱策略本身，不放宽任何拦截。

## 测试（原文）
- 在真实沙箱里运行：写工作区外的文件（有提示）、`false`（无提示）、`grep` 无匹配（无提示）、`git config --get` 不存在的键（无提示）。R2 的用例要先看到失败。
- 沙箱不可用的平台（Windows、CI）上，这些测试要说明覆盖范围，不能让跳过的用例算作证据。

## 验收（真机，编排方发版后做）
信任模式会话运行 `git config --get` 不存在的键，结果里没有「沙箱拦截」；写工作区外的文件仍然被拦并带提示。

## Requirements Traceability
| Req ID | 最低证据 |
| --- | --- |
| R1 | 真实 Seatbelt 错误格式的拒绝识别测试 |
| R2 | 普通失败真实进程退出码与输出断言，先红后绿 |
| R3 | 拒绝输出保留提示和目标路径 |
| R4 | 策略生成代码无修改，既有 M224 测试 |

## Applicable Harnesses
Spec、Compatibility、Payload、Observation、Release（交接，不发版）、AI Collaboration。无 UI 修改，不适用 Viewport。

## Implementation Notes
- 本会话自身受 Seatbelt 约束，`sandbox-exec` 嵌套探测实际返回 `sandbox-exec: sandbox_apply: Operation not permitted`；不能把嵌套失败或跳过当作新建真实沙箱成功。
- 本会话实际 Git 写入拒绝输出为 `error: could not lock config file <path>: Operation not permitted`，用于错误格式回归；不改任何沙箱策略。
- 真机验收由编排方发版后完成；本任务只合并，不发布安装版。
