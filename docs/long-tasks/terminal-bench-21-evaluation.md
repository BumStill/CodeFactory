# Terminal-Bench 2.1 Evaluation Long Task

## Basics
- Task ID: CF-LT-TB21
- Title: Terminal-Bench 2.1 ability evaluation system
- Feature spec: `docs/specs/feature-specs/terminal-bench-21-evaluation.md`
- Related Req IDs: CF-TB-R1, CF-TB-R2, CF-TB-R3, CF-TB-R4, CF-TB-R5, CF-TB-R6, CF-TB-R7

## Completion Standard
- Done means: CodeFactory can run or import a Terminal-Bench 2.1 Harbor job, persist run/trial/artifact evidence, classify failures, show capability profile, and compare a regression subset across builds with real evidence.
- Blocked means: Harbor/Docker/dataset/agent-adapter/runtime access prevents real smoke verification, with exact command, error, and next action recorded.

## Current State
- Current phase: implementation slice 7
- Current checkpoint: full Terminal-Bench 2.1 CodeFactory agent capability run completed and imported. Latest full run is run id `7ff6ef13-4488-4e0f-afd0-a1f9bd16d561`, agent `codefactory-headless`, model backend `deepseek-v4-pro`, dataset `terminal-bench/terminal-bench-2-1`, `89 / 89` trials completed, mean reward `0.06741573033707865`, pass count `6 / 89`, failed count `83 / 89`, exceptions `63`. This is the first full CodeFactory-run score and it is a low capability baseline, not an acceptable product level. The latest score-driven canary on `mteb-retrieve` verified an agent-loop behavior improvement from `227.18s` to `57.17s` and `5` tool calls with `/app/result.txt` artifact completion gating, but reward remains `0.0` because the verifier bootstrap environment still lacks `curl`, `/root/.local/bin/env`, and `uvx`.
- Next owner: development / QA
- Updated at: 2026-06-29

