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

当前最新局部能力闭环来自 2026-06-30 的 6 题 score-holding provider-backed run：

- run: `6bab8a25-da1f-4d18-9e40-a19166227a2d`
- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T17-27-46Z.md`
- subset: `terminal-bench-21-score-holding-canary`
- pass: `6 / 6`
- mean reward: `1.000`
- passing tasks: `write-compressor`, `kv-store-grpc`, `filter-js-from-html`, `count-dataset-tokens`, `build-cython-ext`, `protein-assembly`
- level: `targeted task-family score-holding proof`

这个结果不是完整 89 题总分，也不是 18 题 fixed subset 结论；它证明当前六类已修 task family 能在同一个 CodeFactory provider-backed run 中聚合通过。下一步评分目标必须升级到 18 题 subset aggregate movement：超过 clean baseline `4 / 18`，而不是继续堆单题。

2026-07-02 最新 18 题 current-worktree 诊断聚合已经达到第一阶段 score-growth 目标：

- run: `afa1c9e9-c951-47fa-9dbb-26fbbf34725b`
- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-07-02T09-52-47Z.md`
- subset: `terminal-bench-21-regression-subset-v1`
- pass: `13 / 18`
- mean reward: `0.722`
- aggregate delta: previous fixed-subset diagnostic `11 / 18` -> latest `13 / 18`
- previous pass set preservation: all previous `11` passing tasks stayed reward `1`
- new passes: `torch-tensor-parallelism`, `install-windows-3.11`
- remaining reward-zero tasks: `caffe-cifar-10`, `circuit-fibsqrt`, `configure-git-webserver`, `qemu-startup`, `query-optimize`
- boundary: runner hard-timeout watchdog stopped stale `query-optimize` after `1200s`; evidence still includes local QEMU/emulation warnings and Chrome driver warnings, so this is real product-loop progress but not yet a clean official-comparable release gate.

本轮产品化改动不是 benchmark 特例答案，而是两个系统能力修复：一是 provider bridge 把 heavy verifier 任务的 Harbor `--verifier-timeout-multiplier 3` 真正下传，避免 `torch-tensor-parallelism` 在本地 verifier 900 秒默认值处被误杀；二是 model-backed agent 对 `408` / `409` / `429` / `5xx` transient provider HTTP 错误做有界重试，避免 DeepSeek 单次 `HTTP 500` 把历史通过 task 误归因成 agent 能力失败。

2026-07-02 后续 18 题 current-worktree 诊断聚合已经达到第二阶段 score-growth 目标：

- run: `c3e8a961-f2f4-4357-8dab-835b9a579b4b`
- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-07-02T15-48-42Z.md`
- subset: `terminal-bench-21-regression-subset-v1`
- pass: `14 / 18`
- mean reward: `0.778`
- aggregate delta: previous fixed-subset diagnostic `13 / 18` -> latest `14 / 18`
- previous pass set preservation: all previous `13` passing tasks stayed reward `1`
- new pass: `configure-git-webserver`
- remaining reward-zero / error tasks: `caffe-cifar-10`, `circuit-fibsqrt`, `qemu-startup`, `query-optimize`
- boundary: runner hard-timeout watchdog stopped stale `query-optimize` after `1500s`; `qemu-startup` reached a valid running VM state but continued destructive checks and killed/restarted QEMU, so its remaining blocker is state-satisfaction locking and high-risk process-operation gating. This is a real product-loop score improvement, but it still needs PR/CI, merge, deliberate release, and packaged/headless runtime verification before it is live in the user's CodeFactory product.

本轮新增产品能力不是只针对评测写死答案，而是可产品化的 agent 执行控制改进：服务/SSH/Git 类任务增加 readiness preflight 和自动修复；确认 artifact 已写入后允许停止，减少“已完成后继续读/继续破坏状态”；长任务自动修复保留 `codefactory-*-repair-ok` marker 供 verifier 归因；Windows/QEMU/Torch/HF 类任务暴露出需要 task-family timeout budget 的通用需求。下一步不能只追 `15 / 18`，还必须把这轮候选改动发布到产品中，否则分数不会转化为用户可用能力。

2026-06-30 最新 18 题 current-worktree 诊断聚合已经超过 clean baseline：

- run: `0082cd94-e9f5-479b-8ba8-5561ebd58732`
- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T18-31-05Z.md`
- subset: `terminal-bench-21-regression-subset-v1`
- pass: `9 / 18`
- mean reward: `0.500`
- passing tasks: `build-cython-ext`, `count-dataset-tokens`, `extract-elf`, `filter-js-from-html`, `kv-store-grpc`, `mteb-retrieve`, `protein-assembly`, `sanitize-git-repo`, `write-compressor`
- level: `current-worktree diagnostic aggregate improvement`
- boundary: runner hard-timeout watchdog was enabled and stopped stale `query-optimize` after `1200s`; evidence still includes local QEMU/emulation warnings, so this is real product-loop progress but not yet a clean official-comparable release gate.

