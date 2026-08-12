# CodeFactory 界面排版与间距规范

本规范约束整个产品界面的字号、字重、行高、图标尺寸、间距、点击目标和中文排版。
所有数值均在真实浏览器渲染中实测确认（Vite dev server + 真实组件挂载 + `getComputedStyle`），
不是从代码推断的。

---

## 一、根因：rem 基准被改写，整套比例尺静默缩水 12.5%

`src/styles/globals.css` 把 `html` 的 `font-size` 设成了用户字号（默认 14px）：

```css
html, body, #root { font-size: var(--font-size, 14px); }
```

Tailwind 的 **字号、间距、圆角、宽高全部是 rem**。`1rem` 等于 `html` 的字号，
于是整套设计系统被乘上 `14/16 = 0.875`。实测值：

| 类名 | 设计意图 | 实际渲染 |
|---|---|---|
| `text-xs` | 12px | **10.5px** |
| `text-sm` | 14px | **12.25px** |
| `text-base` | 16px | 14px |
| `gap-1` | 4px | **3.5px** |
| `gap-1.5` | 6px | **5.25px** |
| `gap-2` | 8px | **7px** |
| `px-3` | 12px | **10.5px** |
| `py-1` | 4px | **3.5px** |
| `rounded` | 4px | **3.5px** |
| `rounded-md` | 6px | **5.25px** |
| `w-3 h-3` | 12px | **10.5px** |

三个后果，按严重性排序：

1. **`text-xs` 实际是 10.5px，而它占全部字号用法的 85%**（453/530 处）。
   产品的默认文字尺寸是 10.5px。
2. **4px 网格不存在了**。所有间距落在 3.5 / 5.25 / 7 / 10.5px 这样的半像素上。
   在 1× 显示器（Windows 外接显示器的常见情形）上，浏览器对相邻元素的舍入方向不一致，
   同一排图标之间的实际间距会出现 3px 与 4px 混排。
3. **字号设置滑块（12–20px）会缩放整个布局**，而不只是文字。
   而 `text-[11px]`、`size={12}` 这些写死 px 的地方**不跟着缩放**。
   用户把字号调到 20px 时，间距放大 1.43 倍、图标纹丝不动、部分文字放大部分不放大——
   越调越坏。设置页写的是「字号」和「这是 Npx 的正文文字效果」，承诺的是文字，交付的是半套缩放。

**规范：`html` 的 `font-size` 必须保持浏览器默认（16px），不得被应用覆盖。**
用户字号设置改为驱动一组语义字号变量（见第二节），只影响文字，不影响布局。

---

## 二、字号比例尺

### 2.1 现状：比例尺是平的，而且被大量绕过

- 530 处字号用法中 453 处是 `text-xs`（85%）。`text-sm` 58、`text-base` 9、`text-lg` 6、`text-xl` 2、`text-2xl` 2。
  实际上只有一档在用。
- 同时存在 20+ 处任意值：`text-[10px]`、`text-[11px]`、`text-[12px]`、`text-[13px]`、`text-[15px]`、
  甚至 `text-[9px]`、`text-[7px]`、`text-[6px]`。

这两件事是同一件事：**因为 `text-xs` 实际只有 10.5px，开发者不断用 `text-[11px]`、`text-[13px]` 去救**。
`text-[11px] > text-xs(10.5px)`、`text-[13px] > text-sm(12.25px)`——任意值不是在做特例，是在补偿坏掉的基准。

### 2.2 后果：层级倒挂（已实测确认）

**欢迎页推荐卡片**（`src/components/WelcomeScreen.tsx:165` 与 `:171`）：

| 元素 | 类名 | 实测 |
|---|---|---|
| 卡片标题「了解当前代码库」 | `text-xs font-medium` | **10.5px** |
| 卡片正文「概览这个项目的用途…」 | `text-[11px]` | **11px** |

标题比它自己的正文还小。四张卡片全部如此。

**今日用量卡片**（`src/components/WelcomeUsageCard.tsx:65` 与 `:85`）：
标题「今日用量」10.5px，下方元数据 11px。同样倒挂。