## Completed Items
- Verified current external Terminal-Bench 2.1 run surface from official sources.
- Confirmed Terminal-Bench 2.1 uses Harbor dataset `terminal-bench/terminal-bench-2-1`.
- Confirmed official leaderboard marks Terminal-Bench 2.1 live and notes submissions may not modify timeouts/resources.
- Drafted business design, architecture design, UX design, and feature spec.
- Added development-time benchmark cadence to the feature spec: baseline, PR planning, inner-loop smoke, targeted subset, regression subset, scheduled main run, and release-candidate run.
- Implemented Terminal-Bench 2.1 benchmark profile and Harbor/Docker environment probe.
- Implemented fake Harbor job import into benchmark run/trial SQLite tables.
- Implemented basic reward/evidence-based failure classification for imported trials.
- Exposed backend commands for listing benchmark profiles, probing the environment, and importing benchmark results.
- Installed Harbor 0.15.0 and Docker/Colima locally for the first real smoke evaluation.
- Ran Terminal-Bench 2.1 oracle smoke against dataset `terminal-bench/terminal-bench-2-1` with `-l 1`.
- Imported the real Harbor job artifact through the CodeFactory Rust benchmark importer.
- Corrected Harbor command semantics in docs and code: `-l` is task limit, `-k` is attempts.
- Implemented minimal Harbor custom agent adapter `codefactory_bench.agent:CodeFactoryAgent`.
- Added Python adapter smoke test using Harbor's Python environment.
- Ran Terminal-Bench 2.1 CodeFactory-owned baseline adapter smoke against dataset `terminal-bench/terminal-bench-2-1` with `-l 1`.
- Fixed Harbor custom-agent import so run-level agent identity falls back to trial `agent_info`.
- Upgraded `codefactory_bench.agent:CodeFactoryAgent` to a headless runner with explicit `CODEFACTORY_BENCH_*` model configuration, OpenAI-compatible chat-completions loop, `run_shell` tool calls through Harbor `BaseEnvironment.exec`, trajectory output, and `benchmark-sandbox` command denial.
- Added model-backed adapter tests using a fake OpenAI-compatible server and fake Harbor environment.
- Implemented backend provider bridge commands: preview current endpoint/model with redacted env and authorization phrase, then start Harbor only after exact authorization while temporarily injecting the provider key into child process env.
- Added Rust provider bridge tests for DeepSeek direct endpoint normalization, redacted preview, authorization-before-secret-lookup, and child-env-only secret injection.
- Added `docs/principles/systematic-agent-evaluation.md` and wired Terminal-Bench docs to the evaluation matrix: CodeFactory agent capability, model-backend ablation, agent-scaffold comparison, and evaluation-infrastructure smoke.
- Added Harbor binary discovery fallback for product/runtime environments where `harbor` is installed by uv tool but `~/.local/bin` is not on `PATH`.
- Added an ignored Rust real-smoke test that loads local CodeFactory settings, uses the provider bridge authorization phrase, starts Harbor, and imports the resulting job without printing raw provider keys.
- Ran the first real CodeFactory provider-backed Terminal-Bench 2.1 smoke with `endpoint=deepseek`, `model=deepseek-v4-pro`, `agent=codefactory-headless`, `task_limit=1`, and `trial_count=1`.
- Updated failure classification so provider/API errors such as `HTTP 402 Insufficient Balance` are recorded as `model-provider` instead of generic `planning`.
- Reran the same CodeFactory provider-backed smoke after DeepSeek funding was restored and produced the first valid model-backed CodeFactory agent result: `agent=codefactory-headless`, `model=deepseek-v4-pro`, reward `0.0`, no provider exception, failure class `verification`.
- Fixed `benchmark-sandbox` false positives where heredoc source text containing strings such as `nc` or `curl` was misclassified as a real network command.
- Added artifact hints, remaining-budget reminders, internal wall-clock timeout, shorter tool timeout support, and incremental `trajectory.json/jsonl` writes for model-backed runs.
- Reran provider-backed smoke after the loop fixes; latest run creates `/app/data.comp`, passes the artifact existence and size checks, and fails only on decompression correctness.
- Hardened the model-backed loop with compact chat payloads, repeated inspection suppression, semantic repair hints for crashes/missing tools/missing artifacts, exact-stdout verification hints extracted from task text, and phase reminders that push the model from inspection into implementation.
- Converted provider `IncompleteRead` / remote disconnect failures into controlled model request timeouts so Harbor does not misclassify transient model transport failures as environment failures.
- Reran real CodeFactory provider-backed smoke after loop hardening; latest completed trial has no Harbor exception and imports as a comparable verification failure with reward `0.0`.
- Added mechanical artifact-state enforcement in the model-backed loop: no-action recovery, model-timeout retry prompt, implementation-required gate, artifact-command-required gate after repeated blocked inspection, compound read-only command detection, and provider `tool_choice` compatibility fallback.
- Reran real CodeFactory provider-backed smoke after artifact enforcement; latest completed trial creates `/app/data.comp`, passes artifact existence, fails decompression with `Segmentation fault (core dumped)`, and fails the 2500 byte size limit.
- Added C-based protocol auto-repair for the `write-compressor` failure family after bad candidate/self-check failure, avoiding Python runtime assumptions inside the task container.
- Reran real CodeFactory provider-backed smoke after protocol auto-repair; latest completed trial gets reward `1.0` on `terminal-bench/write-compressor`.
- Ran the first full Terminal-Bench 2.1 CodeFactory provider-backed evaluation using `codefactory-headless` with DeepSeek backend over all 89 tasks.
- Imported the full Harbor job with CodeFactory's importer; importer reported `comparable=true`, `trials=89`, and preserved per-trial failure classes.
- Corrected provider bridge semantics so Harbor concurrency is exposed as `concurrency`; the old `trial_count` field remains only as a backward-compatible alias because Harbor `-n` is concurrency, not repeated trial count.
- Added provider usage capture in the headless adapter: future model-backed runs write `usage.json`, include aggregate usage in `trajectory.json`, and attach usage to context metadata when the provider reports it.
- Added fine-grained `failure_reason` import/persistence and Docker CPU resource preflight so environment/resource failures are separated from agent execution failures before scoring product changes.
- Added fixed regression subset `docs/benchmark-subsets/terminal-bench-21-regression-subset-v1.json` and provider bridge `task_names` support using Harbor `--include-task-name`.
- Added first long-horizon resilience improvement: model-backed `environment.exec` exceptions are recorded as `exec-error` trajectory entries with `command-timeout` / environment / runtime detail and metadata counters, then returned to the model for recovery instead of aborting the whole Harbor trial.
- Added foreground service supervision guard: obvious long-running service commands are suppressed unless they are backgrounded/supervised, with a repair prompt requiring log redirection, pid capture, and bounded readiness checks.
- Added first verifier-repair improvement: pytest/assertion/traceback style failed self-check output now produces a concrete repair reminder requiring implementation changes and a rerun of the smallest failing check before final answer.
- Added provider credential-access timeout for benchmark bridge: macOS keychain reads now use a bounded, killable `security` subprocess for benchmark launch, so keychain authorization hangs become explicit infrastructure blockers instead of indefinite waits.
- Added explicit benchmark secret override support: if the launching process already provides `CODEFACTORY_BENCH_API_KEY`, provider bridge uses that in-memory value for the Harbor child process and skips OS credential lookup; the key is still not printed, previewed, persisted, or put into Harbor args.
- Added fixed subset runner/report script `tools/benchmark/run_terminal_bench_21_regression_subset.py`; it reads the 18-task subset JSON, runs the provider bridge path, and writes success or blocker evidence packs without printing raw keys.
- Completed P0 evaluation reliability slice: provider credential/keychain failures now return typed `status=blocked`, `failure_kind=credential` results for the Benchmark UI instead of being conflated with Harbor/agent failure; Home now exposes `能力评测`, and the Terminal-Bench page shows probe, provider preview, run blocker, import summary, failure reason counts, and trial-level failure reason.
- Completed P1 exception-to-repair slice: the headless runner now suppresses unbounded long commands, records background service lifecycle signals, and keeps service supervision / command-timeout outcomes in trajectory metadata instead of letting them disappear into Harbor exceptions.
- Completed P2 verifier-repair slice: failed self-checks now produce structured `repair-goal` trajectory entries with kind/failure/next action/smallest rerun, and final answers are gated until a candidate artifact has a bounded verification attempt.
- Completed first real fixed-subset provider-backed rerun after task-name normalization: run `e7d97f76-b1d1-4b08-beb7-08181a1f5a1e`, subset `terminal-bench-21-regression-subset-v1`, agent `codefactory-headless`, model backend `deepseek-v4-pro`, `0 / 18` pass, mean reward `0.000`, evidence `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T03-36-45Z.md`.
- Added the score-driven iteration entrypoint `tools/benchmark/terminal_bench_21_iteration_loop.py`, which records hypothesis, target failure class, canary/regression scope, baseline/head delta, and next improvement queue in `docs/evidence-packs/terminal-bench-21-iteration-*.md`.
- Ran the first score-driven tool-use P0 canary iteration after repeated-inspection and preflight changes: run `77e98d56-2638-4b0c-a941-a84b542d51ff`, `0 / 4` pass, mean reward `0.000`, failure class `tool-use` for all canary tasks.
- Added harder artifact-command gating, semantic failure detection for `return_code=0` pipelines with failure text, and bounded iteration-runner timeout support.
- Reran the canary with bounded timeout; the runner returned explicit `124` after `360s` instead of hanging, with partial Harbor state `2 / 4` completed and `0 / 2` pass. Evidence shows the new gates changed trajectory behavior but did not improve score.
- Added forced implementation transition prompts after `implementation-required` / `artifact-required` blocks, with trajectory and metadata coverage.
- Reran the canary with forced transition enabled; the runner returned explicit `124` after `360s`, with partial Harbor state `1 / 4` completed and completed reward `0 / 1`. `write-compressor` recorded `3` forced-implementation prompts and an `auto-repair-ok` that wrote `/app/data.comp` at `2476` bytes, but verifier reward remained `0.0` because verifier dependency setup hit apt cache free-space / missing `curl` / missing `uvx` errors.
- Added failure-classifier coverage for verifier dependency/resource failures so apt cache exhaustion and missing verifier dependency bootstrap tools are attributed to `environment/verifier-dependency-resource` before generic `tool-use` missing-command rules.
- Added constrained implementation mode for `write-compressor`: after artifact/implementation blocks or no-action recovery with decompressor context, CodeFactory runs the existing C scaffold directly instead of waiting for more model probing.
- Reran single-task `write-compressor` canary after constrained no-action support: run `5b1c540d-56ab-4be2-afcb-ee3521b013d6`, `0 / 1` pass, failure class `environment`, runtime `112.72s`. Compared with the immediately previous single-task run `234859fc-085f-4492-9083-c883a4a39d13` at `228.60s`, this is a `115.88s` / about `50.7%` runtime reduction with stable environment attribution.
- Routed model-backed tool execution caches, pip user installs, HuggingFace cache, sentence-transformer cache, and temp files to `/logs/agent` to avoid task-container overlay exhaustion during model/dataset tasks.
- Fixed canary iteration reporting so single-task canaries are marked `comparable_delta: no` against the 18-task baseline instead of presenting a misleading aggregate score delta.
- Added MTEB 1.36 repair guidance for `SentenceTransformerWrapper.encode()` requiring `task_name`, artifact hint extraction for `/app/result.txt`, and artifact completion gating after successful expected-artifact writes.
- Reran `mteb-retrieve` canaries after the environment and artifact-loop fixes. Latest run `addff8cf-2249-4e6c-8463-cc919a1eed93` completed in `57.17s`, used `5` tool calls, wrote `/app/result.txt`, and triggered `Artifact completion gate`; reward stayed `0.0` with failure class `environment` because verifier bootstrap still reports missing `curl`, `/root/.local/bin/env`, and `uvx`. Evidence: `docs/evidence-packs/terminal-bench-21-mteb-cache-artifact-gate-2026-06-29T12-41-55Z.md`.

