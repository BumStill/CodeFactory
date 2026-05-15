# Windows AI 编程客户端项目方案（CodeFactory）

> 一个对标 Claude Code 的 Windows 桌面客户端，模型层接 OpenRouter，可在数百个 LLM 之间自由切换。
>
> **文档版本**：v1.1
> **创建日期**：2026-05-13
> **最后更新**：2026-05-13
> **目标平台**：Windows 10/11 (x64, arm64 后续)

---

## 0. 已锁定决策（Quick Reference）

| # | 决策项 | 结论 | 相关 ADR |
|---|---|---|---|
| 1 | 项目名 | CodeFactory | §14.2 |
| 2 | 桌面框架 | Tauri 2.0 | ADR-001 |
| 3 | 模型层 | OpenRouter（OpenAI 兼容协议） | ADR-002 |
| 4 | 自定义 base_url | ✅ MVP 支持，多 endpoint | ADR-006 |
| 5 | 嵌入式终端 | ✅ MVP 包含（xterm + ConPTY） | ADR-007 |
| 6 | License | Apache-2.0 | ADR-008 |
| 7 | 存储 | SQLite | ADR-003 |
| 8 | 工具协议 | OpenAI tool_calls | ADR-004 |
| 9 | API Key 存储 | Windows Credential Manager (keyring) | ADR-005 |

总工期估算：Phase 0-6 约 **11-12 周** 到 v1.0（含 Phase 3 终端扩到 3 周）。MVP（Phase 0-2）约 **5 周**。

---

## 1. 项目目标与定位

### 1.1 一句话定义
做一个跑在 Windows 上、交互体验对齐 Claude Code、后端通过 OpenRouter 调用任意大模型的 AI 编程 Agent 客户端。

### 1.2 核心价值
- **模型自由**：不绑定单一厂商。同一会话可在 Claude Opus、GPT-5、Gemini 2.5、DeepSeek、Qwen 等模型间切换。
- **本地 Agent**：不只是聊天，能在本机读写文件、执行命令、跑测试、调试代码。
- **可控可审计**：所有破坏性操作走权限确认，所有工具调用有日志。
- **Windows 原生体验**：托盘、文件关联、PowerShell 集成、Windows Terminal 风格。

### 1.3 不做什么（明确边界）
- 不做云端协作 / 多用户
- 不做模型训练 / 微调
- 不做代替 IDE 的完整编辑器（代码展示用轻量 Shiki，不引入 Monaco 这种重型内核）
- MVP 阶段不实现 MCP / 插件市场

### 1.4 目标用户
- 已经在用 Cursor / Claude Code / Cline 但想自由换模型的开发者
- 国内用户：希望走 OpenRouter 中转，避免直连 Anthropic 的网络问题
- 想在同一界面对比不同模型解决同一问题的工程师

---

## 2. 技术选型

### 2.1 桌面框架对比

| 方案 | 包体 | 内存 | 性能 | 开发效率 | 安全模型 | 推荐度 |
|---|---|---|---|---|---|---|
| **Tauri 2.0** | ~10MB | 低 | 高 | 中（需 Rust） | 强（capability 模型） | ★★★★★ |
| Electron | ~100MB | 高 | 中 | 高 | 弱（Node 主进程权限大） | ★★★ |
| WinUI 3 / WPF | ~30MB | 中 | 高 | 低（仅 Win） | 中 | ★★ |
| .NET MAUI | ~50MB | 中 | 中 | 中 | 中 | ★★ |
| Flutter Desktop | ~40MB | 中 | 高 | 中 | 中 | ★★★ |

**决策：Tauri 2.0**

原因：
1. Agent 客户端必须做文件/进程操作，Tauri 的 capability 模型能精确控制每个前端调用能访问哪些路径、能跑哪些命令——比 Electron 的"全开"模式安全得多。
2. 安装包小，国内用户下载友好。
3. Rust 后端跑文件 IO、进程管理、SSE 流式解析比 Node.js 快且稳。
4. 前端依然是 Web 技术栈（React + TS），招人和调试不输 Electron。

### 2.2 完整技术栈

