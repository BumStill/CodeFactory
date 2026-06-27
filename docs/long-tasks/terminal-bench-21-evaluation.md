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
- Current phase: implementation slice 3
- Current checkpoint: first real provider-backed Terminal-Bench 2.1 smoke has run through the CodeFactory provider bridge with `agent=codefactory-headless` and model backend `deepseek-v4-pro`. The run path is verified, but the current DeepSeek provider returned `HTTP 402 Insufficient Balance`, so the first model-backed result is a `model-provider` blocker result rather than an agent capability score.
- Next owner: development / QA
- Updated at: 2026-06-27

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

## Remaining Items
- Rerun the real Harbor smoke after funding the DeepSeek endpoint or selecting another usable CodeFactory provider endpoint, so the run measures CodeFactory agent behavior rather than provider availability.
- Add persisted run fields/UI for evaluation axis, evaluation subject, fixed variables, changed variables, and result attribution.
- Promote `benchmark-sandbox` from adapter-local command gate to shared CodeFactory policy preset with run/task/container binding.
- Add Benchmarks UI for run summary, trial details, failure triage, and capability profile.
- Compare at least one baseline/head subset after an implementation change.

## Blockers
- Current DeepSeek provider run is blocked by provider account state: `HTTP 402 Insufficient Balance`. This is not a CodeFactory planning/tool-use failure; rerun after funding the endpoint or switching to another configured OpenAI-compatible endpoint.
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
- Evidence packs: `docs/evidence-packs/terminal-bench-21-first-smoke-2026-06-27.md`, `docs/evidence-packs/terminal-bench-21-codefactory-baseline-2026-06-27.md`, `docs/evidence-packs/terminal-bench-21-headless-runner-2026-06-27.md`, `docs/evidence-packs/terminal-bench-21-codefactory-provider-deepseek-2026-06-27.md`.
- Systematic evaluation principle: `docs/principles/systematic-agent-evaluation.md`.
- Release evidence: not live.
- Blocking evidence: CodeFactory product bridge launched a real Harbor job and imported it, but DeepSeek returned `HTTP 402 Insufficient Balance`.

## AI Collaboration
- context scope: CodeFactory repo docs, AGENTS rules, current official Terminal-Bench and Harbor docs.
- assumptions: Terminal-Bench 2.1 should be treated as the primary external terminal-agent benchmark; CodeFactory must add headless execution rather than rely on desktop UI approval.
- review point: first implementation slice should be reviewed before starting the Harbor adapter/headless runner slice.
- validation result: Python adapter smoke/model-backed tests, Rust custom-agent import regression, Rust provider bridge tests, ignored real-job import test, and ignored real provider-bridge smoke pass after intentional failing-test steps; first provider-backed run is blocked by provider account balance.

## Stop Boundary
- Do not stop after local-only validation.
- Do not stop after deploy output without live verification.
- Stop only when done or explicitly blocked with evidence.
