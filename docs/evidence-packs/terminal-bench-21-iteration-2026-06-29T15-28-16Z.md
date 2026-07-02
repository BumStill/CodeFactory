# Terminal-Bench 2.1 Product Iteration Report

- generated_at: `2026-06-29T15-28-16Z`
- evaluation_axis: `codefactory-agent-capability`
- evaluation_subject: `codefactory-headless`
- scope: `regression`
- subset_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/docs/benchmark-subsets/terminal-bench-21-regression-subset-v1.json`
- endpoint: `deepseek`
- model: `<settings default>`
- shell_timeout_sec: `300`
- override_storage_mb: `<none>`
- official_comparable: `yes`
- hypothesis: `MTEB scoring canary plus resource preflight aggregate regression subset`
- target_failure_class: `verification`
- ran_command: `yes`
- exit_code: `0`

## Baseline

- path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/docs/evidence-packs/terminal-bench-21-regression-subset-baseline-2026-06-28T15-41-50Z.md`
- run: `not available`
- pass_count: `4`
- trials: `18`
- mean_reward: `0.222222`

## Head

- path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T15-28-16Z.md`
- run: `159041ce-5682-4835-843a-fbed9088aa9d`
- pass_count: `4`
- trials: `18`
- mean_reward: `0.222`

## Delta

- pass_count: `4` -> `4` (`+0`)
- mean_reward: `0.222222` -> `0.222000` (`-0.000222`)

## Failure Class Counts

Baseline:
- `environment`: `2`
- `long-horizon`: `4`
- `pass`: `4`
- `tool-use`: `3`
- `verification`: `5`

Head:
- `long-horizon`: `1`
- `pass`: `4`
- `policy`: `2`
- `tool-use`: `3`
- `verification`: `8`

## Next Improvement Queue

- P0: parse verifier/self-check output into a concrete repair_goal.
- P1: block final answers until the smallest available self-check has run after a candidate fix.

## Command Output Tail

```text
# Terminal-Bench 2.1 regression subset run plan

- subset: `terminal-bench-21-regression-subset-v1`
- subset path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/docs/benchmark-subsets/terminal-bench-21-regression-subset-v1.json`
- tasks: `18`
- endpoint: `deepseek`
- model: `<settings default>`
- concurrency: `4`
- min_docker_cpus: `4.0`
- min_docker_memory_gb: `6.0`
- min_docker_free_gb: `20.0`
- resource_preflight: `enabled`
- override_storage_mb: `<none>`
- official_comparable: `yes`
- explicit CODEFACTORY_BENCH_API_KEY present: `no`
- keychain timeout: `20s`
- job root: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs`
- command: `cargo test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings --lib -- --ignored --nocapture`

Tasks:
- `write-compressor`
- `extract-elf`
- `filter-js-from-html`
- `nginx-request-logging`
- `circuit-fibsqrt`
- `configure-git-webserver`
- `mteb-retrieve`
- `sanitize-git-repo`
- `query-optimize`
- `count-dataset-tokens`
- `install-windows-3.11`
- `protein-assembly`
- `build-cython-ext`
- `kv-store-grpc`
- `sparql-university`
- `torch-tensor-parallelism`
- `caffe-cifar-10`
- `qemu-startup`
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


Evidence report: /Users/leo/Projects/CodeFactory-terminal-bench-21-design/docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T15-28-16Z.md

```
