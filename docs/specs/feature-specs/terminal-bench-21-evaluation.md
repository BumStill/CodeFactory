# Terminal-Bench 2.1 能力评估规格

## 范围

本规格定义 CodeFactory 以 Terminal-Bench 2.1 为产品能力评估目标的首期能力。首期目标不是冲榜，而是建立可复现运行、结果导入、失败分类和产品改进闭环。

相关设计：

- `docs/principles/systematic-agent-evaluation.md`
- `docs/design/terminal-bench-21-business-design.md`
- `docs/design/terminal-bench-21-architecture-design.md`
- `docs/design/terminal-bench-21-ux-design.md`

## Requirements Traceability

| Req ID | User request | Normalized requirement | Surfaces | Validation method | Owner |
| --- | --- | --- | --- | --- | --- |
| CF-TB-R1 | 瞄准 Terminal-Bench 2.1 评估能力 | 仓库内有 Terminal-Bench 2.1 业务、架构、UX 和规格文档，明确官方约束和产品目标 | docs | 文档审查 + governance baseline | planning |
| CF-TB-R2 | 评估我们的能力 | CodeFactory 能保存 benchmark run、trial、reward、artifact 和 build 信息 | backend + sqlite + UI | fake Harbor job 导入测试 + UI summary |
| CF-TB-R3 | 不能只看总分 | 系统生成 capability profile 和 failure taxonomy | backend + UI | fixture run 分类断言 |
| CF-TB-R4 | 能被 Terminal-Bench 2.1 跑 | 提供 Harbor custom agent adapter、显式 env 驱动的 model-backed headless runner，以及从当前 CodeFactory provider 到 benchmark env 的显式授权桥接 | adapter + agent loop + backend command | Python adapter smoke + Harbor CodeFactory baseline run + fake model headless runner integration test + provider bridge unit test + real model smoke |
| CF-TB-R5 | 改进后能回归 | 支持同一 subset 的 baseline/head run 对比 | backend + UI | compare run fixture test |
| CF-TB-R6 | 保持可审计和安全 | benchmark policy 只在 Harbor sandbox 生效，不污染普通项目权限和长期 memory | permission + memory + audit | policy unit test + memory write guard |
| CF-TB-R7 | 区分 agent 评估和模型评估 | 所有 run、PR、证据包和 UI 都必须声明 evaluation axis、evaluation subject、fixed variables、changed variables 和 result attribution | docs + backend + UI | spec review + fixture attribution test |
| CF-TB-R8 | 形成产品能力迭代 loop | 每轮 agent 能力改动必须声明 hypothesis、target failure class、评估 scope，并生成 baseline/head/delta/next queue 的 iteration report | benchmark runner + docs + PR evidence | iteration loop dry-run test + real subset evidence |
| CF-TB-R9 | 防止 task-specific 跑分污染 | adapter 不得读取 hidden verifier/solution，也不得按 task name、固定 repo、artifact、领域答案或 instruction fingerprint 注入专用 hint/auto-repair | adapter + CI | contamination scanner + source review |
| CF-TB-R10 | 主产品与 headless 共享完成语义 | Rust AgentLoop 与 Rust headless sidecar 调用同一 `codefactory-agent-core` policy/completion gate，并加载同一 `agent_contracts/execution_completion.md`；环境枚举、版本打印、路径输出等 inspection 不得冒充 post-change verification；Python 仅为 Harbor JSONL bridge，run metadata 保存 contract SHA-256 | agent core + desktop loop + sidecar + adapter + run ledger | core tests + protocol tests + contract hash assertion + 真实 App |
| CF-TB-R11 | 污染 run 不进入能力基线 | contamination scan、contract hash 或 runtime subject 缺失时，run 只能标记 `benchmark-contaminated diagnostic`，不得用于 release gate、历史最好分或外部水平结论 | importer + UI + evidence | attribution validation test + UI state |
| CF-TB-R12 | 固定 18 题与全量门禁 | 固定 18 题最终门禁为同一发布版本 `18 / 18`；首个恢复发布允许 `>=16 / 18`，但必须对该轮有效历史通过集零回退。其后补齐 clean Linux/x86 复评与完整 89 题评估 | runner + release ledger + CI | fixed-subset aggregate + pass-set diff + clean Linux/x86 job + full 89 report |
| CF-TB-R13 | 依赖获取遵守真实环境能力 | Headless bridge 继承 Harbor 已生效的网络策略；`public` 与 `allowlist` 会如实告知共享 Agent core 可获取任务所需依赖，`no-network` 保持拒绝，具体 host 边界继续由 Harbor 强制执行并写入 run metadata | adapter + agent core + sidecar + run ledger | Python policy inheritance test + Rust prompt/policy tests + real dependency task |
| CF-TB-R14 | 长任务按真实墙钟预算收敛 | Harbor/任务调度器的有效 Agent 时限必须通过 thin bridge 传入 Rust headless；Agent 在剩余三分之二时开始收敛、剩余三分之一时禁止扩 scope，并为最终完成消息保留至少 30 秒。模型重试拥有独立单次 timeout 但同时受 Agent 全局 wall deadline 裁剪，工具 timeout 也不能通过重试叠加突破外部预算 | adapter + agent core + sidecar | Rust time-budget tests + Python budget bridge test + real long-horizon trajectory |
| CF-TB-R15 | 源码交付使用结构化完成证据 | 当需求明确要求 source build/install 时，共享 completion gate 必须记录最后源码修改、成功源码安装、安装后离开源码目录的 runtime/import smoke 和项目验证；任一阶段缺失都不得结束。源码兼容性扫描允许输出摘要，但必须覆盖构建输入并以严格非零退出表达残留命中 | agent core + desktop loop + sidecar | shared core tests + real source-build canary + real App source task |
| CF-TB-R16 | 源码交付阶段按失败证据推进 | 到达交付检查点后，源码修改、安装、源码目录外运行、项目测试必须按顺序推进；成功安装后不得继续猜测性安装依赖，只有紧邻的真实运行或测试失败才能打开依赖恢复与一次诊断/修复循环 | agent core + sidecar | stage-policy tests + real source-build trajectory |
| CF-TB-R17 | 明确要求的项目测试是完成条件 | 当原始需求明确要求 repository/project tests 时，只有最后源码修改、安装和外部运行之后的成功项目测试才能解锁完成；测试运行器缺失、失败测试或被管道掩盖的零退出均不得进入 finalization | agent core + desktop loop + sidecar | completion-evidence tests + real App + canary |
| CF-TB-R18 | 长输出保留可诊断头尾并压缩上下文 | Headless 对编译、安装和测试输出保留命令开头证据与结果/错误尾部，中段压缩；模型上下文达到预算后保留 contract、原始任务和最近完整工具轮次，避免长日志反复进入 provider 请求 | headless sidecar | truncation/compaction tests + usage delta |
| CF-TB-R19 | 确定性改进必须先交付再继续调分 | 每个已通过独立测试的通用能力切片必须先完成真实 App 验证、PR/CI、合并和适用版本发布，再用该发布版本复评；不得连续堆积多轮本地评分改动后才发布，也不得把“位于主产品源码”称为“已产品化” | delivery loop + release ledger + evidence | PR/CI/release URL + installed build SHA/version + released-build rerun |
| CF-TB-R20 | 复合命令中的源码修改不能因后续阶段失败而丢失 | 只要已获准的工具调用包含明确源码/文件修改，后续 build、install、runtime 任一阶段失败时仍记录最后源码修改 sequence，并使旧安装、运行和测试证据失效；纯依赖安装和被 policy 拒绝的命令不得误记为源码修改 | agent core + desktop loop + sidecar | failure-first completion-evidence tests + real App mixed-command task |
| CF-TB-R21 | 昂贵重建前完成别名感知兼容扫描 | 源码兼容任务必须从仓库推导全部本地 import alias，覆盖构建配置观察到的源码/生成/编译输入扩展，批量完成修改并以 clean residual scan 结束；存在残留扫描 blocker 时只允许仓库级 alias discovery、纠正性修改和最终 clean scan，不得进入下一次 build/install，避免部分扫描后反复重建耗尽预算 | shared contract + agent core + desktop loop + sidecar | budget-policy tests + real App compatibility task + released-build source canary |
| CF-TB-R22 | 中文源码交付要求启用同一完成门禁 | CodeFactory 中文主路径中的“兼容/已移除/弃用/源码迁移”“从源码安装/源码构建/编译扩展”“项目测试/测试套件”等表达必须启用与英文任务相同的 compatibility、source delivery 和 project-test gates；不得因任务语言不同降级完成标准 | agent core + desktop loop + sidecar | Chinese instruction gate test + real Chinese App task |
| CF-TB-R23 | 兼容扫描按失败成员收敛并消除 shell 假失败 | Agent 必须从真实 build/runtime/test failure 提取 exact API member，覆盖所有已发现 alias，并只从仓库引用或语言适配器扩展候选拼写；clean scan 标准路径写临时结果文件，保留 `grep`/`rg` 状态，只接受 `0/1`，再以 `test ! -s` 收口，或使用等价结构化退出。脆弱 command substitution、`search && test`、掩盖搜索错误和“修改 + build/install”复合绕过均必须被拒绝；输出声明 zero residual 但命令非零时，completion evidence 必须记录确定性 shell-exit recovery blocker | shared contract + agent core + desktop loop + sidecar | failure-first policy/evidence/prompt tests + non-benchmark App compatibility fixture + released-build canary |
| CF-TB-R24 | endpoint 选择必须贯穿全部 AI 执行路径 | 任务拆解、规范辅助、后台子任务、验收检查和会话后学习不得直接读取可能过期的全局 `default_model`；它们必须按 `default_endpoint` 的 `active_model` 解析一次，并将同一模型写入子会话、执行 Agent 与验收调用，同时按 endpoint `api_style` 构造 URL、认证头和请求/响应格式。没有可用模型或 helper 不支持所选 transport 时应在网络调用前返回可行动错误，不得静默回退到其他 provider 的模型，也不得把 Anthropic/ChatGPT 请求伪装成 OpenAI `chat/completions` | settings + specs + subagent + learning | endpoint/model resolution tests + transport-shape tests + real App parent/child session evidence |
| CF-TB-R25 | 默认文件发现必须控制依赖与构建噪声 | `glob` 在工作区根扫描时默认跳过 `.git`、`.venv`、`venv`、`node_modules`、`target`、`__pycache__`，避免把依赖树和生成物灌入模型上下文；用户或 Agent 显式把被忽略目录设为搜索根时必须允许进入，不得使依赖诊断失去入口 | desktop tools | failure-first glob tests + real App context evidence |
| CF-TB-R26 | 确定性完成约束必须覆盖普通主聊天 | 普通 `Interactive` 主聊天中模型生成的工具调用必须执行与自主任务相同的兼容扫描等确定性 completion invariant，并在模型试图给出最终答复时再次检查完成证据；交互模式仍不受自主任务剩余轮次预算限制。桌面文件工具必须把实际 `path` 以及搜索工具的 `pattern` 写入完成证据，使源码扩展、修改 sequence 和 alias discovery 可被识别。blocker 激活后允许读取相关源码和执行导入别名搜索，但继续阻止无关探索、build/install 和脆弱扫描；标准临时文件与状态归一化扫描必须放行 | desktop loop + shared core | failure-first finalization/routing/path-pattern evidence tests + packaged App edge-path acceptance |
| CF-TB-R27 | 工具拒绝结果必须形成可重放会话历史 | completion policy、权限策略、用户拒绝或 hook 取消产生的每个 assistant `tool_call` 都必须持久化对应 tool result；OpenAI-compatible 与 Anthropic 历史重放前必须修复既有数据库中缺失的结果并丢弃孤立 tool result，避免下一轮因 provider 协议不完整返回 `400`。修复只补协议占位，不得伪造工具成功证据或修改工作区 | desktop loop + session persistence | DB persistence test + history-repair test + same-session packaged App recovery |
| CF-TB-R28 | 干净工作区可复现构建评测 Agent | 固定 subset 的标准 runner 在 Harbor 启动前必须复用显式指定且可执行的 headless binary，或从当前 checkout 的 `src-tauri` workspace 构建 `codefactory-agent-headless`；构建产物缺失、不可执行或构建失败必须作为 preflight blocker 退出，不能生成能力分数。正式证据必须记录 binary 绝对路径、来源和 SHA-256 | benchmark runner + headless sidecar + evidence | failure-first runner tests + clean-checkout launch + evidence metadata |
| CF-TB-R29 | iteration delta 对不可比分数 fail-closed | iteration report 只有在 baseline 与 head 都明确声明 `official_comparable: yes`，具有真实 run、正数 completed trials、pass count、mean reward 且 trial 数相同时才能计算 pass/reward delta；`task_count` 只能表示计划范围，不能替代实际 trials。任一证据为 `no`、缺失评分字段或 trial 数不同，都必须输出 `comparable_delta: no` 和具体原因，不能进入 release gate 或回退结论 | iteration loop + evidence | non-comparable/incomplete head regression tests + report inspection |
| CF-TB-R30 | Docker job root 必须双向可见 | provider token 使用前，标准 runner 必须在当前 checkout 的 job root 完成 host→container 读取和 container→host 写回的双向 bind-mount probe；任一方向失败必须作为 preflight blocker 退出，并提示迁移到 Docker 可共享的持久项目目录。`/private/tmp` 等仅在容器内形成孤立 mount 的路径不能用于正式评测 | benchmark runner + Docker/Harbor + evidence | failure-first mount tests + bad-path real probe + persistent-path real probe |
| CF-TB-R31 | runner 无导入结果时不得声明可比 | provider bridge/cargo runner 只要非零退出或没有导入 Harbor run，证据必须标记 `official_comparable: no` 并记录退出/no-import 原因；编译失败、进程超时和启动前异常均不得进入能力分数 | benchmark runner + evidence | failure-first no-import/nonzero report tests |
| CF-TB-R32 | 后台服务启动语义覆盖复合命令和 daemon 参数 | 共享 Agent core 必须优先识别引号/转义之外任意位置的单 `&`、`-daemonize`、`--daemon` 等后台启动语义，同时排除 `&&`、`2>&1`、`&>`、`|&` 和字符串字面量；同一次工具调用附带 readiness probe 不能把整个调用降级为普通 functional probe。后台服务仍须在后续调用提供 PID、日志和有界功能探测后才能完成 | agent core + desktop loop + sidecar | classifier tests + non-benchmark service acceptance + service-task canary |
| CF-TB-R33 | 模型 transport 重试保留独立尝试窗口 | 可重试的模型 transport 错误必须为后续尝试保留完整的单次 timeout，而不是让第一次慢请求耗尽一个共享请求 timeout；每次尝试仍受 Agent 全局墙钟 deadline 和最终 30 秒保留区约束 | headless sidecar | slow-first/recovered-second transport test + provider canary |
| CF-TB-R34 | 结构化 shell 断言必须算作验证 | 使用临时结果文件、状态变量和最终 `exit $status` 的只读断言必须进入 Verification evidence；不得仅因 `/tmp` 重定向或变量赋值误判为 Mutation，导致 Agent 重复扫描直至耗尽预算。同时，结构化退出不得掩盖对工作区路径的真实重定向写入；`> file`、`>file`、`1>>file`、`2>file` 等合法写法都必须分类为 Mutation，同时排除引号内文本和 `2>&1` 等 fd duplication | agent core + desktop loop + sidecar | positive/negative classifier tests + non-benchmark residual-scan acceptance |
| CF-TB-R35 | 完成前执行受约束 diff scope 审计 | 最终回复前必须把每个修改路径映射到原始需求；任务明确限制范围时，Agent 应撤销本轮产生的无关改动，同时不得触碰会话开始前已存在的用户修改 | shared contract + agent core | prompt/contract tests + constrained-change acceptance |
| CF-TB-R36 | 明确源码修复不得以反复安装代替编辑 | 原始任务使用 modify/update/patch/change/修改/修复/更新等表述要求源码改动并从源码构建时，completion gate 必须要求至少一次真实且成功的源码内容写入；失败的单一或复合编辑命令、以及 `touch/mkdir/rm/cp` 等元数据操作都不得解锁该门禁。由于整体退出码不能证明 `edit && build` 失败发生在哪个阶段，Agent 必须拆出一次独立成功编辑结果后再进入 build/install。到达 source-delivery checkpoint 后，在最新失败诊断和源码编辑发生前拒绝新的 build/install 或范围扩张 | agent core + desktop loop + sidecar | failure-first source-convergence tests + source-build canary |
| CF-TB-R37 | 工具进程树和输出管道必须有界 | 桌面 bash 和 Harbor thin bridge 对每次工具调用建立独立进程组；timeout、取消或 transport 异常必须终止并回收该调用的所有后代，不能遗留持续占用 CPU、端口或输出管道的孤儿进程。Harbor 包装不得把原有 Bash 命令降级到 POSIX sh。父 shell 正常退出而后台服务仍继承一个或两个 stdout/stderr 管道时，输出采集必须并发、有界返回并保留已经完成一侧的输出，但不得误杀该正常后台服务 | desktop tool runtime + Harbor bridge | descendant timeout tests + single/dual inherited-pipe tests + real Linux Bash/process-group CI test + process inspection acceptance |
| CF-TB-R38 | 精确依赖约束必须通过真实执行证明 | 用户指定 tool/library/model/version/revision 时，Agent 的实现和验证必须实际走该命名依赖；仅 import 命名包、实际通过相邻底层依赖完成工作不算符合要求 | shared contract + agent core | contract tests + exact-dependency acceptance |
| CF-TB-R39 | 状态改变必须验证前后可观察差异 | 键盘、控制 socket、设备或服务控制等状态改变不能以命令已接受作为完成证据；必须捕获并断言请求动作前后的可观察状态确有目标变化 | shared contract + agent core | contract tests + state-change acceptance |
| CF-TB-R40 | 长任务必须在最后三分之一前产出首个必需 artifact | 原始需求指定必需输出文件或 artifact 时，Agent 必须在剩余最后三分之一前创建首个候选，后续预算用于验证和修复；不得让调研、依赖安装或重复检查占满预算而最终没有交付物 | shared contract + agent core | convergence prompt tests + required-artifact canary |
| CF-TB-R41 | 零退出显式失败和失败启动不得解锁完成 | stdout/stderr 明确出现 `failed:`、`FAIL:`、`No such file or directory`、`Process dead/not running` 等失败证据时，即使 shell 被 `|| true`、末尾 `echo` 或管道归零，也必须记录 semantic failure。只有整条命令是纯文件不存在或进程已停止断言时，对应的预期负向状态可以豁免；混入其他命令、管道或控制操作时保持 fail-closed，不能全局删除相似错误。任何后台服务启动尝试即使 timeout/非零，也必须激活 service lifecycle gate，直到后续成功 PID、日志和 bounded functional probe 齐全 | agent core + desktop loop + sidecar | failure-first semantic-output/expected-absence/service-attempt tests + locked non-benchmark runtime acceptance |
| CF-TB-R42 | literal data heredoc 源码不得污染 shell 生命周期分类 | 共享 Agent core 在识别后台 `&`、重定向、daemon 和 functional probe 前，只能剔除 complete、expansion-disabled、direct standard `cat/tee` data heredoc payload；C/C++ 地址运算符等源码内容不得触发后台服务门禁。Executable、unquoted、piped、process-substituted、custom/redefined command、malformed delimiter 或 unclosed heredoc 必须 fail-closed，heredoc 结束后的真实后台命令仍必须识别 | agent core + desktop loop + sidecar | failure-first quoted/strip-tabs/following-background/custom/redefined/executable/malformed classifier tests + non-benchmark source-generation acceptance |
| CF-TB-R43 | 明确要求的用户可见状态必须由运行探针观测 | 原始任务使用肯定式 `expect to see`、`wait until` 声明用户可见状态时，共享 completion gate 必须保守提取状态标记，并要求它在最后修改、失败或服务启动之后的成功 RuntimeProbe 或 bounded FunctionalProbe 输出中以肯定状态出现；ReadOnly 文本、普通 test 名称、否定输出、PID、端口、transport connection 和 command acknowledgement 均不能解锁完成 | agent core + desktop loop + sidecar | failure-first requested-state probe/test/read-only/negative-output tests + non-benchmark acceptance |
| CF-TB-R44 | 长任务修改后仍须从检查回到行动 | 共享 `ProgressTracker` 的有限检查窗口必须在每次 mutation 或 functional probe 后重新开始；首次 mutation 不得永久关闭检查预算，失败的 ReadOnly/RuntimeProbe 也消耗窗口而不能重置计数。窗口耗尽后，桌面主 Agent收到收敛提示，headless 拒绝新的纯 ReadOnly 调用，直到执行最小纠正性 mutation 或 bounded functional probe；RuntimeProbe 仍可用于用户可见状态，但不能让重复源码读取无限继续 | agent core + desktop loop + sidecar | failure-first post-mutation/failed-read window tests + headless tests + non-benchmark long-task acceptance |
| CF-TB-R45 | 明确输出行为必须由机器断言验证 | 原始任务明确规定 expected output、return value、应输出或应返回的行为时，最后修改或失败之后必须有能在不一致时非零退出的 `assert`、shell `test`/`diff`、真实测试框架或专用 verifier；只打印 expected/actual、编译成功、普通零退出 runtime、失败退出码被后续命令吞掉的检查，或与 workspace 写入复合在同一 action 的断言不得解锁完成。Executable interpreter/shell heredoc 作为 opaque action 按 Mutation fail-closed，其 payload 内的 `test = ...` 等语言标识不能误判为 shell Verification；inline interpreter 的常见文件写 API 也必须记录 Mutation，后续必须另行执行机器断言 | agent core + desktop loop + sidecar | failure-first printed-output/executable-heredoc/compound-write/inline-write classifier tests + headless tests + non-benchmark expected-output acceptance |
| CF-TB-R46 | 长任务必须从瞬时 provider 响应体损坏中恢复并保留用量 | 所有主 Agent provider 请求必须通过共享 HTTP retry helper 显式协商未压缩响应，避免本地代理/链路压缩解码故障；headless 长任务在已有工具进度后必须允许含首发在内至少 5 次、受 wall deadline 约束的 response-body 总尝试。每个 tool request 必须携带当时累计 usage snapshot；成功但未产生工具调用且尚不能完成的响应必须另发 usage snapshot event。嵌入方即使在后续 provider fatal error 时也能持久化已成功模型请求的 token/request 用量，不得回落为伪 `0` | desktop Agent HTTP helper + sidecar + product acceptance/Harbor bridge | failure-first repeated-truncation/identity-header tests + protocol/fatal-error usage snapshot tests + non-benchmark transient-provider acceptance |

