# Terminal-Bench 2.1 Regression Subset Evidence

- generated_at: `2026-06-29T15-28-16Z`
- subset: `terminal-bench-21-regression-subset-v1`
- source_run_id: `7ff6ef13-4488-4e0f-afd0-a1f9bd16d561`
- task_count: `18`
- endpoint: `deepseek`
- exit_code: `0`
- override_storage_mb: `<none>`
- official_comparable: `yes`
- explicit_key_present: `no`

## Preview

- model: `deepseek-v4-pro`
- task_limit: `18`
- concurrency: `4`
- override_storage_mb: `<none>`
- job_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-141221`

## Result

- run: `159041ce-5682-4835-843a-fbed9088aa9d`
- dataset: `terminal-bench/terminal-bench-2-1`
- agent: `codefactory-headless`
- model: `Some("deepseek-v4-pro")`
- comparable: `true`
- trials: `18`
- pass_count: `4`
- mean_reward: `0.222`

## Trials

| Task | Reward | Failure class |
| --- | ---: | --- |
| `terminal-bench/build-cython-ext` | `0` | `Some("policy")` |
| `terminal-bench/caffe-cifar-10` | `0` | `Some("verification")` |
| `terminal-bench/circuit-fibsqrt` | `0` | `Some("verification")` |
| `terminal-bench/configure-git-webserver` | `0` | `Some("verification")` |
| `terminal-bench/count-dataset-tokens` | `0` | `Some("tool-use")` |
| `terminal-bench/extract-elf` | `1` | `None` |
| `terminal-bench/filter-js-from-html` | `1` | `None` |
| `terminal-bench/install-windows-3.11` | `0` | `Some("verification")` |
| `terminal-bench/kv-store-grpc` | `0` | `Some("policy")` |
| `terminal-bench/mteb-retrieve` | `0` | `Some("long-horizon")` |
| `terminal-bench/nginx-request-logging` | `1` | `None` |
| `terminal-bench/protein-assembly` | `0` | `Some("verification")` |
| `terminal-bench/qemu-startup` | `0` | `Some("tool-use")` |
| `terminal-bench/query-optimize` | `0` | `Some("verification")` |
| `terminal-bench/sanitize-git-repo` | `0` | `Some("verification")` |
| `terminal-bench/sparql-university` | `0` | `Some("tool-use")` |
| `terminal-bench/torch-tensor-parallelism` | `0` | `Some("verification")` |
| `terminal-bench/write-compressor` | `1` | `None` |

## Output Tail

```text
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.53s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-7a021239ec62a2f6)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=18 concurrency=4 trial_count=1 override_storage_mb=<none> job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-141221
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings has been running for over 60 seconds
provider_bridge_result status=completed exit_code=Some(0) job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-141221
provider_bridge_imported run=159041ce-5682-4835-843a-fbed9088aa9d dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=true trials=18 mean_reward=0.222
provider_bridge_trial task=terminal-bench/build-cython-ext reward=0 failure_class=Some("policy")
provider_bridge_trial task=terminal-bench/caffe-cifar-10 reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/circuit-fibsqrt reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/configure-git-webserver reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/count-dataset-tokens reward=0 failure_class=Some("tool-use")
provider_bridge_trial task=terminal-bench/extract-elf reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/filter-js-from-html reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/install-windows-3.11 reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/kv-store-grpc reward=0 failure_class=Some("policy")
provider_bridge_trial task=terminal-bench/mteb-retrieve reward=0 failure_class=Some("long-horizon")
provider_bridge_trial task=terminal-bench/nginx-request-logging reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/protein-assembly reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/qemu-startup reward=0 failure_class=Some("tool-use")
provider_bridge_trial task=terminal-bench/query-optimize reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/sanitize-git-repo reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/sparql-university reward=0 failure_class=Some("tool-use")
provider_bridge_trial task=terminal-bench/torch-tensor-parallelism reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/write-compressor reward=1 failure_class=None
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 173 filtered out; finished in 4555.45s


```
