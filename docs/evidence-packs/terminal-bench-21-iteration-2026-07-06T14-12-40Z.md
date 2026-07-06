# Terminal-Bench 2.1 Product Iteration Report

- generated_at: `2026-07-06T14-12-40Z`
- evaluation_axis: `codefactory-agent-capability`
- evaluation_subject: `codefactory-headless`
- scope: `canary`
- subset_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-16/.codefactory/benchmark-subsets/terminal-bench-21-canary-subset.json`
- endpoint: `deepseek`
- model: `deepseek-v4-pro`
- shell_timeout_sec: `300`
- override_storage_mb: `<none>`
- official_comparable: `yes`
- hypothesis: `Windows GUI runtime tasks should leave a foreground visual-feedback state instead of stopping at a superficially ready Program Manager screen`
- target_failure_class: `gui-runtime-verification`
- ran_command: `yes`
- exit_code: `0`

## Baseline

- path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-16/docs/evidence-packs/terminal-bench-21-regression-subset-baseline-2026-06-28T15-41-50Z.md`
- run: `not available`
- pass_count: `4`
- trials: `18`
- mean_reward: `0.222222`

## Head

- path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-16/docs/evidence-packs/terminal-bench-21-regression-subset-2026-07-06T14-12-40Z.md`
- run: `ccd3bd20-73e4-4ca9-a50b-7e5afbab281a`
- pass_count: `1`
- trials: `1`
- mean_reward: `1.0`

## Product Capability Impact

- verdict: mixed
- capability: CodeFactory improves real interactive runtime reliability by verifying that GUI or VM environments are not merely listening on a port but are visibly ready and responsive to input.
- non_benchmark_example: When a user asks CodeFactory to start a VNC-backed desktop, browser, emulator, or dev VM, it can wait for a usable foreground state and prove keyboard feedback before claiming the environment is ready.
- benchmark_only_boundary: Windows 3.11, /app/isos/win311.img, VNC :1, and /tmp/qemu-monitor.sock are Terminal-Bench scaffold; the reusable product capability is visual readiness, input feedback validation, and deterministic recovery for interactive runtimes.

## Delta

- comparable_delta: `no`
- reason: baseline and head have different trial counts; use this report as targeted canary evidence, not an aggregate score delta.

## Failure Class Counts

Baseline:
- `environment`: `2`
- `long-horizon`: `4`
- `pass`: `4`
- `tool-use`: `3`
- `verification`: `5`

Head:
- `pass`: `1`

## Next Improvement Queue

- P0: inspect the dominant failure class and choose one targeted canary before broader regression.
- P1: rerun the fixed subset only after the targeted canary shows a behavior delta.

## Command Output Tail

```text
# Terminal-Bench 2.1 regression subset run plan

- subset: `terminal-bench-21-regression-subset-v1-canary`
- subset path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-16/.codefactory/benchmark-subsets/terminal-bench-21-canary-subset.json`
- tasks: `1`
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
- trial_hard_timeout_sec: `1500`
- heavy_verifier_timeout_overrides: `torch-tensor-parallelism:2400`
- heavy_verifier_timeout_multiplier: `<none>`
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
- `install-windows-3.11`
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.46s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-2e0d3f031370550b)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=1 concurrency=1 trial_count=1 override_storage_mb=<none> job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-16/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260706-140632
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings has been running for over 60 seconds
provider_bridge_result status=completed exit_code=Some(0) job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-16/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260706-140632
provider_bridge_imported run=ccd3bd20-73e4-4ca9-a50b-7e5afbab281a dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=true trials=1 mean_reward=1.000
provider_bridge_trial task=terminal-bench/install-windows-3.11 reward=1 failure_class=None
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 176 filtered out; finished in 367.94s


Evidence report: /Users/leo/Projects/CodeFactory-terminal-bench-21-16/docs/evidence-packs/terminal-bench-21-regression-subset-2026-07-06T14-12-40Z.md

```
