# Terminal-Bench 2.1 系统性改进方案

## 当前评分结论

当前已验证的 CodeFactory Terminal-Bench 2.1 完整基线来自 run `7ff6ef13-4488-4e0f-afd0-a1f9bd16d561`：

- dataset: `terminal-bench/terminal-bench-2-1`
- agent: `codefactory-headless`
- model backend: `deepseek-v4-pro`
- task count: `89`
- pass: `6 / 89`
- mean reward: `0.06741573033707865`
- failed: `83 / 89`
- Harbor exceptions: `63`
- passing tasks: `write-compressor`, `vulnerable-secret`, `openssl-selfsigned-cert`, `nginx-request-logging`, `filter-js-from-html`, `extract-elf`

这个分数说明 CodeFactory 现在已经具备可跑、可导入、可归因的评估链路，但 agent 能力还处在低基线阶段。主要问题不是单个模型回答质量，而是 agent 系统没有稳定地把长任务拆解、执行、验证和修复闭环跑完。

固定 18 题 regression subset 的离线映射基线是：

- source: 完整 run `7ff6ef13-4488-4e0f-afd0-a1f9bd16d561`
- subset: `terminal-bench-21-regression-subset-v1`
- report: `docs/evidence-packs/terminal-bench-21-regression-subset-baseline-2026-06-28T15-41-50Z.md`
- pass: `4 / 18`
- mean reward: `0.222222`
- level: `early scaffold baseline`

这个 subset 分数是从完整 run 离线投影出来的，不是新的 provider-backed rerun。它比完整 89 题总分高，是因为 subset 刻意包含 4 个已通过任务作为回归哨兵；它的用途是比较后续 agent-loop 改动是否真实改善失败桶，而不是替代完整总分。

## 问题分层

### 1. 评测基础设施还不够产品化

已看到的问题：

- 早期 cost/token 为空，无法衡量同等分数下的成本效率。
- Harbor exception、环境资源、agent 执行失败、模型 provider 失败容易混在一起。
- macOS keychain 授权会让真实 regression run 卡在 Harbor spawn 前。
- 没有固定 subset 时，单题 smoke 容易被误解为能力改善。

当前已落地：

- provider usage capture。
- `failure_reason`。
- Docker CPU preflight。
- `task_names` 固定 subset 支持。
- provider secret lookup timeout。
- 显式 `CODEFACTORY_BENCH_API_KEY` override。
- 18 题固定 regression subset。

下一步标准：

- 每个 agent 能力 PR 必须至少跑固定 subset，或明确记录 credential/runtime blocker。
- UI/证据包必须展示 `score + failure class + failure reason + cost + duration + subset id`。
- 不允许再把 credential/keychain 卡住、Docker 资源不足、provider 402 这类问题计入 agent 能力退化。

### 2. Agent loop 的长任务控制能力不足

完整 run 中大量失败来自 command timeout、agent timeout、服务未就绪、长命令阻塞或没有及时收敛。Terminal-Bench 2.1 不是简单问答，它要求 agent 长时间管理 shell、文件、服务、编译、测试和修复。

已落地：

- `environment.exec` 异常变成 `exec-error` trajectory，不再直接 Harbor exception。
- command timeout 进入 metadata 计数。
- 前台服务命令被要求后台化、记录 pid/log、做 readiness check。
- 剩余 step/budget reminder。
- repeated inspection suppression。

下一步要做：

- 引入任务阶段状态机：inspect -> implement -> self-check -> repair -> final。
- 给服务任务内置模板：start background、poll port/health、tail log、run client、cleanup。
- 给编译/训练/下载类任务加 bounded command policy：先小样本/ dry-run，再长任务。
- 每个 tool call 写入结构化 outcome，供下一轮 prompt 只读取摘要而不是完整日志。

验收：

- 固定 subset 中 `kv-store-grpc`、`sparql-university`、`torch-tensor-parallelism`、`build-cython-ext` 的 failure reason 应从 `command-timeout` / exception 转为 verifier failure 或 pass。

### 3. Verifier-driven repair 仍然弱

当前 agent 经常完成一次自检失败后就结束，或者看到 verifier-like 输出但没有形成补丁目标。

已落地：

- pytest/assertion/traceback 类自检失败会生成 repair reminder。
- missing artifact、segfault、missing tool、timeout 等会生成具体 repair focus。
- `write-compressor` 有 task-specific protocol auto-repair。

下一步要做：

- 把 verifier/self-check 输出解析为 `repair_goal`：失败断言、期望文件、实际文件、命令、最小复现。
- 每轮 final 前强制检查：expected artifact exists、quick verifier command passed 或明确无法验证。
- 将 task-specific auto-repair 泛化为 repair recipes：artifact protocol、service readiness、dependency fallback、parser/format mismatch、permission/path mismatch。