```
┌─────────────────────────────────────────────┐
│  前端 (WebView2)                            │
│  React 18 + TypeScript + Vite               │
│  Tailwind CSS + shadcn/ui (按需引入)        │
│  Zustand (状态管理)                          │
│  Shiki (代码高亮) + diff2html (diff 渲染)   │
│  xterm.js (终端 UI)                          │
└─────────────┬───────────────────────────────┘
              │ Tauri IPC (invoke / event)
┌─────────────┴───────────────────────────────┐
│  Tauri 后端 (Rust)                           │
│  ├─ 模型层: openrouter-client (自研)         │
│  │   reqwest + eventsource-stream (SSE)     │
│  ├─ 工具层: tools/ (read/write/edit/bash...)│
│  ├─ 进程: portable-pty (跨平台 PTY)          │
│  ├─ 存储: tauri-plugin-sql (SQLite)         │
│  └─ 配置: serde + toml/json                  │
└─────────────────────────────────────────────┘
              │ HTTPS (SSE)
┌─────────────┴───────────────────────────────┐
│  OpenRouter API                              │
│  /api/v1/chat/completions                    │
│  /api/v1/models                              │
│  /api/v1/credits                             │
└─────────────────────────────────────────────┘
```

### 2.3 关键依赖清单

**Rust crates**
- `tauri = "2"` 桌面框架
- `tokio` 异步运行时
- `reqwest = { features = ["stream", "json"] }` HTTP 客户端
- `eventsource-stream` 解析 SSE
- `serde / serde_json` 序列化
- `portable-pty` 跨平台 PTY（嵌入终端用）
- `notify` 文件系统监听
- `globset` glob 模式匹配
- `regex` 正则
- `keyring` 跨平台凭据存储（API Key 不落明文）
- `tauri-plugin-sql` SQLite 集成
- `tauri-plugin-shell` 受控的命令执行
- `tauri-plugin-dialog` 系统对话框

**前端 npm 包**
- `react / react-dom`
- `@tauri-apps/api` Tauri 前端 SDK
- `tailwindcss` + 按需引入的 `@radix-ui/*` 组件（shadcn 基础，不整包安装）
- `shiki` 代码高亮（~300KB，替代 Monaco 的 ~10MB）
- `diff2html` + `diff` 轻量 diff 渲染
- `xterm` + `xterm-addon-fit` + `xterm-addon-webgl`
- `zustand`
- `react-markdown` + `rehype-shiki` 渲染模型回复中的代码块
- `lucide-react` 图标（tree-shaking 友好）
- `cmdk` 命令面板（Slash 命令用）

---

## 3. 系统架构

### 3.1 模块划分

```
codefactory/
├─ src-tauri/                  # Rust 后端
│  ├─ src/
│  │  ├─ main.rs               # Tauri 入口
│  │  ├─ commands/             # 暴露给前端的 #[tauri::command]
│  │  │  ├─ session.rs         # 会话 CRUD
│  │  │  ├─ chat.rs            # 触发对话 / 流式输出
│  │  │  ├─ tools.rs           # 工具调用入口
│  │  │  ├─ settings.rs        # 配置读写
│  │  │  └─ models.rs          # 拉取 OpenRouter 模型列表
│  │  ├─ openrouter/
│  │  │  ├─ client.rs          # HTTP + SSE 客户端
│  │  │  ├─ types.rs           # ChatRequest / ToolCall / Delta
│  │  │  └─ stream.rs          # 增量解析 + 工具调用聚合
│  │  ├─ agent/
│  │  │  ├─ loop.rs            # Agent 主循环（思考-工具-观察）
│  │  │  ├─ context.rs         # 消息历史、token 估算、压缩
│  │  │  └─ permissions.rs     # 权限决策
│  │  ├─ tools/
│  │  │  ├─ read.rs
│  │  │  ├─ write.rs
│  │  │  ├─ edit.rs
│  │  │  ├─ bash.rs            # 走 PowerShell / cmd
│  │  │  ├─ glob.rs
│  │  │  ├─ grep.rs            # 内置 ripgrep（bundled）
│  │  │  ├─ web_fetch.rs
│  │  │  └─ registry.rs        # 工具注册表 + JSON Schema
│  │  ├─ storage/
│  │  │  ├─ db.rs              # SQLite migrations
│  │  │  └─ models.rs          # Session / Message / ToolCall
│  │  ├─ config/
│  │  │  ├─ settings.rs        # 用户配置 / 项目配置
│  │  │  └─ memory.rs          # CLAUDE.md 等价物
│  │  └─ secrets.rs            # keyring 包装
│  ├─ tauri.conf.json
│  └─ Cargo.toml
│
├─ src/                        # React 前端
│  ├─ main.tsx
│  ├─ App.tsx
│  ├─ pages/
│  │  ├─ Chat/                 # 主聊天界面
│  │  ├─ Settings/             # 设置中心
│  │  ├─ Sessions/             # 会话历史
│  │  └─ Models/               # 模型选择
│  ├─ components/
│  │  ├─ MessageList.tsx
│  │  ├─ MessageInput.tsx
│  │  ├─ ToolCallCard.tsx      # 工具调用气泡（可展开看输入输出）
│  │  ├─ DiffViewer.tsx        # Monaco diff
│  │  ├─ PermissionDialog.tsx  # 工具确认弹窗
│  │  ├─ Terminal.tsx          # xterm 包装
│  │  ├─ ModelPicker.tsx
│  │  └─ SlashCommandMenu.tsx
│  ├─ stores/
│  │  ├─ chat.ts
│  │  ├─ settings.ts
│  │  └─ session.ts
│  ├─ lib/
│  │  ├─ tauri.ts              # invoke / event 包装
│  │  ├─ markdown.tsx
│  │  └─ tokens.ts
│  └─ styles/
│
├─ docs/
│  ├─ PROJECT_PLAN.md          # 本文件
│  ├─ ARCHITECTURE.md
│  ├─ TOOLS.md
│  └─ ROADMAP.md
└─ package.json
```