同一轮失败归因中，`nginx-request-logging` 从历史通过项回落为 `tool-use`。根因不是模型单独写错，而是 CodeFactory agent 的工具环境污染了容器内服务自检：benchmark sandbox 把 `curl http://localhost:8080` 当成外网工具拒绝；放开 loopback 后，又因为 Docker apt proxy 被注入为 `HTTP_PROXY` 但未设置 `NO_PROXY`，`curl localhost` 走代理返回 `502`。

已完成服务任务通用修复：

- benchmark sandbox 允许 loopback-only `curl` / `wget` / `nc` / `netcat` / `ssh` 等自检，继续拒绝外网 host。
- Docker apt proxy 注入同时设置 `NO_PROXY` / `no_proxy=localhost,127.0.0.1,127.0.0.0/8,::1,0.0.0.0`，避免容器内 localhost 服务检查走代理。
- artifact missing repair hint 收窄为同一行 artifact path + missing error，避免 artifact 已存在时被其他 `not found` 文本误触发。
- canary run `29e515ce-08ab-4b1f-bf3d-f8ceb8cdbe9b`，report `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T18-47-19Z.md`，`nginx-request-logging` `1 / 1`，mean reward `1.000`。

按这个 canary 归因，当时 18 题诊断聚合的预期目标是 `10 / 18`；真正的下一道门槛是重新跑完整 18 题并尽量移除 watchdog/本地 verifier 不稳定因素，形成 clean aggregate gate。

该预期已经被 2026-06-30 后续 18 题 current-worktree 诊断验证：

- run: `ed478add-95a8-4c82-940d-40ce99617a84`
- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T19-56-04Z.md`
- subset: `terminal-bench-21-regression-subset-v1`
- pass: `10 / 18`
- mean reward: `0.556`
- passing tasks: `build-cython-ext`, `count-dataset-tokens`, `filter-js-from-html`, `kv-store-grpc`, `mteb-retrieve`, `nginx-request-logging`, `protein-assembly`, `sanitize-git-repo`, `torch-tensor-parallelism`, `write-compressor`
- aggregate delta: previous current-worktree diagnostic `9 / 18` -> latest `10 / 18`
- boundary: `query-optimize` still required the `1200s` watchdog and is classified as environment failure; `extract-elf` regressed from the previous diagnostic pass to verifier failure, so the next aggregate target must recover that regression and reduce long-horizon watchdog use.

本轮后续 `caffe-cifar-10` canary 没有带来分数提升，但沉淀了一个通用 agent-loop 修复：

- first canary: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T20-05-35Z.md`, run `d50e433d-486f-40a6-bb08-4567b4ecb6e3`, `0 / 1`, failure class `tool-use`
- second canary: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T20-21-02Z.md`, run `75e59480-c933-4503-b8a5-823bdc27db21`, `0 / 1`, failure class `verification`
- finding: shell pipelines using `tee` or `tail` can return `0` while stdout already contains `timeout: failed to run command`, `g++: not found`, `make: *** Error 127`, `Could not get lock`, `Unable to acquire the dpkg frontend lock`, or `Failed to fetch`.
- product fix: `codefactory-headless` now treats these as `semantic-failure`, writes structured repair goals, and avoids accepting the pipeline status as proof of success.
- interpretation: this is a trajectory-quality and wasted-loop reduction fix, not yet a score improvement. It should improve future build/install-heavy tasks, but the next score-facing run should target aggregate movement, not claim a `caffe-cifar-10` pass.

`extract-elf` 回归已经完成一个真实 score-facing 修复闭环：

- failed canary: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T20-38-18Z.md`, run `5d3d2184-8002-409e-b8f9-3eaea85cff60`, `0 / 1`, failure class `verification`
- passed canary: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T20-43-37Z.md`, run `56cab043-4016-4e5e-bfa7-e0a597def46b`, `1 / 1`, mean reward `1.000`
- finding: `PT_LOAD` segment coverage alone was not enough; the verifier also expects unsigned 32-bit integer values. JS bitwise reads coerced high-bit values to signed negatives, producing inconsistent values against the reference.
- product fix: ELF task-family hint now requires `PT_LOAD` mapping from `p_offset` to `p_vaddr` and unsigned `Buffer.readUInt32*` reads; protocol auto-repair writes a reusable `/app/extract.js` scaffold and self-checks key count plus unsigned value range.
- interpretation: this is a real CodeFactory agent capability improvement, not a DeepSeek-only result, but it is not a completed product-improvement loop until the fixed 18-task subset also moves.

`extract-elf` 修复后的第一次 18 题聚合复测没有达到预期：

- run: `e270ef52-fc6b-47fb-b9aa-b6f31d315cbe`
- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T21-23-41Z.md`
- subset: `terminal-bench-21-regression-subset-v1`
- pass: `9 / 18`
- mean reward: `0.500`
- aggregate delta: previous current-worktree diagnostic `10 / 18` -> latest `9 / 18`
- passing tasks: `build-cython-ext`, `count-dataset-tokens`, `filter-js-from-html`, `kv-store-grpc`, `mteb-retrieve`, `nginx-request-logging`, `protein-assembly`, `sanitize-git-repo`, `write-compressor`
- score-holding regressions: `extract-elf` and `torch-tensor-parallelism`
- environment/runtime boundary: `query-optimize` still ended as watchdog-stopped `RewardFileNotFoundError`; verifier warnings still include QEMU/emulation `ERROR: unknown platform bitness` and missing Chrome driver.
- root interpretation: the system has reached a real 18-task diagnostic score around `50%`, but it is unstable. A single task-family canary can pass while the aggregate regresses, so the next product target is score-holding reliability, not another isolated one-off repair.

