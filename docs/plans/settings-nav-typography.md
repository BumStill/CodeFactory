# 设置页侧边栏菜单层级排版改进方案

## 问题

设置页改为纵向分组导航后，观感异常：**分组标题（目录）是小字，菜单项（子项）是大字**，
用户觉得"目录小、内容大"很怪。

当前实现（`src/pages/Settings/SettingsPage.tsx` L656-683）：

```tsx
{/* 分组标题：10px、medium、大写、加宽字距、灰色 */}
<div className="px-4 pb-1.5 text-[10px] font-medium uppercase tracking-wider text-gray-600">
{/* 菜单项：12px */}
<button className="... px-4 py-1.5 text-left text-xs ...">  {/* 未选中 text-gray-500，选中 text-gray-100 */}
```

## 业界基准（GitHub / VS Code / Linear / Notion / Figma 设置侧栏）

这些产品的侧边栏分组导航共识是：

| 元素 | 字号 | 字重 | 颜色 | 角色 |
|---|---|---|---|---|
| 分组标题 | 11–12px | 500–600 | 灰（弱于内容） | **弱化标签**：告诉用户"下面是哪一类"，刻意不抢眼 |
| 菜单项 | 13–14px | 400（常规） | 高于标题对比度 | **主要内容**：可点击项，需要被看见 |

即：**"分组标题小字、菜单项大字"本身就是业界主流做法**（GitHub 设置左侧
"Code and automation / Integrations" 等分组标题就是小号大写灰字）。

## 为什么当前实现观感怪

1. **10px 偏小**：业界下限约 11px，中文渲染下 10px 接近不可读，像"水印/脚注"。
2. **`uppercase` + `tracking-wider` 对中文无效**：分组标题是中文（工作流/模型与连接/应用），
   中文没有大小写，这两个声明纯属无效装饰，白白把标题"压小"。
3. **层级差只有 2px**（10 vs 12）：拉不开"标签 vs 内容"的对比，看起来像"目录被缩小"，
   而不是"目录是标签"。
4. **分组之间仅靠 `mb-4` 留白**：三组视觉上平铺，缺少分组感。

## 推荐方案 A（对齐 GitHub/VS Code 主流，最小改动）

`src/pages/Settings/SettingsPage.tsx` L658 分组标题：

```diff
- <div className="px-4 pb-1.5 text-[10px] font-medium uppercase tracking-wider text-gray-600">
+ <div className="px-4 pb-1.5 text-[11px] font-semibold text-gray-500">
```

L672 菜单项：

```diff
- className={`border-l-2 px-4 py-1.5 text-left text-xs transition-colors ${
+ className={`border-l-2 px-4 py-1.5 text-left text-[13px] transition-colors ${
       tab === t.id
         ? "border-accent bg-surface-2 text-gray-100"
-       : "border-transparent text-gray-500 hover:bg-surface-2 hover:text-gray-300"
+       : "border-transparent text-gray-400 hover:bg-surface-2 hover:text-gray-200"
```

要点：
- 标题 10→11px + semibold，去掉对中文无效的 uppercase/tracking-wider；
- 项 12→13px，未选中色 gray-500→gray-400 提高一档对比度；
- 选中态（accent 左边框 + bg-surface-2）保持不变；
- 分组间距 `mb-4` 可加到 `mb-5`，或第一组外每组加细分隔线 `border-t border-border/60 pt-3` 增强分组感（可选）。

效果：标题小而有存在感（"标签"），项大而清晰（"内容"），层级自然成立。

## 备选方案 B（Apple 系统设置风格，不推荐）

若希望"目录更像主标题"：标题 13px semibold text-gray-300、项 13px text-gray-400，
同字号靠字重/颜色区分。这牺牲可点击项的突出度，且偏离 Web 产品主流习惯。

## 验证

1. `pnpm test`（vitest）——无测试断言现有样式类，应全绿。
2. `pnpm dev` 实地查看：深色/浅色主题下分组标题与菜单项层级清晰，选中态不回归。
