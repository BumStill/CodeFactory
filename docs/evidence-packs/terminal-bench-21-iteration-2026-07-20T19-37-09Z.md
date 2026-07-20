# Terminal-Bench 2.1 Product Iteration Report

- generated_at: `2026-07-20T19-37-09Z`
- evaluation_axis: `codefactory-agent-capability`
- evaluation_subject: `codefactory-headless`
- scope: `canary`
- subset_path: `/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-subsets/terminal-bench-21-canary-subset.json`
- endpoint: `deepseek`
- model: `deepseek-v4-pro`
- shell_timeout_sec: `300`
- override_storage_mb: `<none>`
- official_comparable: `no`
- hypothesis: `Released v1.51.3 generic post-mutation inspection pressure prevents long coding tasks from repeatedly rereading source and returns the Agent to corrective edits or bounded functional verification.`
- target_failure_class: `long-horizon`
- ran_command: `yes`
- exit_code: `0`

## Baseline

- path: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-07-20T14-20-53Z.md`
- run: `dd54bc63-0f54-4243-ba0e-a1f5d81b9562`
- pass_count: `6`
- trials: `18`
- mean_reward: `0.333`

## Head

- path: `/Users/leo/Projects/CodeFactory-tb21-v1511/docs/evidence-packs/terminal-bench-21-regression-subset-2026-07-20T19-37-09Z.md`
- run: `15f8ff2a-e0e4-455f-ac1f-884e724539f7`
- pass_count: `0`
- trials: `1`
- mean_reward: `0.0`

## Product Capability Impact

- verdict: product-capability
- capability: CodeFactory long coding sessions remain action-oriented after candidate edits instead of entering unlimited source reread loops.
- non_benchmark_example: After modifying a numerical library, CodeFactory may inspect a bounded set of affected source sections, then must make the next corrective edit or run a bounded behavioral test before reading more.
- benchmark_only_boundary: Harbor task selection, container lifecycle, verifier execution, score import, and evaluation reporting remain benchmark infrastructure; no task identity or expected answer enters the product Agent.

## Delta

- comparable_delta: `no`
- reason: head evidence is marked non-comparable.

## Failure Class Counts

Baseline:
- `model-provider`: `2`
- `pass`: `6`
- `verification`: `10`

Head:
- `verification`: `1`

## Next Improvement Queue

- P0: inspect the dominant failure class and choose one targeted canary before broader regression.
- P1: rerun the fixed subset only after the targeted canary shows a behavior delta.

## Command Output Tail

```text
# Terminal-Bench 2.1 regression subset run plan

- subset: `terminal-bench-21-regression-subset-v1-canary`
- subset path: `/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-subsets/terminal-bench-21-canary-subset.json`
- tasks: `1`
- endpoint: `deepseek`
- model: `deepseek-v4-pro`
- concurrency: `1`
- min_docker_cpus: `4.0`
- min_docker_memory_gb: `6.0`
- min_docker_free_gb: `20.0`
- resource_preflight: `enabled`
- bind_mount_preflight: `enabled`
- preflight_retries: `1`
- agent_binary: `<build from current source before launch>`
- agent_build_timeout_sec: `900`
- override_storage_mb: `<none>`
- official_comparable: `no`
- explicit CODEFACTORY_BENCH_API_KEY present: `no`
- keychain timeout: `20s`
- trial_hard_timeout_sec: `900`
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
- `circuit-fibsqrt`

Verifying bidirectional Docker bind mounts...
- Docker bind mount is bidirectional: /Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-preflight

Preparing current-source CodeFactory headless Agent...
- agent binary source: built from current source (/Users/leo/Projects/CodeFactory-tb21-v1511/src-tauri/target/debug/codefactory-agent-headless)
- agent binary sha256: 5fa79c007aa348311b5faf05d4358befc0dfe67b729a0dfc53579554572a53db
   Compiling codefactory v1.51.3 (/Users/leo/Projects/CodeFactory-tb21-v1511/src-tauri)
   Compiling codefactory-agent-core v0.1.0 (/Users/leo/Projects/CodeFactory-tb21-v1511/src-tauri/crates/agent-core)
warning: multiple fields are never read
   --> src/agent/journal.rs:133:9
    |
131 | pub struct JournalRow {
    |            ---------- fields in this struct
132 |     pub task_id: String,
133 |     pub session_id: String,
    |         ^^^^^^^^^^
134 |     pub hash_version: i64,
135 |     pub local_digest: String,
    |         ^^^^^^^^^^^^
136 |     pub dispatch_key: String,
137 |     pub dep_keys_json: String,
    |         ^^^^^^^^^^^^^
138 |     pub resolved_model: String,
    |         ^^^^^^^^^^^^^^
139 |     pub resolved_tools_json: String,
    |         ^^^^^^^^^^^^^^^^^^^
...
146 |     pub checkpoint_id: Option<String>,
    |         ^^^^^^^^^^^^^
147 |     pub base_sha: Option<String>,
    |         ^^^^^^^^
...
151 |     pub completed_at: String,
    |         ^^^^^^^^^^^^
152 |     pub updated_at: String,
    |         ^^^^^^^^^^
    |
    = note: `JournalRow` has derived impls for the traits `Clone` and `Debug`, but these are intentionally ignored during dead code analysis
    = note: `#[warn(dead_code)]` (part of `#[warn(unused)]`) on by default

warning: field `max_parallel` is never read
   --> src/agent/scheduler.rs:174:9
    |
172 | pub struct TaskScheduler {
    |            ------------- field in this struct
173 |     pub pool: SqlitePool,
174 |     pub max_parallel: usize,
    |         ^^^^^^^^^^^^
    |
    = note: `TaskScheduler` has a derived impl for the trait `Clone`, but this is intentionally ignored during dead code analysis

warning: method `is_empty` is never used
  --> src/storage/tasks.rs:31:12
   |
30 | impl TaskConnectorContext {
   | ------------------------- method in this implementation
31 |     pub fn is_empty(&self) -> bool {
   |            ^^^^^^^^

warning: function `mark_task_started` is never used
   --> src/storage/tasks.rs:209:14
    |
209 | pub async fn mark_task_started(pool: &SqlitePool, id: &str) -> Result<()> {
    |              ^^^^^^^^^^^^^^^^^

warning: `codefactory` (lib test) generated 4 warnings
    Finished `test` profile [unoptimized + debuginfo] target(s) in 13.02s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-5a29f7e844d0b967)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=1 concurrency=1 trial_count=1 override_storage_mb=<none> job_path=/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260720-192457
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings has been running for over 60 seconds
provider_bridge_result status=completed exit_code=Some(0) job_path=/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260720-192457
provider_bridge_imported run=15f8ff2a-e0e4-455f-ac1f-884e724539f7 dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=true trials=1 mean_reward=0.000
provider_bridge_trial task=terminal-bench/circuit-fibsqrt reward=0 failure_class=Some("verification")
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 380 filtered out; finished in 731.64s


Evidence report: /Users/leo/Projects/CodeFactory-tb21-v1511/docs/evidence-packs/terminal-bench-21-regression-subset-2026-07-20T19-37-09Z.md

```
