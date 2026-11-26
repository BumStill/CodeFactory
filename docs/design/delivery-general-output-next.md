# R2 通用产物与 Windows：下一最小切片

## 已验证起点

父会话于2026-11-12回传独立Mac回执：base `989c8da9c272de73142099ef25ed2eb7e3daa88b`，精确tree `66d1cdffb10a4e06bf12573a68f68f58f390769e`；一次性detached副本、仓库cargo-shared、offline，七组435/435通过。另原独立兼容测试只调整插入位置，native shell生成/修改与任务预暂存交付2/2通过；applied receipts=2、TaskDeliveryScope=Ok；测试后恢复精确树。此为父会话提供的可信回执，不冒称本执行器读取了Mac原始日志。Linux原三项OS凭据失败在Mac未出现。证据是native工具/Git/SQLite/本地bare，不是GUI、provider或Windows。

现有产品代码与该候选保持相同。本轮不启用新的通用执行路由，不改14个未映射文件对应的registry/validator/hooks/保护规则，不push/PR/发布。

## 架构判断与源码依据

1. `tools/bash.rs::sandbox::docker_invocation`把实时cwd读写挂载，网络开启；Windows分支直接拒绝该Docker模式。不能只把挂载改名便声称私有写层。
2. `command_sandbox.rs::build_profile`允许全盘读取；`agent/trusted_sandbox.rs`放行workspace/temp/cache写入。因此当前Mac配置不是输入来源封闭、输出独占的执行器。
3. `util/process_tree.rs`已有Windows Job Object生命周期设施，但现有attach接收已创建child。Job Object不等于文件隔离，也不能代替启动前纳管和后代退出证明。
4. 复用`content_provenance.rs`的身份/run/epoch/Stop，`delivery_hook_commit.rs`的固定parent/tree和正常hooks、持久intent与index lock/ref CAS。不得另造业务完成状态。

## 技术路线与明确准入条件

### A. 平台无关的输入、收集、封存协议

宿主固定可信基树、已证明改动、项目/Objective/run/epoch、命令与工具链身份。只有来源受控且本次允许读取的依赖能进快照；不能把整个脏workspace、用户凭据或共享可写缓存复制进去。包/SDK可以作为明确准入、可追溯的工具链输入，不把文件摘要本身当作来源认证。未知输入在应用里说明缺项，不自动洗白。

每次执行使用独立写层，实时工作区、宿主数据库、journal、broker控制端和其他任务写层均不可由候选进程访问。cwd、绝对路径与环境映射必须明确，不静默更换平台语义。依赖下载与工具链准备应单独纳入授权和来源记录；最初离线实验不能代表联网构建已支持。

宿主在全部后代停止、退出结果确定后收集；不信任stdout中的manifest/receipt，不执行输出内的程序来验证。收集端拒绝穿越、重复/别名路径、symlink/hardlink、设备及资源超限，记录新增/修改/删除/rename两端、mode和字节。输出先成为不可变候选，不直接变成applied。

**控制权也是准入条件**：运行时实例ID必须由宿主创建并绑定；结果层停止后仍可被其他控制端写入则不能签发可信证明。收集后复制到候选进程无法写的宿主对象存储，并验证对象身份与内容。仅hash或随机目录名不足以证明独占写者；旧命令不能借新run/新epoch继续。

### B. 先验证一个真实平台适配器，Windows并行制定原生验收

Linux/macOS的容器候选可使用无live bind mount的私有层，但当前共享Docker控制端不满足独占控制假设，必须在既有权限模型内限制所有不可信工具对该控制端的访问，或采用真正受保护的broker。不能把对同一daemon的另一写者排除在测试之外。Mac容器成功只代表Linux工具链；macOS本机SDK产物需单独适配和验证。

Windows候选研究方向是受限进程身份/AppContainer的文件权限与私有目录边界，加上启动时即纳管的Job生命周期；并不是“已有Job Object即可上线”。保留真实PowerShell、Python/Node Windows版本、MSVC或项目选定Windows编译器。需验证系统版本、SDK/COM/子进程依赖、reparse points、ADS、大小写/路径别名、只读依赖与网络准入。没有Windows执行环境时只记not run，不以Linux成功替代，也不将不支持当成结案。

官方参考：
- https://learn.microsoft.com/en-us/windows/win32/secauthz/implementing-an-appcontainer
- https://learn.microsoft.com/en-us/windows/win32/secauthz/appcontainer-isolation
- https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects

这些文档支持候选机制，不证明当前项目Windows适配器、编译器兼容或双向独占写层已经成立。若采用方案必须安装服务、扩展权限或改变OS安全配置，应先给精确操作与风险；本轮未做这些操作。

### C. 落地与既有 hooks

输入稳定性和结果身份均通过后，先形成不可变导入计划。任何live文件/index/HEAD变化均保留原状并显式冲突。多文件落地需要持久进度、每路径前置条件及崩溃恢复；多个rename不是共同原子事务，应用内互斥锁也挡不住任意外部编辑器。没有已证明的写入排他/无丢失更新协议前，只保存候选和diff，不自动覆盖live工作区。

