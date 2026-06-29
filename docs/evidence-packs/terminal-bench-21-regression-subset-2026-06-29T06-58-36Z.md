# Terminal-Bench 2.1 Regression Subset Evidence

- generated_at: `2026-06-29T06-58-36Z`
- subset: `terminal-bench-21-canary-subset-v1`
- source_run_id: ``
- task_count: `4`
- endpoint: `deepseek`
- exit_code: `0`
- explicit_key_present: `no`

## Preview

- model: `deepseek-v4-pro`
- task_limit: `4`
- concurrency: `2`
- job_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-064042`

## Result

- run: `77e98d56-2638-4b0c-a941-a84b542d51ff`
- dataset: `terminal-bench/terminal-bench-2-1`
- agent: `codefactory-headless`
- model: `Some("deepseek-v4-pro")`
- comparable: `true`
- trials: `4`
- pass_count: `0`
- mean_reward: `0.000`

## Trials

| Task | Reward | Failure class |
| --- | ---: | --- |
| `terminal-bench/count-dataset-tokens` | `0` | `Some("tool-use")` |
| `terminal-bench/filter-js-from-html` | `0` | `Some("tool-use")` |
| `terminal-bench/mteb-retrieve` | `0` | `Some("tool-use")` |
| `terminal-bench/write-compressor` | `0` | `Some("tool-use")` |

## Output Tail

```text
    Finished `test` profile [unoptimized + debuginfo] target(s) in 1.58s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-7a021239ec62a2f6)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=4 concurrency=2 trial_count=1 job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-064042
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings has been running for over 60 seconds
provider_bridge_result status=completed exit_code=Some(0) job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-064042
provider_bridge_imported run=77e98d56-2638-4b0c-a941-a84b542d51ff dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=true trials=4 mean_reward=0.000
provider_bridge_trial task=terminal-bench/count-dataset-tokens reward=0 failure_class=Some("tool-use")
provider_bridge_trial task=terminal-bench/filter-js-from-html reward=0 failure_class=Some("tool-use")
provider_bridge_trial task=terminal-bench/mteb-retrieve reward=0 failure_class=Some("tool-use")
provider_bridge_trial task=terminal-bench/write-compressor reward=0 failure_class=Some("tool-use")
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 171 filtered out; finished in 1073.48s


```