## Primary User Path

P-TB-1: 用户打开 CodeFactory 的 `Benchmarks / Terminal-Bench 2.1` 页面。系统检查 Harbor、Docker 和 CodeFactory agent adapter 状态。用户启动 smoke run 前，系统基于当前 endpoint/model 生成 provider bridge preview，展示不可修改的官方 dataset `terminal-bench/terminal-bench-2-1`、agent/model、policy preset、artifact path、redacted env 和命令 preview。用户必须确认授权短语后，后端才从 OS credential store 读取当前 endpoint key，并只把它临时注入本次 Harbor child process env。run 完成后，CodeFactory 导入 Harbor job 目录，展示 reward、trial 列表、verifier 输出、trajectory 和 failure class。用户选择失败类别，创建后续产品改进 slice，并能用同一 subset 在修复后回归对比。

## 开发内嵌评估节奏

Terminal-Bench 2.1 不是发版前偶尔运行的榜单检查，而是 CodeFactory 能力开发的反馈系统。所有面向 agent 能力的非平凡 PR 都必须声明它预计改善哪类 benchmark failure，并选择对应评估层级。

| 阶段 | 何时运行 | 评估范围 | 必须回答的问题 | 产物 |
| --- | --- | --- | --- | --- |
| Baseline | Terminal-Bench 2.1 支持落地后、每个 release baseline 或重大 agent loop 改动前 | 当前 `main` 或 release build 的 smoke/subset | CodeFactory 现在主要输在哪类能力？ | baseline run、failure taxonomy、artifact refs |
| PR planning | PR 开发前 | 不运行或导入已有失败集 | 这个 PR 预计改善 planning/context/tool-use/verification/policy/environment 中哪一类？ | PR 假设、目标 subset |
| Inner loop smoke | adapter、runner、policy、importer、agent loop 改动中 | 1 到 5 个 task 或 fake Harbor fixture | Harbor -> CodeFactory -> verifier -> import 链路有没有断？ | smoke job、import result |
| Targeted subset | 能力 PR 合并前 | 5 到 20 个历史失败同类 task | 原目标失败是否改善？是否转移成其他失败类型？ | baseline/head 对比 |
| Regression subset | 触碰共享 agent loop、tool runtime、context builder、permission、verification 时 | 固定代表性 subset | 核心能力有没有退化？cost/latency 有没有恶化？ | regression report |
| Main scheduled | `main` 定期运行，默认每日或每周 | 固定 subset + rotating subset | 多个 PR 叠加后的真实趋势是什么？ | trend snapshot、failure queue |
| Release candidate | 发版候选或 leaderboard 相关准备 | 更大 subset，必要时接近完整 Terminal-Bench 2.1 | 相比上个 release 是否可接受？是否满足 comparable 约束？ | release evidence pack |

