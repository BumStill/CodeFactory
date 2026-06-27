# Terminal-Bench 2.1 CodeFactory Provider Run Evidence

## Scope

- Date: 2026-06-27
- Branch: `codex/terminal-bench-21-design`
- PR: `#90`
- Benchmark: Terminal-Bench 2.1
- Dataset: `terminal-bench/terminal-bench-2-1`
- Evaluation subject: `codefactory-headless`
- Model backend: CodeFactory endpoint `deepseek`, model `deepseek-v4-pro`
- Product path: `start_provider_benchmark_run` through CodeFactory settings and explicit provider bridge authorization

This is a CodeFactory agent run using the DeepSeek backend. It must not be reported as a standalone DeepSeek benchmark result.

## Run Command

```bash
CODEFACTORY_RUN_REAL_PROVIDER_BRIDGE=1 \
CODEFACTORY_BENCH_ENDPOINT=deepseek \
CODEFACTORY_BENCH_TASK_LIMIT=1 \
CODEFACTORY_BENCH_TRIAL_COUNT=1 \
CODEFACTORY_BENCH_MODEL_TIMEOUT_SEC=120 \
CODEFACTORY_BENCH_SHELL_TIMEOUT_SEC=120 \
cargo test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings \
  --lib -- --ignored --nocapture
```

The ignored Rust test loads local CodeFactory settings, previews the provider bridge, uses the generated authorization phrase, reads the endpoint key through the product bridge path, launches Harbor, and imports the job into the benchmark schema. Raw provider keys are not printed.

## Preview Evidence

```text
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=1 trial_count=1 job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260627-083120
```

## Result

- Harbor job path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260627-083120`
- Run id: `01801dd1-b725-45d8-844d-c0cc6b608803`
- Dataset: `terminal-bench/terminal-bench-2-1`
- Dataset ref: `sha256:7d7bdc1cbedad549fc1140404bd4dc45e5fd0ea7c4186773687d177ad3a0699a`
- Agent: `codefactory-headless`
- Agent version: `1.40.1`
- Model: `deepseek-v4-pro`
- Trial count: 1
- Task: `terminal-bench/write-compressor`
- Mean reward: `0.000`
- Trial reward: `0.0`
- Comparable import: `true`
- Harbor stats: `n_completed_trials=1`, `n_errored_trials=1`
- Exception type: `RuntimeError`
- Failure class after import: `model-provider`

Observed test output:

```text
provider_bridge_result status=completed exit_code=Some(0) job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260627-083120
provider_bridge_imported run=01801dd1-b725-45d8-844d-c0cc6b608803 dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=true trials=1 mean_reward=0.000
provider_bridge_trial task=terminal-bench/write-compressor reward=0 failure_class=Some("planning")
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok
```

The first import happened before the importer separated provider/API failures from agent planning. The same job was re-imported after the classifier fix:

```bash
CODEFACTORY_BENCHMARK_JOB_PATH=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260627-083120 \
cargo test benchmark::tests::import_harbor_job_from_env_path --lib -- --ignored --nocapture
```

Re-import evidence:

```text
imported run=01801dd1-b725-45d8-844d-c0cc6b608803 dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless comparable=true trials=1
trial=terminal-bench/write-compressor reward=0 failure_class=Some("model-provider")
test benchmark::tests::import_harbor_job_from_env_path ... ok
```

## Failure Cause

The provider request failed before CodeFactory could perform the task:

```text
RuntimeError: model request failed: HTTP 402: {"error":{"message":"Insufficient Balance","type":"unknown_error","param":null,"code":"invalid_request_error"}}
```

This is a `model-provider` blocker result. It proves the CodeFactory product bridge, Harbor launch, adapter identity, and import path work with a real configured endpoint, but it is not a meaningful CodeFactory task-solving capability score until the endpoint has usable balance or another configured provider is selected.

## Product Findings

- The default shell environment did not include `~/.local/bin`, so `harbor` was initially not discoverable even though uv tool had installed it.
- CodeFactory now resolves Harbor from `PATH`, `~/.local/bin/harbor`, or `~/.local/share/uv/tools/harbor/bin/harbor`.
- Provider/API errors are now included in trial evidence and classified as `model-provider` instead of falling through to `planning`.

## Next Run

Rerun the same ignored test after funding the DeepSeek endpoint or selecting another configured OpenAI-compatible endpoint:

```bash
CODEFACTORY_RUN_REAL_PROVIDER_BRIDGE=1 \
CODEFACTORY_BENCH_ENDPOINT=<endpoint> \
CODEFACTORY_BENCH_TASK_LIMIT=1 \
CODEFACTORY_BENCH_TRIAL_COUNT=1 \
cargo test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings \
  --lib -- --ignored --nocapture
```