Latest `extract-elf` evidence is now score-holding rather than only canary-level: the later 18-task rerun below imported `extract-elf` as reward `1`. The earlier generated `/app/extract.js` had already used `readUInt32LE`, produced `698` keys, `0` negative values, and max value `4294967140`; the previous failure was verifier/runtime instability where `gcc` hit `internal compiler error: Segmentation fault signal terminated program collect2` before comparing outputs. Harbor reward still controls the score, but this task is no longer the immediate P0 blocker.

`torch-tensor-parallelism` regression was product-facing: `ColumnParallelLinear` failed four verifier cases with `RuntimeError: element 0 of tensors does not require grad and does not have a grad_fn`. The root cause was that bare `dist.all_gather` detached the output path from autograd. This has now been turned into a CodeFactory adapter contract and auto-repair path:

- canary run: `64e26356-77b3-4165-b2e1-d16a42fadb79`
- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T21-49-52Z.md`
- task: `torch-tensor-parallelism`
- pass: `1 / 1`
- mean reward: `1.000`
- verifier: `13 passed`
- product fix: `ColumnParallelLinear` uses an autograd-preserving `torch.autograd.Function` all-gather whose backward returns the rank-local gradient slice; `RowParallelLinear` reduces local partial outputs and adds full zero bias after reduction.

The score-holding gate after this repair did produce aggregate movement:

- run: `7f7366c9-393e-41ca-880d-46e81d9f7616`
- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T22-28-58Z.md`
- subset: `terminal-bench-21-regression-subset-v1`
- pass: `11 / 18`
- mean reward: `0.611`
- aggregate delta: latest failed aggregate `9 / 18` -> latest improved aggregate `11 / 18`
- passing tasks: `build-cython-ext`, `count-dataset-tokens`, `extract-elf`, `filter-js-from-html`, `kv-store-grpc`, `mteb-retrieve`, `nginx-request-logging`, `protein-assembly`, `sanitize-git-repo`, `torch-tensor-parallelism`, `write-compressor`
- boundary: runner hard-timeout watchdog stopped `query-optimize` after `1200s`, so this is a real current-product diagnostic improvement but not yet a clean official-comparable release gate.

This closes the immediate “canary passed but aggregate did not move” failure mode for this loop. The next P0 is no longer torch; it is to separate long-running verifier/runtime instability from agent failures, starting with `query-optimize`, and then attack the remaining verifier-zero families.

2026-07-02 继续完成第一阶段 score-growth gate：

- torch canary run: `5dbe0915-c165-4bfd-858d-1c033ca71dcb`
- torch canary report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-07-02T08-25-20Z.md`
- full aggregate run: `afa1c9e9-c951-47fa-9dbb-26fbbf34725b`
- full aggregate report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-07-02T09-52-47Z.md`
- pass: `13 / 18`
- mean reward: `0.722`
- aggregate delta: `11 / 18` -> `13 / 18`
- score interpretation: first-stage target `>= 12 / 18` is met, and the previous `11` pass set did not regress.
- product conclusion: the next product target should be `>= 14 / 18` with repeated score-holding, not another isolated canary. The highest-leverage work is now to convert one of the remaining verifier-zero families into a real product capability while reducing local runtime noise enough that the score is reproducible.

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

