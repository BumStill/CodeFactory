# 会话自动语义命名规格

## 问题与目标

当前新会话标题只是首条消息前缀，中文输入尤其接近原文截断。该行为既不能稳定概括会话主题，也会把 prompt 中的路径、凭据、账号、代码或日志暴露到侧栏、搜索、Welcome 和用量页。

本特性把新会话标题升级为经过裁剪、脱敏和校验的语义短语，同时保证正文对话优先、同 Provider 边界、人工重命名优先和旧数据兼容。业务、架构和交互决策分别见：

- `docs/design/session-auto-title-business-design.md`
- `docs/design/session-auto-title-architecture-design.md`
- `docs/design/session-auto-title-ux-design.md`

## Requirements Traceability

| Req ID | 要求 | Surface | 验证 |
| --- | --- | --- | --- |
| CF-SAT-R1 | 自动标题必须语义概括“主题 + 预期结果”，使用名词短语；不得继续把首条消息按词数或字符数截断后直接作为标题 | title generation | 语义 fixture + negative prefix assertions |
| CF-SAT-R2 | 命名输入只允许首个有信息量的 root 用户可见正文，最多 2,000 grapheme；低信息输入可在首轮完成后追加最多 800 grapheme 的用户可见助手结论并重试一次。system/developer prompt、memory、reasoning、工具参数/结果、附件正文/名称、cwd、分支和隐藏上下文不得进入输入 | input builder | unit + payload negative tests |
| CF-SAT-R3 | 标题跟随用户主语言；中文目标 8–20 字、英文 3–8 词、全语言硬上限 40 grapheme；结果必须单行且无 Markdown、引号、`标题：` 前缀、句末标点和“请/帮我/能否/please/can you”等请求措辞 | output validator | multilingual property tests |
| CF-SAT-R4 | 首条消息持久化后进入异步命名生命周期；命名只在正文回合结束并经过短暂 idle grace 后启动，不阻塞正文请求、流式输出和错误处理，也不与已经排队的下一正文回合争用 Provider；成功后标题保持稳定 | chat + background title job | integration + latency/failure injection + real app |
| CF-SAT-R5 | Session 必须持久化 `title_source`，区分 `placeholder/generated/fallback/manual/legacy`；自动结果只可 CAS 覆盖 `placeholder`，手动重命名永久优先，迟到结果不得覆盖；自动命名本身不得推进最近活动排序 | SQLite + update_session_title | migration + CAS race + ordering tests |
| CF-SAT-R6 | 命名请求使用 session 已解析的 `endpoint_id + model_id`，只调用同一已选 Provider，不得失败后跨 Provider；不支持、超时、Provider 错误或非法输出时走本地安全 fallback，绝不退回原文前缀，且不影响聊天 | model route + fallback | provider contract + timeout + no-cross-route assertions |
| CF-SAT-R7 | 输入和输出均须脱敏；标题不得包含 secret/token、邮箱、手机号、URL、绝对路径、UUID、长 hash、代码或日志行，除技术专名外不得复制超过 12 个连续中文字符或 6 个连续原文词；日志和事件不得记录命名输入、生成标题或完整 Provider payload | redaction + logging | adversarial corpus + log scan |
| CF-SAT-R8 | 侧栏、顶栏、搜索、Welcome 和用量详情必须读取同一规范化标题并通过 `session_updated` 一致刷新；视觉截断、tooltip 和 accessible name 均不得回退原文。Session 标题不得作为 commit、PR、release title、发布 slot 或交付幂等键 | UI projections + delivery boundary | component/integration + repository negative audit + real app |
| CF-SAT-R9 | 匿名会话固定为“匿名会话”，不持久化、不发额外命名请求、不产生 title Usage；旧会话迁移为 `legacy` 且默认不回填。Provider 实际返回的用量写入 `model_usage_events`，surface 为稳定枚举 `session_title`；所有逻辑命名任务另写 `session_title_attempts` 的状态、稳定 failure code 和时延，二者均不含正文 | compatibility + usage | SQLite migration + usage reconciliation + anonymous negative test |
| CF-SAT-R10 | 发布前必须完成中英文正常路径、低信息延迟重试、敏感输入、Provider 不支持/超时/非法输出、手动竞态、匿名、旧会话和所有标题 surface 的真实 App 验收；只通过 mock、HTTP 200 或单元测试不得声称完成 | end-to-end | CodeFactoryDev + packaged artifact + evidence pack |

## Primary User Path