### 3.2 Agent 主循环

核心是一个"模型 → 工具 → 观察 → 模型"的循环，对齐 Claude Code 的行为：

```
┌──────────────────────────────────────────────┐
│ 1. 用户输入消息 / 项目状态 / 系统 Prompt       │
└──────────────┬───────────────────────────────┘
               ▼
┌──────────────────────────────────────────────┐
│ 2. 调用 OpenRouter (stream=true, tools=[...])│
│    边接收边把文本 token 流推给前端              │
└──────────────┬───────────────────────────────┘
               ▼
       ┌───────┴────────┐
       │ 模型停止原因？  │
       └───┬────────┬───┘
           │        │
       stop│        │tool_calls
           ▼        ▼
      ┌───────┐  ┌───────────────────────────┐
      │ 结束  │  │ 3. 解析 tool_calls         │
      └───────┘  │    对每个工具：             │
                 │    - 检查权限策略           │
                 │    - 需确认 → 弹窗等用户    │
                 │    - 执行工具               │
                 │    - 把结果 append 进历史   │
                 └──────┬────────────────────┘
                        │
                        └──→ 回到第 2 步
```

实现要点：
- 用 `tokio::mpsc` channel 把 SSE 增量事件推到 Tauri event，前端订阅渲染
- 工具调用聚合：SSE 里 `tool_calls` 是分片到达的，需按 index 拼接 `arguments` JSON
- 上下文管理：消息总 token 接近模型上限时，自动调用模型做摘要压缩（保留最近 N 条原文）
- 中断：前端发 `cancel` 事件 → Rust 侧 drop SSE stream → 把"用户已中断"作为 assistant message 写回

### 3.3 数据模型（SQLite）

```sql
CREATE TABLE sessions (
  id TEXT PRIMARY KEY,                   -- uuid
  title TEXT NOT NULL,
  cwd TEXT NOT NULL,                     -- 项目根目录
  model_id TEXT NOT NULL,                -- 例如 anthropic/claude-opus-4.7
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  total_input_tokens INTEGER DEFAULT 0,
  total_output_tokens INTEGER DEFAULT 0,
  total_cost_usd REAL DEFAULT 0
);

CREATE TABLE messages (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  role TEXT NOT NULL,                    -- user / assistant / tool / system
  content TEXT NOT NULL,                 -- JSON: 文本块 + 工具调用块
  created_at INTEGER NOT NULL,
  parent_id TEXT,                        -- 用于消息树（未来分支会话）
  model_id TEXT,
  input_tokens INTEGER,
  output_tokens INTEGER
);

CREATE TABLE tool_calls (
  id TEXT PRIMARY KEY,
  message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  tool_name TEXT NOT NULL,
  arguments TEXT NOT NULL,               -- JSON
  result TEXT,                           -- JSON
  status TEXT NOT NULL,                  -- pending / approved / denied / done / error
  error TEXT,
  duration_ms INTEGER,
  created_at INTEGER NOT NULL
);

CREATE INDEX idx_messages_session ON messages(session_id, created_at);
CREATE INDEX idx_tool_calls_message ON tool_calls(message_id);
```

---

## 4. OpenRouter 集成

### 4.1 协议要点
- 默认 Base URL：`https://openrouter.ai/api/v1`
- **支持自定义 base_url**（MVP 即支持）：用户可在设置中改成任意 OpenAI 兼容端点
  - 官方 OpenRouter：`https://openrouter.ai/api/v1`
  - 自建 OneAPI / NewAPI / FastGPT：`https://your-relay.example.com/v1`
  - 直连 OpenAI（备用）：`https://api.openai.com/v1`
  - 任意 LiteLLM / Ollama OpenAI 兼容端点
