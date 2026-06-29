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

2026-06-29 已完成第一次真实 fixed subset provider-backed rerun：

- run: `e7d97f76-b1d1-4b08-beb7-08181a1f5a1e`
- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T03-36-45Z.md`
- agent: `codefactory-headless`
- model backend: `deepseek-v4-pro`
- pass: `0 / 18`
- mean reward: `0.000`
- result attribution: `codefactory-agent-capability`

这个结果说明固定 subset 的真实 provider-backed 当前状态低于离线投影基线。它不是 DeepSeek 单独能力结论，而是 CodeFactory headless agent loop、tool policy、验证修复和环境 preflight 的系统性能力结论。后续不再把“跑通一次”作为目标，必须进入“hypothesis -> canary/subset -> delta -> improvement queue”的迭代闭环。

2026-06-29 首轮 score-driven tool-use canary 结果：

- 完整 canary run: `77e98d56-2638-4b0c-a941-a84b542d51ff`
- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T06-58-36Z.md`
- scope: `terminal-bench-21-canary-subset-v1`
- pass: `0 / 4`
- mean reward: `0.000`
- failure class: `tool-use` for all four tasks
- result attribution: `codefactory-agent-capability`

首轮改动后的 bounded canary 进一步暴露了评测 runner 和 agent 策略问题：

- report: `docs/evidence-packs/terminal-bench-21-canary-timeout-2026-06-29T07-15-12Z.md`
- runner exit: `124`
- timeout: `360s`
- partial Harbor state: `2 / 4` completed, completed reward `0 / 2`
- conclusion: repeated-inspection suppression、artifact gate、semantic failure detection 能改变轨迹并提升可观测性，但还没有把被拦截状态转成有效实现策略，因此没有分数提升。

2026-06-29 forced implementation transition canary 结果：

- report: `docs/evidence-packs/terminal-bench-21-forced-transition-timeout-2026-06-29T08-01-36Z.md`
- runner exit: `124`
- timeout: `360s`
- partial Harbor state: `1 / 4` completed, completed reward `0 / 1`
- observed behavior: `write-compressor` 真实触发 `3` 个 forced-implementation prompt，并触发 `auto-repair-ok` 写出 `/app/data.comp` `2476` bytes，但 verifier dependency setup 因 apt cache 空间不足、`curl` 缺失和 `uvx` 缺失失败，reward 为 `0.0`
- conclusion: prompt-only forced transition 已经被证明不足；下一步必须做 constrained implementation mode 或 deterministic scaffold，而不是继续叠自然语言提醒。同时 verifier dependency/resource failure 必须从 agent tool-use failure 中分离。

2026-06-29 constrained implementation single-task 结果：

- before report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T12-03-31Z.md`
- after report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T12-07-06Z.md`
- comparison: `docs/evidence-packs/terminal-bench-21-constrained-scaffold-2026-06-29T12-07-06Z.md`
- task: `terminal-bench/write-compressor`
- before runtime: `228.60s`
- after runtime: `112.72s`
- delta: `-115.88s`, about `50.7%` faster
- failure class: stable `environment`
- score: unchanged at `0.000`
- conclusion: constrained implementation mode closed one modification loop: it reduced wasted model/probe time and reached artifact-producing scaffold faster. The next blocker is verifier environment/resource readiness, not this task's artifact generation path.

2026-06-29 `mteb-retrieve` environment/agent-loop canary 结果：

