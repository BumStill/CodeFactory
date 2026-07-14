# Terminal-Bench 2.1 Regression Subset Evidence

- generated_at: `2026-07-14T03-59-42Z`
- subset: `terminal-bench-21-regression-subset-v1`
- source_run_id: `7ff6ef13-4488-4e0f-afd0-a1f9bd16d561`
- task_count: `18`
- endpoint: `deepseek`
- exit_code: `0`
- override_storage_mb: `<none>`
- official_comparable: `no`
- explicit_key_present: `no`
- trial_hard_timeout_sec: `<disabled>`
- heavy_verifier_timeout_overrides: `<none>`
- heavy_verifier_timeout_multiplier: `<none>`
- verifier_uv_torch_backend: `<none>`
- partial_import_diagnostic: `enabled`

## Comparability Notes

- imported Harbor run was marked non-comparable

## Preview

- model: `deepseek-v4-pro`
- task_limit: `18`
- concurrency: `4`
- override_storage_mb: `<none>`
- job_path: `/Users/leo/Projects/CodeFactory-agent-score-recovery-16/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260714-015143`

## Provider Bridge

- status: `completed`
- exit_code: `Some(0)`
- job_path: `/Users/leo/Projects/CodeFactory-agent-score-recovery-16/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260714-015143`

## Agent Usage

- trials_with_metadata: `18`
- model_requests: `303`
- prompt_tokens: `3365787`
- completion_tokens: `147942`
- total_tokens: `3513729`
- tool_calls: `870`

## Result

- run: `677cbb1a-b24b-422b-8a64-d4895b15af07`
- dataset: `terminal-bench/terminal-bench-2-1`
- agent: `codefactory-headless`
- model: `Some("deepseek-v4-pro")`
- comparable: `false`
- trials: `18`
- pass_count: `6`
- mean_reward: `0.333`

## Trials

| Task | Reward | Failure class |
| --- | ---: | --- |
| `terminal-bench/build-cython-ext` | `0` | `Some("long-horizon")` |
| `terminal-bench/caffe-cifar-10` | `0` | `Some("long-horizon")` |
| `terminal-bench/circuit-fibsqrt` | `0` | `Some("long-horizon")` |
| `terminal-bench/configure-git-webserver` | `1` | `None` |
| `terminal-bench/count-dataset-tokens` | `0` | `Some("long-horizon")` |
| `terminal-bench/extract-elf` | `1` | `None` |
| `terminal-bench/filter-js-from-html` | `0` | `Some("verification")` |
| `terminal-bench/install-windows-3.11` | `0` | `Some("verification")` |
| `terminal-bench/kv-store-grpc` | `1` | `None` |
| `terminal-bench/mteb-retrieve` | `0` | `Some("verification")` |
| `terminal-bench/nginx-request-logging` | `1` | `None` |
| `terminal-bench/protein-assembly` | `0` | `Some("long-horizon")` |
| `terminal-bench/qemu-startup` | `1` | `None` |
| `terminal-bench/query-optimize` | `0` | `Some("long-horizon")` |
| `terminal-bench/sanitize-git-repo` | `0` | `Some("verification")` |
| `terminal-bench/sparql-university` | `1` | `None` |
| `terminal-bench/torch-tensor-parallelism` | `0` | `Some("long-horizon")` |
| `terminal-bench/write-compressor` | `0` | `Some("long-horizon")` |

## Verifier Environment Warnings

These warnings do not change Harbor rewards, but they mark local verifier runtime conditions that can weaken score interpretation.

| Trial | Category | Evidence |
| --- | --- | --- |
| `caffe-cifar-10__Qcav3aR` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `circuit-fibsqrt__opMcMF3` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `configure-git-webserver__VGXS5QA` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `count-dataset-tokens__fNTTfXi` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `extract-elf__3cbfAt8` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `filter-js-from-html__RrPutpj` | `browser-driver-unavailable` | `Failed to create driver or process file: Message: Unable to obtain driver for chrome; For documentation on this error, please visit: https://www.selenium.dev/documentation/webdriver/troubleshooting/errors/driver_location` |
| `filter-js-from-html__RrPutpj` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `install-windows-3.11__du2f5qo` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `nginx-request-logging__syTVtNQ` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `protein-assembly__mSds8yY` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `qemu-startup__ZvxJkTQ` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `query-optimize__PfDK5sr` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `sanitize-git-repo__4VFPKJa` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `sparql-university__TBtKSqw` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `torch-tensor-parallelism__saLCTGy` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |
| `write-compressor__LSYhYWV` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |

## Output Tail

```text

# Provider bridge attempt 1/3
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.58s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-c06edd870a2c5ba3)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=18 concurrency=4 trial_count=1 override_storage_mb=<none> job_path=/Users/leo/Projects/CodeFactory-agent-score-recovery-16/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260714-015143
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings has been running for over 60 seconds
provider_bridge_result status=completed exit_code=Some(0) job_path=/Users/leo/Projects/CodeFactory-agent-score-recovery-16/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260714-015143
provider_bridge_imported run=677cbb1a-b24b-422b-8a64-d4895b15af07 dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=false trials=18 mean_reward=0.333
provider_bridge_trial task=terminal-bench/build-cython-ext reward=0 failure_class=Some("long-horizon")
provider_bridge_trial task=terminal-bench/caffe-cifar-10 reward=0 failure_class=Some("long-horizon")
provider_bridge_trial task=terminal-bench/circuit-fibsqrt reward=0 failure_class=Some("long-horizon")
provider_bridge_trial task=terminal-bench/configure-git-webserver reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/count-dataset-tokens reward=0 failure_class=Some("long-horizon")
provider_bridge_trial task=terminal-bench/extract-elf reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/filter-js-from-html reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/install-windows-3.11 reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/kv-store-grpc reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/mteb-retrieve reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/nginx-request-logging reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/protein-assembly reward=0 failure_class=Some("long-horizon")
provider_bridge_trial task=terminal-bench/qemu-startup reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/query-optimize reward=0 failure_class=Some("long-horizon")
provider_bridge_trial task=terminal-bench/sanitize-git-repo reward=0 failure_class=Some("verification")
provider_bridge_trial task=terminal-bench/sparql-university reward=1 failure_class=None
provider_bridge_trial task=terminal-bench/torch-tensor-parallelism reward=0 failure_class=Some("long-horizon")
provider_bridge_trial task=terminal-bench/write-compressor reward=0 failure_class=Some("long-horizon")
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 205 filtered out; finished in 7678.78s


```