合并标准不是“分数一定上涨”，而是必须解释变化：

- reward delta、pass/fail delta、cost/duration delta。
- 原失败 task 是否改善。
- 是否新增 regression。
- failure class 是否从一种产品问题转移成另一种。
- 如果没跑对应 subset，PR 必须说明 blocker 和替代证据。

PR 描述必须包含：

- `Evaluation axis`: `codefactory-agent-capability`、`model-backend-ablation`、`agent-scaffold-comparison` 或 `evaluation-infrastructure-smoke`。
- `Evaluation subject`: 被评价对象；默认是 `codefactory-headless` 或具体 CodeFactory agent。
- `Fixed variables`: 为了归因而固定的 benchmark subset、model backend、policy、runner、build 等变量。
- `Changed variables`: 本 PR 或本次实验实际改变的 build、model、adapter、policy 或 runner。
- `Result attribution`: 结论归属给 CodeFactory agent、model backend、agent scaffold 还是 evaluation infrastructure。
- `Benchmark hypothesis`: 本 PR 预计改善的 failure class。
- `Benchmark scope`: smoke、targeted subset、regression subset、full 或 not run。
- `Baseline`: 对比基线 run id 或明确 `not available`。
- `Result`: reward/failure/cost 变化和 artifact path。
- `Interpretation`: 为什么可以合并，或为什么只能作为实验合并。