- 协议：OpenAI Chat Completions 兼容
- 鉴权：`Authorization: Bearer <API_KEY>`
- 推荐 header：
  - `HTTP-Referer: https://github.com/<your-repo>` （展示在 OpenRouter 排行榜，自建中转可忽略）
  - `X-Title: CodeFactory`
- 流式：`"stream": true`，返回 SSE，每个 chunk 是 `data: {...}\n\n`，结尾 `data: [DONE]`

### 4.1.1 多 Endpoint 配置
设置中支持多个命名 endpoint，每个独立保存 key（都进 keyring）：

```jsonc
{
  "endpoints": {
    "openrouter": {
      "baseUrl": "https://openrouter.ai/api/v1",
      "keyRef": "codefactory.endpoint.openrouter"   // keyring 中的别名
    },
    "my-oneapi": {
      "baseUrl": "https://oneapi.mycompany.com/v1",
      "keyRef": "codefactory.endpoint.my-oneapi"
    }
  },
  "defaultEndpoint": "openrouter"
}
```

模型选择器顶部加一个 endpoint 切换。模型列表从当前 endpoint 的 `/models` 拉取——非 OpenRouter 的端点可能返回的模型字段不全（缺 pricing），UI 优雅降级显示"价格未知"。

### 4.2 请求示例

```json
POST /api/v1/chat/completions
{
  "model": "anthropic/claude-opus-4.7",
  "messages": [
    {"role": "system", "content": "You are CodeFactory..."},
    {"role": "user", "content": "帮我修一下 src/main.rs 的编译错误"}
  ],
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "read_file",
        "description": "Read a file from disk",
        "parameters": {
          "type": "object",
          "properties": {
            "path": {"type": "string"}
          },
          "required": ["path"]
        }
      }
    }
  ],
  "tool_choice": "auto",
  "stream": true,
  "temperature": 0.2,
  "max_tokens": 8192,
  "usage": {"include": true}
}
```

### 4.3 模型管理
- 启动时拉取 `GET /api/v1/models`，缓存 24h
- 模型选择器按 family 分组：Anthropic / OpenAI / Google / DeepSeek / Qwen / Meta / Mistral 等
- 每个模型展示：context length、price (input/output per 1M tokens)、是否支持 tool_calls、是否支持 vision
- 项目级默认模型，会话级覆盖
- "智能路由"：用户可选 `openrouter/auto` 让 OpenRouter 自动选最合适的模型

### 4.4 token / 成本统计
- 启用 `usage.include = true`，每次响应末尾会带 `usage` 字段（`prompt_tokens` / `completion_tokens` / `cost`）
- 累加到 session 表，前端实时显示当前会话费用
- 每周 / 每月统计图表（二期）

### 4.5 已知差异
| 模型 | tool_calls 支持 | 流式工具调用 | 备注 |
|---|---|---|---|
| Claude (Anthropic) | ✅ 完整 | ✅ | 推荐主力 |
| GPT-4o / GPT-5 | ✅ 完整 | ✅ | 备选主力 |
| Gemini 2.5 Pro | ✅ | ✅ | 长上下文优势 |
| DeepSeek-V3 | ✅ | ✅ | 性价比最高 |
| Qwen3 系列 | ✅ | ✅ | 国产备选 |
| 部分开源模型 | ⚠️ prompt 模拟 | ❌ | MVP 不重点支持 |

---

## 5. 工具系统设计

### 5.1 工具清单（MVP）

| 工具 | 功能 | 风险等级 | 默认权限 |
|---|---|---|---|
| `read_file` | 读取文件内容（支持行偏移、limit） | 低 | 自动允许 |
| `write_file` | 创建/覆盖文件 | 高 | 弹窗确认 |
| `edit_file` | 字符串替换式编辑 | 高 | 弹窗确认（首次后可"本会话允许"） |
| `glob` | 文件名模式搜索 | 低 | 自动允许 |
| `grep` | 内容搜索（bundled ripgrep） | 低 | 自动允许 |
| `bash` | 执行 shell 命令（PowerShell / Git Bash 可选） | 高 | 弹窗确认 + 命令白名单 |
| `web_fetch` | 抓取 URL 转 markdown | 中 | 域名白名单 |
| `list_dir` | 列目录 | 低 | 自动允许 |