2026-06-29 resource-preflighted 18-task regression 结果：

- run: `159041ce-5682-4835-843a-fbed9088aa9d`
- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T15-28-16Z.md`
- iteration report: `docs/evidence-packs/terminal-bench-21-iteration-2026-06-29T15-28-16Z.md`
- scope: `terminal-bench-21-regression-subset-v1`
- comparable: `true`
- trials: `18`
- pass: `4 / 18`
- mean reward: `0.222`
- Harbor exceptions: `0`
- result attribution: aggregate recovered from the earlier real provider-backed `0 / 18` run, but it has not exceeded the old offline projection baseline `4 / 18`.
- infrastructure movement: local Colima was raised from `2 CPU / 4GB / 30GB overlay` to `4 CPU / 8GB / 99GB overlay`, and the runner now blocks provider-backed runs unless Docker CPU, memory, root free space, and apt bootstrap smoke pass.
- current failure mix: `pass=4`, `verification=8`, `tool-use=3`, `policy=2`, `long-horizon=1` in the raw evidence. After classifier fix, verifier dependency network timeouts such as the observed MTEB `UV_HTTP_TIMEOUT` failure should route to `environment/verifier-dependency-resource`.
- conclusion: the modification loop is now real at the infrastructure/aggregate level (`0 / 18` -> `4 / 18`, exceptions eliminated), but the next product target is score movement beyond `4 / 18`.

2026-06-29 `sanitize-git-repo` score canary 结果：

- passing run: `4618230e-7c00-449b-b565-64e108822d93`
- evidence: `docs/evidence-packs/terminal-bench-21-sanitize-auto-repair-2026-06-29T17-03-26Z.md`
- iteration report: `docs/evidence-packs/terminal-bench-21-iteration-2026-06-29T17-03-26Z.md`
- task: `terminal-bench/sanitize-git-repo`
- score: `1 / 1`, mean reward `1.000`
- comparable: `true`
- failure class: `None`
- same-task delta: `0 -> 1`; prior same-task failures were `06920772-c3a4-4705-a5cf-a376925190e9` and `537c907c-e7c8-432d-81be-83a01ac255ae`.
- product conclusion: deterministic bounded repair plus stop-after-success converted a verifier failure into a pass and reduced tool calls from `42` to `9`. This is a targeted score improvement, not yet an aggregate 18-task improvement.
- next gate: rerun the fixed 18-task regression subset with the current worktree agent loaded explicitly, then decide whether the aggregate can move beyond `4 / 18`.

2026-06-29 post-`sanitize` 18-task regression 结果：

- run: `b0aa1607-fe64-4fbb-adf2-65e80962f1bd`
- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T19-10-27Z.md`
- scope: `terminal-bench-21-regression-subset-v1`
- provider bridge status: `completed`
- trials: `18`
- pass: `3 / 18`
- mean reward: `0.167`
- passing tasks: `write-compressor`, `mteb-retrieve`, `sanitize-git-repo`
- failed tasks: `14` reward-zero tasks plus `query-optimize` timeout/error counted as reward `0`
- operator note: `query-optimize` verifier ran for about `20m` with pytest still consuming about `98%` CPU; the trial container was stopped manually so Harbor could finish the queue. Treat this run as real current-worktree product evidence, but not as a clean regression-gate improvement over the previous `4 / 18` baseline.
- product conclusion: `sanitize-git-repo` fix did hold inside aggregate regression, but aggregate score regressed from the latest clean `4 / 18` baseline to `3 / 18`. The next loop must target broad failure classes rather than adding more one-off task repairs.

2026-06-30 `query-optimize` SQL-loop canary 结果：

