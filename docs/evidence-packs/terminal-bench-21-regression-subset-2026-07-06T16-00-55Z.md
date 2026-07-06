# Terminal-Bench 2.1 Regression Subset Evidence

- generated_at: `2026-07-06T16-00-55Z`
- subset: `terminal-bench-21-regression-subset-v1`
- source_run_id: `7ff6ef13-4488-4e0f-afd0-a1f9bd16d561`
- task_count: `18`
- endpoint: `deepseek`
- exit_code: `0`
- override_storage_mb: `<none>`
- official_comparable: `no`
- explicit_key_present: `no`
- trial_hard_timeout_sec: `1200`
- heavy_verifier_timeout_overrides: `torch-tensor-parallelism:2400`
- heavy_verifier_timeout_multiplier: `3`
- verifier_uv_torch_backend: `cpu`
- partial_import_diagnostic: `enabled`

## Comparability Notes

- runner-level trial hard timeout watchdog was enabled
- watchdog stopped one or more stale trial containers

## Preview

- model: `deepseek-v4-pro`
- task_limit: `18`
- concurrency: `1`
- override_storage_mb: `<none>`
- job_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-16/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260706-141342`

## Provider Bridge

- status: `completed`
- exit_code: `Some(0)`
- job_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-16/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260706-141342`

## Result

- run: `565ecdd4-7694-42aa-a3c7-a3bd38f15146`
- dataset: `terminal-bench/terminal-bench-2-1`
- agent: `codefactory-headless`
- model: `Some("deepseek-v4-pro")`
- comparable: `true`
- trials: `18`
- pass_count: `16`
- mean_reward: `0.889`

## Trials

| Task | Reward | Failure class |
| --- | ---: | --- |
| `terminal-bench/build-cython-ext` | `1` | `None` |
| `terminal-bench/caffe-cifar-10` | `0` | `Some("verification")` |
| `terminal-bench/circuit-fibsqrt` | `1` | `None` |
| `terminal-bench/configure-git-webserver` | `1` | `None` |
| `terminal-bench/count-dataset-tokens` | `1` | `None` |
| `terminal-bench/extract-elf` | `1` | `None` |
| `terminal-bench/filter-js-from-html` | `1` | `None` |
| `terminal-bench/install-windows-3.11` | `1` | `None` |
| `terminal-bench/kv-store-grpc` | `1` | `None` |
| `terminal-bench/mteb-retrieve` | `1` | `None` |
| `terminal-bench/nginx-request-logging` | `1` | `None` |
| `terminal-bench/protein-assembly` | `1` | `None` |
| `terminal-bench/qemu-startup` | `1` | `None` |
| `terminal-bench/query-optimize` | `0` | `Some("environment")` |
| `terminal-bench/sanitize-git-repo` | `1` | `None` |
| `terminal-bench/sparql-university` | `1` | `None` |
| `terminal-bench/torch-tensor-parallelism` | `1` | `None` |
| `terminal-bench/write-compressor` | `1` | `None` |

## Watchdog Interventions

The regression runner stopped stale trial containers so the remaining matrix could finish.

| Trial | Elapsed sec | Action | Containers |
| --- | ---: | --- | --- |
| `query-optimize__YL4Y2G6` | `1200` | `docker-stop` | `query-optimize__yl4y2g6-main-1` |

## Verifier Environment Warnings

These warnings do not change Harbor rewards, but they mark local verifier runtime conditions that can weaken score interpretation.

| Trial | Category | Evidence |
| --- | --- | --- |
| `caffe-cifar-10__8DrSoPc` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `circuit-fibsqrt__foJ3TGD` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `configure-git-webserver__gyfn38b` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `count-dataset-tokens__a4qwhLQ` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `extract-elf__AWjg6L6` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `filter-js-from-html__fjZsjJH` | `browser-driver-unavailable` | `Failed to create driver or process file: Message: Unable to obtain driver for chrome; For documentation on this error, please visit: https://www.selenium.dev/documentation/webdriver/troubleshooting/errors/driver_location` |
| `filter-js-from-html__fjZsjJH` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `install-windows-3.11__GBKLxsW` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `nginx-request-logging__4HQNyyD` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `protein-assembly__iujKSxM` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `qemu-startup__QDLvf9G` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `query-optimize__YL4Y2G6` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `sanitize-git-repo__mm77RfX` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `sparql-university__V3K2L7x` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `torch-tensor-parallelism__TqJLHze` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `write-compressor__dPqRxqa` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |

## Output Tail

```text

# Provider bridge attempt 1/6
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.55s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-2e0d3f031370550b)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=18 concurrency=1 trial_count=1 override_storage_mb=<none> job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-16/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260706-141342
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings has been running for over 60 seconds
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


```
