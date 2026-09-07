# E2E-001 可信执行切片设计（Bootstrap-1a）

## 目标与边界

对应 CF-STG-R20/R23/R26/R28/R29/R30/R31。沿用 `scenario_execution.py` 和既有 `scenario-gate-pr`，不新增 runner、required context 或第二套场景计数。把 M1b 正式二进制产生的观察值接入可信判定，关闭只检查顶层 `ok`、候选自报 `passed`、重复 target 和错平台回执造成的假绿。

本切片只验证 PR 阶段的 durable-state/process/side-effects/delivery oracle。UI 明确为 `not_required_for_stage`，原因固定为 `headless_worker_has_no_webview`。不提升 11 个完整 case 的状态，不代替真实 WebView、nightly 故障矩阵或 release exact-artifact 验收。

## 分两份 PR 实施

1. 普通前置 PR：将 unattended CLI 从大型 `lib.rs` 抽出，`main.rs` 第一个分支直接调用独立模块；driver 直接挂载自己的 observation 与 process-tree 源码。先测试旧分发顺序失败，再验证真实编译分发、正式 binary 成功与失败路径。不修改线上 gate。
2. 信任根升级 PR：实现以下 plan/execution/aggregate/await 契约，补齐回归与独立审查。由于修改 judge，旧默认分支必须拒绝这份 PR 自证；只有另获明确批准后，才能按最小门禁迁移、立即恢复和 canary 流程合入。

## 数据流与身份绑定

| 阶段 | 输入与动作 | 拒绝条件 |
| --- | --- | --- |
| trusted plan | base registry 按 diff 选择 target；只要选到 canonical binary（含 alias），恰好生成一个 E2E-001 case plan | 缺实现文件、alias 跨平台、fixture 无效 |
| unprivileged execute | 重新生成计划；检查实际 HEAD、OS/arch、受保护 driver 摘要及入口；执行正式 binary | 修改计划、入口改向、错误平台、driver 变化、退出非零 |
| runner receipt | 保存白名单、严格类型的 raw 投影及 case receipt；成功/失败都清理临时回执目录 | 缺字段、错误类型、清理失败、回执不完整 |
| trusted aggregate | runner/target/case 完整唯一集合；由 base adapter 从 raw 重算 receipt | 缺失、重复、多余、错 SHA/runner、候选声明与重算不一致 |
| independent await | 重新生成 base/head plan，从 exact-head workflow artifact 再次重算 | 旧 schema、错误执行 run、缺 artifact、修改后的结果 |

execution envelope 升为 schema v2；case receipt 继续使用 M1a 的 schema v2。旧 execution v1 不作隐式降级兼容，迁移后的 canary 必须使用新默认分支重新执行。

PR build identity 绑定实际 checkout HEAD 与 `CODEFACTORY_BUILD_GIT_SHA`，formal smoke 的编译时 SHA 必须相同。执行前后检查 tracked tree 不得变脏，并使用 Cargo `--locked` 禁止构建时悄悄改变锁文件。Python 入口要求 3.11 以上（CI 为 3.11/3.12）。PR 的 executable/artifact digest、tag 与 version 留空，明确不提供安装包身份保证；release 的更高要求不因此放宽。

runner policy 固定 Windows x86_64 与 macOS aarch64，并在实际执行进程中检查 OS/arch。`macos-14` 的当前官方标签对应 arm64；来源为 [GitHub runner images](https://github.com/actions/runner-images)。标签平台未来变化时应明确升级 policy，不自动接受身份漂移。

## 信任根范围

- 整文件保护短小的 `main.rs`、专用 CLI、unattended driver、observation adapter、process-tree 实现、`build.rs` 和根 `.cargo/config.toml`。
- `lib.rs` 只固定从文件起始位置到首个无条件 `pub mod unattended_smoke_cli;` 的精确前缀；前缀前不允许 cfg、path attribute 或其他代码。其余产品实现仍允许普通 PR 修改。CLI 直接 `#[path]` 挂载受保护 driver，driver 直接挂载受保护 process-tree，避免可变 re-export 改指向。
- 检查 Cargo 的 library/binary/build 入口，拒绝新增 extensionless `.cargo/config` 和子目录 config 变体。否则 Cargo runner 可在编译真 binary 后替换成伪造脚本。
- 保护 Python planner/executor、case adapter、receipt verifier 与 synthetic fixture manifest；摘要从 base checkout 读取，不能来自 candidate 的自报值。
- 文件与父目录均不得为 symlink。manifest 摘要证明契约身份；实际 fixture 构造和清理由受保护 driver 执行，不能把摘要本身当作执行证据。

这仍不是对任意敌对构建脚本的 OS 级沙箱：candidate 和 policy 在执行 job 内使用同一 runner 用户，独立 final gate 不执行 candidate 代码，但完整供应链、所有 delegated scripts 与运行用户隔离仍属于后续 trust closure。不得宣称 Bootstrap-1a 已完成全部 M1 或 M7。

## 失败与隐私合同

- bool 必须是 bool，计数必须是非负整数；`true == 1`、`false == 0`、浮点数和字符串不能成为成功证据。
- raw 只保留已定义的枚举、布尔、计数与 SHA。原始 error、路径、session/objective ID、凭据和未知键不上传。聚合后的非 case 字段同样检查 schema、只导出安全字段。
- 非零退出即使写出声称通过的 raw JSON，target 仍失败；缺失或坏 JSON 仍形成失败 case receipt。
- runner、target、case 都必须恰好匹配计划；不能用同名成功项覆盖失败项，也不能静默丢弃损坏 artifact。
- aggregate 与 final gate 都重算整个 case receipt（含 evidence/run digest），单独重算公开自哈希不构成证据真实性证明。
- 临时回执目录由 executor 在 finally 中回收；清理失败产生失败诊断，不能使成功 artifact 继续变绿。

## 验收与迁移

必须覆盖 source/entry/config 改向、缺失/重复/错误平台、类型混淆、伪造 passed、修改 raw 后重哈希、隐私旁路、非零退出和清理失败。先在本地通过 Python 负例与正式 binary 的真实 hard-kill/重启/幂等/清理路径，再提交独立升级 PR。

线上迁移前必须展示完整差异、前置 PR 和 CI 证据及用户批准范围。只临时移除必要的 `scenario-gate-pr`，保留其他五项 required checks、strict/active/no-bypass 与 PR 保护，merge 后在 finally 恢复原始规则集并读回对账；随后用新基线 canary 验证 exact-head、完整 target 集合和 E2E-001 重算。未完成迁移与 canary 时状态必须为 **not live**。
