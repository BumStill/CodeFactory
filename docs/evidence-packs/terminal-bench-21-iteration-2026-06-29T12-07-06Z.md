# Terminal-Bench 2.1 Product Iteration Report

- generated_at: `2026-06-29T12-07-06Z`
- evaluation_axis: `codefactory-agent-capability`
- evaluation_subject: `codefactory-headless`
- scope: `canary`
- subset_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-subsets/terminal-bench-21-canary-subset.json`
- endpoint: `deepseek`
- model: `<settings default>`
- hypothesis: `tool-use P0 constrained scaffold also handles no-action after decompressor inspection`
- target_failure_class: `tool-use`
- ran_command: `yes`
- exit_code: `0`

## Baseline

- path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/docs/evidence-packs/terminal-bench-21-regression-subset-baseline-2026-06-28T15-41-50Z.md`
- run: `not available`
- pass_count: `4`
- trials: `18`
- mean_reward: `0.222222`

## Head

- path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T12-07-06Z.md`
- run: `5b1c540d-56ab-4be2-afcb-ee3521b013d6`
- pass_count: `0`
- trials: `1`
- mean_reward: `0.0`

## Delta

- pass_count: `4` -> `0` (`-4`)
- mean_reward: `0.222222` -> `0.000000` (`-0.222222`)

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

- P0: reduce repeated inspection by escalating to artifact implementation earlier.
- P0: add command preflight for missing files, wrong cwd, command-not-found, and obvious non-productive reads.
- P1: feed compact workspace inventory into the model before broad exploration.

## Command Output Tail

```text
# Terminal-Bench 2.1 regression subset run plan

- subset: `terminal-bench-21-regression-subset-v1-canary`
- subset path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-subsets/terminal-bench-21-canary-subset.json`
- tasks: `1`
- endpoint: `deepseek`
- model: `<settings default>`
- concurrency: `1`
- explicit CODEFACTORY_BENCH_API_KEY present: `no`
- keychain timeout: `20s`
- job root: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs`
- command: `cargo test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings --lib -- --ignored --nocapture`

Tasks:
- `write-compressor`
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.48s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-7a021239ec62a2f6)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=1 concurrency=1 trial_count=1 job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-120513
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings has been running for over 60 seconds
provider_bridge_result status=completed exit_code=Some(0) job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-120513
provider_bridge_imported run=5b1c540d-56ab-4be2-afcb-ee3521b013d6 dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=true trials=1 mean_reward=0.000
provider_bridge_trial task=terminal-bench/write-compressor reward=0 failure_class=Some("environment")
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 171 filtered out; finished in 112.72s


Evidence report: /Users/leo/Projects/CodeFactory-terminal-bench-21-design/docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T12-07-06Z.md

```