- run: `556937a9-cb5f-4a45-af9f-8eaf1f91454a`
- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T01-49-12Z.md`
- scope: local untracked `terminal-bench-21-query-optimize-canary`
- score: `0 / 1`, mean reward `0.000`
- agent behavior delta: prior run copied the original slow SQL after a failed long heredoc/tool-call path; this run saw the correlated-subquery plan, executed automatic SQL rewrite, wrote `/app/sol.sql`, ran bounded `sqlite3` sample execution, and emitted `codefactory-sql-repair-ok`.
- blocker: official verifier still did not produce reward locally because its first correctness test executes the original slow query under Mac/QEMU; the container was stopped after the pytest process stayed at about full CPU.
- product conclusion: this is a real agent-loop improvement but not a score improvement. `query-optimize` should stay in diagnostic coverage, but it should not be the next score-facing canary on this Mac runner.

2026-06-30 `kv-store-grpc` score canary 结果：

- run: `cb4a43a8-c36b-4364-a979-ceaf983f628c`
- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T01-57-44Z.md`
- scope: local untracked `terminal-bench-21-kv-store-grpc-canary`
- comparability: `official_comparable: yes`, `trial_hard_timeout_sec: <disabled>`, no resource override
- score: `1 / 1`, mean reward `1.000`
- verifier: reward `1.0`, failure class `None`
- agent behavior delta: previous run created only proto/bindings and failed because verifier could not import `grpc`, `/app/server.py` was missing, and no port `5328` server was running. New run installs `grpcio` / `grpcio-tools` system-wide with `--no-user`, generates bindings, writes `/app/server.py`, starts the gRPC server in the background, and passes a real client self-check before verifier.
- product conclusion: this is a confirmed score improvement and a reusable service-task pattern: verifier-visible dependency install + generated interface artifacts + supervised background service + protocol-level self-check.

2026-06-30 post-`kv-store-grpc` 18-task diagnostic attempt:

- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T02-10-05Z.md`
- Harbor run: `c9c68248-c12e-474b-849e-ca71299c0a28`
- scope: `terminal-bench-21-regression-subset-v1`
- result: not an aggregate score update
- partial state: `2` completed trials, `1` errored trial, `16` pending trials, `1` cancelled trial
- failure: provider bridge returned non-zero after Harbor hit `httpx.ConnectError`; verifier logs also showed apt network fetch failures against Ubuntu mirrors.
- product conclusion: the score loop needs partial-import resilience. CodeFactory now imports completed Harbor trials even after non-zero Harbor/provider exits, persists unfinished runs as `partial_import`, and writes provider status plus partial-import notes into evidence packs.

2026-06-30 partial-import 18-task diagnostic result:

- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T02-52-55Z.md`
- Harbor run: `be0cd4c6-f7b9-41b5-879b-fdcfad8358be`
- scope: `terminal-bench-21-regression-subset-v1`
- imported trials: `7 / 18`
- partial pass: `2 / 7`
- partial mean reward: `0.286`
- passing tasks: `write-compressor`, `kv-store-grpc`
- non-comparable reason: runner hard-timeout watchdog was enabled and provider bridge failed with `httpx.ConnectError` before the full matrix completed.
- product conclusion: `kv-store-grpc` is now visible inside the 18-task matrix, and partial import prevents losing diagnostic evidence. This is not a replacement for the clean `4 / 18` aggregate baseline because `11` tasks did not complete.

2026-06-30 `filter-js-from-html` auto-repair / verifier-bootstrap diagnostic:

- run: `67a5c6ae-5504-4dec-b777-97ec583f2d73`
- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T03-06-08Z.md`
- score: `0 / 1`, mean reward `0.000`
- agent behavior delta: the prior 18-task diagnostic repeated `cat /app/filter.py` after a self-check showed `style="...javascript:..."` remained. The new run triggered HTML sanitizer auto-repair after oversized heredoc tool JSON failed, wrote a stdlib-only `/app/filter.py`, self-checked dangerous `script` / event handler / URI / `style` patterns, and emitted `codefactory-html-filter-repair-ok`.
- remaining blocker: verifier bootstrap failed before assertions because Debian apt mirror access through `198.18.0.15` could not install `curl`, and `/root/.local/bin/env` / `uvx` were missing.
- follow-up evidence: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T03-07-54Z.md` shows the strengthened runner preflight now blocks the same apt/curl verifier-bootstrap failure before Harbor/provider launch.
- product conclusion: this is an agent-loop improvement plus infrastructure-preflight improvement, not a clean score improvement. Clean scoring should resume only after Docker Debian mirror / verifier bootstrap health passes.

2026-06-30 `filter-js-from-html` score canary 结果：