**会话消息正文——主路径上最严重的一处**（`src/components/MessageList.tsx`）：

| 元素 | 类名 | 实测 |
|---|---|---|
| 段落正文 | 容器 `text-[15px]` | 15px |
| 列表项 | 继承 | 15px |
| **`h3` 三级标题** | `text-sm` (`:244`) | **12.25px** |
| **`h4` 四级标题** | `text-sm` (`:247`) | **12.25px** |
| 内联代码 | `text-[12px]` (`:179`) | 12px |
| **表格单元格** | `text-xs` (`:265`) | **10.5px** |
| **代码块 `<pre>`** | `text-xs` (`:123`) | **10.5px** |

一个 AI 编程产品，**正文 15px，代码块 10.5px**（正文的 70%），
**Markdown 标题比它统领的段落小 2.75px**。这是主路径上每天被看几百次的表面。

### 2.3 规范：语义化字号 token

停止在业务代码里写 `text-xs` / `text-[Npx]`。定义 8 档语义 token（随用户字号设置整体位移）：

| Token | 字号 / 行高 | 用途 |
|---|---|---|
| `text-caption` | 11 / 16 | 时间戳、计数、路径、次要元数据 |
| `text-label` | 12 / 18 | 控件标签、chip、徽章、按钮内文字 |
| `text-note` | 13 / 20 | 阅读流里的次要文字；**以及全部代码**（配 `font-mono`） |
| `text-body` | 14 / 22 | 界面正文、列表主文案、表单值、分组小标题 |
| `text-reading` | 15 / 24 | 会话消息正文（唯一的长文阅读流） |
| `text-title` | 16 / 24 | 卡片与面板标题 |
| `text-heading` | 18 / 26 | 页面标题 |
| `text-display` | 22 / 30 | 欢迎页主标题、大数字指标 |

代码不单列 token：等宽字体在同一 px 下视觉比无衬线大，`text-note` 的 13px 配在
15px 的 `text-reading` 正文旁刚好，一个档位同时满足「代码不缩水」和「代码不喧宾夺主」。

比例尺在 `tailwind.config.js` 里**替换**了 Tailwind 原生档位而不是扩展它，
所以 `text-xs` 这类类名根本不存在，写了不会生效。

**硬性规则**

1. 任何**标题**的字号必须 ≥ 它所统领内容的字号——包括分组小标题。
   一个 11px 的 eyebrow 标签压在 14px 的卡片标题上面同样算倒挂。
2. **代码最小 13px**（`text-note`），含 Markdown `<pre>`、内联代码、表格、diff、终端回显。
   代码是本产品的主要内容，不是脚注。
3. `text-caption` (11px) 是**下限**，不得出现 10px 及以下的文字。
   仅热力图格子里的状态记号（`×` / `·`）暂时豁免并登记在门禁的白名单里，
   最终应改为图形而非文字。
4. 业务代码不得出现 `text-[Npx]`。需要新档位时先改本规范。

### 2.4 字重

现状 `font-medium` 112 处、`font-semibold` 96 处、`font-normal` 8、`font-bold` 1，基本可用。规范收敛为三档：

- `font-normal` (400)：正文、元数据
- `font-medium` (500)：列表主文案、控件标签、卡片标题
- `font-semibold` (600)：页面标题、大数字、需要打断扫读的强调

不使用 `font-bold` (700)——在 Inter/PingFang 的小字号下 600 与 700 难以区分，只增加渲染重量。

### 2.5 行高

现状 `leading-5` 61 处、`leading-relaxed` 32、`leading-snug` 7、`leading-6` 4、`leading-4` 4、`leading-[10px]` 1，
且 rem 基准问题让 `leading-5` 实际是 17.5px。

规范：**行高跟随字号 token，不单独声明**。仅两个例外允许覆盖：
- 单行截断的 chip/徽章：`leading-none`
- 需要更松散的长文阅读：`text-reading` 已含 24px，不再加 `leading-*`

---

## 三、字体族

### 3.1 现状问题

