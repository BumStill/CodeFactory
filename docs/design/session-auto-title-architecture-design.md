# 会话自动语义命名架构设计

## 总体数据流

```text
草稿发送首条消息
  -> 事务内创建 session(title="新会话", title_source="placeholder") + user message
  -> 正文模型请求照常开始
  -> 正文回合结束，等待短暂 idle grace；若下一正文回合已开始则延后
  -> 后台构造 bounded + redacted TitleInput
  -> 使用 session 已解析的 endpoint_id + model_id 生成标题
  -> 输出校验与二次脱敏
  -> CAS 更新仍为 placeholder 的 session
  -> session_updated 事件刷新所有 UI surface

失败 / 超时 / Provider 不支持
  -> 本地安全 fallback
  -> CAS 写入 fallback，或保留“新会话”
  -> 不影响正文回合
```

新草稿物化路径和旧的 create-then-send 兼容路径必须调用同一命名服务。现有“取前 6 个空白词、截 40 字符”的重复逻辑应移除，不能让两个入口继续产生不同标题。

## 输入契约

### 首次输入

只读取首个有信息量的 root 用户消息可见正文，最多 2,000 个 Unicode grapheme。构造输入前移除或抽象化：

- system/developer prompt、memory、reasoning 和内部恢复提示；
- 工具名称以外的参数、结果、命令和日志；
- 文件/附件正文及附件名；
- cwd、绝对路径、分支名、模型名和隐藏上下文；
- secret、token、邮箱、手机号、URL、UUID 和长 hash；
- 大段代码块、堆栈和重复日志行。

“继续”“看看这个”“按上面做”等低信息输入不直接生成同名标题。系统可以等首轮结束后进行一次延迟重试，只追加最多 800 grapheme 的用户可见助手结论；内部 reasoning、工具证据和隐藏候选仍不得进入命名输入。

### 输出契约

模型只返回一个标题，不返回解释。后处理必须保证：

- 中文目标 8–20 字；英文目标 3–8 词；任何语言硬上限 40 grapheme；
- 单行、无 Markdown、引号、`标题：` 前缀和句末标点；
- 使用用户主语言，混合输入只保留必要的产品或技术专名；
- 采用“主题 + 预期结果”的名词短语，去除“请”“帮我”“能否”“please”“can you”等请求前缀；
- 除技术专名外，不复制超过 12 个连续中文字符或 6 个连续原文词；
- 通过秘密、身份信息、路径、URL、代码和日志模式扫描。

任何校验失败都进入 fallback，不允许把未经校验的模型原文写入 session。

## Provider 与模型路由

命名请求绑定物化后 session 的 `endpoint_id`、`model_id` 和实际 `api_style`，不能重新读取可能已变化的全局默认值。这样，新会话草稿中已选择的模型与实际命名 Provider 保持一致。

- 只允许调用同一已选 Provider；失败后不得自动切换 OpenRouter、DeepSeek、Anthropic、ChatGPT 或其他 endpoint。
- OpenAI-compatible、Anthropic 和 ChatGPT subscription 三种 `ApiStyle` 均复用桌面完整 transport，并使用各自正确的短输出字段；不得用某种 dialect 假装兼容另一种。
- 一个逻辑命名任务受独立 12 秒 deadline 约束，可在同一 Provider 内使用共享 transport 的瞬时错误重试和协议适配，但绝不跨 Provider；正文回合结束并确认没有已排队的下一回合后才允许启动。
- 命名调用不进入聊天 history，不产生用户/助手 message，也不触发工具、交付或学习流程。

## 持久化、CAS 与人工优先

`sessions` 增加可兼容迁移的 `title_source`。建议语义：

| 值 | 含义 | 是否允许自动覆盖 |
| --- | --- | --- |
| `placeholder` | 新会话等待自动命名 | 是 |
| `generated` | 已通过模型和校验生成 | 否 |
| `fallback` | 已使用本地安全标题或最终保留“新会话” | 否 |
| `manual` | 用户手动设置 | 否 |
| `legacy` | 升级前既有会话 | 否 |

