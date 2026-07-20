# Terminal-Bench 2.1 Product Iteration Report

- generated_at: `2026-07-20T18-44-00Z`
- evaluation_axis: `codefactory-agent-capability`
- evaluation_subject: `codefactory-headless`
- scope: `canary`
- subset_path: `/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-subsets/terminal-bench-21-canary-subset.json`
- endpoint: `deepseek`
- model: `deepseek-v4-pro`
- shell_timeout_sec: `300`
- override_storage_mb: `<none>`
- official_comparable: `no`
- hypothesis: `Released v1.51.2 generic safe heredoc classification and explicit observable-state completion evidence improve artifact and service-state tasks without regressing shared Agent behavior.`
- target_failure_class: `long-horizon`
- ran_command: `yes`
- exit_code: `124`

## Baseline

- path: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-07-20T14-20-53Z.md`
- run: `dd54bc63-0f54-4243-ba0e-a1f5d81b9562`
- pass_count: `6`
- trials: `18`
- mean_reward: `0.333`

## Head

- path: `not available`
- run: `not available`
- pass_count: `unknown`
- trials: `unknown`
- mean_reward: `unknown`

## Partial Non-Comparable Result

- source_job: `6e44ee48-b722-4d97-a41b-acbc2020af4b`
- scheduled_trials: `6`
- completed_result_entries: `6`
- reward_1: `1`
- reward_0: `3`
- errored_or_cancelled: `2`
- usage_lower_bound_trials: `5`
- model_requests_lower_bound: `329`
- total_tokens_lower_bound: `2692272`
- tool_calls_all_trials: `329`
- comparability: `no`; the outer `1800s` timeout cancelled `circuit-fibsqrt`, and `qemu-startup` produced no reward file after its emulated main container stopped.

| Task | Terminal result | Product diagnosis |
| --- | --- | --- |
| `configure-git-webserver` | reward `1` | Historical canary pass held. |
| `build-cython-ext` | reward `0` | Source install/runtime/project-test blockers remained after 80 model requests. |
| `write-compressor` | reward `0` | Literal heredoc no longer activated service lifecycle, but the generated compressor still failed functional verification. |
| `sanitize-git-repo` | reward `0` | Agent exhausted 80 requests without a successful final verification. |
| `qemu-startup` | `RewardFileNotFoundError` | The requested `login` state was correctly required and never observed; continued recovery stopped the emulated Harbor main container, so verifier output was unavailable. |
| `circuit-fibsqrt` | `CancelledError` | After an initial candidate mutation, the Agent repeatedly re-read source slices instead of converging on the next edit or functional test; cancellation lost partial model usage. |

## Product Capability Impact

- verdict: product-capability
- capability: CodeFactory distinguishes literal source payloads from shell control flow and refuses to complete user-visible runtime tasks until the requested state is observed after the latest mutation or service start.
- non_benchmark_example: A user asks CodeFactory to generate source containing shell-like characters, launch a local service, and wait for a login or readiness message; the Agent must preserve the source literally and prove the requested visible state before completion.
- benchmark_only_boundary: Only Harbor task selection, Docker preflight, provider transport, scoring import, and evidence reporting are benchmark infrastructure; no task name, expected answer, artifact-specific branch, or verifier logic enters the product Agent.

## Delta

- comparable_delta: `no`
- reason: baseline or head evidence is unavailable.

## Failure Class Counts

Baseline:
- `model-provider`: `2`
- `pass`: `6`
- `verification`: `10`

Head:
- no trial failure table available

## Next Improvement Queue

- P0: apply the bounded inspection window after every mutation, not only before the first implementation; after exhaustion require a corrective edit or bounded functional probe.
- P1: persist partial usage before cancellation/timeout so long-task cost does not disappear from diagnostics.
- P1: move nested-QEMU confirmation to clean Linux/x86; do not treat this Mac emulation failure as a product reward or customize the Agent for the fixture.
- P2: rerun a released long-task canary with an outer timeout that can collect the selected per-trial terminal result. Run the fixed 18 only after a score-facing behavior delta appears.

## Command Output Tail

```text
# Terminal-Bench 2.1 regression subset run plan

- subset: `terminal-bench-21-regression-subset-v1-canary`
- subset path: `/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-subsets/terminal-bench-21-canary-subset.json`
- tasks: `6`
- endpoint: `deepseek`
- model: `deepseek-v4-pro`
- concurrency: `4`
- min_docker_cpus: `4.0`
- min_docker_memory_gb: `6.0`
- min_docker_free_gb: `20.0`
- resource_preflight: `enabled`
- bind_mount_preflight: `enabled`
- preflight_retries: `1`
- agent_binary: `<build from current source before launch>`
- agent_build_timeout_sec: `900`
- override_storage_mb: `<none>`
- official_comparable: `yes`
- explicit CODEFACTORY_BENCH_API_KEY present: `no`
- keychain timeout: `20s`
- trial_hard_timeout_sec: `<disabled>`
- heavy_verifier_timeout_overrides: `<none>`
- heavy_verifier_timeout_multiplier: `<none>`
- docker_apt_proxy: `<none>`
- verifier_proxy: `<none>`
- provider_proxy: `<none>`
- provider_bridge_retries: `2`
- verifier_uv_http_timeout_sec: `<none>`
- verifier_uv_torch_backend: `<none>`
- partial_import_diagnostic: `enabled`
- job root: `/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-jobs`
- agent PYTHONPATH root: `/Users/leo/Projects/CodeFactory-tb21-v1511`
- command: `cargo test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings --lib -- --ignored --nocapture`

Tasks:
- `write-compressor`
- `circuit-fibsqrt`
- `configure-git-webserver`
- `qemu-startup`
- `sanitize-git-repo`
- `build-cython-ext`

Verifying bidirectional Docker bind mounts...
- Docker bind mount is bidirectional: /Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-preflight

Preparing current-source CodeFactory headless Agent...

BENCHMARK_RUN_TIMEOUT: exceeded 1800 seconds

```
