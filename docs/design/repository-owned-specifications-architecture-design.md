# 仓库归属规范与会话内计划：架构设计

## 现状

```text
Workspace BookOpen ──> SpecsPage ──> useSpecsStore ──> specs Tauri commands
                                                └──> .codefactory/specs/*.md
Remote Issue ──> issue_to_spec ───────────────────────┘

Agent session ──> delegate_tasks ──> task_runs / scheduler / ExecutionStream
Repository ──> docs/specs + docs/design
```

`src-tauri/src/commands/specs.rs` 还混入了通用 one-shot 模型调用，学习复盘和子 Agent
验收会复用它。直接删除该文件会误伤不相关能力。

## 目标架构

```text
Repository authority
  AGENTS.md
  docs/specs/**/*.md
  docs/design/**/*.md
        │ bounded index
        ▼
Conversation Agent ── read_file / write_file / edit_file ──> ordinary Git diff
        │
        └── delegate_tasks ──> task_runs / scheduler / in-session execution detail

Generic one-shot transport ──> learning + subagent acceptance
```

## 模块变更

1. **Workspace**：删除 `BookOpen`、`specsOpen` 与 `SpecsPage` overlay；任务详情继续由
   `delegate_tasks` 生成后按状态显示。
2. **Spec product module**：删除 `SpecsPage.tsx`、`stores/specs.ts`、Specs CRUD/AI/decompose
   commands 与 Tauri 注册。
3. **通用 AI transport**：把 `AiMessage`、`build_one_shot_request`、
   `run_one_shot_text` 和 transport tests 移入中性的 `ai_text` 模块；学习和子 Agent
   改用该模块。
4. **Repository authority**：Agent prompt 高优先级注入有界的 `AGENTS.md` 内容，并发现
   仓库已经存在的 `docs/specs`、`docs/design` 相对路径。只注入索引，不批量塞入所有正文；
   Agent 按任务使用 `read_file` 读取相关文件，且以仓库自己的规则决定长期文档位置。
5. **Control Plane**：`docs/specs` 是唯一规范 authority item；删除
   `.codefactory/specs` item。
6. **Remote Git**：删除 Issue Detail 的“创建为规范”、store 方法和 `issue_to_spec`
   command。Issue 仍可被浏览；用户可在会话中要求 Agent 处理并决定是否沉淀仓库文档。
7. **Repository migration**：把本仓库两个已跟踪旧规范移动到
   `docs/specs/feature-specs/` 并更新索引。

## Context budget 与文件安全

- `AGENTS.md` 内容单独设字符上限；超长时明确截断。
- 规范/设计目录递归发现只接受普通 `.md` 文件，跳过 symlink，限制文件数量与总字符。
- 索引使用相对仓库路径；不读取或暴露仓库外文件。
- 不解析或改写用户规范格式，不要求 YAML frontmatter。

## 兼容合同

- `task_runs.spec_req_id`、`task_runs.spec_title` 保持 nullable、可读、可序列化。
- `start_implementation`、Evidence Pack、toast 与 manifest 行为不变。
- 旧任务显示的“来自规范”仅作为历史 provenance，不表示存在独立工作台。
- `spec_approved` hook enum/配置值本轮保留，避免破坏旧 settings；不再产生新的 UI emit。

## 回退

本变更不做数据库和持久格式迁移。代码回退可恢复旧工作台；Git move 的两个 Markdown
文件可由 Git 逆向移动。已迁移后的文档内容不会丢失。