```ts
FONT_FAMILIES = {
  inter:            "Inter, system-ui, sans-serif",
  system:           "system-ui, -apple-system, sans-serif",
  "jetbrains-mono": "'JetBrains Mono', Consolas, Menlo, monospace",
}
```

1. **Inter 没有被打包**。仓库里没有任何 `.woff2`，`index.html` 没有 `@font-face`，也没有 webfont 链接。
   用户机器上没装 Inter（Windows 上几乎必然没有）时，默认选项静默退化成 `system-ui`——
   与「System UI」选项完全一致。设置页的两个选项在多数机器上是同一个东西，
   连预览用的「Aa Bb Cc」也长得一样。
2. **字体栈里没有中文族**。这是一个 `lang="zh-CN"` 的产品，中文用什么字体完全交给 `system-ui` 兜底，
   跨平台不可控。Inter 与 PingFang SC 的 x-height 和基线不同，中英混排时基线会轻微错位。
3. **把 JetBrains Mono 作为整个界面字体是一个不该提供的选项**。它没有中文字形，
   选中后界面会变成「等宽拉丁 + 比例中文」的混合体。等宽字体的正确作用域是代码、路径、数字，不是界面。

### 3.2 规范

```
UI:   Inter, -apple-system, "PingFang SC", "Microsoft YaHei UI", system-ui, sans-serif
Mono: "JetBrains Mono", "SF Mono", Consolas, Menlo, monospace
```

- Inter 与 JetBrains Mono **随应用打包**：`globals.css` 引入
  `@fontsource-variable/inter/wght.css` 与 `@fontsource-variable/jetbrains-mono/wght.css`
  （可变字体，仅正体；斜体在本产品只出现在引用块，合成倾斜足够）。
  产物 12 个 woff2 合计 312KB，浏览器按 `unicode-range` 只取用得上的子集。
  桌面应用没有理由让排版依赖用户机器上碰巧装了什么。
- **字体引入写在 `globals.css` 而不是 `main.tsx`**：`src/acceptance/` 下的验收入口
  各有自己的 entry，不经过 `main.tsx`。放在 entry 里会让验收页渲染出一套没有
  自带字体的界面——第一版就是这样，实测 `document.fonts` 为空。
- 中文族显式写进栈里，不依赖 `system-ui` 兜底。
- 字体设置拆成两项：**界面字体**（Inter / 系统默认）与**等宽字体**
  （JetBrains Mono / 系统等宽）。等宽族不再出现在界面字体选项里。
  旧配置里 `font_family: "jetbrains-mono"` 的用户，界面字体回落到 Inter，
  等宽字体保留 JetBrains Mono——意图落在它说得通的那个轴上。
- 数字对齐场景（用量、成本、token 计数）用 `tabular-nums` 工具类，避免数字跳动。

---

## 四、图标

### 4.1 现状：18 种尺寸

`size={6,7,8,9,10,11,12,13,14,15,16,17,18,20,22,23,24,32}`，其中
`size={12}` 132 处、`size={11}` 98 处、`size={14}` 67 处、`size={10}` 36 处、`size={13}` 25 处、`size={15}` 11 处。

两个问题：

1. **奇数尺寸导致描边落在半设备像素上**。lucide 的默认 `strokeWidth` 是 2，viewBox 是 24。
   `size={11}` 的实际描边是 `11/24×2 = 0.917px`；`size={13}` 是 1.083px。
   在 1× 显示器上这些描边会被渲染成灰色模糊线，而同屏的 `size={12}`（1.0px）是清晰的黑线。
   同一排图标粗细不一致。
2. **同一语义的图标在不同页面尺寸不同**，因为尺寸是逐处手写的，没有约束。

### 4.2 规范

只允许四档，且全部为偶数：

| 尺寸 | 配对文字 | 用途 |
|---|---|---|
| 14 | `text-caption` / `text-label` | 行内图标、chip 内图标 |
| 16 | `text-body` / 按钮 | 按钮、工具栏、列表项 |
| 20 | `text-title` / `text-heading` | 面板标题、次级空状态 |
| 24 | `text-display` | 空状态主图、引导页 |

描边一致性用一条 CSS 规则解决，不做组件封装：

