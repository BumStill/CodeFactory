# Terminal-Bench 2.1 Product Iteration Report

- generated_at: `2026-07-06T16-00-55Z`
- evaluation_axis: `codefactory-agent-capability`
- evaluation_subject: `codefactory-headless`
- scope: `regression`
- subset_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-16/docs/benchmark-subsets/terminal-bench-21-regression-subset-v1.json`
- endpoint: `deepseek`
- model: `deepseek-v4-pro`
- shell_timeout_sec: `300`
- override_storage_mb: `<none>`
- official_comparable: `yes`
- hypothesis: `After field-level HF token-count audit and foreground GUI visual-feedback preparation, the fixed 18-task subset should recover to at least 16/18 while preserving qemu-startup as the newly conquered family`
- target_failure_class: `regression-stability`
- ran_command: `yes`
- exit_code: `0`

## Baseline

- path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-16/docs/evidence-packs/terminal-bench-21-regression-subset-baseline-2026-06-28T15-41-50Z.md`
- run: `not available`
- pass_count: `4`
- trials: `18`
- mean_reward: `0.222222`

## Head

- path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-16/docs/evidence-packs/terminal-bench-21-regression-subset-2026-07-06T16-00-55Z.md`
- run: `565ecdd4-7694-42aa-a3c7-a3bd38f15146`
- pass_count: `16`
- trials: `18`
- mean_reward: `0.889`

## Product Capability Impact

- verdict: mixed
- capability: CodeFactory improves real long-running task reliability through auditable data computations, transient model-network retry, explicit dependency routing, source-clean artifact verification, and visual readiness checks for interactive runtimes.
- non_benchmark_example: On a real user repository behind a local proxy, CodeFactory can survive a temporary DeepSeek/OpenRouter disconnect, route apt/PyPI/GitHub dependencies explicitly, verify native-extension installs outside the source tree, estimate token cost with field-level audit, and confirm a VNC-backed dev environment is visibly interactive before claiming completion.
- benchmark_only_boundary: The fixed 18-task subset, Terminal-Bench task names, local proxy endpoints, qemu monitor paths, and retry/watchdog values are benchmark-environment specific; the reusable product capability is bounded model-network retry, explicit dependency routing, auditable data workflows, source-clean artifact verification, and GUI runtime readiness validation.

## Delta

- pass_count: `4` -> `16` (`+12`)
- mean_reward: `0.222222` -> `0.889000` (`+0.666778`)

## Failure Class Counts

Baseline:
- `environment`: `2`
- `long-horizon`: `4`
- `pass`: `4`
- `tool-use`: `3`
- `verification`: `5`

Head:
- `docker-stop`: `1`
- `environment`: `1`
- `pass`: `16`
- `verification`: `1`

## Next Improvement Queue

- P0: inspect the dominant failure class and choose one targeted canary before broader regression.
- P1: rerun the fixed subset only after the targeted canary shows a behavior delta.

## Command Output Tail

```text
# Terminal-Bench 2.1 regression subset run plan

- subset: `terminal-bench-21-regression-subset-v1`
- subset path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-16/docs/benchmark-subsets/terminal-bench-21-regression-subset-v1.json`
- tasks: `18`
- endpoint: `deepseek`
- model: `deepseek-v4-pro`
- concurrency: `1`
- min_docker_cpus: `4.0`
- min_docker_memory_gb: `6.0`
- min_docker_free_gb: `20.0`
- resource_preflight: `enabled`
- preflight_retries: `1`
- override_storage_mb: `<none>`
- official_comparable: `no`
- explicit CODEFACTORY_BENCH_API_KEY present: `no`
- keychain timeout: `20s`
- trial_hard_timeout_sec: `1200`
- heavy_verifier_timeout_overrides: `torch-tensor-parallelism:2400`
- heavy_verifier_timeout_multiplier: `3`
- docker_apt_proxy: `http://host.docker.internal:7897`
- verifier_proxy: `http://host.docker.internal:7897`
- provider_proxy: `http://127.0.0.1:7897`
- provider_bridge_retries: `5`
- verifier_uv_http_timeout_sec: `120`
- verifier_uv_torch_backend: `cpu`
- partial_import_diagnostic: `enabled`
- job root: `/Users/leo/Projects/CodeFactory-terminal-bench-21-16/.codefactory/benchmark-jobs`
- agent PYTHONPATH root: `/Users/leo/Projects/CodeFactory-terminal-bench-21-16`
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
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.55s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-2e0d3f031370550b)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=18 concurrency=1 trial_count=1 override_storage_mb=<none> job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-16/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260706-141342
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings has been running for over 60 seconds
benchmark_watchdog_timeout trial=query-optimize__YL4Y2G6 elapsed_sec=1200 containers=query-optimize__yl4y2g6-main-1 action=docker-stop
provider_bridge_result status=completed exit_code=Some(0) job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-16/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260706-141342
provider_bridge_imported run=565ecdd4-7694-42aa-a3c7-a3bd38f15146 dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=true trials=18 mean_reward=0.889
provider_bridge_trial task=terminal-bench/build-cython-ext reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/caffe-cifar-10 reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/circuit-fibsqrt reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/configure-git-webserver reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/count-dataset-tokens reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/extract-elf reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/filter-js-from-html reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/install-windows-3.11 reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/kv-store-grpc reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/mteb-retrieve reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/nginx-request-logging reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/protein-assembly reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/qemu-startup reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/query-optimize reward=0 failure_class=Some("environment")
provider_bridge_trial task=terminal-bench/sanitize-git-repo reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/sparql-university reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/torch-tensor-parallelism reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/write-compressor reward=1 failure_class=None
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 176 filtered out; finished in 6433.23s


Evidence report: /Users/leo/Projects/CodeFactory-terminal-bench-21-16/docs/evidence-packs/terminal-bench-21-regression-subset-2026-07-06T16-00-55Z.md

```