## 系统化评估矩阵

本规格继承 `docs/principles/systematic-agent-evaluation.md`。Terminal-Bench 结果默认是 agent 系统结果，不是单独的模型结果。

| Evaluation axis | 固定什么 | 变化什么 | 允许结论 | 禁止结论 |
| --- | --- | --- | --- | --- |
| `codefactory-agent-capability` | Terminal-Bench subset、model backend、policy、runner | CodeFactory build、agent loop、context/tool/policy 实现 | CodeFactory agent 能力变化 | 某模型独立能力排名 |
| `model-backend-ablation` | CodeFactory build、agent adapter、subset、policy、runner | provider/model | 模型作为 CodeFactory 组件的影响 | CodeFactory 产品能力整体提升 |
| `agent-scaffold-comparison` | provider/model、subset、runner | CodeFactory adapter、simple baseline、oracle 或其他 scaffold | agent scaffold / 产品机制强弱 | provider/model 本身优劣 |
| `evaluation-infrastructure-smoke` | oracle 或 no-model diagnostic、runner | Harbor、Docker、importer、schema、UI | 评测基础设施是否打通 | CodeFactory agent 已具备任务能力 |

命名规则：

- 正确：`CodeFactory agent using DeepSeek`、`agent=codefactory-headless model_backend=DeepSeek`。
- 错误：`DeepSeek 跑出了 CodeFactory 的 Terminal-Bench 结果`。
- 第一次有效 CodeFactory 能力结果必须满足 `evaluation_axis=codefactory-agent-capability` 且 `agent_name=codefactory-headless`。