```css
.lucide { vector-effect: non-scaling-stroke; stroke-width: 1.5; }
```

`non-scaling-stroke` 让描边在**最终坐标系**里度量而不是 viewBox 单位，
于是任何尺寸下都恰好是 1.5px。**这里刻意没有引入 `<Icon>` 封装**——
封装要改约 350 个调用点、每个都得换 import，而它解决的问题（描边不一致）
一行 CSS 就够；尺寸档位由静态门禁约束即可。等到需要按尺寸切换图标线条粗细
或做图标名注册表时，再考虑封装。

**状态点不是图标。** 用 `<span className="h-1.5 w-1.5 rounded-full bg-current" />`
这类 CSS 图形，不要把图标缩到 6px 去当圆点——那既落在比例尺之外，又会得到一条
需要抗锯齿的发丝描边。

### 4.3 图标与文字的间距

现状 `gap-2` 148 处、`gap-3` 40、`gap-1` 39、`gap-1.5` 34、`gap-0.5` 4、`gap-2.5` 1——
同一种「图标 + 文字」的组合在不同文件里用了 5 种间距。

规范：

| 场景 | 间距 |
|---|---|
| 图标 + 紧邻文字（同一语义单元） | 6px |
| 同组控件之间 | 8px |
| 不同语义组之间 | 12px |
| 区块之间 | 16px / 24px |

图标必须与文字**光学基线对齐**而非盒模型居中：行内图标用 `flex items-center` 时，
若图标高度大于文字 cap-height，需要 `translate-y` 微调，不要靠加大行高解决。

---

## 五、间距与圆角

### 5.1 间距

`html` 恢复 16px 后，Tailwind 的 rem 间距自然回到 4px 网格。规范只用这几档：

```
4 (gap-1) · 6 (gap-1.5) · 8 (gap-2) · 12 (gap-3) · 16 (gap-4) · 24 (gap-6) · 32 (gap-8)
```

半档（`gap-1.5` = 6px、`px-2.5` = 10px）是**允许**的，Tailwind 原生就有 2.5/3.5。

初审曾判定要删掉 `*-2.5`，理由是它们「在半像素基准下试图找回视觉平衡」——这条**作废**。
当时 `px-2.5` 渲染成 8.75px 才是问题；rem 基准修好后它就是干净的 10px，
而本规范自己的控件密度表（`lg` = 16/10）本来就用着 10px。机械收敛只会是无收益的改动。

真正剩下的问题不是某个档位，而是**20 种 `px-* py-*` 组合缺乏密度规范**，见下一节。

### 5.2 内边距组合

现状 20 种 `px-* py-*` 组合。规范收敛为五种控件密度：

| 密度 | padding | 最小高度 | 用途 |
|---|---|---|---|
| `xs` | 4 / 2 | 20 | 徽章、chip（非交互） |
| `sm` | 8 / 4 | 28 | 密集工具栏按钮 |
| `md` | 12 / 6 | 32 | 默认按钮、输入框 |
| `lg` | 16 / 10 | 40 | 主操作、对话框按钮 |
| `panel` | 16 | — | 卡片、面板容器 |

### 5.3 圆角

原先 `rounded` 381 处、`rounded-lg` 141、`rounded-md` 25、`rounded-xl` 22。
`rounded`(4px) 与 `rounded-md`(6px) 只差 2px，肉眼无法区分，纯属噪音。

规范四档：

| Token | 半径 | 用途 |
|---|---|---|
| `rounded` | 4 | 徽章、内联代码、小控件 |
| `rounded-lg` | 8 | 按钮、输入框、列表项、卡片 |
| `rounded-xl` | 12 | 面板、对话框、大容器 |
| `rounded-2xl` | 16 | 消息气泡、输入框外壳 |

`rounded-full` 保留给纯圆形（头像、状态点、胶囊按钮），方向性变体
（`rounded-br-sm` 之类，用于气泡尖角）不在收敛范围内。

