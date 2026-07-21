# CodeFactory Agent 宿主截止时间与取消传播证据

- Req ID: `CF-TB-R49`
- 日期: `2026-07-21`
- 状态: candidate, not live
- Proof tier: `controlled-headless-runtime`
- 失败基线: public `v1.51.7` / `50a52bed2ac429b086da687b96b90e92cb506e87`
- candidate base: current `origin/main` / public `v1.51.8` / `eb6068d`
- candidate headless SHA-256: `0fbcd73c4a1b2b9b23d6cb3c1323806d6e68bc521ef75dd349bc03f9faccc02d`

## Released-Build 失败证据

`v1.51.7` 的 `circuit-fibsqrt` 同参数 canary 在 sequence `38` 完成最后一次成功 source
mutation 后，没有得到成功 machine verification。runner 在 900 秒停止 task container，但
Harbor bridge 没有把外层截止时间传播给 headless sidecar。sidecar 随后继续到 `80` 次模型请求，
并产生 `31` 次针对已停止 workspace 的必然失败工具调用。

- run: `d6ad2b52-412c-48d4-8a6e-3f4ed80e9920`
- reward / failure class: `0 / environment`
- duration: `1,348.95s`
- model requests / tokens / tool calls: `80 / 686,352 / 69`
- raw evidence:
  `docs/evidence-packs/terminal-bench-21-regression-subset-2026-07-21T00-45-00Z.md`

该单题 run 启用了 watchdog，因此 `official_comparable=no`，只用于定位通用生命周期缺陷，不能
替代固定 18 题总分。当前有效固定 18 题仍为 `6 / 18`。

## 通用产品修复

- runner 根据每题 trial hard timeout 计算 lifecycle host cap，固定预留 120 秒给 verifier、元数据
  和 workspace 清理；该 cap 不混入 run-phase 预算。sidecar 的有效执行预算再取 host 剩余时间、
  官方任务预算和显式 Agent wall timeout 的最小值。
- 无 watchdog 的官方运行不注入 lifecycle host cap；heavy trial override 逐题计算，显式更短 run
  budget 继续优先，但只从 Agent run 开始计时。
- Harbor adapter 与 runner 都以 trial `config.json` 创建时间为 lifecycle 起点；adapter 从 setup
  开始只创建一个绝对 deadline，并将同一 deadline 应用于网络 bootstrap、
  workspace/project 解析、sidecar 启动、协议读写、工具执行、project refresh、stderr 和最终退出。
- 官方 run-phase execution budget 不再折入 setup lifecycle cap。sidecar 启动时取官方预算、显式
  wall timeout 与 host 剩余时间的最小值，setup 只消耗 host 时间，不私自吞掉官方 Agent 预算。
- 每个 `tool_request` 在实际执行前先持久化 trajectory 和 usage。deadline 取消 in-flight tool 后，
  adapter 清理受管进程组，终止 sidecar，并写入 `status=cancelled`、失败类型和最后 usage snapshot。
- sidecar 以独立 session/process group 启动，并在不含 key 的 `sidecar-runtime.json` 记录 PID、
  binary、随机 trial runtime token 和 PGID。若 adapter 没能及时退出，runner 仅在四项完全匹配时
  向专属进程组发 `TERM`、必要时发 `KILL`；确认进程退出后才停止 trial container。终止失败时
  workspace 保持运行并在下一 watchdog poll 重试；runtime JSON 缺失/暂时不可读、`ps` unknown 或
  identity mismatch 同样 fail-closed 重试。同 binary 的其他 trial、PID 或 PGID 复用不会被误杀。
- Rust headless 在同一模型响应包含多个工具调用时，会在每个工具前重新检查 30 秒 wall reserve；
  第一个工具跨入 reserve 后，后续工具不再启动，并返回 `completed=false` 与累计 usage。
- 共享执行合同明确：调用方开始 workspace 清理后，任何 child、模型请求或工具都不得继续使用该
  workspace。该合同同时约束桌面和 headless 主 Agent，不是 Terminal-Bench task 定制。