验收：

- `circuit-fibsqrt`、`configure-git-webserver`、`mteb-retrieve`、`sanitize-git-repo`、`query-optimize` 这类 verifier-zero 任务至少能产生结构化 repair goal，不再只记录笼统 verification failure。

### 4. Tool-use discipline 需要从 prompt 变成机制

当前失败里有明显 bad command、missing file、错误目录、无意义重复探索、把服务以前台方式启动等问题。只靠 prompt 不能稳定解决。

已落地：

- benchmark sandbox deny network/exfiltration。
- repeated inspection suppression。
- artifact-required / implementation-required gate。
- foreground service supervision guard。

下一步要做：

- tool planner 在执行前做静态风险/收益检查：路径、命令是否存在、是否会常驻、是否需要 cwd、是否会修改 host。
- 对 `command not found`、`no such file`、`permission denied` 自动生成替代动作建议。
- 建立 task workspace inventory：文件、可执行、端口、语言生态、测试入口。

验收：

- 固定 subset 中 `count-dataset-tokens`、`install-windows-3.11`、`protein-assembly` 的 `tool-use` failure 应下降，至少转为 verifier failure。

### 5. 产品闭环要从“跑分”升级为“改进队列”

Terminal-Bench 2.1 对 CodeFactory 的价值不是一次总分，而是持续生成产品改进队列。

需要固化的产品机制：

- Benchmark Run 页面：总分、subset、cost、duration、agent/model/build、comparable 状态。
- Failure Triage 页面：按 `failure_class` / `failure_reason` 聚合，点进 task 查看 trajectory、verifier output、artifact。
- Improvement Queue：从失败聚合生成工程任务，例如 `long-horizon/service-readiness`、`verifier-repair/assertion-parser`。
- Regression Gate：agent loop / tool runtime / verifier repair 改动必须跑固定 subset。
- Release Evidence：发版只报告经过 subset 或 full run 验证的趋势，不用单题 smoke 代表整体能力。

## 优先级路线

### P0: 让评测本身可靠

目标：任何一次失败都能被归到 environment、provider、credential、agent、verification 或 tool-use，不再出现无法解释的挂起。

交付：

- credential/keychain blocker UI。
- explicit key injection path。
- subset runner 一键命令。
- result import 后展示 failure reason。

### P1: 把 exception 变成可修复失败

目标：减少 Harbor exception 数量，让更多任务进入 verifier failure 或 pass。

交付：

- exec-error recovery。
- service supervision templates。
- long command policy。
- background process lifecycle record。

### P2: 提升 verifier repair

目标：让 agent 能根据失败输出修改实现，而不是一次失败后结束。

交付：

- verifier output parser。
- repair_goal message。
- final-before-verify gate。
- reusable repair recipes。

### P3: 提升真实得分

目标：先在 18 题 subset 上证明改善，再回到完整 89 题。

建议 gate：

- 18 题 subset pass 从当前 full-run映射的约 `4 / 18` 附近提升到 `7 / 18` 以上，再跑完整 89 题。
- 完整 89 题 pass 从 `6 / 89` 提升到 `15 / 89`，才算第一阶段 agent loop 改进有效。
- 同时记录 cost 和 duration，避免靠无限重试换分数。

## 下一次真实评估命令

当前 Codex shell 没有 `CODEFACTORY_BENCH_API_KEY`，且 keychain 授权不可用。授权后用固定 subset 运行：

```bash
TASKS=$(jq -r '.tasks[].name' docs/benchmark-subsets/terminal-bench-21-regression-subset-v1.json | paste -sd, -)
CODEFACTORY_RUN_REAL_PROVIDER_BRIDGE=1 \
CODEFACTORY_BENCH_ENDPOINT=deepseek \
CODEFACTORY_BENCH_TASK_NAMES="$TASKS" \
CODEFACTORY_BENCH_TASK_LIMIT=18 \
CODEFACTORY_BENCH_CONCURRENCY=4 \
CODEFACTORY_BENCH_MODEL_TIMEOUT_SEC=120 \
CODEFACTORY_BENCH_SHELL_TIMEOUT_SEC=120 \
CODEFACTORY_BENCH_AGENT_WALL_TIMEOUT_SEC=780 \
cargo test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings --lib -- --ignored --nocapture
```

如果不使用 keychain，而由调用进程显式注入 key：

```bash
CODEFACTORY_BENCH_API_KEY=<provided-by-launcher> \
...same command...
```

不要把这个 key 写入文档、日志、PR、SQLite 或 shell history。
