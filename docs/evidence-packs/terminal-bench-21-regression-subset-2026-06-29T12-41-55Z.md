# Terminal-Bench 2.1 Regression Subset Evidence

- generated_at: `2026-06-29T12-41-55Z`
- subset: `terminal-bench-21-regression-subset-v1-canary`
- source_run_id: `7ff6ef13-4488-4e0f-afd0-a1f9bd16d561`
- task_count: `1`
- endpoint: `deepseek`
- exit_code: `0`
- explicit_key_present: `no`

## Preview

- model: `deepseek-v4-pro`
- task_limit: `1`
- concurrency: `1`
- job_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-124058`

## Result

- run: `addff8cf-2249-4e6c-8463-cc919a1eed93`
- dataset: `terminal-bench/terminal-bench-2-1`
- agent: `codefactory-headless`
- model: `Some("deepseek-v4-pro")`
- comparable: `true`
- trials: `1`
- pass_count: `0`
- mean_reward: `0.000`

## Trials

| Task | Reward | Failure class |
| --- | ---: | --- |
| `terminal-bench/mteb-retrieve` | `0` | `Some("environment")` |

## Output Tail

```text
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.54s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-7a021239ec62a2f6)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=1 concurrency=1 trial_count=1 job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-124058
provider_bridge_result status=completed exit_code=Some(0) job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-124058
provider_bridge_imported run=addff8cf-2249-4e6c-8463-cc919a1eed93 dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=true trials=1 mean_reward=0.000
provider_bridge_trial task=terminal-bench/mteb-retrieve reward=0 failure_class=Some("environment")
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 171 filtered out; finished in 57.17s


```