对 CodeFactory 主产品的直接提升：用户取消长任务、IDE/调度器到达任务期限、后台构建超时，或
workspace 被关闭时，Agent 不会继续消耗模型额度或对失效目录重复执行命令；已经产生的模型 usage
和工具轨迹仍可用于会话复盘与下一轮恢复。不同项目、模型、命令和任务预算走同一生命周期机制。

## Failure-First

实现前独立复现了以下失败：

- 900 秒 watchdog 只停止 Docker container，sidecar 继续请求模型和工具；
- adapter 只约束 sidecar stdout，setup、目录解析、工具和 finished 后的 process exit 可越过期限；
- 同一模型响应中的第二个工具不会在第一个工具执行后重新检查 wall reserve；
- PID runtime 记录存在时，watchdog 仍只执行 `docker stop`；
- sidecar 发送 `finished` 后保持存活，父进程会无上限等待到它自行退出。

修复后回归覆盖：

- 900 秒 trial 生成 `780` 秒 task host cap；无 watchdog 时清理继承的旧 cap；
- 显式更短/更长预算、官方任务预算和 heavy override 按最小值组合；小于等于 150 秒的 trial
  watchdog fail-closed；
- in-flight tool 被 deadline 取消并执行进程组 cleanup；已读到的 request usage 在取消前落盘；
- finished 后挂起的真实子进程在 deadline 内被杀死并记录 `cancelled`；
- finished 消息的最终 usage 在等待进程退出前落盘，已完成 tool result 在尝试交付前落盘；
- verified sidecar process group 在 container 前停止，binary/token 不匹配时不 kill；
- `TERM/KILL` 未能确认退出时不停止 workspace、不记录完成 intervention，并在下一 poll 重试；
- runner 与 adapter 均从 trial config 创建时间计算绝对期限，启动延迟不会越过 outer deadline；
- 同一响应的第一个工具跨入 reserve 后，第二个工具不会启动。

## 当前验证

- adapter: `26 passed / 2 skipped`；跳过项为 macOS 上的 Linux process-group 专项
- regression subset runner: `51 passed`
- headless: `22 passed`
- shared agent core: `115 passed`
- desktop Rust after latest-main sync: `389 passed / 6 ignored`
- canary dry-run: `circuit-fibsqrt:780`
- targeted headless Clippy: passed with two unchanged baseline lint allows；全依赖 `-D warnings`
  仍被 R49 diff 外的 `agent-core` 11 个 Rust 1.96 style lint 拦截
- targeted Rust formatting、Python compile、governance baseline、long-task validator、
  `git diff --check`: passed
- candidate source contamination: passed
- first independent review: 找到 2 个 blocker、2 个 P1、2 个 P2；均已转为 failure-first 回归并修复
- second independent review: 找到 1 个 blocker、2 个 P1、1 个 P2；已补齐 unknown-state retry、
  refresh 前落盘、run/lifecycle 分离和真实 PGID 校验
- third independent review: 找到 2 个 blocker、1 个 P1；已把 runtime 缺失/identity mismatch 改为
  fail-closed retry，并让 lifecycle config 锚点读取失败立即耗尽 host deadline
- final narrow independent review: `no blocker / no P1`
- macOS controlled process-group probe: real PID/PGID/binary/token state `matches`
- final independent re-review: `no blocker / no P1`
- PR/CI、remote real-App、release: 待执行

该修复不依赖本机 GUI 或解锁状态；本地证明使用真实 headless 子进程和受控 environment，PR 的
远端 macOS GUI 与 Linux bridge 会继续作为交付门禁。当前仍是 `not live`，只有合并、Auto Release、
公开产物复验和同参数 released-build canary 完成后才能改为 live。

## 发布后复评门禁

在精确发布 tag 上保持 `circuit-fibsqrt`、DeepSeek `deepseek-v4-pro`、并发 `1`、900 秒 outer
watchdog、subset、resource 和 verifier 不变。合格的 R49 差异应为：Agent 在约 780 秒 host cap
内干净结束，runner watchdog 不介入，不出现 container stop 后的新模型/工具调用，usage 如实保留，
总成本显著低于 v1.51.7 的 `686,352` tokens。reward 仍由 verifier 决定；单题结果不改变固定 18
题分数。