## Applicable Harnesses

- Spec Harness: 本规格、Req ID、主路径、测试矩阵和证据要求必须存在。
- Compatibility Harness: 新表、新 settings、新 agent adapter 不得破坏旧 session、tool runtime、permissions、memory。
- Observation Harness: run/trial/artifact/reward/failure classification 必须可审计。
- Payload Harness: Harbor artifacts、trajectory、verifier output、result JSON 都是 payload，导入和导出必须脱敏并记录来源。
- AI Collaboration Harness: 失败分类和改进建议必须记录 assumptions、review point、validation result。
- Release Harness: 如果 benchmark runner 进入安装包或公开 release，必须验证真实 packaged app/headless runner。

## 数据契约

### Benchmark Profile

| Field | Terminal-Bench 2.1 value |
| --- | --- |
| `id` | `terminal-bench-2.1` |
| `dataset` | `terminal-bench/terminal-bench-2-1` |
| `harness` | `harbor` |
| `official_url` | `https://www.tbench.ai/docs/run-terminal-bench-2-1` |
| `leaderboard_url` | `https://www.tbench.ai/leaderboard/terminal-bench/2.1` |
| `comparable_constraints` | no timeout/resource changes, dataset fixed, agent/model/build recorded |

### Harbor Command Semantics

当前本地验证使用 Harbor 0.15.0。命令语义必须按实际 CLI 处理：

- `-d` / `--dataset`: 选择数据集，例如 `terminal-bench/terminal-bench-2-1`。
- `-l` / `--n-tasks`: 限制 task 数量，smoke run 默认用 1 到 5。
- `-k` / `--n-attempts`: 每个 trial 的 attempts，不得误用为 task 数量。
- `--agent-import-path`: 自定义 CodeFactory agent adapter 的 import path。
- `-a oracle`: 只用于验证 Harbor/Docker/dataset/verifier/import 链路，不代表 CodeFactory agent 能力。

Runner 默认不得修改 Harbor 的 task、agent、verifier timeout 或 resource。`agent wall timeout`、trial watchdog、heavy-verifier watchdog、verifier timeout multiplier 和 storage override 均为显式诊断选项；任一启用后，报告必须自动标记 `official_comparable=no` 并列出干预原因，不能进入发布门禁分数。

当前已验证的 CodeFactory import path 是 `codefactory_bench.agent:CodeFactoryAgent`。历史首个真实 run 使用 `codefactory-headless-baseline` / `baseline-no-model`，只证明 Harbor 能运行 CodeFactory-owned adapter 并把结果导回 CodeFactory；不得把该 0 分 baseline 声明为完整 CodeFactory agent 能力。

当前 adapter 名称为 `codefactory-headless`，支持两种模式：

- `baseline-no-model`: 未提供显式 benchmark model env 时，只跑 sandbox 诊断和导入链路。
- `model-backed`: 提供 `CODEFACTORY_BENCH_API_KEY` 且提供 `CODEFACTORY_BENCH_MODEL` 或 Harbor `-m <model>` 时，调用 OpenAI-compatible chat-completions 接口，通过 `run_shell` 工具在 Harbor task container 内执行。

Model-backed 模式只能读取显式 `CODEFACTORY_BENCH_*` 配置，不读取 CodeFactory desktop settings、macOS keychain、通用 provider env 或用户凭据。

Model-backed loop 必须把 provider 临时错误和 task 能力失败分开。对 `408`、`409`、`429`、`500`、`502`、`503`、`504` 这类 transient chat-completions HTTP 状态，adapter 默认必须重试后再判定失败；重试次数和延迟由 `CODEFACTORY_BENCH_MODEL_HTTP_RETRIES` / `CODEFACTORY_BENCH_MODEL_HTTP_RETRY_DELAY_SEC` 控制。单次 DeepSeek/OpenAI-compatible `HTTP 500` 不得直接让一个历史通过 task 变成 agent exception。

Model-backed loop 必须把 task container 内的 `environment.exec` 异常记录成 trajectory 中的 `exec-error` tool result，而不是让整个 trial 直接变成 Harbor agent exception。至少记录：

- `status=exec-error`
- `error_type`: `command-timeout`、`environment-exec-error` 或 `exec-runtime-error`
- `timeout_sec`
- 原始 command 的单行摘要
- `context.metadata.exec_errors` 和 `context.metadata.command_timeouts`

对于自检命令返回非零且 stdout/stderr 包含 pytest failure、traceback 或 assertion 失败时，adapter 必须追加 verifier-repair 提示，要求模型基于失败断言修改实现并重跑最小失败检查后再结束。

对于 shell `return_code=0` 但 stdout/stderr 已经包含明确失败文本的情况，adapter 不得把管道退出码当作成功。常见例子包括 `tee`、`tail`、`head` 等管道隐藏了上游命令失败：`timeout: failed to run command`、shell 报 `No such file or directory` / `not found`、`make: *** Error <code>`、apt/dpkg lock failure、`Failed to fetch`。这类 tool result 必须标记为 `semantic-failure`，写入结构化 repair goal，并提示模型用 `set -o pipefail` 或更小的直接命令重跑后再修复。

