# M258：交付工具把构建目录提交进分支，超时后还说是网络问题

## 背景

### 现场（2026-11-07，v1.102.0，M257）

- 会话用 `.m257-target` 做 cargo 构建目录。`deliver_changes` 把它一起提交，分支多了 5008 个文件、6.5 万行。
- 推送 3 次都超过 30 秒，工具报：`交付卡在 push：已尝试 3/3 次；等待网络恢复，由系统续接`。实际网络正常，`git ls-remote` 只要 1 秒。
- 会话据此空等、反复重试，直到编排方查 `git diff --stat` 才发现。

### 用户决策

- 交付工具必须在提交前自己看一遍"将要提交什么"，而不是等推送超时再让模型猜。
- 构建产物永远不该进入交付提交；这是模型可以自己修的问题，不需要用户操作，也不该进入等待状态。
- 判定要基于文件事实（目录结构、文件类型），而不是维护一份构建目录名清单——`.m257-target`、`.m258-target` 这类名字每次都不一样。
- 真正的大改动（比如一次大规模重构）必须有明确通过路径，不能被一刀切拦死。

## 决策

1. 在 `deliver_changes` 的 stage/commit 之间新增"交付内容体积守卫"（`inspect_delivery_size` + `delivery_size_verdict`），在**任何真实 index / ref / 远端动作之前**运行。
2. 构建产物按**文件事实**识别：`CACHEDIR.TAG`、`.rustc_info.json`、`.fingerprint/` 目录、目标文件扩展名（`.o`/`.obj`/`.rlib`/`.rmeta`/`.d`/`.pdb`/`.dylib`/`.so`/`.a`/`.exe`/`.dll`/`.class`/`.pyc`/`.pcm`/`.gcno`/`.gcda`）、`.dSYM/` 包；并且沿每个待提交路径向上找，只要某个祖先目录里有 `CACHEDIR.TAG` 或 `.rustc_info.json`，该路径就属于构建目录（名字无关）。
3. 规模阈值有明确依据（见下），超阈值且**没有**产物事实时给出显式通过路径（`allow_large_change: true`），而不是硬拦。
4. push 超时后先做因果判定：探测远端可达性；可达 → 报内容过大并给出产物处理办法，**不重试、不进入等待**；不可达 → 保持原有网络等待语义。
5. 不放宽任何现有交付门禁：pre-push 钩子、receipt、mutation permit、sync gate 全部保持原样；守卫只做"拒绝并把原因说清楚"。

### 阈值与依据

| 阈值 | 值 | 依据 |
|---|---|---|
| 待提交文件数 | 1000 | 本仓库单次合法 PR 的改动集在数十到数百文件量级（本次回归的构建目录是 5008 文件）；1000 是合法批次上界的数倍，仍远低于事故规模 |
| 改动行数 | 200000 | 事故 6.5 万行构建产物之外，真实源码单批次极少超过 10 万行；20 万行给大规模重构留出余量 |
| 产物文件数 | ≥1 | 构建产物没有合法例外：它们由构建生成，可从源重建，提交它们只会污染历史 |

## Requirements Traceability

| Req ID | 需求 | 最低证据 |
|---|---|---|
| R1 | `deliver_changes` 提交前检查待提交内容；发现构建产物或规模异常时不提交不推送，返回路径、文件数、处理办法 | 真实 git 仓库集成测试：工作区含 cargo 构建目录时 `deliver` 被拦，返回消息包含产物路径与文件数，分支 HEAD 不变；单元测试覆盖文件事实识别（含任意名字的 target 目录） |
| R2 | 提示可照做：把产物移到 `.codefactory-cache/` 下或排除后再交付；不进入等待状态、不要用户操作 | 消息断言包含 `.codefactory-cache` 与"未执行提交/推送"，且 `final_state` 为 blocked 而非 waiting |
| R3 | 查清现有提交文件选择逻辑，并给出合理大改动的通过路径 | 大改动通过路径测试：超阈值但无产物事实且 `allow_large_change=true` 时不被守卫拦截；默认仍拦截并说明如何放行 |
| R4 | push 超时后先判因：网络可达而内容过大报内容过大；只有真连不上才说等待网络 | 超时检测真实进程测试 + 两种判定分支的消息测试：可达消息不含"等待网络"、不可达消息包含；内容过大不重试 |
| R5 | 不放宽现有交付门禁、不绕过 pre-push 钩子 | 现有 delivery 测试套件全绿；守卫在 mutation intent 之前返回，未新增任何 bypass |

## Applicable Harnesses

- Spec Harness（默认）：本文件即规格，Req ID ↔ 测试映射见 PR body。
- Compatibility Harness：只新增 `DeliverOpts.allow_large_change`（`#[serde(default)]`），旧调用不需要改动。
- Release Harness：release-facing 修复，发版后由编排方真机验收；会话内证据为 Rust 集成测试。
- Observation Harness：R4 的判定消息是用户可见的观察面，测试逐字断言两种消息的差异。

不适用：Payload Harness（不涉及上传/对象存储）、Viewport Harness（无 UI 变化）。

## 测试矩阵

| 用例 | 类型 | 期望 |
|---|---|---|
| cargo target 目录（任意名字）在工作区时交付 | 真实 git 仓库集成 | 被拦；`final_state != pushed`；消息含产物路径、文件数、`.codefactory-cache`；分支 HEAD 未移动 |
| 普通小改动 | 集成 | 照常 commit/push，守卫不产生步骤 |
| 1000+ 真实源码文件改动，默认 | 单元 + 集成 | 被拦并提示 `allow_large_change: true` |
| 同上，`allow_large_change=true` | 单元 + 集成 | 放行，守卫记录一条 advisory |
| 子进程超时检测 | 真实进程 | 300ms 上限对 `sleep` 返回超时标记 |
| push 超时且远端可达 | 单元 | 消息含"内容过大"与处理办法，不含"等待网络" |
| push 超时且远端不可达 | 单元 | 消息含"等待网络" |
| 既有交付套件 | 全量 | 全绿 |