**`rounded-2xl` 保留是对初审结论的修正。** 初审把它和 `rounded-md` 一并划入删除，
理由不成立：12px 与 16px 差 4px 是能分辨的，而消息气泡和输入框外壳合理需要更大的圆角。
被删掉的只有 `rounded-md`（25 处并入 `rounded-lg`）。

---

## 五之二、会话列宽度

会话列（`--reading-column`，由 `globals.css` 定义，`WorkspacePage` 与 `MessageList`
两处引用）：

```css
--reading-column: min(calc(1200px * var(--font-scale, 1)), 64vw);
```

两条约束各修一个问题。

### 跟字号走：修的是「每行字数漂移」

原先是写死的 `max-w-[880px]`，而文字随 `--font-scale` 缩放，列不缩放。
结果每行中文字数在滑块上从 **68 字（12px 设置）荡到 41 字（20px 设置）**——
用户每动一次字号，行长就变一次。乘上同一个系数就钉住了。

### 跟窗口走：修的是「大屏留白过多」

| 全屏宽度 | 改前（880 写死） | 改后 |
|---|---|---|
| 1920 | 两侧各 384px，占可用宽 53% | 两侧各 224px，**72%** |
| 2560 | 两侧各 **704px**，占 38% | 两侧各 544px，**52%** |

### 为什么上限是 80 字而不是 50 字

初审时按「中文长文舒适区 30–45 字」判定 880px（58 字）已经偏宽，据此把列收窄到
750px。**这个判断是错的，已作废**——它衡量的是这个列里并不存在的内容。

真实内容是技术输出：环境变量名、仓库路径、完整 ssh URL、内联代码。
一张 Windows 2560 全屏截图上，`ssh://git@codehub-dg-g.huawei.com:2222/NIS_Pre-` 被迫
从中间断成两行，而右侧空着 700px。长文的度量标准套在这种内容上只会制造这种断行。

**教训**：定行长之前先看这个容器实际装的是什么。拿错了参照系，再精确的测量也是错的。

上限之外还想填满屏幕，正解是右侧辅助面板（≥1440px 自动 dock）——
2560 全屏开着面板时，会话列两侧只剩约 284px。

## 六、点击目标最小尺寸

实测到的过小目标：

| 位置 | 实测尺寸 | 说明 |
|---|---|---|
| `TokenUsageTrend.tsx:58,83` 趋势柱 | **4 × 11px** | 零用量/缺失日的柱子高度写死 4px，柱子本身就是 `<button>` |
| 会话侧边栏图标按钮 | **16.5 × 16.5px** | 两处 |
| `WelcomeUsageCard.tsx:70`「查看详情」 | **23.5px 高** | 差 0.5px 达标 |
| `TokenUsageHeatmap.tsx:159` 网格单元 | `min-h-2` = **7px** | 长按/精确点击困难 |

规范（对齐 WCAG 2.2 SC 2.5.8 Target Size Minimum）：

- **任何交互元素的可点击区域不小于 24 × 24px**，图标按钮建议 28 × 28px。
- 视觉尺寸可以更小，但必须把**命中区**撑到 24px。趋势柱的做法是：
  按钮撑满整列高度做命中区，柱子降级为内部 `<span>` 只负责视觉。
- 输入框要 `h-full` 撑满它的 label 外壳，否则用户看到的是 32px 控件、
  实际 `<input>` 只有 20px。
- 相邻交互元素的命中区之间至少留 4px，避免误触。

**Essential 例外必须登记。** 密集数据网格（4 周趋势图 28 列排在约 280px 内、
Token 消耗地图的日历格子）无法在不破坏可视化的前提下让每格达到 24px，
属于 SC 2.5.8 的 Essential 例外。它们保留等价可达路径：完整方向键导航 + 逐日 aria-label。

例外**按轴登记**在 `scripts/verify-hit-target-headless.mjs` 里，不是整体豁免——
趋势图只豁免宽度，高度仍然强制 24px。第一版写成整体豁免，等于把「柱子只有 4px 高」
这个本该被抓住的缺陷一起放过了。一个要写下来并说明理由的例外才会被复查；
一个被悄悄调低的阈值不会。

---

## 七、中文排版

### 7.1 `uppercase` + `tracking-*` 被套在中文上（41 处）