对于 artifact、编译扩展、服务或库 API 任务，Agent 只能从用户原始需求、仓库公开源码、构建配置和实际错误中推导实现，不得由 adapter 注入 task family、固定仓库、领域算法、答案 scaffold 或预期 marker。完成前必须把每个明确点名的组件或行为映射到真实功能检查；文件存在、import 成功、编译成功或单一 happy path 不能替代行为验收。源码构建还必须枚举构建配置引用的实际输入，包括生成源码和编译源码，避免只扫描熟悉的文件后缀。

任何 score-facing canary 通过后，都不得直接声明本轮产品能力已经改进完成。系统必须执行固定 18 题 regression subset 或生成明确 blocker evidence，并比较上一轮 fixed subset 的 pass count、mean reward、历史通过项和 failure class 变化。如果 aggregate 没有提升、历史通过项回落，或 canary pass 在 aggregate 中被 verifier/runtime 问题掩盖，本轮只能标记为 `targeted canary pass, aggregate not held`，下一轮 P0 必须先处理 score-holding regression 或 runtime classification。

评测基础设施必须区分 verifier/runtime failure 和 artifact assertion failure。若 verifier 在真正比较目标产物前失败，例如 `gcc internal compiler error`、Chrome driver unavailable、QEMU/proc/netlink limitation、verifier watchdog stop 后缺失 `reward.txt` / `reward.json`，evidence pack 必须记录 `artifact_state`、`runtime_failure_evidence` 和 `score_interpretation`。这不改变 Terminal-Bench reward，但它决定下一轮是修 CodeFactory agent 能力、修 runner/preflight，还是重跑确认本机 runtime 偶发性。

对于明显的前台服务启动命令，例如 `python -m http.server`、`uvicorn`、`flask run`、`npm start`、`redis-server` 等，adapter 必须要求后台启动、日志重定向、pid 记录和 bounded readiness check；不得直接执行会常驻到 tool timeout 的前台服务命令。已显式后台化、`nohup`、`setsid`、`timeout` 或 daemon 模式的命令不在该拦截范围内。

Benchmark bridge 不得用固定布尔值覆盖评测环境能力。它必须继承 Harbor 已生效的网络策略：`public` 允许任务所需的依赖或源码获取；`allowlist` 允许 Agent 发起命令，但 host 白名单仍由 Harbor 环境强制执行；`no-network` 只允许 loopback 自检，例如 `curl http://localhost:<port>`、`curl http://127.0.0.1:<port>`、`nc -z localhost <port>`。系统提示、命令 policy 和 run metadata 必须反映同一个有效策略。桌面主路径继续使用用户权限审批与 shell safety policy，不继承 benchmark sandbox 的权限捷径。

当 runner 或 adapter 为 Docker task container 注入 `HTTP_PROXY`、`HTTPS_PROXY`、`ALL_PROXY` 或小写等价变量时，必须同时注入 `NO_PROXY` / `no_proxy`，至少覆盖 `localhost,127.0.0.1,127.0.0.0/8,::1,0.0.0.0`，保证容器内服务自检不会通过宿主机代理返回假阳性或 `502`。

### Provider Bridge

产品侧允许用户把当前 CodeFactory endpoint/model 用于一次 benchmark run，但必须经过显式授权桥接：

- `preview_benchmark_provider_bridge(request)` 只读取 settings 中的 endpoint/model/key_ref，返回 redacted env、command preview、job path 和授权短语；不得读取或返回 raw API key。
- `start_benchmark_provider_run(request)` 只有在授权短语完全匹配时才读取 OS credential store，并把 key 作为 `CODEFACTORY_BENCH_API_KEY` 注入 Harbor child process env。
- raw key 不写入 command preview、frontend state、SQLite run record、Harbor args、日志或 evidence pack。
- provider key lookup 必须有有界超时；如果 OS credential store 需要交互授权或挂起，run 必须返回明确 blocker，不得无限等待或误报为 agent 失败。
- 如果调用进程已经显式提供 `CODEFACTORY_BENCH_API_KEY`，provider bridge 可以把它作为本次 benchmark launch 的授权 secret 来源并跳过 OS credential lookup；该值仍然不得进入 preview、日志、SQLite、Harbor args 或 evidence pack。
- 当前 bridge 只支持 OpenAI-compatible `chat/completions` endpoint；DeepSeek 这类 direct provider 需要用 `normalize_model_id` 去掉 OpenRouter vendor 前缀。
- ChatGPT OAuth、Anthropic 原生 Messages API、需要浏览器会话或非 API key 的 provider 暂不支持 benchmark bridge。
- `concurrency` 是 Harbor `-n` / `--n-concurrent`，不是 trial count；`trial_count` 只作为旧客户端兼容 alias。
- `task_names` 使用 Harbor `--include-task-name` 过滤固定 subset；当提供 `task_names` 且未显式提供 `task_limit` 时，默认 `task_limit=task_names.length`，避免固定 subset 被默认 smoke limit 截断。

### Regression Subset

首个固定回归子集为 `docs/benchmark-subsets/terminal-bench-21-regression-subset-v1.json`。

该子集来自第一次完整 CodeFactory Terminal-Bench 2.1 run `7ff6ef13-4488-4e0f-afd0-a1f9bd16d561`，包含 18 个任务，覆盖：

- passed smoke: `write-compressor`, `extract-elf`, `filter-js-from-html`, `nginx-request-logging`
- verifier-zero: `circuit-fibsqrt`, `configure-git-webserver`, `mteb-retrieve`, `sanitize-git-repo`, `query-optimize`
- tool-use: `count-dataset-tokens`, `install-windows-3.11`, `protein-assembly`
- command-timeout: `build-cython-ext`, `kv-store-grpc`, `sparql-university`, `torch-tensor-parallelism`
- environment/resource: `caffe-cifar-10`, `qemu-startup`

后续 agent-loop、tool runtime、verification repair、resource/preflight 改动默认至少跑该 subset 或说明 blocker。

固定 subset 的标准执行入口是 `tools/benchmark/run_terminal_bench_21_regression_subset.py`。该脚本从 subset JSON 读取任务列表，生成 provider bridge 环境变量，调用真实 Harbor provider-backed ignored test，并在成功或阻塞时写入 `docs/evidence-packs/terminal-bench-21-regression-subset-*.md`。脚本不得打印 raw provider key；无显式 `CODEFACTORY_BENCH_API_KEY` 且 OS credential store 不可用时，必须生成 credential blocker evidence。

固定 subset 的离线基线入口是 `tools/benchmark/summarize_terminal_bench_21_subset_baseline.py`。该脚本只读取已完成 full Harbor job 和 subset JSON，不调用 provider、不读取 secret，用于在 credential 或 provider 暂不可用时仍能生成同口径的 subset baseline evidence。该报告必须明确标注 `offline subset projection`，不能冒充新的 provider-backed rerun。