### 5.2 工具定义规范
每个工具实现以下 trait：

```rust
trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> serde_json::Value;        // JSON Schema
    fn risk_level(&self) -> RiskLevel;            // Low / Medium / High
    async fn execute(&self, args: Value, ctx: &ExecCtx) -> Result<ToolOutput>;
}
```

`ExecCtx` 携带：当前 cwd、权限决策器、取消信号、事件发射器（用于前端展示进度）。

### 5.3 权限模型

三级策略，对齐 Claude Code 的 settings.json：

```jsonc
// %APPDATA%\CodeFactory\settings.json
{
  "permissions": {
    "allow": [
      "read_file",
      "glob",
      "grep",
      "list_dir",
      "bash(git status)",
      "bash(git diff:*)",
      "bash(npm test*)"
    ],
    "ask": [
      "write_file",
      "edit_file",
      "bash(*)"
    ],
    "deny": [
      "bash(rm -rf*)",
      "bash(format*)",
      "bash(* /f /q*)"
    ]
  },
  "defaultMode": "acceptEdits"   // / planMode / strictAsk
}
```

匹配规则：先看 deny → 再看 allow → 都不命中走 ask。`bash(...)` 内是命令前缀匹配 + glob。

### 5.4 用户确认 UI
- 卡片形式展示：工具名 + 参数 diff（write/edit 显示 unified diff）+ 三个按钮：允许一次 / 本会话总是允许 / 拒绝
- 危险命令（rm / format / reg / shutdown）始终弹窗，无法"总是允许"
- 拒绝时把 "User denied this tool call. Try a different approach." 作为工具结果回喂给模型

---

## 6. UI / UX 设计

### 6.1 主界面布局

```
┌──────────────────────────────────────────────────────────┐
│ ⚙ CodeFactory   D:\projects\my-app   [模型: Opus 4.7 ▾] │
├─────────┬────────────────────────────────────────────────┤
│ 历史会话 │  ┌─ assistant ──────────────────────────────┐ │
│         │  │ 我先看下项目结构...                        │ │
│  + 新会话│  │ [🔧 list_dir(".")]  ← 可点开看输入输出   │ │
│         │  │ [🔧 read_file("package.json")]           │ │
│ ▸ 修复 X │  │ 找到了问题，需要改 src/api.ts...          │ │
│ ▸ 重构 Y │  │ [🔧 edit_file] ← 显示 diff，待确认       │ │
│ ▸ 写测试│  └──────────────────────────────────────────┘ │
│         │  ┌─ user ──────────────────────────────────┐ │
│         │  │ [输入框] /sh /model /clear /help ...    │ │
│         │  │ ↵ 发送  ⏵︎ 执行    [图片]    [@文件]    │ │
│         │  └─────────────────────────────────────────┘ │
│         │  💰 $0.034  📊 12.3k tokens                   │
└─────────┴────────────────────────────────────────────────┘
```

### 6.2 设计原则
- **会话即工作目录**：每个会话绑定一个 cwd，所有相对路径基于它
- **流式优先**：输入还没结束就要看到 token 涌出来
- **工具调用可视化**：每个工具调用是一张卡片，可折叠/展开，写文件类显示 diff
- **键盘优先**：`/` 唤起 Slash 菜单，`@` 唤起文件选择，`Ctrl+K` 命令面板，`Esc` 中断，`Ctrl+J` 多行
- **暗色为默认**：开发者夜间使用居多，配合 Tailwind 的 `dark:` 类

### 6.3 内置 Slash 命令（MVP）
- `/clear` 清空当前会话
- `/model <name>` 切换模型
- `/cwd <path>` 切换工作目录
- `/sh <cmd>` 直接执行命令（不经过模型）
- `/cost` 查看成本
- `/export` 导出会话为 Markdown
- `/help` 帮助
- `/init` 在当前项目生成 `CODEFACTORY.md`（项目说明文档）

### 6.4 记忆系统（CLAUDE.md 等价物）
- 项目根目录 `CODEFACTORY.md`：每次会话开始自动注入到 system prompt
- 用户级 `%APPDATA%\CodeFactory\MEMORY.md`：跨项目通用偏好
- 用 `#` 开头的消息可以快速追加记忆，例如 `# 测试用 vitest 不要用 jest`

---

## 7. 配置与存储