这是把英文的「SMALL CAPS 分组标签」惯用法直接搬到中文界面的结果。实测确认：

```
「可以试试」  letter-spacing: 0.275px  （中文字之间被拉开）
「Tokens」    text-transform: uppercase → 渲染为 TOKENS
「输入 token」 uppercase → 渲染为「输入 TOKEN」
「总 Token 数」uppercase → 渲染为「总 TOKEN 数」
```

- `uppercase` 对中文**无效**，只会把混排在中文里的英文单词意外大写，
  同一个词在不同页面渲染成 `Token` / `TOKEN` / `Tokens` / `TOKENS` 四种样子。
- `tracking-wider` (0.05em) 对中文是**有害的**：中文字本身是等宽方块，字间距由字形自带，
  再加 letter-spacing 会破坏词组的视觉黏合，读起来像被拆散。

### 7.2 规范

- **含中文的文本节点禁止使用 `uppercase` 和 `tracking-*`。**
- 分组小标题靠**字重 + 颜色 + 间距**建立层级，不靠字母间距。
  推荐：`text-caption font-medium text-gray-500`。
- `uppercase` 仅允许用于**纯拉丁的短标签**（如 `GET` / `POST` / `PR` / `CI`），且必须显式确认该字符串不含中文。
- 中英混排的空格：现状已经**一致**（`过去 4 周`、`个 Token`、`于 Docker`），继续保持——
  中文与拉丁字母/数字之间加一个半角空格，标点前后不加。

### 7.3 术语表

| 统一写法 | 禁止 | 现状 |
|---|---|---|
| `Token` | `token` / `TOKEN` / `Tokens` / `TOKENS` | 四种写法并存 |
| 会话 | 对话（作名词时） | 会话 221 处 / 对话 14 处 |
| 项目 | 工程 | 项目 106 处（`仓库` 指 git 仓库，是另一个概念，保留） |
| `…`（U+2026） | `...` | `…` 108 处 / `...` 5 处（「加载中...」「读取中...」「保存中...」） |

---

## 八、首屏主题闪白

`index.html` 上写的是 `class="dark"`，但 `tailwind.config.js` 的 `darkMode` 配置是
`["selector", '[data-theme="dark"]']`——**这个 class 不起任何作用**。

而所有主题变量只定义在 `[data-theme="dark"]` 和 `[data-theme="light"]` 下，`:root` 上没有兜底。
`data-theme` 属性由 `src/stores/settings.ts:70` 在 `get_settings` 这个异步 IPC 返回后才写上。
在此之前 `--surface-0` 未定义，`background-color: rgb(var(--surface-0))` 是无效声明，
**每次冷启动都会先画一帧无主题的白底**。

规范：

- 在 `:root` 上定义一套兜底变量（取深色值，与应用默认主题一致）。
- `index.html` 直接写 `data-theme="dark"`，并删除无效的 `class="dark"`。
- 更好的做法是在 `<head>` 内联一小段脚本，从 `localStorage` 读上次的主题并同步写入 `data-theme`，
  异步设置返回后再校正。

---

## 九、落地计划

### P0 — 修根因 ✅ 已完成

1. `globals.css` 移除 `html` 上的 `font-size` 覆盖，rem 基准回到平台默认 16px；
   正文字号由 `body` 上的 `calc(14px * var(--font-scale))` 承载。
2. 用户字号设置改为写 `--font-scale`（`font_size / 14`），只被字号 token 消费，
   不影响间距、圆角、图标盒。
3. `:root` 兜底主题变量 + `index.html` 写 `data-theme="dark"` 并删除失效的 `class="dark"`，
   消除首屏闪白。

**实测**：`gap-2 = 8px`、`rounded = 4px`、`p-4 = 16px`，4px 网格恢复；
把 `--font-scale` 调到 20/14 后 `text-body` 变 20px 而 `gap-2` 仍是 8px。

### P1 — 语义 token 与层级 ✅ 已完成