截至 2026-07-10 的完整性复审发现，历史 `16 / 18` 与当前 `12 / 18` adapter 内含 fixed-task instruction fingerprint、专用 hint/auto-repair、成功 marker，并在 setup 路径读取 `/tests/test.sh`。这些 run 保留为历史诊断证据，但重新分类为 `benchmark-contaminated diagnostic`，不得继续作为 CodeFactory 主产品能力基线。下一次有效基线必须先满足 CF-TB-R9 至 R11，再重新运行固定 18 题。

发布门禁按两阶段执行：第一阶段在同一个候选版本上恢复有效 `>=16 / 18`，且相对该版本迭代循环中已经诚实通过的任务集合零回退；第二阶段继续提升到固定 18 题 `18 / 18`。这两个分数都必须来自 thin Harbor bridge、Rust headless、共享 contract 和完整 integrity metadata。完成固定 18 题后，必须在 clean Linux/x86 环境复现，再运行完整 89 题；macOS/QEMU diagnostic 不能替代 clean Linux/x86 证据。

### Iteration Loop

标准产品能力迭代入口是 `tools/benchmark/terminal_bench_21_iteration_loop.py`。它把单次评测变成可重复的开发闭环：

1. 输入本轮 `hypothesis` 和 `target_failure_class`。
2. 选择 `canary` 或 `regression` scope。
3. 可选执行 provider-backed run；不执行时仍生成 dry-run iteration report。
4. 读取 baseline/head evidence，生成 pass、mean reward 和 failure class delta。
5. 写入 `docs/evidence-packs/terminal-bench-21-iteration-*.md`，其中必须包含下一步 improvement queue。
6. 每轮必须先给 `product_capability_verdict`：`product-capability`、`mixed` 或 `benchmark-only`，再写清楚产品能力影响：`product_capability_impact`、一个非评测场景 `product_example`，以及 `benchmark_only_boundary`。`tools/benchmark/terminal_bench_21_iteration_loop.py` 必须在 CLI 和报告生成层拒绝缺失这些字段；如果某次改动只对跑分 task scaffold 有效，报告必须直接标明，不得包装成 CodeFactory 整体智能化能力的大幅提升。
7. 本机代理、Docker apt bootstrap、verifier `uv` / PyPI / GitHub 下载、provider bridge transient retry 相关配置必须通过 iteration loop 的正式参数传递，例如 `--provider-proxy`、`--docker-apt-proxy`、`--verifier-proxy` 和 `--provider-bridge-retries`，不得绕开标准 loop 手写一次性 runner 命令；这样 blocker、evidence 和产品化解释才能留在同一条评估链路里。
8. 确定性通用修复通过独立测试后，先用非 benchmark 的真实 App 任务验证，再走 PR/CI/合并/发布和安装包验证；随后只能用该发布 tag 构建的 headless 复评。复评失败转化为下一轮通用产品缺陷，但不撤销已验证发布事实，也不得把 targeted canary 当作固定 18 题总分。

默认 canary 文件为 `docs/benchmark-subsets/terminal-bench-21-canary-subset-v1.json`，任务为 `write-compressor`、`filter-js-from-html`、`mteb-retrieve`、`count-dataset-tokens`，用于在完整 18 题 regression 前快速验证 agent loop 是否真正改善。canary 只用于开发内循环，不能替代 regression subset 或 full run 作为 release 结论。

### Run Summary

每次 run 至少记录：

- benchmark id、dataset、dataset version 或 resolved package id。
- evaluation axis、evaluation subject、fixed variables、changed variables、result attribution。
- agent name、agent version、model、provider。
- CodeFactory app version、git sha、build time。
- Harbor version、Docker/provider 类型。
- full command、job path、started/finished time、status。
- comparable flag 和不 comparable 的原因。
- reward summary、pass/fail/partial counts、cost/duration。

### Trial Summary

每个 trial 至少记录：

- task name、category、difficulty、tags。
- reward、duration、verifier exit status。
- trajectory path、verifier stdout/stderr path、artifacts path。
- failure class、classifier confidence、human review status。

## 测试矩阵