用户新建会话并在草稿中选择项目、模型和权限。发送首条实质消息后，会话以“新会话”物化，正文请求立即开始；正文回合结束且没有已排队的下一回合后，后台仅向该会话已经选择的 Provider 发送经过裁剪和脱敏的命名输入。合规结果通过 CAS 保存并同步到侧栏、顶栏、搜索、Welcome 和用量详情。用户可以随时手动改名，人工标题不会被迟到的自动结果覆盖。

## 标题契约

### 输入

- 首个有信息量的 root 用户可见正文，最多 2,000 grapheme。
- “继续”“看看这个”等低信息输入不直接命名；首轮完成后可使用用户可见结论进行一次延迟重试。
- 不读取未发送草稿、隐藏上下文、文件正文、工具证据或本机路径。

### 输出

- 主题明确、稳定、可搜索的名词短语。
- 使用用户主语言，必要技术专名保持原样。
- 通过长度、格式、命令措辞、秘密和原文重合度校验后才能写库。

### 生命周期

```text
草稿“新会话”
  -> 首条消息物化为 placeholder
  -> 异步 generated 或 fallback
  -> 稳定标题

任意自动阶段 -> 用户重命名为 manual -> 永久保持人工标题
```

## Applicable Harnesses

- Spec Harness：CF-SAT-R1..R10。
- Compatibility Harness：旧数据库 `legacy`、两个 session 创建入口、Provider/api style 与现有 Session 类型。
- Payload Harness：输入裁剪、附件/路径/secret/日志排除、输出脱敏和日志扫描。
- Viewport Harness：侧栏、顶栏、tooltip、accessible name、375×812、800×600 和 200% zoom。
- Observation Harness：真实 Tauri 中 placeholder → generated/fallback、搜索和重启持久化。
- AI Collaboration Harness：模型提示、非确定性输出校验、关键假设、独立隐私/UX 审查。
- Release Harness：PR+CI、正式安装产物和精确版本真实主路径。

## 测试矩阵

| 场景 | 正常路径 | 边界路径 |
| --- | --- | --- |
| 语义 | 中文、英文、混合语言生成主题短语 | 中文无空格长句、命令前缀、超长输出 |
| 输入 | 首个实质用户消息 | “继续”、附件型指代、代码块、日志和隐藏上下文 |
| 隐私 | 技术专名可保留 | token、邮箱、手机号、URL、绝对路径、UUID、hash、代码和日志必须排除 |
| Provider | 已选 endpoint/model 成功 | 不支持 api style、断网、限流、超时、空值、非法输出；不得跨 Provider |
| 竞态 | placeholder CAS 为 generated | 生成在途手动改名、重复请求、session 删除、App 重启 |
| 兼容 | 新 project/quick session | anonymous 无调用、legacy 不回填、旧 create-then-send 入口 |
| Surface | 侧栏、顶栏、搜索、Welcome、用量一致 | tooltip/accessible name、窄屏、200% zoom、标题更新不重排 |
| Usage | `session_title` 成功请求计量 | 失败计量无正文、匿名无事件、日志无输入/输出 |

## Evidence Pack Requirements

- 先有失败测试，证明现实现会复制中文原文前缀并可能暴露敏感内容。
- 纯函数、Provider contract、SQLite migration/CAS、store/event、Usage 和 repository negative tests 通过。
- `pnpm test`、相关 Rust 测试、typecheck/build、治理基线和 `git diff --check` 通过。
- CodeFactoryDev 走过正常中文命名、低信息重试、敏感输入、Provider 失败和手动竞态。
- 正式发布安装产物重复同一路径；未完成安装包和真实 App 验证前标记 `not live`。
- 证据只记录脱敏后的标题类别、状态、时延和断言，不保存测试 secret 或原始 prompt。

## 非目标与已知风险

- 不自动重命名旧会话，也不新增“恢复自动命名”入口。
- 不把标题生成扩展成会话全文摘要、记忆、画像或检索索引。
- 不使用 Session 标题驱动 commit、PR、release 或交付恢复。
- `chat_task_segments.title` 与 checkpoint `label` 仍可能保存原文片段。本特性不修改这两个对象；它们必须作为后续 Payload/隐私任务处理，验收时不得把 session 标题通过描述成全产品原文泄漏已关闭。

## 发布边界

本特性是用户可见 `fix`。PR、CI 和本地测试通过只是中间证据；只有对应正式安装产物在真实新会话路径中证明语义标题、隐私、fallback、手动优先和跨 surface 一致性后，才能称为可用。
