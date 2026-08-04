# 方案：会话列表的时间与排序应反映「最近活动」

> 状态: **待批准**（提案，不是已批准规格）
>
> 提出日期: 2026-08-03
>
> 触发：用户观察到侧边栏会话列表的时间"貌似是会话创建时间"，并质疑文件夹内是否应按最近消息排序。

---

## 一句话结论

**用户的直觉对了，但归因差一层。** UI 显示和排序用的**已经是** `updated_at`，不是
`created_at`；真正的问题是 **`updated_at` 在收到消息时根本不被更新**，所以它退化成了
创建时间。修 UI 没用，要修的是那个字段的维护。

---

## 证据

### 显示与排序都已经用 updated_at

```
src/components/SessionSidebar.tsx:348
  <span className="shrink-0">{formatRelativeTime(session.updated_at)}</span>

src-tauri/src/commands/session.rs:197
  ORDER BY updated_at DESC LIMIT 200
```

前端不再二次排序，直接用后端顺序。

### 但 updated_at 只在少数几处被写

全仓 `UPDATE sessions ... updated_at` 只出现在：

| 位置 | 触发时机 |
| --- | --- |
| `commands/session.rs:333` | 切换模型 |
| `commands/session.rs:430` | 重命名会话 |
| `commands/chat.rs:499` | 自动生成标题 |

而**插入消息的所有路径都不碰它**：`agent/persistence.rs`（210/247/314）、
`agent/subagent.rs:159`、`trajectory.rs:263` 附近的 `UPDATE sessions` 出现次数为 **0**。

### 因此

自动标题是会话开头就生成的，之后除非改模型或手动重命名，`updated_at` 再不变化。
一个聊了两小时的会话，时间戳仍停在第一条消息那一刻——**看起来就是创建时间**，
排序也因此把活跃会话压在下面。

---

## 方案

### 决策 1：语义确定为「最近活动时间」

列表要回答的问题是「我最近在哪儿干活」，不是「这个会话哪天建的」。所以：

- **显示**：最近一条消息的时间。
- **排序**：同一字段倒序，文件夹内同理。

### 决策 2：修字段维护，而不是改查询

有两条路：

| 方案 | 做法 | 取舍 |
| --- | --- | --- |
| **A（推荐）** | 在消息落库的同一事务里 `UPDATE sessions SET updated_at = ?` | 列表查询不变、无需 JOIN、200 条列表零额外成本；写入侧多一次极轻的 UPDATE |
| B | 列表查询改为 `LEFT JOIN (SELECT session_id, MAX(created_at) ...) ` | 不改写入路径，但每次列表都要聚合全表消息；随消息量增长而变慢，且 `ORDER BY` 无法走索引 |

选 **A**。理由：列表是高频读、消息写入是低频且已经在事务里，把成本放在写侧更划算；
B 还会让「文件夹内排序」这类后续需求继续背着聚合成本。

### 决策 3：哪些写入算「活动」

只有**真实对话推进**才应刷新时间，否则列表会被后台噪音搅乱：

- ✅ 用户消息、助手消息（`agent/persistence.rs` 的三处插入）
- ✅ 子代理产生的可见消息（`agent/subagent.rs`）
- ❌ 纯内部记录（如 trajectory 落盘）——若它不产生用户可见消息，不刷新
- ❌ 工具调用明细的单独写入——已由所属助手消息代表

**边界判断**：宁可少刷新也不要多刷新。一个因后台作业被顶到顶部、点进去却没有新内容
的会话，比时间戳略微保守更让人困惑。

### 决策 4：迁移既有数据

现存会话的 `updated_at` 全是错的（≈创建时间）。上线时做一次性回填：

```sql
UPDATE sessions
SET updated_at = (SELECT MAX(created_at) FROM messages WHERE messages.session_id = sessions.id)
WHERE EXISTS (SELECT 1 FROM messages WHERE messages.session_id = sessions.id);
```

无消息的会话保持原值。**不回填的话，老会话会永远排在新会话下面**，用户看到的仍是坏
的顺序，等于没修。

---

## 需要确认的边界

1. **文件夹内排序**：截图显示 `CodeFactory` 文件夹下有 5 个会话。当前是否已按
   `updated_at` 排？后端 `ORDER BY updated_at DESC` 是全局的，需确认前端分组时
   有没有保序（JS 的 `Array.prototype.sort` 稳定，但 `groupBy` 之后的插入顺序要确认）。
   若分组时打乱了，需在分组后按同一字段重排。
2. **时间显示的粒度**：`formatRelativeTime` 现有的「7 分钟前 / 3 天前 / 2026/7/27」
   分级看起来合理，本方案不动。

---

## 验证方式

- 单元测试：插入用户消息与助手消息后，`sessions.updated_at` 必须前移；插入不产生
  用户可见消息的记录时不得前移。
- 迁移测试：构造一个 `updated_at` 停在创建时刻、但有较新消息的会话，跑迁移后时间
  必须等于最新消息时间；无消息的会话保持不变。
- 排序测试：三个会话按消息时间交错更新后，列表顺序必须与最近消息顺序一致，且在
  文件夹分组后仍然保持。
- 实地：真实 App 里对一个旧会话发一条消息，确认它跳到列表顶部且时间变为「刚刚」。