新持久会话使用 `title="新会话"`、`title_source="placeholder"`。旧行迁移为 `legacy`，避免升级后意外改名。`update_session_title` 必须写入 `manual`。

自动结果采用 compare-and-set：

```sql
UPDATE sessions
SET title = ?, title_source = 'generated'
WHERE id = ? AND title_source = 'placeholder';
```

fallback 使用相同 CAS，只把 source 写为 `fallback`。返回 0 行表示用户已手动改名、另一个生成请求已完成或 session 已删除；这是正常竞态，不重试、不覆盖。

低信息输入在没有用户可见结论时保持 `placeholder`，下一次成功正文回合可携结论尝试；一旦生成或 fallback，source 即终止后续请求。进程内 guard 与 SQLite `session_title_jobs` lease 保证单飞，崩溃后的 stale lease 可恢复；每个实际逻辑任务写唯一 attempt id。自动标题更新不应单独推进 `sessions.updated_at`；最近活动时间由真实消息维护，避免后台命名改变列表排序。

## Fallback

fallback 先使用本地、安全的意图类别，例如“界面体验优化”“代码问题排查”“文档内容整理”“上传图片分析”。它不能复用“首 6 词”或任意原文前缀。无法得到足够安全且具体的类别时，最终标题保持“新会话”，source 写为 `fallback` 以终止重复请求。

以下情况统一进入 fallback：

- endpoint/api style 不支持命名请求；
- 无凭据、限流、网络错误或超时；
- 返回空值，或在去除首行包装、Markdown、引号、前缀、句末标点并按 grapheme 截断后仍不合法；
- 输出包含秘密、身份信息、路径、URL、代码或高比例原文复制；
- session 在请求完成前已删除。

## Usage 与可观测性

Provider 实际返回的 token/cost 写入 `model_usage_events`，surface 使用稳定枚举 `session_title`。所有逻辑任务（包括 timeout、credential/provider error、非法输出、fallback 和 CAS 丢失）写入 `session_title_attempts`，只记录 attempt id、session id、endpoint、model、状态、稳定 failure code 与时延。日志、事件和这两张表都不得记录原始输入、生成标题、秘密或完整 Provider payload；命名请求的瞬时 HTTP 错误日志也必须隐藏 response body。

建议状态码包括 `unsupported_api_style`、`timeout`、`provider_error`、`empty_output`、`invalid_output`、`redaction_rejected`、`cas_lost`。`cas_lost` 是正常竞态，不显示为用户错误。

## UI 与下游边界

沿用 `session_updated:<id>` 事件更新 store，侧栏、顶栏、Welcome、搜索和用量详情继续读取同一 `sessions.title`。事件只携带规范化 Session，不携带命名 prompt 或模型原始输出。

Session 标题是导航元数据，不得传入 commit message、PR title、release slot 或交付幂等键。相关下游必须继续根据分支、提交和真实 diff 决定名称与发布语义。

## 兼容与隐私边界

- 匿名会话维持前端内存中的“匿名会话”，不写 DB、不发额外命名请求、不产生 `session_title` Usage。
- 旧会话保持 `legacy` 标题，不做批量后台回填。
- 旧前端读取新增字段时允许缺省为 `legacy`；新前端读取旧数据库时由迁移补齐字段。
- `chat_task_segments.title` 与 checkpoint `label` 的原文片段是已知后续风险，不得与本 schema 改动混为一谈；后续修复应复用同一输入脱敏与输出校验组件。

## 验证策略

- 纯函数测试：grapheme 长度、语言、命令前缀、秘密/路径/URL/UUID/代码/日志过滤和原文重合度。
- Provider contract 测试：同 endpoint/model、支持与不支持 api style、超时和非法输出，不发生跨 Provider fallback。
- SQLite 测试：新行 source、旧行迁移、generated/fallback CAS、manual race、删除竞态和重启幂等。
- 集成测试：两个 session 创建入口使用同一服务；事件刷新所有 surface；命名请求不进入聊天 history。
- Usage 测试：成功与失败均按 `session_title` 计量，匿名会话无记录，日志不含输入或输出正文。
- 真实 App：正常中文命名、低信息延迟重试、敏感输入、Provider 失败、手动竞态、匿名与旧会话路径。
