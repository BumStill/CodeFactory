# 设置页内部结构统一审视与改进方案

## 问题清单（现状）

### P0｜双标题：页眉卡片 + 页内标题并存且文字重复

每个 tab 顶部都有统一页眉卡片（`SettingsPage.tsx` L693-696）：

```tsx
<div className="mb-5 max-w-3xl rounded-lg border border-border bg-surface-1 px-3 py-2.5">
  <div className="text-sm font-medium text-gray-200">{activeTab.label}</div>
  <p className="mt-1 text-xs leading-5 text-gray-500">{activeTab.description}</p>
</div>
```

但多个 tab 内容区**又有自己的第二个标题**，且与页眉文字重复：

| Tab | 页内标题位置 | 样式 | 重复内容 |
|---|---|---|---|
| 功能 | L702 `<h2 id="settings-capabilities-title">功能</h2>` | `text-base font-semibold text-gray-100` | "功能"×2 |
| 通用 | L823 `<h2>通用</h2>` | `text-xs ... uppercase tracking-wider` | "通用"×2 |
| 浏览器会话 | L1259 `<h2 id="browser-sessions-title">受管浏览器会话</h2>` | `text-base font-semibold text-gray-100` | "浏览器会话"×2 |
| 用量 | `UsageDashboardSection.tsx` L236 `<h2>` | `text-sm font-semibold text-gray-200` | 页内再起一层标题 |

### P1｜三种标题样式混用

同一页面内存在 3+ 种标题格式，无层级体系：

1. `text-sm font-medium text-gray-200`（页眉卡标题）
2. `text-base font-semibold text-gray-100`（capabilities / browser / about 页内 h2）
3. `text-xs font-semibold text-gray-400 uppercase tracking-wider`（general / endpoints / hooks / remotes / appearance 小节标题）
4. `text-sm font-semibold text-gray-200`（UsageDashboardSection h2）
5. `text-xs font-medium text-gray-300`（UsageDashboardSection h3）

### P1｜`uppercase tracking-wider` 对中文无效

`hooks`(L1395)、`remotes`(L1571)、`appearance`(L1767/1788/1814)、`general`(L823)、
`endpoints`(L775)、`UsageDashboardSection`(L102/419/431)、`about`(L2447) 等小节标题
全部使用 `uppercase tracking-wider`。标题均为中文，无大小写概念，这两条声明纯属
无效装饰（上一轮侧栏标题已去掉，内容区仍残留大量同类问题）。

### P2｜描述文字颜色/字号不一致

- 页眉描述：`text-xs text-gray-500`
- capabilities 页内描述：`text-xs text-gray-400`（L703）
- browser 页内描述：`text-xs text-gray-400`（L1262）
- 多处引导文字：`text-[11px] text-gray-500/600`

### P2｜内容宽度不一致

- `max-w-3xl`：功能 / 浏览器会话 / 用量
- `max-w-xl`：端点 / 通用 / 钩子 / 远程仓库
- `max-w-md`：关于

同属一个设置页体系，宽度不统一导致内容区左右留白参差。

### P2｜卡片圆角/边框/背景混用

- `rounded-lg` vs `rounded-xl`（如 L718 vs L1296/L1395 附近）
- `border-border` vs `border-border/70`
- `bg-surface-1` 为主，个别 `bg-surface-2`

## 业界实践基准（GitHub / VS Code / Linear / Notion / Apple 设置）

1. **每页一个标题**：页面顶部唯一 H1/H2（16–18px semibold）+ 一行副标题描述（12–13px
   灰色），页头下方直接是内容，**不存在第二个同级标题**。
2. **小节标题唯一格式**：12–13px semibold 灰色；英文产品用 11px uppercase，中文产品
   直接去掉 uppercase/tracking（无效装饰）。
3. **页头不包边框卡片**：标题 + 描述裸排（或浅背景无边框），避免"卡片式页眉 + 页内
   再一个标题"的双标题观感。
4. **层级=字号+字重+颜色**：页头 16px → 小节 12px → 内容 11–13px，间距统一。

## 统一规范（目标状态）

```
页头（唯一，每页一个）
  <div className="mb-5">
    <h2 className="text-base font-semibold text-gray-100">{title}</h2>
    <p className="mt-1 text-xs leading-5 text-gray-500">{description}</p>
  </div>

小节标题（唯一格式）
  <h3 className="mb-2 text-xs font-semibold text-gray-400">{label}</h3>

内容行/卡片
  统一 rounded-lg、border-border、bg-surface-1
```

### 具体改动清单

| 位置 | 改动 |
|---|---|
| L693-696 页眉卡 | 去边框卡样式 → 裸排页头（标题 16px semibold + 描述 12px gray-500，mb-5）；仍由 `activeTab` 驱动，所有 tab 自动统一 |
| capabilities L702-704 | 删除页内 `<h2>功能</h2>` 及其描述（并入页头：页头描述改为"管理跨会话能力；当前会话的模型、Git 和检查点仍留在工作区顶栏"），`aria-labelledby` 改指页头标题 id |
| browser L1259-1265 | 删除页内 h2"受管浏览器会话"；描述并入页头（"查看并管理 CodeFactory 创建的自动化浏览器与已连接的用户 Chrome…"） |
| general L823 | 删除页内 `<h2>通用</h2>`（与页头重复） |
| usage（UsageDashboardSection） | 内部 h2（L236）降级为小节标题或并入页头；内部 `uppercase tracking-wider`（L102/419/431）去掉，统一小节样式 |
| endpoints L775 | "API 端点"保留为小节标题，改为统一小节格式（去 uppercase/tracking） |
| hooks L1395 | 同上 |
| remotes L1571 | 同上 |
| appearance L1767/1788/1814 | "主题 / 字体 / 字号"三个小节统一 |
| about L2447 | "软件更新"小节统一 |
| 内容宽度 | 各 tab 内容统一 `max-w-3xl`（宽表单页可 `max-w-2xl`，二选一，保持一致） |
| 描述颜色 | 统一 `text-xs leading-5 text-gray-500` |
| 卡片 | 统一 `rounded-lg border-border`（个别 rounded-xl / border/70 收敛） |

### 不动的东西

- 侧栏导航（上一轮已改好的 11px semibold 分组 + 13px 菜单项）
- 选中态（accent 左边框 + bg-surface-2）
- 各 tab 的业务功能与布局骨架（卡片网格、表单、列表）
- 独立子组件（EndpointCard / AddEndpointModal / 钩子表单等）内部字段结构

## 验证

1. `pnpm test`（vitest）：现有 480 用例不回归；无测试断言标题样式类（已核）。
2. `pnpm dev` 实地走 9 个 tab：每页唯一标题、小节格式统一、深/浅色主题下层级清晰、
   无重复标题文字；侧栏选中态不回归。
3. 目检截图对比：页头 → 小节 → 内容三级层级一目了然。