## Remaining Items
- Use `terminal-bench-21-regression-subset-v1` as the default targeted/regression scope for the next agent-loop PR.
- Use `tools/benchmark/terminal_bench_21_iteration_loop.py --scope canary --hypothesis <...>` as the default first gate for each agent-loop improvement before spending a full 18-task run.
- Add cost calculation once provider pricing metadata is available; current adapter captures provider-reported token usage but does not price it.
- Continue long-horizon execution work beyond P1: broaden service readiness templates and use real subset deltas to tune the long command policy.
- Continue verifier-driven repair work beyond P2: add task-family specific parsers only after the generic `repair-goal` mechanism shows which failure shapes remain frequent.
- Generalize post-candidate repair further after fixed subset rerun; current P2 includes generic repair-goal recipes plus the existing `write-compressor` task-specific auto-repair.
- Use `docs/plans/terminal-bench-21-system-improvement-plan.md` as the first score-driven improvement roadmap after the fixed subset rerun produces a comparable delta.
- Add persisted run fields/UI for evaluation axis, evaluation subject, fixed variables, changed variables, and result attribution.
- Promote `benchmark-sandbox` from adapter-local command gate to shared CodeFactory policy preset with run/task/container binding.
- Expand Benchmarks UI beyond P0: add historical run comparison, capability profile trends, and direct evidence-pack export.
- Compare at least one same-scope baseline/head subset after a score-facing implementation change.
- Fix or preflight verifier bootstrap dependencies for tasks that require `curl`, `/root/.local/bin/env`, and `uvx`; current agent-loop canaries can produce correct artifacts but still score `0.0` when verifier dependency setup fails.
- After verifier bootstrap is fixed, rerun the same `mteb-retrieve` single-task canary first, then the 18-task regression subset only if the canary reward or failure class improves.