- comparison report: `docs/evidence-packs/terminal-bench-21-mteb-cache-artifact-gate-2026-06-29T12-41-55Z.md`
- latest run: `addff8cf-2249-4e6c-8463-cc919a1eed93`
- latest report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T12-41-55Z.md`
- task: `terminal-bench/mteb-retrieve`
- runtime: `227.18s` -> `57.17s`
- latest tool calls: `5`
- score: unchanged at `0.000`
- behavior: `/app/result.txt` is now extracted as the artifact target, the agent writes the expected line, and `Artifact completion gate` stops further tool use.
- remaining blocker: verifier bootstrap still fails with missing `curl`, `/root/.local/bin/env`, and `uvx`.
- conclusion: this is a verified agent-loop improvement but not a scoring improvement. The next score-facing step must repair or preflight verifier bootstrap dependencies before running broader regression.

2026-06-29 `mteb-retrieve` scoring canary 结果：

- diagnostic storage override run: `0224b9ba-e6f4-4b45-8bd8-1249b8911561`
- diagnostic report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T12-58-09Z.md`
- diagnostic result: reward `0.0`, failure class `environment`, `official_comparable: no`
- root cause: local Docker overlay was full (`30G / 30G`, `100%`), causing apt package index/signature and verifier bootstrap failures; a clean `python:3.10-slim-bookworm` `apt-get update` smoke passed after targeted cleanup of unused Terminal-Bench images.
- implementation changes: MTEB guidance now uses `mteb.get_model("BAAI/bge-small-zh-v1.5", revision=...)`, `task_name="SciFact"`, and `PromptType.query` / `PromptType.passage`; `Implementation hint:` messages survive context compaction; benchmark shell timeout default is `300s`.
- passing run: `5a4e758d-f949-40ba-8f2d-e0017fa9b722`
- passing report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T13-33-52Z.md`
- iteration report: `docs/evidence-packs/terminal-bench-21-iteration-2026-06-29T13-33-52Z.md`
- score: `1 / 1`, mean reward `1.000`
- comparable: `true`
- failure class: `None`
- conclusion: this is the first confirmed targeted score improvement in the iteration loop. It is not an aggregate 18-task or 89-task score movement; the next gate must rerun the fixed 18-task regression subset.

## 问题分层

### 1. 评测基础设施还不够产品化

已看到的问题：

- 早期 cost/token 为空，无法衡量同等分数下的成本效率。
- Harbor exception、环境资源、agent 执行失败、模型 provider 失败容易混在一起。
- macOS keychain 授权会让真实 regression run 卡在 Harbor spawn 前。
- 没有固定 subset 时，单题 smoke 容易被误解为能力改善。

当前已落地：

- provider usage capture。
- canary iteration reports now mark mismatched trial-count comparisons as `comparable_delta: no`, preventing single-task canaries from being reported as aggregate 18-task score deltas.
- explicit storage override plumbing for infrastructure diagnosis only, with `official_comparable: no` when a resource override is used.
- `failure_reason`。
- Docker CPU preflight。
- Docker storage/rootfs failure has now been proven as a real verifier blocker; storage/apt-smoke preflight must join CPU preflight before future score runs.
- `task_names` 固定 subset 支持。
- provider secret lookup timeout。
- 显式 `CODEFACTORY_BENCH_API_KEY` override。
- 18 题固定 regression subset。
- score-driven iteration runner。
- iteration runner wall-time timeout：评测卡住时返回 `124` 并写 report，而不是无限等待。
- verifier dependency/resource failure classifier：apt cache free-space、package index/signature、`curl` / `uvx` bootstrap 缺失等 verifier 环境问题归为 `environment/verifier-dependency-resource`。

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
- command-not-found preflight。
- `return_code=0` 但输出包含 `ERROR` / traceback / no-space-left 等语义失败时，记录 `semantic-failure` 并生成 repair goal。
- forced implementation transition prompt：当 `implementation-required` / `artifact-required` 触发后，把模型切到“下一条命令必须产生产物”的结构化提示，并记录 `forced-implementation` 轨迹节点。
- constrained implementation mode：在 `write-compressor` family 中，当 artifact/implementation block 或 no-action recovery 已经具备 decompressor context 时，系统直接运行 deterministic C scaffold，不再继续消耗模型轮次等待自觉转向。

下一步要做：

- 将 constrained implementation mode 从 `write-compressor` family 扩展到更多有明确 artifact recipe 的任务；没有安全 recipe 的任务先只拒绝 probe-only 命令并生成结构化 implementation plan。
- 增加 max-blocks escape hatch：同一任务多次被 artifact gate 拦截后，不再继续把控制权交给自由探索，而是进入 constrained implementation prompt 或自动生成最小 scaffold。
- 扩展 verifier/resource preflight：`write-compressor` 这次 `/app/data.comp` 已写出且小于 byte limit，但 verifier 因 apt/cache/dependency bootstrap 失败给 `0.0`；这类结果不能算 agent artifact repair 失败。
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

当前落地的迭代入口：

- `tools/benchmark/terminal_bench_21_iteration_loop.py`
- 默认 canary：`docs/benchmark-subsets/terminal-bench-21-canary-subset-v1.json`
- 输出：`docs/evidence-packs/terminal-bench-21-iteration-*.md`
- 每轮必须声明：`hypothesis`、`target_failure_class`、`scope`、baseline/head evidence、delta 和 next improvement queue。

推荐循环：

1. 选一个目标 failure class，例如 `tool-use`。
2. 写最小产品能力改动，例如减少重复 inspection 或强化 artifact-first 执行。
3. 先跑 canary iteration。
4. canary 有正向行为 delta 后再跑 18 题 regression subset。
5. 根据 iteration report 更新下一轮 improvement queue。

当前下一轮 P0：

1. 把 Docker overlay/storage、apt bootstrap smoke、Harbor cache footprint 加入 benchmark preflight；失败时直接生成 `environment` blocker report，不启动 provider-backed scoring run。
2. 在当前 PR 合并前后跑一次固定 18 题 regression subset，建立 canary 改动后的 aggregate score；没有这个 same-scope 结果，不得声称总体能力提升。
3. 针对 passing MTEB 轨迹里的剩余低效，产品化三条 agent-loop 规则：保留任务族实现 hint、减少重复 artifact inspection、final 前只允许 bounded verification 或明确 blocker。

首轮 canary 的具体调整：

1. 不再继续单纯加 blocker。已有 blocker 能证明坏行为，但不能自动产生好实现。
2. 下一轮优先做 `constrained implementation mode`：当 inspection budget 或 artifact gate 触发后，把模型从自由工具调用切到结构化实现模式；如果仍继续 probe，系统应拒绝并执行 scaffold/recipe，而不是只提示。
3. canary gate 保持 4 题，但 runner 必须带 `--run-timeout-sec`，timeout 也要进入 evidence。

## 优先级路线

### P0: 让评测本身可靠

目标：任何一次失败都能被归到 environment、provider、credential、agent、verification 或 tool-use，不再出现无法解释的挂起。

交付：

- credential/keychain blocker UI：Home 增加 `能力评测` 入口，`Benchmarks / Terminal-Bench 2.1` 页面展示 environment probe、provider bridge preview、credential blocker、run status 和 imported failure reason。
- explicit key injection path：`CODEFACTORY_BENCH_API_KEY` 由启动进程显式注入时跳过 OS credential lookup，仍不进入 preview、日志、SQLite、Harbor args 或 evidence pack。
- subset runner 一键命令：`tools/benchmark/run_terminal_bench_21_regression_subset.py` 读取固定 18 题 subset 并生成 success/blocker evidence。
- result import 后展示 failure reason：Rust importer 持久化 `failure_reason`，前端 Benchmark 页面按 failure reason 聚合并展示 trial 列表。
- storage/bootstrap preflight：在 Harbor/provider 启动前执行 Docker rootfs free-space、apt update smoke、Harbor job-root footprint 检查；否则当前 MTEB 这类环境问题会伪装成 agent 低分。

### P1: 把 exception 变成可修复失败

目标：减少 Harbor exception 数量，让更多任务进入 verifier failure 或 pass。

交付：

- exec-error recovery。
- service supervision templates。
- long command policy：无界 `tail -f`、`watch`、长 `sleep`、未设 sample/step bound 的训练/benchmark 命令会被 suppress，并要求 `timeout`、小样本或 deterministic health/self-check。
- background process lifecycle record：后台服务命令记录 log、pid、readiness check 是否存在，并进入 trajectory metadata。

### P2: 提升 verifier repair

目标：让 agent 能根据失败输出修改实现，而不是一次失败后结束。

交付：

- verifier output parser：从 assertion、traceback、missing tool、crash、exec-error、service lifecycle 和 bounded-command failure 中提取结构化修复目标。
- repair_goal message：trajectory 写入 `repair-goal`，并把 kind、failure、next_action、smallest_rerun 回灌给模型。
- final-before-verify gate：产物已生成但未运行 bounded verification 时，final answer 会被拦住，要求先跑最小验证。
- reusable repair recipes：首批 recipe 覆盖 artifact protocol、service lifecycle、bounded command、missing tool、assertion failure 和 crash；`write-compressor` 仍保留 task-specific auto-repair，后续以真实 subset delta 决定是否继续泛化。

### P3: 提升真实得分

目标：先在 18 题 subset 上证明改善，再回到完整 89 题。

建议 gate：

- 当前 targeted canary gate 已经达到 `mteb-retrieve` `1 / 1`、mean reward `1.000`；下一步必须跑 18 题 fixed subset，不能继续用单题结果代表系统能力。
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