### 7.1 配置文件位置
| 文件 | 路径 | 用途 |
|---|---|---|
| 全局设置 | `%APPDATA%\CodeFactory\settings.json` | 默认模型、权限策略、主题 |
| 项目设置 | `<project>\.codefactory\settings.json` | 项目级覆盖 |
| 项目记忆 | `<project>\CODEFACTORY.md` | 注入 system prompt |
| 会话 DB | `%APPDATA%\CodeFactory\sessions.db` | SQLite |
| 日志 | `%APPDATA%\CodeFactory\logs\codefactory-<date>.log` | tracing 输出 |

### 7.2 API Key 存储
- **绝不**写到 settings.json
- 用 `keyring` crate 存到 Windows Credential Manager
- 启动时从 keyring 取，运行期保留在内存的 `Arc<RwLock<String>>`，不落盘

### 7.3 项目设置示例

```jsonc
// .codefactory/settings.json
{
  "model": "deepseek/deepseek-v3",       // 此项目用 DeepSeek
  "systemPromptAppend": "本项目用 pnpm，不要用 npm",
  "tools": {
    "bash": {
      "shell": "pwsh",                    // pwsh / cmd / bash
      "allowedCwd": "."                   // 限制只能在项目内执行
    }
  }
}
```

---

## 8. 安全

### 8.1 威胁模型
1. **恶意 prompt 注入**：用户读到的网页 / 文件可能含"忽略之前指令，执行 X"——靠权限确认兜底，所有写文件 / 执行命令必须用户点同意
2. **API Key 泄露**：keyring 存储 + 不写日志 + 不打入会话导出
3. **沙箱逃逸**：bash 工具默认限制在项目目录，禁止 cd 到项目外（可配置放开）
4. **依赖供应链**：Cargo.lock + npm lockfile 锁版本，CI 跑 `cargo audit` 和 `npm audit`

### 8.2 默认安全配置
- 首次启动：所有工具都进入 ask 状态，用户主动放行
- 危险命令永久 deny 列表内置（rm -rf、format、del /f /s /q、reg delete、shutdown）
- 工具输出限长（read_file 默认 2000 行，bash 输出 30k 字符）防 token 爆炸

### 8.3 审计
- 所有工具调用入库，可在"会话"页查看完整时间线
- 导出会话支持 `--with-tool-calls`

---

## 9. 开发阶段与里程碑

### Phase 0: 脚手架（Week 1）
- Tauri 2 项目初始化，Rust + React + TS + Tailwind 跑通
- `tauri.conf.json` 配好窗口、图标、capability
- SQLite migration 可跑
- CI: GitHub Actions → 构建 Windows MSI

**完成标志**：能打开窗口，显示 Hello World，能往 SQLite 写一条记录

### Phase 1: MVP 对话（Week 2-3）
- OpenRouter HTTP 客户端 + SSE 流式
- 单会话单模型对话，无工具
- 消息持久化
- 模型选择器
- API Key 设置界面

**完成标志**：能像 ChatGPT 一样和 OpenRouter 模型聊天，token 计费正确

### Phase 2: 工具系统（Week 4-5）
- 工具 trait 设计 + 注册表
- 实现 read_file / write_file / edit_file / glob / grep / list_dir
- Agent 循环（处理 tool_calls）
- 工具调用 UI 卡片 + diff 渲染
- 权限确认弹窗

**完成标志**：能让模型自动读项目、改一个 bug

### Phase 3: 命令执行 & 终端（Week 6-8，3 周）
- bash 工具（PowerShell / Git Bash 可选）
- 权限策略 settings.json
- 命令白名单 / 黑名单匹配
- **嵌入式 xterm 终端**：
  - 前端：`xterm.js` + `xterm-addon-fit` + `xterm-addon-webgl`
  - 后端：`portable-pty` crate，Windows 上走 ConPTY（要求 Win10 1809+；低版本退化为无 PTY 的行模式 + 提示用户升级）
  - 双向桥：前端按键 → Tauri event → PTY stdin；PTY stdout → Tauri emit → xterm.write
  - 同会话内"AI 区"和"终端区"共享 cwd，模型 bash 工具的输出也实时映射到这个终端
- /sh slash 命令（直接在终端执行，不走模型）

**完成标志**：能让模型跑测试、git commit、npm install；用户也能在嵌入式终端里手动操作

### Phase 4: 上下文与记忆（Week 8）
- 上下文压缩（接近 token 上限自动摘要）
- CODEFACTORY.md 自动注入
- 用户记忆（# 开头的消息）
- 中断 / 取消
- 会话导出 Markdown

**完成标志**：长会话不爆 context，记忆能跨会话生效

