# Terminal-Bench 2.1 Regression Subset Evidence

- generated_at: `2026-07-14T09-55-29Z`
- subset: `terminal-bench-21-source-build-canary-v1`
- source_run_id: ``
- task_count: `1`
- endpoint: `deepseek`
- exit_code: `0`
- override_storage_mb: `<none>`
- official_comparable: `yes`
- explicit_key_present: `no`
- trial_hard_timeout_sec: `<disabled>`
- heavy_verifier_timeout_overrides: `<none>`
- heavy_verifier_timeout_multiplier: `<none>`
- verifier_uv_torch_backend: `<none>`
- partial_import_diagnostic: `enabled`

## Preview

- model: `deepseek-v4-pro`
- task_limit: `1`
- concurrency: `1`
- override_storage_mb: `<none>`
- job_path: `/Users/leo/Projects/CodeFactory-agent-score-recovery-16/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260714-094017`

## Provider Bridge

- status: `completed`
- exit_code: `Some(0)`
- job_path: `/Users/leo/Projects/CodeFactory-agent-score-recovery-16/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260714-094017`

## Agent Usage

- trials_with_metadata: `1`
- model_requests: `19`
- prompt_tokens: `216297`
- completion_tokens: `4139`
- total_tokens: `220436`
- tool_calls: `19`

## Result

- run: `9b978180-44e1-4f72-bf32-6a0571672db8`
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
| `terminal-bench/build-cython-ext` | `0` | `Some("verification")` |

## Output Tail

```text

# Provider bridge attempt 1/3
   Compiling codefactory-agent-core v0.1.0 (/Users/leo/Projects/CodeFactory-agent-score-recovery-16/src-tauri/crates/agent-core)
   Compiling codefactory v1.42.8 (/Users/leo/Projects/CodeFactory-agent-score-recovery-16/src-tauri)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.46s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-c06edd870a2c5ba3)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=1 concurrency=1 trial_count=1 override_storage_mb=<none> job_path=/Users/leo/Projects/CodeFactory-agent-score-recovery-16/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260714-094017
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings has been running for over 60 seconds
provider_bridge_result status=completed exit_code=Some(0) job_path=/Users/leo/Projects/CodeFactory-agent-score-recovery-16/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260714-094017
provider_bridge_imported run=9b978180-44e1-4f72-bf32-6a0571672db8 dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=true trials=1 mean_reward=0.000
provider_bridge_trial task=terminal-bench/build-cython-ext reward=0 failure_class=Some("verification")
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 206 filtered out; finished in 912.36s


```
