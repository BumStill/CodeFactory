# Terminal-Bench 2.1 Regression Subset Evidence

- generated_at: `2026-06-29T03-36-45Z`
- subset: `terminal-bench-21-regression-subset-v1`
- source_run_id: `7ff6ef13-4488-4e0f-afd0-a1f9bd16d561`
- task_count: `18`
- endpoint: `deepseek`
- exit_code: `0`
- explicit_key_present: `no`

## Preview

- model: `deepseek-v4-pro`
- task_limit: `18`
- concurrency: `4`
- job_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-024231`

## Result

- run: `e7d97f76-b1d1-4b08-beb7-08181a1f5a1e`
- dataset: `terminal-bench/terminal-bench-2-1`
- agent: `codefactory-headless`
- model: `Some("deepseek-v4-pro")`
- comparable: `true`
- trials: `18`
- pass_count: `0`
- mean_reward: `0.000`

## Trials

| Task | Reward | Failure class |
| --- | ---: | --- |
| `terminal-bench/build-cython-ext` | `0` | `Some("policy")` |
| `terminal-bench/caffe-cifar-10` | `0` | `Some("environment")` |
| `terminal-bench/circuit-fibsqrt` | `0` | `Some("tool-use")` |
| `terminal-bench/configure-git-webserver` | `0` | `Some("tool-use")` |
| `terminal-bench/count-dataset-tokens` | `0` | `Some("tool-use")` |
| `terminal-bench/extract-elf` | `0` | `Some("tool-use")` |
| `terminal-bench/filter-js-from-html` | `0` | `Some("tool-use")` |
| `terminal-bench/install-windows-3.11` | `0` | `Some("tool-use")` |
| `terminal-bench/kv-store-grpc` | `0` | `Some("policy")` |
| `terminal-bench/mteb-retrieve` | `0` | `Some("tool-use")` |
| `terminal-bench/nginx-request-logging` | `0` | `Some("policy")` |
| `terminal-bench/protein-assembly` | `0` | `Some("tool-use")` |
| `terminal-bench/qemu-startup` | `0` | `Some("tool-use")` |
| `terminal-bench/query-optimize` | `0` | `Some("verification")` |
| `terminal-bench/sanitize-git-repo` | `0` | `Some("tool-use")` |
| `terminal-bench/sparql-university` | `0` | `Some("tool-use")` |
| `terminal-bench/torch-tensor-parallelism` | `0` | `Some("tool-use")` |
| `terminal-bench/write-compressor` | `0` | `Some("tool-use")` |

## Output Tail

```text
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.38s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-7a021239ec62a2f6)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=18 concurrency=4 trial_count=1 job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-024231
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings has been running for over 60 seconds
provider_bridge_result status=completed exit_code=Some(0) job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-024231
provider_bridge_imported run=e7d97f76-b1d1-4b08-beb7-08181a1f5a1e dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=true trials=18 mean_reward=0.000
provider_bridge_trial task=terminal-bench/build-cython-ext reward=0 failure_class=Some("policy")
provider_bridge_trial task=terminal-bench/caffe-cifar-10 reward=0 failure_class=Some("environment")
provider_bridge_trial task=terminal-bench/circuit-fibsqrt reward=0 failure_class=Some("tool-use")
provider_bridge_trial task=terminal-bench/configure-git-webserver reward=0 failure_class=Some("tool-use")
provider_bridge_trial task=terminal-bench/count-dataset-tokens reward=0 failure_class=Some("tool-use")
provider_bridge_trial task=terminal-bench/extract-elf reward=0 failure_class=Some("tool-use")
provider_bridge_trial task=terminal-bench/filter-js-from-html reward=0 failure_class=Some("tool-use")
provider_bridge_trial task=terminal-bench/install-windows-3.11 reward=0 failure_class=Some("tool-use")
provider_bridge_trial task=terminal-bench/kv-store-grpc reward=0 failure_class=Some("policy")
provider_bridge_trial task=terminal-bench/mteb-retrieve reward=0 failure_class=Some("tool-use")
provider_bridge_trial task=terminal-bench/nginx-request-logging reward=0 failure_class=Some("policy")
provider_bridge_trial task=terminal-bench/protein-assembly reward=0 failure_class=Some("tool-use")
provider_bridge_trial task=terminal-bench/qemu-startup reward=0 failure_class=Some("tool-use")
provider_bridge_trial task=terminal-bench/query-optimize reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/sanitize-git-repo reward=0 failure_class=Some("tool-use")
provider_bridge_trial task=terminal-bench/sparql-university reward=0 failure_class=Some("tool-use")
provider_bridge_trial task=terminal-bench/torch-tensor-parallelism reward=0 failure_class=Some("tool-use")
provider_bridge_trial task=terminal-bench/write-compressor reward=0 failure_class=Some("tool-use")
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 171 filtered out; finished in 3254.20s


```