- failing clean run: `a95b7996-66f6-481f-915f-4695ad29d7b0`
- failing report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T03-40-10Z.md`
- failing score: `0 / 1`, mean reward `0.000`, failure class `verification`
- failing root cause: the agent wrote `/app/filter.py` and triggered auto-repair, but the generated sanitizer rewrote clean HTML formatting/entities/self-closing tags, so `test_clean_html_unchanged` failed.
- passing run: `ba9b0f4d-3834-4415-8b44-c1a5b1a49c45`
- passing report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T04-02-30Z.md`
- score: `1 / 1`, mean reward `1.000`
- comparable: `true`
- failure class: `None`
- verifier: `test_filter_blocks_xss` and `test_clean_html_unchanged` both passed.
- product conclusion: this is a confirmed same-task score improvement. The reusable product change is not just an HTML sanitizer recipe: the agent now treats zero tool calls on recognized artifact tasks as a no-action failure, triggers deterministic auto-repair, and the recipe preserves verifier-normalized clean input while removing dangerous script/event/URI/style vectors.

2026-06-30 18-task diagnostic / score-holding canary 结果：

- 18-task diagnostic report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T06-25-27Z.md`
- 18-task diagnostic run: `16cd508e-33ba-442a-815e-51cd0f5cdfbf`
- result: not an aggregate score update; provider bridge failed with `httpx.ConnectError` after `2 / 18` imported trials.
- important finding: `write-compressor` agent self-check succeeded (`verification-ok`), but verifier failed during Ubuntu Noble apt/bootstrap (`archive.ubuntu.com` / `security.ubuntu.com` connection failures). This showed the previous Debian-only preflight was insufficient for the actual task mix.
- product fix: runner preflight now checks both Debian Bookworm and Ubuntu Noble containers for root free space, `apt-get update`, `curl` install, and `curl --version` before any provider-backed run.
- score-holding run: `fc6dc276-d3ea-4957-820e-72a7dcd6b03a`
- score-holding report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-30T08-04-02Z.md`
- score-holding scope: local `terminal-bench-21-score-holding-canary`
- score: `3 / 3`, mean reward `1.000`
- comparable: `true`
- passing tasks: `write-compressor`, `kv-store-grpc`, `filter-js-from-html`
- product conclusion: the latest single-task fixes now hold together in one official-comparable provider run. This is still not an 18-task aggregate improvement, but it is stronger than isolated single-task canaries and should be the regression sentinel before the next broad 18-task attempt.

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
- fixed subset runner hard preflight: Docker CPU, memory, root free space, and apt bootstrap smoke must pass before Harbor/provider starts; otherwise the run exits with blocker evidence instead of consuming provider tokens.
- Ubuntu verifier bootstrap smoke: preflight now covers both `python:3.10-slim-bookworm` and `ubuntu:24.04`, because Terminal-Bench tasks can use Ubuntu Noble verifier images and Debian-only apt/curl health did not catch Ubuntu mirror failures.
- verifier dependency network timeout classifier: `Failed to download distribution due to network timeout` / `UV_HTTP_TIMEOUT` now maps to `environment/verifier-dependency-resource`.
- `task_names` 固定 subset 支持。
- provider secret lookup timeout。
- 显式 `CODEFACTORY_BENCH_API_KEY` override。
- 18 题固定 regression subset。
- score-driven iteration runner。
- iteration runner wall-time timeout：评测卡住时返回 `124` 并写 report，而不是无限等待。
- verifier dependency/resource failure classifier：apt cache free-space、package index/signature、`curl` / `uvx` bootstrap 缺失等 verifier 环境问题归为 `environment/verifier-dependency-resource`。
- provider bridge partial import：Harbor/provider 非 0 退出时，只要 job directory 存在就导入已完成 trial；有 pending/running/cancelled 的 unfinished run 标记为 `partial_import`，证据包保留 provider status、exit code、partial trials 和 partial-import note。
- verifier bootstrap preflight now installs and runs `curl` in the Docker smoke container, so the observed Debian mirror / `curl` / `uvx` failure is reported as an environment blocker before provider spend.

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