1. `tailwind.config.js` 用 8 档语义 token **替换**了 Tailwind 原生字号档位。
2. 迁移 1036 处：453 `text-xs`、251 `text-[11px]`、164 `text-[10px]`、61 `text-[13px]` 等。
   其中 9 类是等值改名（改基准后 `text-xs` 恰好等于 `text-label` 的 12px），
   只有 3 类刻意上移：`text-lg`→heading、`text-[10px]`/`text-[9px]`→caption。
3. 层级倒挂已修：
   - 会话消息 `h3`/`h4` 从 12.25px 提到 15px（不低于它统领的段落），靠字重分层；
   - 代码块、内联代码、表格从 10.5–12px 提到 13px；
   - 欢迎页卡片标题、今日用量标题提到 14px；
   - 分组小标题「可以试试」「继续之前的会话」提到 14px（原来比卡片标题还小）。

### P1.5 — 中文排版与术语 ✅ 已完成

1. 清理 41 处中文上的 `uppercase` / `tracking-*`（静态门禁 35 处 + 手工处理 6 处
   文案由表达式传入、门禁看不见的）。保留的 9 处 `uppercase` 全部确认是纯拉丁短标签
   （`Score`、`Files`、`Risks`、`Authorization phrase` 等）。
2. 术语统一：`Token` 单一写法（原先 Token/token/Tokens/TOKENS 四种并存）、
   省略号统一 `…`。

### P2 — 图标、命中区、圆角 ✅ 已完成

1. **图标收敛**：322 处收敛到 14/16/20/24 四档；描边用 `.lucide` 的
   `non-scaling-stroke` 统一到 1.5px（实测每个图标都是 1.5px，不再随尺寸漂移）；
   Git 状态点从 `Circle size={6}` 改为 CSS 圆点。
2. **命中区补齐到 24px**：趋势柱从 4 × 11px 改为整列高度（40px）命中区、
   侧边栏「更多操作」从 18px 改为 24 × 24、搜索框 `<input>` 从 20px 撑满到 30px。
   趋势图与热力图的宽度按 Essential 例外登记在案。
3. **圆角**：`rounded-md` 25 处并入 `rounded-lg`；`rounded-2xl` 经复核后保留。

### P3 — 字体与比例尺回调 ✅ 已完成

1. **字体打包**：Inter Variable 与 JetBrains Mono Variable 随应用发布，
   字体栈补中文族，界面字体与等宽字体拆成两项设置。
2. **热力图状态记号改为图形**：7px 的 `×`、6px 的 `·`、7px 的 `!` 换成 CSS 图形，
   字号门禁的亚 11px 豁免名单**已清空**。
3. **比例尺顶部回调**：`heading` 20→18、`display` 24→22，
   并把迁移时放大过头的几处拉回（卡片标题 14→13、卡片正文 11→12、
   分组标题 14→13）。原因见下。
4. 间距档位收敛**作废**——理由随 rem 基准修复一起消失了，见 5.1。

#### 为什么要回调

第一轮迁移把「等值改名」和「层级修复」混在一起做，结果层级修复用力过猛：

| 位置 | 原渲染 | 第一轮 | 回调后 |
|---|---|---|---|
| `text-lg`（h1 级） | 15.75px | 20px (+27%) | 18px |
| 欢迎页卡片标题 | 10.5px | 14px (+33%) | 13px |
| 欢迎页卡片正文 | 11px | 11px | 12px |
| 分组标题 | 11px | 14px (+27%) | 13px |

叠加 rem 修复本身的全局 +14.3%，观感就是「很多地方偏大、大小差距太大」。
**修倒挂只需要标题 ≥ 正文，不需要标题远大于正文**——把正文抬一档比把标题抬三档好。

回调后欢迎页实际渲染为 11 / 12 / 13 / 18 / 22，主体收在 11–13；
全产品 1042 处 token 用法里 891 处（85%）落在 11–12px。

---

## 十、门禁

规范不进 CI 就会在三个 PR 内失效。

### 已落地

按仓库既有的 `src/styles/lightModeAudit.test.ts` 模式，随 `pnpm test` 一起跑：

