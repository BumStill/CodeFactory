# 工具输出与模型上下文边界

## 业务设计

### 事故

本机生产会话在执行仓库全文 `grep` 后中断。进程和数据库均正常，失败点是一个压缩后的 `dist/assets` 文件以单行形式命中，工具结果达到 2,298,886 个字符。会话只剩两个用户回合时，旧压缩策略为保留最近消息而不处理这条最新工具结果；ChatGPT 首次请求和上下文溢出后的紧急重试因此都超过模型窗口。

这个问题不能靠删除会话或让用户重新开会话解决。CodeFactory 必须同时限制新工具输出，并在 provider 请求边界防御已经落库的旧超大结果。

### 目标

- 新的 `grep` 调用不能因 minified 文件、source map 或超长生成文件产生 MiB 级单行结果。
- 已经含有超大工具结果的旧会话无需删历史，下一次发送时即可恢复。
- 模型仍能看到匹配词、文件与行号，以及超大结果的头尾证据。
- 原始 SQLite 历史不被重写；压缩只发生在工具返回值和 provider replay 副本。
- 不以自动切换模型掩盖上下文溢出；路由、模型策略和副作用门禁保持原语义。

### 非目标

- 不删除或迁移旧消息。
- 不把任意大的用户原始输入静默改写；用户输入超过模型窗口时仍应给出明确错误。
- 不在本切片中加入全文索引或替换 `grep` 搜索引擎。

## Requirements Traceability

| Req ID | 要求 | Surface | 验证 |
| --- | --- | --- | --- |
| CF-CTX-R1 | `grep` 单个匹配行最多返回约 4,000 字符的匹配点周边片段，保留匹配内容并明确标记前后截断 | Rust tool | 2.30M minified-line unit |
| CF-CTX-R2 | `grep` 同时受 500 条和约 64,000 字符总预算约束；达到预算时返回明确截断标记 | Rust tool | multi-result unit |
| CF-CTX-R3 | 当 prompt 超过压缩阈值时，任意位置的 tool/assistant 单消息都受动态硬上限约束；大窗口最多保留 64 Ki 字符，小窗口按窗口缩小 | shared agent loop | production-shape unit |
| CF-CTX-R4 | 单消息兜底压缩保留头尾、角色和 `tool_call_id` / tool-call envelope；用户消息保持原文 | provider replay | protocol assertions |
| CF-CTX-R5 | 压缩只修改本次 provider 请求中的消息副本，不更新 SQLite 原始内容 | storage compatibility | code review + isolated DB |
| CF-CTX-R6 | 仅剩两个用户回合且最近工具结果约 2.30M 字符时，压缩后估算 prompt 不超过 `gpt-5.5` 的 272K 窗口 | shared agent loop | exact-size regression |
| CF-CTX-R7 | 上下文溢出重试继续使用同一端点、模型和会话策略；本缺陷不触发不诚实 failover | runtime routing | route-attempt evidence |
| CF-CTX-R8 | PR、main CI、公开安装包和真实旧历史恢复路径完成前保持 `not live` | release | release evidence |

## 架构设计

### 第一层：工具源头限流

`grep` 先用正则定位命中的 byte range，再以 Unicode 字符边界截取匹配点两侧上下文。单行命中本身超过预算时保留命中范围的头尾。文件路径、行号和截断标记均计入返回结果，累计接近总预算后停止扫描。

该层保护所有 provider 和新会话，也降低 WebView、SQLite 与审计日志的后续负担。

### 第二层：provider replay 防线

共享 `ContextCompactor` 在总 prompt 超过 75% 窗口时，先处理所有位置的 tool/assistant 消息，而不是只处理旧历史。单消息保留预算为：

```text
min(64 Ki characters, context_limit / 2 characters)
```

保留内容采用 head/tail 结构并写明原字符数、估算 token 数与省略量。之后才运行既有的旧半区压缩和按回合淘汰。这样最近工具调用的协议顺序仍在，同时单个异常结果不能垄断窗口。

压缩函数接收并返回内存消息副本；持久化层不参与，因此修复天然适用于发布前已经损坏的会话。

### 兼容与风险

- 中段证据可能被省略；模型可以根据保留的文件/匹配线索发起更窄的 `grep` 或 `read`。
- 采用字符预算是保守近似，不替代 provider tokenizer；总 prompt 仍经过现有 token 估算和硬窗口检查。
- tool/assistant 内容可压缩，角色、调用 ID、函数声明和用户输入不能被压缩器破坏。
- 回滚不会损坏数据库，但旧版本再次打开同一会话仍可能复现溢出。

## UX 设计

- 新 `grep` 结果在工具卡中显示有界片段；发生截断时用户和模型都能看到明确标记，不伪装成完整结果。
- 旧会话继续发送时沿用现有“上下文已压缩后重试”低干扰提示，无需新增阻断弹窗或要求用户删除历史。
- 若用户需要完整中段，可展开原历史记录或发起更窄搜索；数据库中的旧原文保持可审计。
- 恢复成功后，失败详情仍保留在历史中，新的回合正常流式完成。

## Primary User Paths

### 新工具成功路径

用户让模型在包含 minified bundle 的仓库中搜索符号。`grep` 返回带文件、行号、匹配词和截断标记的短片段；模型继续读取源文件，不产生 MiB 级 replay。

### 旧会话恢复路径

用户打开已经因 2.30M 工具结果中断的会话并继续。发送前 compactor 对旧结果的内存副本保留头尾，prompt 落入 272K 窗口内，同一 ChatGPT 模型完成新回合；SQLite 旧记录字节数不变。

### 边界路径

- 匹配词位于超长行中间或 Unicode 文本附近。
- 正则本身匹配超过 4,000 字符。
- 64K 总预算先于 500 条数量预算耗尽。
- 超大消息位于最近半区，且只剩两个用户回合。
- 压缩消息含 `tool_call_id`。

## Applicable Harnesses

- Spec Harness：CF-CTX-R1..R8。
- Compatibility Harness：旧 SQLite、ChatGPT replay、tool protocol、fixed/prefer/auto 策略。
- Payload Harness：minified 2.30M 单行、64K 工具总预算、272K 模型窗口。
- Observation Harness：route attempt、failure code、prompt estimate、SQLite 前后大小。
- Release Harness：PR/main CI、macOS/Windows 构建、公开安装包和真实恢复路径。

## 测试矩阵

| 层级 | 场景 | 断言 |
| --- | --- | --- |
| Rust tool unit | 2.30M 单行中间命中 | 返回包含匹配词和截断标记，结果小于 70K 字符 |
| Rust tool unit | 多文件多命中 | 500 条或 64K 预算生效且有截断标记 |
| Agent-loop unit | 两用户回合 + 最近 2.30M tool result | prompt 估算小于等于 272K，头尾与调用 ID 保留 |
| Agent-loop regression | 旧半区 assistant/tool 与多回合历史 | 原有压缩、回合骨架和 overflow detector 不回归 |
| Isolated DB/App | 生产历史等比例副本 | 旧原文不变，新回合不再 `CONTEXT_OVERFLOW` |
| Release App | 公开安装包 | 精确版本、真实 ChatGPT 路由和恢复回合成功 |

## 完成边界

单测、构建、PR 或合并都不是完成。只有公开版本安装后，生产形状的旧会话副本能在不改写原历史的前提下完成新回合，且公开 updater 元数据可用，才可标记 `live`。