### Phase 5: 体验完善（Week 9-10）
- Slash 命令系统 + cmdk 命令面板
- @文件 mention（拖文件 / 选择器）
- 图片粘贴（vision 模型）
- 多会话切换 / 历史搜索
- 主题切换 / 字体设置
- 系统托盘 / 全局快捷键

**完成标志**：日常可用，体验对齐 Cursor / Cline

### Phase 6: 打包与发布（Week 11）
- 应用签名（购买代码签名证书或用 SmartScreen 友好的方式）
- MSI / NSIS 安装包
- 自动更新（tauri-plugin-updater）
- 官网 + 文档站

**完成标志**：v1.0 发布

### Phase 7（可选，二期）
- MCP 服务器支持（接入 Claude Code 生态的 MCP）
- 自定义 Slash 命令（Markdown 文件即命令）
- Hooks 系统
- 子 Agent / 任务编排
- 跨平台（macOS / Linux）
- IDE 扩展（VSCode 侧边栏调用本客户端）

---

## 10. 项目结构（最终态）

```
D:\CodeFactory\
├─ src-tauri/              # Rust 后端
│  ├─ src/                 # 源码（见 §3.1）
│  ├─ migrations/          # SQL migrations
│  ├─ icons/
│  ├─ capabilities/        # Tauri capability JSONs
│  ├─ tauri.conf.json
│  └─ Cargo.toml
├─ src/                    # React 前端（见 §3.1）
├─ docs/
│  ├─ PROJECT_PLAN.md      # 本文件
│  ├─ ARCHITECTURE.md
│  ├─ TOOLS.md
│  ├─ ROADMAP.md
│  └─ CONTRIBUTING.md
├─ tests/
│  ├─ rust/                # 单元 + 集成测试
│  └─ e2e/                 # Playwright (可选)
├─ scripts/
│  ├─ release.ps1
│  └─ dev.ps1
├─ .github/workflows/
│  ├─ ci.yml
│  └─ release.yml
├─ package.json
├─ pnpm-lock.yaml
├─ tsconfig.json
├─ vite.config.ts
├─ tailwind.config.js
├─ .gitignore
├─ LICENSE
└─ README.md
```

---

## 11. 风险与缓解

| 风险 | 概率 | 影响 | 缓解措施 |
|---|---|---|---|
| OpenRouter 限流 / 不稳定 | 中 | 高 | 自动重试 + 错误时允许切换 provider；二期支持直连 Anthropic / OpenAI |
| 不同模型 tool_calls 行为不一致 | 高 | 中 | 内部抽象一层 normalize；MVP 只主推 Claude / GPT / DeepSeek |
| Windows 权限弹窗（UAC）干扰 | 中 | 中 | 只在用户主动允许的目录下操作；不申请管理员权限 |
| WebView2 在老 Win 10 上版本不一 | 中 | 中 | 安装包内置 WebView2 Bootstrapper |
| ConPTY 在 Win10 1809 以下不可用 | 低 | 中 | 启动时检测系统版本；不可用时禁用嵌入终端 + 提示升级；bash 工具仍可用（无 PTY 模式） |
| xterm + ConPTY 在 Windows 上 bug 较多（颜色、Resize、Ctrl+C） | 中 | 中 | 借鉴 Windows Terminal / vscode-pty 的成熟方案；Phase 3 多留 1 周缓冲 |
| Rust + React 双栈拖慢迭代 | 中 | 中 | 严格分层：业务在 Rust，UI 在 React，IPC 接口稳定后两边独立演进 |
| 模型自动操作出现破坏 | 中 | 高 | 默认 strict ask 模式；危险命令永久 deny；所有写操作可在 UI 撤销（保留备份） |
| OpenRouter 计费失控 | 中 | 中 | 会话级预算上限 + 单次请求 max_tokens 限制 + 实时成本展示 |
| 国内网络访问 OpenRouter 不稳 | 高 | 高 | 设置中支持 HTTP 代理 + 自定义 base_url（也可指 OneAPI / NewAPI 自建中转） |

---

## 12. 关键设计决策记录（ADR 摘要）

### ADR-001: 选择 Tauri 而非 Electron
- **决策**：用 Tauri 2.0
- **理由**：包体小 10×、内存低、Rust 后端做 Agent 工具更安全
- **代价**：需要 Rust 工程师；npm 上某些 Electron 专用库不可用

### ADR-002: 用 OpenRouter 而非直连各家 API
- **决策**：MVP 只接 OpenRouter
- **理由**：一套 OpenAI 兼容协议跑所有模型，省一半适配工作；国内用户可走 OpenRouter 中转
- **代价**：依赖第三方稳定性；按 OpenRouter 涨价付费
- **缓解**：抽象 `LlmProvider` trait，二期可加直连

