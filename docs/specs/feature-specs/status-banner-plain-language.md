# 会话顶部状态横幅只显示用户能看懂的真实状态

## Background and user decision

用户原则：内部控制环状态不得出现在 UI；用户看到的任何内容都要普通用户能看懂、符合常识；能删的 UI 优先删掉而不是改名。

证据：

- 2026-10-09 10:38，M36 会话（4d6bbf58）：顶部横幅显示
  `系统仍在处理 · 恢复中 · objective-supervisor:chat · 正在执行命令 · 下次观察 0ms 后 · 25.0s`。
  `objective-supervisor:chat` 是内部标识，`下次观察 0ms 后` 没有意义（0ms）。
- 2026-10-08 11:4x，M31 会话：同一横幅显示 `下次观察 0ms 后`。
- 更早记录（09-2x）：某会话恢复耗尽后仍显示停止按钮，横幅与后端不同步。

用户无法从这条横幅判断系统是否在工作、是否需要自己做点什么、什么时候会继续。

## Decision

会话顶部状态横幅只告诉用户三件事：系统现在在做什么；是否需要用户行动；如果正在等待，大概多久会继续。凡是无法如实表述的内容都不显示。

## Requirements Traceability

| Req ID | Requirement | Minimum evidence |
| --- | --- | --- |
| CF-RSB-R1 | 横幅中不出现任何内部标识或代号（例如 `objective-supervisor:*`、失败码、recovery/remediation/objective/generation 等内部术语）。每一段文字都必须是普通用户能看懂的自然语言 | 覆盖每一种横幅状态的内部词汇守卫测试 |
| CF-RSB-R2 | 时间提示必须如实：绝不出现 `0ms` 或负数时间；下一次尝试临近或已过期时说 `马上重试 / 正在重试`；无法确定时不显示时间；时长使用人类单位（秒 / 分钟） | 表驱动测试：未来 / 临近 / 过期 / 未知 → 对应措辞 |
| CF-RSB-R3 | 横幅状态与真实状态一致：确实在跑时显示 `正在执行`；等系统自动续跑时显示等待加如实估计；任务已结束（completed / failed / cancelled）时不显示“仍在处理”横幅，也不显示停止按钮 | 四种状态的组件测试 + 一个端到端合成状态转换 |
| CF-RSB-R4 | 用户确实需要行动时（授权、需要决定、已失败的终态），直白说明该做什么，并与结果卡、失败摘要保持一致 | 组件测试 |
| CF-RSB-R5 | 对用户决策没有帮助的信息直接删除，而不是改名 | PR 中的前后截图对比 |

## Applicable Harnesses

Spec Harness；Viewport Harness（会话横幅，浅色与深色，窄窗口）；AI Collaboration Harness。

## Test matrix

- 正常：正在执行、等待重试（估计在未来）。
- 边界：重试已过期 / 临近、没有估计、等待很久。
- 终态：completed / failed / cancelled（无横幅、无停止按钮）。
- 需要用户：授权、需要决定。
- 视口：真实浏览器对上述每种状态在浅色和深色模式下的截图，另加一张窄窗口截图。

## Implementation Notes

（不改变任何需求或验收标准。）

- 纯函数位于 `src/lib/statusBanner.ts`：`statusBannerView()` 生成唯一的横幅视图，
  `formatRetryHint()` 负责如实的时间提示，`containsInternalVocabulary()` 是内部词汇守卫。
- 横幅组件 `ActiveTurnProgress`（`src/components/MessageList.tsx`，无 plan 分支）只渲染
  上面这些纯函数的结果；原始的 `recoveryOwner` 与后端 `label` 不再直接上屏。
- 与 M37（顶部进度条与等待原因文案）并行：本任务只改动无 plan 的横幅分支与其文案，
  不触碰 `TurnProgress`（有 plan 分支）。
