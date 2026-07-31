# 修正方案：token/context 条移到聊天输入框上方

## 背景

用户反馈：新版把 token 消耗 + context 显示放在聊天区**最底部**，占用明显空间。
第一轮修复（已提交 `cbf000c`）误把方向做成了「折叠成胶囊」；用户澄清真实意图是：

> 「把最下边的 token 消耗和上下文条**换到聊天输入窗上面**才更好看」

即：不做折叠交互，把这条条**移到 MessageInput 上方**，作为 composer 输入区的头部行，
消息列表不再被它占底。

## 当前实现

`src/pages/Workspace/WorkspacePage.tsx`（composer shell 内，约 473-508 行）：

```
<div data-testid="workspace-composer-shell">
  <div rounded-2xl card>
    {queue.length > 0 && <QueueBadge />}
    {activeDraft && <DraftScopeBar />}
    <MessageInput ... />        ← 输入框
    <ContextUsageBar ... />     ← 现在在输入框【下方】（消息列表正下方）
  </div>
</div>
```

`src/components/ContextUsageBar.tsx`：包含折叠状态（`expanded`、胶囊按钮、`ChevronDown` 收起），
头部注释也是「collapses to a slim pill」——全部需要撤销。

## 目标方案

1. **`ContextUsageBar.tsx`**：撤销折叠交互
   - 删除 `expanded` state、胶囊按钮、`ChevronDown` 收起按钮及其 import；
   - 恢复**单行完整显示**：左侧 会话/今日 tokens + 费用来源，右侧 上下文 label + 进度条 + 百分比；
   - 压缩 toast 保留；
   - 根容器 `border-t` 改为 `border-b`（它现在是输入框上方的 header 条）；
   - 加 `data-testid="context-usage-bar"` 便于顺序断言。
2. **`WorkspacePage.tsx`**：把 `<ContextUsageBar />` 移到 `<MessageInput />` **之前**，
   紧贴输入框上方、QueueBadge/DraftScopeBar 之下。
3. **测试**：
   - 删除/改写 `src/components/ContextUsageBar.fold.test.tsx`（折叠契约不再成立）；
   - 新增顺序契约测试：在 composer shell 内 `context-usage-bar` 出现在 `message-input` 之前
     （参照 `TaskCreator.test.tsx` 的 mock 模式，把 ContextUsageBar mock 成带 testid 的元素）；
   - 保留 `ContextUsageBar.test.ts`（formatContextTokens）与 presentation 相关既有测试。
4. **验证**：`npx vitest run`、`npx tsc --noEmit`、`npx vite build`。
5. **交付**：修正提交（覆盖 `cbf000c` 的方向性错误），切功能分支，`deliver_changes through_release`。

## 备注

- 之前的 `cbf000c fix(ui): collapse token/context footer...` 提交的是错误方向，
  交付链尚未 push；修正后应让最终提交反映「moved above the input box」语义。
- 工作区仍有无关的 browser pane WIP 已 stash，交付时不携带。