### ADR-003: SQLite 而非文件系统存会话
- **决策**：SQLite
- **理由**：消息检索 / 全文搜索 / 跨会话统计都靠 SQL 一行搞定
- **代价**：不能用文本编辑器直接看 / 改

### ADR-004: 工具调用走 OpenAI tool_calls 协议
- **决策**：用 OpenAI 风格 `tools` + `tool_calls`，不发明私有协议
- **理由**：OpenRouter 上几乎所有商用模型原生支持
- **代价**：纯开源模型（无 tool 微调）需要 prompt 模拟，MVP 不支持

### ADR-005: API Key 存 Windows Credential Manager
- **决策**：keyring crate
- **理由**：不落明文，跟 Windows 凭据生态一致
- **代价**：用户重装系统需重新输入 key

### ADR-006: 支持自定义 base_url（多 endpoint）
- **决策**：MVP 即支持任意 OpenAI 兼容端点（OpenRouter / OneAPI / NewAPI / 直连）
- **理由**：国内网络对 OpenRouter 不稳；企业用户有自建中转需求；几乎零额外成本
- **代价**：模型列表字段不统一时 UI 要做兼容降级（pricing 可能缺失）

### ADR-007: MVP 包含嵌入式终端
- **决策**：Phase 3 一并交付 xterm + ConPTY 嵌入式终端
- **理由**：模型 bash 输出有颜色/进度条/Resize 等真实终端体验；用户手动操作不用切窗
- **代价**：Phase 3 从 2 周扩到 3 周；Windows ConPTY 边界 case 多
- **风险缓解**：Win10 1809 以下退化到无 PTY 模式；borrow Windows Terminal 实现

### ADR-008: 项目 License = Apache-2.0
- **决策**：Apache-2.0
- **理由**：自带专利授权与反诉条款，保护项目作者；与 Tauri / Rust 生态主流一致；企业友好
- **代价**：相比 MIT 多一个 NOTICE 文件维护；源文件需加 SPDX 头

---

## 13. 立即可启动的下一步

按优先级排序，下一个对话/PR 应该做的事：

1. **初始化 Tauri 项目**：`npm create tauri-app@latest` 选 React + TypeScript
2. **配置基础栈**：Tailwind + shadcn/ui + Zustand + Monaco
3. **实现 OpenRouter 客户端**：`src-tauri/src/openrouter/client.rs`，先做非流式 chat completions
4. **跑通端到端**：前端发消息 → 后端调 OpenRouter → 返回展示
5. **加流式**：SSE + 前端打字机效果
6. **加 SQLite**：会话持久化
7. **设计工具 trait**：开始实现第一个工具 read_file
8. **进入 Phase 2**

---

## 14. 附录

### 14.1 参考资料
- Tauri 2 文档：https://tauri.app
- OpenRouter API：https://openrouter.ai/docs
- Claude Code 公开行为参考（不复制代码）：https://docs.claude.com/en/docs/claude-code
- xterm.js：https://xtermjs.org
- Monaco Editor：https://microsoft.github.io/monaco-editor

### 14.2 项目命名
**CodeFactory** — 已最终确定。

应用 ID / 包名约定：
- Windows AppUserModelID：`com.codefactory.app`
- Tauri identifier：`com.codefactory.app`
- 可执行文件：`CodeFactory.exe`
- 安装目录默认：`%LOCALAPPDATA%\CodeFactory\`
- 配置目录：`%APPDATA%\CodeFactory\`
- 项目级目录名：`.codefactory/`
- 项目记忆文件：`CODEFACTORY.md`
- 用户级记忆：`%APPDATA%\CodeFactory\MEMORY.md`
- OpenRouter 上报 header：`X-Title: CodeFactory`

### 14.3 License
**Apache-2.0** — 已最终确定。

落地动作：
- 根目录添加 `LICENSE` 文件（Apache-2.0 全文）
- 根目录添加 `NOTICE` 文件（列出依赖项的版权声明，Apache 强制要求）
- 每个源文件顶部加 SPDX 短标识：`// SPDX-License-Identifier: Apache-2.0`
- `Cargo.toml` 和 `package.json` 的 `license` 字段填 `"Apache-2.0"`
- 第三方依赖审计：用 `cargo-deny` 检查传染性 License（GPL / AGPL），如有立即换

---

**文档维护**：方案变更时更新本文件 + 在 git 提交里写明决策依据。重大架构调整另起 ADR。
