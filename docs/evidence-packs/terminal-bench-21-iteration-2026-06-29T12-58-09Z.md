# Terminal-Bench 2.1 Product Iteration Report

- generated_at: `2026-06-29T12-58-09Z`
- evaluation_axis: `codefactory-agent-capability`
- evaluation_subject: `codefactory-headless`
- scope: `canary`
- subset_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-subsets/terminal-bench-21-canary-subset.json`
- endpoint: `deepseek`
- model: `<settings default>`
- override_storage_mb: `65536`
- official_comparable: `no`
- hypothesis: `environment P0 verifier bootstrap storage override`
- target_failure_class: `environment`
- ran_command: `yes`
- exit_code: `0`

## Baseline

- path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/docs/evidence-packs/terminal-bench-21-regression-subset-baseline-2026-06-28T15-41-50Z.md`
- run: `not available`
- pass_count: `4`
- trials: `18`
- mean_reward: `0.222222`

## Head

- path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T12-58-09Z.md`
- run: `0224b9ba-e6f4-4b45-8bd8-1249b8911561`
- pass_count: `0`
- trials: `1`
- mean_reward: `0.0`

## Delta

- comparable_delta: `no`
- reason: baseline and head have different trial counts; use this report as targeted canary evidence, not an aggregate score delta.

## Failure Class Counts

Baseline:
- `environment`: `2`
- `long-horizon`: `4`
- `pass`: `4`
- `tool-use`: `3`
- `verification`: `5`

Head:
- `environment`: `1`

## Next Improvement Queue

- P0: preflight Docker CPU/memory/storage before counting the run as agent capability.
- P1: tag environment failures as blocked and reroute to infrastructure queue.

## Command Output Tail

```text
# Terminal-Bench 2.1 regression subset run plan

- subset: `terminal-bench-21-regression-subset-v1-canary`
- subset path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-subsets/terminal-bench-21-canary-subset.json`
- tasks: `1`
- endpoint: `deepseek`
- model: `<settings default>`
- concurrency: `1`
- override_storage_mb: `65536`
- official_comparable: `no`
- explicit CODEFACTORY_BENCH_API_KEY present: `no`
- keychain timeout: `20s`
- job root: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs`
- command: `cargo test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings --lib -- --ignored --nocapture`

Tasks:
- `mteb-retrieve`
   Compiling codefactory v1.40.1 (/Users/leo/Projects/CodeFactory-terminal-bench-21-design/src-tauri)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 6.59s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-7a021239ec62a2f6)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=1 concurrency=1 trial_count=1 override_storage_mb=65536 job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-125623
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings has been running for over 60 seconds
provider_bridge_result status=completed exit_code=Some(0) job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-125623
provider_bridge_imported run=0224b9ba-e6f4-4b45-8bd8-1249b8911561 dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=false trials=1 mean_reward=0.000
provider_bridge_trial task=terminal-bench/mteb-retrieve reward=0 failure_class=Some("environment")
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 173 filtered out; finished in 105.48s


Evidence report: /Users/leo/Projects/CodeFactory-terminal-bench-21-design/docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T12-58-09Z.md

```