1. 已完成：评测 runner 增加 per-trial verifier hard timeout 和 non-comparable 证据语义；诊断 run 不再只能靠人工 stop 才能收口。Provider bridge 也已支持 partial import，避免 Harbor/provider/network 单点失败让已完成 trials 丢失。
2. 已完成一半：`query-optimize` agent loop 已能从 correlated-subquery plan 进入 SQL rewrite auto-repair，并且不再把 `EXPLAIN QUERY PLAN` 当完成证据。剩余问题是本机 Mac/QEMU verifier 会在原始慢查询 correctness test 上卡住；这属于评测运行环境/任务可比性问题，不再作为下一轮 score canary。
3. 已完成一个 score-facing 服务任务：`kv-store-grpc` 已从 verification failure 变成 clean `1 / 1`。下一轮 score-facing：继续修 `protein-assembly`，以及 `caffe-cifar-10` / `nginx-request-logging` / `qemu-startup` 这类 tool-use failure，需要服务 readiness、长构建小样本和前后台进程模板。
4. 已完成一个 artifact-repair score-facing 任务：`filter-js-from-html` 已从 invalid oversized heredoc / `javascript:` residual self-check / zero-tool no-artifact 路径进入 deterministic sanitizer auto-repair，并在 clean official-comparable canary 中从 `0 / 1` 变为 `1 / 1`。下一步不是继续堆 HTML 特例，而是把“明确 artifact 任务的 no-action failure -> deterministic repair / scaffold”泛化到更多输出文件任务。
5. 已完成一个 score-holding gate：`write-compressor` + `kv-store-grpc` + `filter-js-from-html` 在同一个 official-comparable provider run 中达到 `3 / 3`。下一轮 broad run 之前应继续保留这个小矩阵作为快速回归哨兵。
6. 已完成一个远程数据分析 score-facing 任务：`count-dataset-tokens` 已从 default-config 误读后写 `0`，变成 clean official-comparable `1 / 1`。落地能力是 HuggingFace dataset token-count task 的 metadata-config guidance、science-domain 映射、早期 artifact pressure 调宽，以及 deterministic metadata-config repair fallback；这轮实际通过路径是 guidance + gate，`auto_protocol_repairs=0`。
7. 已完成一个 legacy source-build score-facing 任务：`build-cython-ext` 已从 `0 / 1` verifier failure 变成 official-comparable `1 / 1`。落地能力是旧 Python/Cython 包源码构建 recipe：Numpy 2.x alias repair、`fractions.gcd` repair、可选 GUI/native dependency fallback、system-global dependency install、`CFLAGS=-O0` build resilience、global install、README self-check、repo-test self-check和 marker-based loop 收口。不要再把 `count-dataset-tokens` 或 `build-cython-ext` 当待修任务，它们应该进入 broader diagnostic 的 score-holding matrix。
8. 已完成一个 biological artifact score-facing 任务：`protein-assembly` 已从缺少 `/app/gblock.txt` / 长脚本 JSON 失效 / 反复 PDB 探索变成 provider-backed `1 / 1`。落地能力是明确 gBlock artifact 任务的短 Python generator、翻译/顺序/linker/长度/GC 自检、PDB/API/缺 Biopython/重复读取触发 deterministic repair，以及可选 Docker apt proxy 注入来稳定本机 verifier bootstrap。
9. 已完成 6 题 score-holding clean gate：先通过 `write-compressor` verifier uvx/proxy 修复把单题从 `0 / 1` 拉到 `1 / 1`，再通过 HF token-count 依赖/metadata 修复把 `count-dataset-tokens` 单题稳定到 `1 / 1`，最终 run `6bab8a25-da1f-4d18-9e40-a19166227a2d` 在 `write-compressor`、`kv-store-grpc`、`filter-js-from-html`、`count-dataset-tokens`、`build-cython-ext`、`protein-assembly` 上达到 `6 / 6`、mean reward `1.000`。这证明当前六个 task-family 修复可以在同一个 provider-backed CodeFactory run 中聚合生效，不再只是单题 smoke。
10. 当前 6 题结果仍有解释边界：本机 QEMU/emulation 下 verifier warnings 包含 `browser-driver-unavailable` 和 `ERROR: unknown platform bitness`，尤其 `filter-js-from-html` 的 Selenium/Chrome driver 缺失会削弱浏览器类断言解释。分数记录为有效 Harbor reward，但产品下一步必须把 browser/verifier runtime 稳定化作为 P0 评测基础设施任务，而不是把 `6 / 6` 等同于所有 runtime 风险已解决。
11. 最新 18 题聚合复测已经达到 `14 / 18`、mean reward `0.778`，从上一轮 `13 / 18` 继续提升，并且历史通过项没有回退。`configure-git-webserver` 已在同一固定 subset 中从 reward `0` 变成 reward `1`。
12. 当前下一轮 P0 顺序改为：先把 `14 / 18` 候选改动走完产品交付链，完成 PR/CI、合并、刻意发版和真实 packaged/headless runtime 验证；然后把 `14 / 18` 做成可重复 score-holding，再选择一个 remaining failure family 冲到 `>= 15 / 18`。优先级是：`qemu-startup` 的状态满足后停止/高风险进程操作门、`query-optimize` verifier watchdog/root-cause 分离与可比性改造、`caffe-cifar-10` 的真实构建/训练小样本闭环、`circuit-fibsqrt` 的逻辑综合与自检生成。`configure-git-webserver`、`install-windows-3.11` 和 `sparql-university` 已在最新 18 题 aggregate 中通过，进入 score-holding 集合。

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
- curl/bootstrap preflight：Docker smoke 必须在 Debian Bookworm 和 Ubuntu Noble 中都能 `apt-get install curl` 并执行 `curl --version`；否则 `curl` / `uvx` verifier bootstrap 或 Ubuntu apt mirror 缺失会伪装成 agent 低分。
- concurrency/resource policy：根据 Docker CPU/memory 和 subset task mix 给出默认 concurrency 或 hard block；当前 `4 CPU + concurrency=4` 可以完成，但 MTEB verifier dependency download 在并发下仍不稳定。
- verifier hard timeout：每个 trial 的 verifier 必须有可配置硬超时，超时记录 `verification-timeout`、reward `0`、保留 stdout tail 和容器状态，并继续剩余队列；禁止再次出现人工 stop 才能收尾的完整评测。
- partial import：provider bridge 失败时仍导入 Harbor 已完成 trials，run status 使用 `partial_import`，这样 18 题诊断至少能留下可评分样本和失败归因。
- verifier/runtime instability classifier：如果 verifier 自身在测试目标产物前崩溃，例如 `gcc internal compiler error`、Chrome driver unavailable、QEMU/proc/netlink limitation、missing reward file from stopped verifier，evidence 必须区分 `artifact looked valid but verifier runtime failed` 与 `artifact failed verifier assertion`。这不改变 Harbor reward，但会决定产品改进队列优先级。
- score-holding aggregate gate：每个 score-facing canary 通过后必须跑固定 18 题或明确 blocker；如果 aggregate 下降或历史通过项回落，下一轮 P0 必须先修回归或分类 runtime instability，不能继续声明该 slice 完成。

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