只有整批成功落地且最终观察匹配才签发来源记录；部分成功/结果未知不补造applied，不以回滚覆盖用户后来写入。之后复用已验收的正常hooks与固定树提交。hook拒绝/改树/未知副作用仍停止；不绕hook，不默认采用hook新内容。来源准入、执行许可、远端交付授权继续独立。

## 本轮实际隔离实验

脚本与回执：`/workspace/delivery-scope-evidence/general-path-study/private_layer_probe.py`、`private-layer-receipt.json`。仅合成输入，已有本地Docker镜像，未挂载宿主目录/daemon/凭据；network=none、非root、cap-drop ALL、no-new-privileges；未改系统设置，已清理自己创建的容器。未测试或宣称PID/内存/CPU硬隔离。

- Python动态生成源码、真实C编译及生成程序执行成功；生成文件、删除、rename由宿主在停止后收集。
- 实时宿主同时出现的外来文件不在私有输出中；容器尝试写精确宿主fixture路径失败。这仅证明无挂载时该路径不可写，不是已暴露宿主目录的ACL攻击验证。
- 输出伪造applied JSON不被采纳为回执；首次收集未见late.txt。尚未完成延迟窗口后的复查或全部后代退出证明，不能据此声称晚写被阻止。
- collector对输出符号链接拒绝，不向宿主展开不可信tar路径。
- **负例成立**：同一授权Docker控制端的第二个客户端可向已停止层注入文件，collector会看到它。故现有runtime不具备完整独占控制证明，自动来源准入仍关闭。

这验证了通用生成/编译与宿主收集的可行部分，并暴露实际控制权缺口；不是生产handler接线、完整执行隔离、CAS落地、hooks联合E2E或Windows验收。

独立复核发现实验collector还不能进入生产：stdout.read位于timeout之前，等待EOF可能无限阻塞；重复路径只比较原始字符串，尚未拒绝规范化别名；目录条目没有计入数量预算。上述缺口须先修复并以阻塞流、路径别名及目录洪泛失败用例验收。本轮只测了链接拒绝，不能将其概括为完整收集器安全通过。

## 验收矩阵与切片顺序

| 切片 | 必须成功 | 必须拒绝/恢复 | 当前状态 |
|---|---|---|---|
| 输入与候选收集 | Python/Node生成、实际编译、create/edit/delete/rename/mode | 外来文件、依赖来源不明、伪manifest、链接/超限 | Linux独立实验部分实测；Node/完整协议未做 |
| 运行时控制与封存 | 精确实例、工具链、全部后代退出、宿主不可变对象 | 共享控制端注入、活跃后代、Stop/超时/旧run/ABA | 共享控制端注入反例已实测，可信适配器未具备 |
| live落地与恢复 | 已证明来源计划、原index保留、精确内容 | 并发同路径/index/HEAD、部分落地崩溃、重复事件 | 新通用路径未实现；现有固定树交付已另行验收 |
| hooks联合 | 正常hook、原生路径/分支敏感hook | 拒绝/改树/未知结果不得push | 旧候选hooks已验收；新写层联合尚未做 |
| Windows | PowerShell+Windows生成器/编译器真实运行 | reparse/ADS/别名、后代与控制句柄、读写逃逸 | not run，需真实Windows执行环境 |
| Mac | 容器Linux工具链与Mac原生工具链分列 | 对共享temp/cache/控制端攻击 | 旧候选435+2通过；新通用适配器未验收 |

最小下一实现批次应先做平台无关collector/不可变候选/冲突计划及故障注入，不授予执行可信身份；随后只选一个证明完整的适配器联合接线。正式注册仍是交付门槛，不应用候选registry自证，不通过本轮实验增加产品可达行为。

## 无可信执行适配器时的正常应用内采用流程（待产品审核）

通用历史/外部生成内容不应要求用户在外部聊天逐文件发授权命令。应用内在交付面板展示“这些现有变更尚无自动来源证明”，提供本次完整diff、来源说明、逐项选择和批量采用按钮；未知内容默认不选。用户确认的是固定内容纳入此任务，不是授予执行、远端写入或信任整个会话。

宿主固定snapshot ID、项目/Objective/epoch、baseline/head/index、精确tree和所选blob/mode；独立content-adoption purpose纳入签名域，摘要不能放进会被剔除的_permission字段。复用PermissionDialog外壳但禁用“信任会话并允许”，不调用设置delivery_authorized/清Stop fence的远端批准分支。重复点击一次消费、拒绝/过期无副作用；内容或epoch变化立即失效并在应用内刷新diff；重开恢复同一固定快照，不重新取内容套旧批准。结果标为“用户采用”，不能伪装执行回执。

此流程可覆盖无历史记录和暂不可隔离的Windows工具链，但涉及新的用户可见授权语义，本轮仅设计，未接线或视为已获采用授权。没有任何用户源文件被要求在聊天中手工批准。