| Path type | Scenario | Expected result | Evidence |
| --- | --- | --- | --- |
| Primary | 导入完整 Harbor job fixture | run 和 trial 入库，summary 正确 | unit/integration output |
| Primary | 页面展示 latest run | reward、task count、artifact path、failure classes 可见 | UI test |
| Primary | 同一 subset 对比两个 run | reward delta、regression task、improved task 可见 | compare test |
| Adapter | custom agent adapter command 生成 | 使用 `terminal-bench/terminal-bench-2-1` 和 import path | command assertion |
| Adapter | CodeFactory baseline adapter smoke | Harbor 能 import `codefactory_bench.agent:CodeFactoryAgent`，trial 无 exception，CodeFactory importer 读回 agent identity 和 reward | Harbor job + ignored real import test |
| Adapter | Model-backed headless loop | fake OpenAI-compatible server 返回 `run_shell` tool call，adapter 执行 Harbor environment command 并写 trajectory | Python integration test |
| Core | Inspection budget | 连续只读检查达到通用预算后，`ProgressTracker` 要求形成最小候选实现；任一 mutation 重置预算 | Rust core tests + real product/canary trajectory |
| Core | Semantic failure | `return_code=0` 但输出含明确失败证据时，不得解锁完成门禁，后续必须修改或重新验证 | Rust core tests |
| Core | Inspection is not verification | 依赖安装或修改失败后，打印环境、版本、搜索路径或文件列表即使返回 0 也不能解锁完成；必须运行任务相关测试、构建、项目入口或功能探针 | shared core tests + real App |
| Core | Final-before-verify gate | 最后一次 mutation 后没有更新的成功 build/runtime/test 证据时，桌面与 headless 都不能结束 | shared core tests + desktop tests + real App |
| Core | Background service lifecycle | 服务启动后必须存在 PID/pidfile/process handle、日志、bounded readiness 和真实 functional/client probe | shared core tests + real App service scenario |
| Desktop | Streaming response integrity | OpenAI-compatible 与 ChatGPT Responses SSE 必须看到 `[DONE]`、`finish_reason` 或 `response.completed`，且不得存在 malformed data line 或残留半行；已展示的半截响应报明确传输错误，不自动重放工具 | Rust desktop stream tests + real App interrupted-stream scenario |
| Headless | Model response recovery | 响应体截断、瞬时 transport 或 `429/5xx` 只在有限预算内重试；最终错误保留状态和截断诊断，不能被 sidecar cleanup 覆盖 | Rust HTTP tests + Python bridge lifecycle test |
| Headless | Tool failure protocol | 命令超时、环境异常或 policy deny 可以返回 `return_code=null` 与结构化 error；Rust sidecar 将其记录为失败证据并继续模型闭环，不得因 JSON 类型错误终止 trial | Rust JSONL protocol test + real timeout trajectory |
| Headless | Total timeout ownership | Harbor/产品任务调度器是总时限唯一来源；bridge 把该有效时限传入 sidecar，模型重试、工具执行和最终回复共用同一墙钟预算，并保留 30 秒结束窗口 | Python bridge + Rust runner tests + Harbor task timeout evidence |
| Core | Source delivery stages | source-build/install 需求必须依次满足源码修改后的全输入 clean scan、成功安装、源码目录外 runtime/import smoke 和项目验证；日志摘要不能因非空而误判 clean scan 失败 | shared core tests + source-build canary |
| Core | Compatibility scan convergence | 真实失败中的 API member 扩展到全部 alias，并只按源码或语言适配器证据扩展拼写；不可靠 clean scan 和含 build/install 的复合绕过在执行前被 policy 拒绝，zero-residual/nonzero 结果生成“临时文件 + 状态归一化 + `test ! -s`”恢复 blocker | shared core failure-first tests + real App fixture + released-build source canary |
| Desktop | Endpoint-authoritative AI routing | 规划、规范辅助、子任务、验收和学习使用当前 endpoint 的 active model；父子会话与真实请求模型一致 | Rust tests + session database evidence + real App run |
| Desktop | Bounded file discovery | 根级 glob 不返回常见依赖/构建目录，显式目录根仍可搜索 | Rust tool tests + real App trajectory context size |
| Desktop | Interactive completion invariant | 普通聊天在源码修改后拒绝脆弱兼容扫描、放行可靠扫描；没有 completion blocker 时不启用自主轮次预算 | desktop routing tests + packaged App edge path |
| Headless | Context compaction | 保留共享 contract、原始任务和最近完整 tool round；旧输出压缩后不突破上下文预算 | Rust headless tests + usage metadata |
| Adapter | Thin bridge protocol | Python 只启动 sidecar、转发 `ToolRequest`/`ToolResult`、记录 metadata，不包含模型调用、prompt、policy、任务分类或 repair | Python integration + contamination scan |
| Policy | Bounded command policy | hidden verifier、solution、secret 始终拒绝；外部网络按有效环境策略决定，loopback probe 与正常 workspace build/test 允许 | Rust core policy tests |
| Adapter | Provider bridge preview | 当前 DeepSeek endpoint/model 生成 redacted env 和 Harbor command preview，不暴露 raw key | Rust unit test |
| Adapter | Provider bridge authorization | 授权短语不匹配时不得 lookup secret；匹配后只把 key 放入 child env | Rust unit test |
| UI | Benchmark credential blocker | provider keychain/credential failure 返回 `status=blocked`、`failure_kind=credential`，Benchmark 页面展示 blocker，不记为 agent failure | Rust unit test + frontend build |
| Attribution | Evaluation axis contract | run/PR/evidence 区分 CodeFactory agent 能力、模型后端影响、agent scaffold 对比和评测基础设施 smoke | spec review + fixture test |
| Regression | Fixed subset runner | 从固定 subset JSON 生成 18 题 provider-backed run；credential 不可用时生成 blocker evidence，不伪造 agent score | runner dry-run + blocker evidence |
| Regression | Iteration loop runner | 声明 hypothesis/target failure class 后生成 baseline/head/delta/next queue report；可 dry-run 或执行 canary/regression | Python iteration loop test + evidence report |
| Regression | Score-holding aggregate gate | 单题 canary pass 后必须跑固定 18 题或生成 blocker；aggregate 回落时标记 `targeted canary pass, aggregate not held`，不得声明总体能力提升 | fixed subset evidence comparison |
| Policy | benchmark-sandbox policy in task container | workspace command/file edit 自动允许，host path/secret deny | policy unit test |
| Policy | network policy inheritance | fake Harbor `public`/`allowlist`/`no-network` 策略分别映射到共享 core 能力，metadata 记录有效策略，host enforcement 不被 bridge 绕过 | Python bridge test + Rust core policy test |
| Failure | 缺失 `result.json` | 标记 `partial_import`，列出缺失文件 | importer test |
| Failure | Harbor 不存在 | UI 显示 blocker，不影响其他页面 | environment probe test |
| Failure | timeout/resource 被修改 | comparable=false，官方可比状态标红 | config validation test |
| Failure | verifier/runtime instability | verifier 自身在比较产物前崩溃、缺 driver、QEMU/proc/netlink 受限或 watchdog 后缺 reward 时，evidence 区分 runtime failure 与 artifact assertion failure | evidence classifier fixture |
| Observation | classifier 输出 failure class | 每个失败 trial 有 evidence refs 和 assumptions | classifier fixture |
| Payload | trajectory/artifact 导出 | 不写入长期 memory，不自动复制任务全文 | memory guard test |

## Evidence Pack Requirements

- 官方资料核准时间和链接。
- 环境检查输出：Harbor、Docker、provider、dataset。
- run command preview 和实际 command。
- Harbor job path。
- `config.json`、`result.json`、trial result、trajectory、verifier output 摘要。
- CodeFactory build identity。
- comparable flag 和约束检查结果。
- failure taxonomy summary。
- 改进前后 subset 对比报告。

## 发布边界

- 在 headless runner 和 Harbor adapter 真正可跑前，产品只能声明 `design ready`，不得声明 Terminal-Bench 2.1 已支持。
- 在至少一次真实 Harbor smoke run 成功导入前，不能声明 `evaluation path verified`。oracle smoke 只能证明 Harbor 环境和导入链路，不能证明 CodeFactory agent 能力。
- `codefactory-headless-baseline` 成功运行后，可以声明 `CodeFactory-owned adapter path verified`，但在 model-backed headless runner 跑通前，不能声明 `CodeFactory agent capability evaluated`。
- fake model 测试通过后，只能声明 `model-backed runner implementation verified locally`；在显式模型 env 下跑完真实 Terminal-Bench smoke 前，不能声明 `model-backed CodeFactory score available`。
- provider bridge 测试通过后，只能声明 `current provider can be authorized for CodeFactory agent benchmark launch by backend contract`；在真实 Harbor run 完成并导入前，不能声明当前本机已产生 CodeFactory agent Terminal-Bench 分数。
- 使用 DeepSeek/Claude/GPT 等模型后端完成的 run，结果仍归属 `CodeFactory agent using <model backend>`；不得写成模型本身的 Terminal-Bench 结果，除非 evaluation axis 明确是 `model-backend-ablation` 且 CodeFactory build/agent adapter/subset/policy/runner 已固定。
- 在 packaged app 或 release artifact 中验证前，不能声明 `live`。
- 官方 leaderboard submission 需要单独 release/QA gate；本规格首期只覆盖本地可复现能力评估。
