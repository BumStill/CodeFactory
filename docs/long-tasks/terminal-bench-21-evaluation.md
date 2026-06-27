# Terminal-Bench 2.1 Evaluation Long Task

## Basics
- Task ID: CF-LT-TB21
- Title: Terminal-Bench 2.1 ability evaluation system
- Feature spec: `docs/specs/feature-specs/terminal-bench-21-evaluation.md`
- Related Req IDs: CF-TB-R1, CF-TB-R2, CF-TB-R3, CF-TB-R4, CF-TB-R5, CF-TB-R6

## Completion Standard
- Done means: CodeFactory can run or import a Terminal-Bench 2.1 Harbor job, persist run/trial/artifact evidence, classify failures, show capability profile, and compare a regression subset across builds with real evidence.
- Blocked means: Harbor/Docker/dataset/agent-adapter/runtime access prevents real smoke verification, with exact command, error, and next action recorded.

## Current State
- Current phase: implementation slice 2
- Current checkpoint: model-backed `codefactory-headless` runner entry exists and is locally tested with a fake OpenAI-compatible server; real model-backed Terminal-Bench smoke is waiting on explicit `CODEFACTORY_BENCH_*` model configuration.
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

## Remaining Items
- Run real Harbor smoke evaluation with model-backed CodeFactory headless execution after explicit benchmark model env is configured.
- Promote `benchmark-sandbox` from adapter-local command gate to shared CodeFactory policy preset with run/task/container binding.
- Add Benchmarks UI for run summary, trial details, failure triage, and capability profile.
- Compare at least one baseline/head subset after an implementation change.

## Blockers
- Current evaluation environment does not expose explicit `CODEFACTORY_BENCH_API_KEY`, `CODEFACTORY_BENCH_MODEL`, or `CODEFACTORY_BENCH_BASE_URL`, so the next real Terminal-Bench run would fall back to no-model mode rather than produce a model-backed capability score.
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
- Post-upgrade no-model Harbor smoke: `harbor run -d terminal-bench/terminal-bench-2-1 --agent-import-path codefactory_bench.agent:CodeFactoryAgent -l 1 -n 1 -o .codefactory/benchmark-jobs --job-name cf-tb21-codefactory-headless-nomodel-20260627-1205 -y`.
- Post-upgrade no-model result: run id `19e42aa8-9e97-4f3b-8965-21993f081ae5`, task `terminal-bench/write-compressor`, agent `codefactory-headless`, mode `baseline-no-model`, reward `0.0`, mean `0.000`, exceptions `0`, runtime `1m 0s`.
- Post-upgrade no-model import evidence: `CODEFACTORY_BENCHMARK_JOB_PATH=.codefactory/benchmark-jobs/cf-tb21-codefactory-headless-nomodel-20260627-1205 cargo test benchmark::tests::import_harbor_job_from_env_path --lib -- --ignored --nocapture` imported 1 trial with `agent=codefactory-headless`, `comparable=true`, `failure_class=Some("verification")`.
- Evidence packs: `docs/evidence-packs/terminal-bench-21-first-smoke-2026-06-27.md`, `docs/evidence-packs/terminal-bench-21-codefactory-baseline-2026-06-27.md`, `docs/evidence-packs/terminal-bench-21-headless-runner-2026-06-27.md`.
- Release evidence: not live.
- Blocking evidence: no explicit benchmark model env is configured, so real model-backed scoring is not yet runnable in this local environment.

## AI Collaboration
- context scope: CodeFactory repo docs, AGENTS rules, current official Terminal-Bench and Harbor docs.
- assumptions: Terminal-Bench 2.1 should be treated as the primary external terminal-agent benchmark; CodeFactory must add headless execution rather than rely on desktop UI approval.
- review point: first implementation slice should be reviewed before starting the Harbor adapter/headless runner slice.
- validation result: Python adapter smoke/model-backed tests, Rust custom-agent import regression, and the ignored real-job import test pass after intentional failing-test steps.

## Stop Boundary
- Do not stop after local-only validation.
- Do not stop after deploy output without live verification.
- Stop only when done or explicitly blocked with evidence.
