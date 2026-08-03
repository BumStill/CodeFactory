# 统一聊天输入区与会话信息区的底色

## 问题（用户原话）

> 聊天输入框区域的底色为啥跟上面会话信息框的底色不一样？应该保持一样更好看一些。

## 根因

会话窗口（中间列）的背景层级有两层：

| 区域 | 文件:行 | Tailwind 类 | 深色主题实际色值 |
|---|---|---|---|
| 会话信息区（MessageList 所在） | `src/pages/Workspace/WorkspacePage.tsx` `<main>` (L538) | `bg-surface-2` | `rgb(17 24 36)` |
| 输入框外壳（composer shell） | `src/pages/Workspace/WorkspacePage.tsx` L556 | `bg-surface-1` | `rgb(12 17 26)` |
| 输入卡片本体（含 DraftScopeBar / ContextUsageBar / MessageInput） | 同上 L557 | `bg-surface-2` + border + shadow | `rgb(17 24 36)` |

外壳 `bg-surface-1` 的颜色从输入卡片的上下内边距（`px-3 pb-3 pt-2`）和左右空隙透出，
形成一条与上面 `bg-surface-2` 明显不同的色带。输入卡片本身是 `bg-surface-2`，与信息区同色，
但被更暗/更浅的外壳底色包围，看起来"输入区底色不一样"。

浅色主题同理：外壳 `rgb(247 249 252)` vs 信息区纯白 `#fff`。

## 修复方案（一行）

`src/pages/Workspace/WorkspacePage.tsx` L556：

```diff
- <div data-testid="workspace-composer-shell" className="shrink-0 bg-surface-1 px-3 pb-3 pt-2">
+ <div data-testid="workspace-composer-shell" className="shrink-0 bg-surface-2 px-3 pb-3 pt-2">
```

改动后中间列上下同色，输入卡片仍靠 `border border-border/80 shadow-lg rounded-2xl`
保持可辨识的卡片层次，不丢失视觉边界。

## 影响面核对（已查）

- 无测试断言外壳背景色：`WorkspacePage.usageBarPlacement.test.tsx` 只断言
  `workspace-composer-shell` 包含 `message-input`，不受影响。
- 外壳内子组件（QueueBadge / DraftScopeBar / ContextUsageBar / MessageInput）
  均以卡片自身 `bg-surface-2` 为底设计，不受外壳改色影响。
- 会话工具栏（L345 `bg-surface-1/95`）与侧栏（L512）不在此列，保持原样。

## 验证步骤

1. `pnpm test`（vitest）全量跑一遍，重点看 `src/pages/Workspace/*.test.tsx`。
2. `pnpm tauri dev`（或 `pnpm dev`）实地查看：深色/浅色主题下输入区与信息区底色一致，
   输入卡片边框阴影正常；流式输出 + 输入多行时背景不变。

## 结论

根因是输入框外壳用了比信息区低一级的 `bg-surface-1`，一行类名即可统一为 `bg-surface-2`。