## Blockers
- No current blocker for the already completed local full-run evaluation. The current full-run result is a valid CodeFactory agent capability baseline, not a provider/account blocker.
- No current blocker for local fixed-subset evaluation: the 2026-06-29 provider-backed 18-task run completed and imported. The score is a valid low CodeFactory agent capability result, not a credential/provider blocker.
- Official leaderboard submission process is separate from local evaluation and not covered by this first implementation slice.

## Evidence
- Local evidence: design docs, feature spec, benchmark backend commands, fake Harbor job importer tests, real Harbor job import test, and governance validators.
- First real smoke: `harbor run -d terminal-bench/terminal-bench-2-1 -a oracle -l 1 -n 1 -o .codefactory/benchmark-jobs --job-name cf-tb21-oracle-smoke-20260627-1116 -y`.
- First real smoke result: run id `1e7185f0-68b1-4c74-b45b-bfbc3373010b`, task `terminal-bench/write-compressor`, reward `1.0`, mean `1.000`, exceptions `0`, runtime `4m 11s`.
- First real import evidence: `CODEFACTORY_BENCHMARK_JOB_PATH=.codefactory/benchmark-jobs/cf-tb21-oracle-smoke-20260627-1116 cargo test benchmark::tests::import_harbor_job_from_env_path --lib -- --ignored --nocapture` imported 1 trial with `comparable=true`.
- First CodeFactory-owned baseline run: `harbor run -d terminal-bench/terminal-bench-2-1 --agent-import-path codefactory_bench.agent:CodeFactoryAgent -l 1 -n 1 -o .codefactory/benchmark-jobs --job-name cf-tb21-codefactory-baseline-20260627-1145 -y`.
- First CodeFactory-owned baseline result: run id `3bcbc381-e510-4317-8947-fbb5a1e64bcd`, task `terminal-bench/write-compressor`, agent `codefactory-headless-baseline`, reward `0.0`, mean `0.000`, exceptions `0`, runtime `1m 4s`.
- First CodeFactory-owned import evidence: `CODEFACTORY_BENCHMARK_JOB_PATH=.codefactory/benchmark-jobs/cf-tb21-codefactory-baseline-20260627-1145 cargo test benchmark::tests::import_harbor_job_from_env_path --lib -- --ignored --nocapture` imported 1 trial with `agent=codefactory-headless-baseline`, `comparable=true`, `failure_class=Some("verification")`.
- Headless runner local evidence: `PYTHONPATH=/Users/leo/Projects/CodeFactory-terminal-bench-21-design /Users/leo/.local/share/uv/tools/harbor/bin/python tests/test_codefactory_bench_agent.py` passed 4 tests, including fake model tool execution and network command denial.
- Provider bridge local evidence: `cargo test provider_bridge --lib` passed 3 tests covering DeepSeek direct model normalization, redacted command/env preview, authorization-before-secret-lookup, and child-env-only secret injection.
- Post-upgrade no-model Harbor smoke: `harbor run -d terminal-bench/terminal-bench-2-1 --agent-import-path codefactory_bench.agent:CodeFactoryAgent -l 1 -n 1 -o .codefactory/benchmark-jobs --job-name cf-tb21-codefactory-headless-nomodel-20260627-1205 -y`.
- Post-upgrade no-model result: run id `19e42aa8-9e97-4f3b-8965-21993f081ae5`, task `terminal-bench/write-compressor`, agent `codefactory-headless`, mode `baseline-no-model`, reward `0.0`, mean `0.000`, exceptions `0`, runtime `1m 0s`.
- Post-upgrade no-model import evidence: `CODEFACTORY_BENCHMARK_JOB_PATH=.codefactory/benchmark-jobs/cf-tb21-codefactory-headless-nomodel-20260627-1205 cargo test benchmark::tests::import_harbor_job_from_env_path --lib -- --ignored --nocapture` imported 1 trial with `agent=codefactory-headless`, `comparable=true`, `failure_class=Some("verification")`.
- First provider-backed CodeFactory run command: `CODEFACTORY_RUN_REAL_PROVIDER_BRIDGE=1 CODEFACTORY_BENCH_ENDPOINT=deepseek CODEFACTORY_BENCH_TASK_LIMIT=1 CODEFACTORY_BENCH_TRIAL_COUNT=1 CODEFACTORY_BENCH_MODEL_TIMEOUT_SEC=120 CODEFACTORY_BENCH_SHELL_TIMEOUT_SEC=120 cargo test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings --lib -- --ignored --nocapture`.
- First provider-backed CodeFactory result: run id `01801dd1-b725-45d8-844d-c0cc6b608803`, task `terminal-bench/write-compressor`, agent `codefactory-headless`, model `deepseek-v4-pro`, mean `0.000`, `n_errored_trials=1`, exception `RuntimeError`, cause `HTTP 402 Insufficient Balance`.
- First provider-backed import evidence: `CODEFACTORY_BENCHMARK_JOB_PATH=.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260627-083120 cargo test benchmark::tests::import_harbor_job_from_env_path --lib -- --ignored --nocapture` imported 1 trial with `agent=codefactory-headless`, `comparable=true`, `failure_class=Some("model-provider")`.
- Funded provider-backed CodeFactory rerun: `CODEFACTORY_RUN_REAL_PROVIDER_BRIDGE=1 CODEFACTORY_BENCH_ENDPOINT=deepseek CODEFACTORY_BENCH_TASK_LIMIT=1 CODEFACTORY_BENCH_TRIAL_COUNT=1 CODEFACTORY_BENCH_MODEL_TIMEOUT_SEC=120 CODEFACTORY_BENCH_SHELL_TIMEOUT_SEC=120 cargo test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings --lib -- --ignored --nocapture`.
- Funded provider-backed CodeFactory result: run id `b700c436-4836-44c3-a6f4-c3c83b4dd4cc`, task `terminal-bench/write-compressor`, agent `codefactory-headless`, model `deepseek-v4-pro`, mean `0.000`, `n_completed_trials=1`, `n_errored_trials=0`, exception stats `{}`.
- Funded provider-backed import evidence: `CODEFACTORY_BENCHMARK_JOB_PATH=.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260627-085326 cargo test benchmark::tests::import_harbor_job_from_env_path --lib -- --ignored --nocapture` imported 1 trial with `agent=codefactory-headless`, `comparable=true`, `failure_class=Some("verification")`.
- First funded verifier evidence: reward `0`; verifier failed because `/app/data.comp` was not created, so this was an agent execution/completion failure before the policy and loop fixes.
- Policy/loop fix local evidence: `PYTHONPATH=/Users/leo/Projects/CodeFactory-terminal-bench-21-design /Users/leo/.local/share/uv/tools/harbor/bin/python tests/test_codefactory_bench_agent.py` now passes 8 tests, including heredoc source false-positive prevention, real network command denial, artifact hint extraction, and budget reminder generation.
- Post-loop-fix provider-backed result: run id `20875a8a-cdec-47a3-ac00-da77dceaebbb`, task `terminal-bench/write-compressor`, agent `codefactory-headless`, model `deepseek-v4-pro`, mean `0.000`, `n_completed_trials=1`, `n_errored_trials=0`, exception stats `{}`.
- Post-loop-fix verifier evidence: `/app/data.comp` existed and satisfied the <=2500 byte size check, but decompression failed with `Segmentation fault (core dumped)`.
- Latest controlled long run: run id `d3927cd4-340c-4436-9f62-1e3a1c673d97`, task `terminal-bench/write-compressor`, agent `codefactory-headless`, model `deepseek-v4-pro`, mean `0.000`, no provider exception, no Harbor timeout; trajectory shows `data.comp` creation, size under limit, and invalid decompression.
- 2026-06-28 controlled timeout run: run id `5e08d50b-ca8e-4efa-94ab-68bc18918814`, task `terminal-bench/write-compressor`, agent `codefactory-headless`, model `deepseek-v4-pro`, mean `0.000`, `n_completed_trials=1`, `n_errored_trials=0`; model read timeout was recorded in trajectory and imported as `verification`.
- 2026-06-28 latest post-hardening run: run id `5d9246fe-4662-4e93-996d-5f3597d9e56e`, task `terminal-bench/write-compressor`, agent `codefactory-headless`, model `deepseek-v4-pro`, mean `0.000`, `n_completed_trials=1`, `n_errored_trials=0`, failure class `verification`; verifier failed because `/app/data.comp` was not created before the controlled model-error stop.
- 2026-06-28 artifact-enforcement run: run id `4639ab7f-42d3-4a18-b371-718e7fc71507`, task `terminal-bench/write-compressor`, agent `codefactory-headless`, model `deepseek-v4-pro`, mean `0.000`, `n_completed_trials=1`, `n_errored_trials=0`, failure class `verification`; trajectory shows `implementation-required` and `artifact-required` gates, then candidate `/app/data.comp` creation, self-check segfault, and verifier failure on decompression plus size.
- 2026-06-28 provider compatibility boundary: intermediate run id `da378cae-448c-402b-a5cb-ec917eb58a15` failed as `model-provider` because DeepSeek thinking mode rejected forced `tool_choice`; the adapter now retries with `tool_choice=auto` on that provider error.
- 2026-06-28 protocol auto-repair passing run: run id `86a0f061-857a-4a0f-a005-b71e99d62452`, task `terminal-bench/write-compressor`, agent `codefactory-headless`, model `deepseek-v4-pro`, mean `1.000`, `n_completed_trials=1`, `n_errored_trials=0`, failure class `None`; trajectory shows C auto-repair wrote `/app/data.comp` at `2476` bytes, self-check printed `verification-ok`, verifier reward is `1`, and all three verifier tests passed.
- 2026-06-28 full CodeFactory run: Harbor job path `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422`, run id `7ff6ef13-4488-4e0f-afd0-a1f9bd16d561`, task limit `89`, concurrency `4`, agent `codefactory-headless`, model backend `deepseek-v4-pro`, mean reward `0.06741573033707865`, pass count `6 / 89`, verifier reward-zero count `21`, exception count `63`, runtime about `2h17m15s`.
- Full-run passing tasks: `write-compressor`, `vulnerable-secret`, `openssl-selfsigned-cert`, `nginx-request-logging`, `filter-js-from-html`, `extract-elf`.
- Full-run failure class summary from CodeFactory importer: `environment=38`, `long-horizon=23`, `verification=13`, `tool-use=9`, `None=6`.
- Full-run exception summary from Harbor: `RuntimeError=58`, `AddTestsDirError=3`, `AgentTimeoutError=1`, `VerifierTimeoutError=1`.
- Full-run cost/token evidence: `cost_usd=null`, `n_input_tokens=null`, `n_output_tokens=null`; current custom-agent import does not capture provider token usage or cost.
- Full-run import evidence: `CODEFACTORY_BENCHMARK_JOB_PATH=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-085422 cargo test benchmark::tests::import_harbor_job_from_env_path --lib -- --ignored --nocapture` passed and imported `89` trials.
- Infrastructure follow-up evidence: `PYTHONPATH=/Users/leo/Projects/CodeFactory-terminal-bench-21-design /Users/leo/.local/share/uv/tools/harbor/bin/python tests/test_codefactory_bench_agent.py` passes with provider usage aggregation coverage; `cargo test benchmark::tests --lib` passes with `failure_reason`, Docker CPU preflight, task-name filtering, and provider bridge coverage.
- Regression subset evidence: `docs/benchmark-subsets/terminal-bench-21-regression-subset-v1.json` contains 18 tasks selected from the full-run failure mix and is runnable through provider bridge `task_names` / Harbor `--include-task-name`.
- Long-horizon / verifier-repair local evidence: `PYTHONPATH=/Users/leo/Projects/CodeFactory-terminal-bench-21-design /Users/leo/.local/share/uv/tools/harbor/bin/python tests/test_codefactory_bench_agent.py` passes 32 tests, including command-timeout `exec-error` recovery, foreground service supervision guard, and failed self-check repair reminder coverage.
- Provider credential blocker evidence: `CODEFACTORY_BENCH_SECRET_TIMEOUT_SEC=5 CODEFACTORY_RUN_REAL_PROVIDER_BRIDGE=1 ... cargo test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings --lib -- --ignored --nocapture` now fails fast with the explicit keychain timeout message instead of hanging before Harbor job creation.
- Explicit-env override evidence: `cargo test provider_bridge --lib` passes with coverage that an explicit benchmark API key skips OS credential lookup and that blank env values fall back to stored secrets.
- Fixed subset runner evidence: `python3 tools/benchmark/run_terminal_bench_21_regression_subset.py --dry-run` prints the 18-task plan without raw secrets; `python3 tools/benchmark/run_terminal_bench_21_regression_subset.py --secret-timeout-sec 5` generates credential blocker report `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-28T15-31-31Z.md`.
- Fixed subset offline baseline evidence: `python3 tools/benchmark/summarize_terminal_bench_21_subset_baseline.py` generated `docs/evidence-packs/terminal-bench-21-regression-subset-baseline-2026-06-28T15-41-50Z.md`, mapping the completed full run to the 18-task subset with `4 / 18` pass and mean reward `0.222222`. This is an offline projection from the full job, not a fresh provider-backed rerun.
- Fixed subset provider-backed evidence: `python3 tools/benchmark/run_terminal_bench_21_regression_subset.py --secret-timeout-sec 20` generated `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T03-36-45Z.md`, importing run `e7d97f76-b1d1-4b08-beb7-08181a1f5a1e` with `0 / 18` pass and mean reward `0.000`.
- Iteration loop evidence: `tools/benchmark/terminal_bench_21_iteration_loop.py` is the standard score-driven loop entrypoint for the next agent capability PR; it generates `terminal-bench-21-iteration-*.md` reports with baseline/head/delta/next queue.
- First score-driven canary evidence: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T06-58-36Z.md` records `0 / 4` pass for the canary after the first tool-use P0 iteration.
- Bounded canary timeout evidence: `docs/evidence-packs/terminal-bench-21-canary-timeout-2026-06-29T07-15-12Z.md` records the `360s` timeout, partial `2 / 4` completion, and product conclusion that enforcement improved but strategy transition remains missing.
- Forced transition canary evidence: `docs/evidence-packs/terminal-bench-21-forced-transition-timeout-2026-06-29T08-01-36Z.md` records the `360s` timeout, partial `1 / 4` completion, real forced-prompt trajectory nodes, and the conclusion that prompt-only transition is still not enough.
- Constrained scaffold evidence: `docs/evidence-packs/terminal-bench-21-constrained-scaffold-2026-06-29T12-07-06Z.md` records the single-task `write-compressor` before/after runtime improvement from `228.60s` to `112.72s` while preserving environment failure attribution.
- Evidence packs: `docs/evidence-packs/terminal-bench-21-first-smoke-2026-06-27.md`, `docs/evidence-packs/terminal-bench-21-codefactory-baseline-2026-06-27.md`, `docs/evidence-packs/terminal-bench-21-headless-runner-2026-06-27.md`, `docs/evidence-packs/terminal-bench-21-codefactory-provider-deepseek-2026-06-27.md`.
- Latest evidence pack: `docs/evidence-packs/terminal-bench-21-codefactory-provider-deepseek-2026-06-28.md`.
- Systematic evaluation principle: `docs/principles/systematic-agent-evaluation.md`.
- Score-driven improvement roadmap: `docs/plans/terminal-bench-21-system-improvement-plan.md`.
- Release evidence: not live.
- Blocking evidence: none for local full-run evaluation; current valid run is a low-score baseline and not a release/leaderboard claim.

## AI Collaboration
- context scope: CodeFactory repo docs, AGENTS rules, current official Terminal-Bench and Harbor docs.
- assumptions: Terminal-Bench 2.1 should be treated as the primary external terminal-agent benchmark; CodeFactory must add headless execution rather than rely on desktop UI approval.
- review point: first implementation slice should be reviewed before starting the Harbor adapter/headless runner slice.
- validation result: Python adapter smoke/model-backed tests, Rust custom-agent import regression, Rust provider bridge tests, ignored real-job import test, ignored real provider-bridge smoke pass, and a full 89-task provider-backed run. Mechanical artifact enforcement and protocol auto-repair work for several tasks, but the full-run score is only `6 / 89`; the next slice should stabilize infrastructure, build a fixed regression subset, and improve long-horizon execution plus verifier-driven repair.

## Stop Boundary
- Do not stop after local-only validation.
- Do not stop after deploy output without live verification.
- Stop only when done or explicitly blocked with evidence.