| 测试 | 断言 |
|---|---|
| `remBaselineAudit.test.ts` | `globals.css` 不在 `html` 上设 `font-size`；字号走 `--font-scale` |
| `typographyScaleAudit.test.ts` | 不出现 Tailwind 原生字号档位与 `text-[Npx]`，token 在 config 里有定义 |
| `cjkTypographyAudit.test.ts` | 元素内出现中文时，className 不得含 `uppercase` 或 `tracking-` |
| `themeBootstrapAudit.test.ts` | `index.html` 首帧带 `data-theme`、无失效 `class="dark"`；`:root` 有变量兜底 |
| `iconScaleAudit.test.ts` | `size={N}` 只允许 14/16/20/24；`globals.css` 有统一描边规则 |
| `radiusScaleAudit.test.ts` | 不出现被淘汰的 `rounded-md` / `rounded-sm` / `rounded-3xl` |
| `MessageList.theme.test.tsx` | 会话列用 `max-w-[var(--reading-column)]`，不是写死 px |
| `fontStackAudit.test.ts` | 字体栈含中文族；被提供的选项确实随应用打包；界面字体不是等宽族 |

真实渲染值只有真浏览器能给，补了两个 headless 脚本（都需先起 dev server）：

- `pnpm test:typography:headless`——根字号、8 档 token 实际 px、4px 网格、
  `--font-scale` 只缩放文字不缩放布局、页面上无 11px 以下文字、无层级倒挂、
  会话列每行字数在各字号下恒定（视口取 3000px，避开 `vw` 项）。
- `pnpm test:hit-target:headless`——遍历 `button/a/[role=button]/input/select`，
  命中区宽高均 ≥ 24px，Essential 例外按轴登记。

**两个必须记住的坑**：

- **不要用 `[^>]*` 匹配 JSX 开标签**。属性里的箭头函数 `onClick={() => …}` 自带一个 `>`，
  正则会提前收尾，所有带箭头函数的元素被静默跳过——第一版 CJK 守卫就是这样漏掉
  `CheckpointsPanel` 那个「无文件差异」按钮的。要跟踪花括号深度和引号状态。
- **也不要写「开标签 + 内容 + 闭标签」的单条正则**：两段可变长度夹一个字面量会指数级回溯，
  实测让一个纯静态扫描的测试文件跑了 179 秒。

### 待补

| 测试 | 断言 |
|---|---|
| `paddingDensityAudit.test.ts` | 控件 padding 落在 5 种密度预设上（当前 20 种组合，约 350 个按钮，需逐一判断） |

### 实地验证的边界

**AGENTS.md 的硬规则同样适用**：本规范涉及的全部是 UX 行为变更，`pnpm test` 通过不构成完成证据。

headless 验证跑在 Chromium 上，而 Tauri 在 macOS 上用 WKWebView。
rem 解析、`calc()`、字号与间距的取值在两者之间是同一套 CSS 计算，
所以本规范关心的量可以互认；但字体回退、字形度量、亚像素渲染不能互认，
那部分仍需在 `pnpm tauri dev` 的真实应用里看。

**并行 worktree 的坑**：`vite.config.ts` 里 `port: 1420` + `strictPort: true`，
多个 worktree 同时开发时，后起的 dev server 起不来，而验证脚本会静默连上
**另一个 checkout** 的服务，把别人的 CSS 当成自己的来断言。
`verify-typography-headless.mjs` 因此支持 `CODEFACTORY_VITE_URL` 指定地址——
跑之前先确认连的是自己那份。

---

## 附：证据来源

- 真实渲染测量：Vite dev server + `sidebar-expansion-acceptance.html`、
  `usage-acceptance.html`、`streaming-markdown-acceptance.html` 挂载真实组件，
  浏览器内 `getComputedStyle` 取值，viewport 1440×900，dpr 2。
- 用法频次：对 `src/**/*.tsx` 的静态统计。
- 层级倒挂的三处，均在浏览器中用组件真实的 className 组合复现并测得。
- 改后回归：`pnpm test` 578 passed、`pnpm exec tsc --noEmit` 通过、
  `pnpm test:typography:headless` 六组断言全过、深浅两个主题各截图确认。