- 当前 fixed subset 最新真实聚合是 run `c3e8a961-f2f4-4357-8dab-835b9a579b4b` 的 `14 / 18`、mean reward `0.778`。它高于最早 full-run 离线投影基线 `4 / 18`，也高于上一轮当前 worktree 诊断 `13 / 18`，所以当前水平已经进入可产品化候选阶段，但仍不是 clean official-comparable gate。
- score-holding 必须维护最新 `14 / 18` pass set：`build-cython-ext`、`configure-git-webserver`、`count-dataset-tokens`、`extract-elf`、`filter-js-from-html`、`install-windows-3.11`、`kv-store-grpc`、`mteb-retrieve`、`nginx-request-logging`、`protein-assembly`、`sanitize-git-repo`、`sparql-university`、`torch-tensor-parallelism`、`write-compressor` 不得回退。
- 下一道有效产品门槛是先让这轮 `14 / 18` 候选进入真实产品发布链；发布后再跑同口径 fixed subset 证明 live build 仍达到 `>= 14 / 18` 且无新增历史通过项回归。随后把 `>= 15 / 18` 作为第三阶段 score-growth 目标。
- 完整 89 题只有在 18 题 fixed subset 发布版稳定 `>= 14 / 18`、`query-optimize` 这类长 verifier 能被 cleanly classified、并且本机 verifier warnings 有明确解释后才值得重跑。否则完整 89 题会继续混淆 agent 能力、运行环境和长尾 verifier 卡死。
- 完整 89 题 pass 从 `6 / 89` 提升到 `15 / 89`，才算第一阶段 agent loop 改进有效。
- 同时记录 cost 和 duration，避免靠无限重试换分数。

## 下一次真实评估命令

默认产品诊断 run 用固定 subset runner，它会做 Docker/apt preflight、从本地 settings/keychain 取 DeepSeek endpoint，并写 evidence pack。这个模式启用 runner-level trial hard timeout，因此不是官方 clean comparable，但能防止 `query-optimize` 这类 verifier 长挂死整个矩阵：

```bash
PYTHONPATH="$PWD" python3 tools/benchmark/run_terminal_bench_21_regression_subset.py --secret-timeout-sec 20 --concurrency 2
```

如果要做 clean comparable gate，必须显式关闭 runner hard timeout；这样如果 verifier 挂住，结果仍可能无法自动收口：

```bash
PYTHONPATH="$PWD" python3 tools/benchmark/run_terminal_bench_21_regression_subset.py --secret-timeout-sec 20 --concurrency 2 --trial-hard-timeout-sec 0
```

如果不使用 keychain，而由调用进程显式注入 key：

```bash
CODEFACTORY_BENCH_API_KEY=<provided-by-launcher> \
...same command...
```

不要把这个 key 写入文档、日志、PR、SQLite 或 shell history。
