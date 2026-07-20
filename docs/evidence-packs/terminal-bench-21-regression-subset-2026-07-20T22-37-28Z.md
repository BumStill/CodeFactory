# Terminal-Bench 2.1 Regression Subset Evidence

- generated_at: `2026-07-20T22-37-28Z`
- subset: `terminal-bench-21-regression-subset-v1-canary`
- source_run_id: `7ff6ef13-4488-4e0f-afd0-a1f9bd16d561`
- task_count: `1`
- endpoint: `deepseek`
- exit_code: `0`
- override_storage_mb: `<none>`
- official_comparable: `no`
- explicit_key_present: `no`
- trial_hard_timeout_sec: `900`
- heavy_verifier_timeout_overrides: `<none>`
- heavy_verifier_timeout_multiplier: `<none>`
- verifier_uv_torch_backend: `<none>`
- partial_import_diagnostic: `enabled`

## Agent Binary Preflight

- agent binary source: built from current source (/Users/leo/Projects/CodeFactory-tb21-v1511/src-tauri/target/debug/codefactory-agent-headless)
- agent binary sha256: ff0d58765206de502090a59c15912fa3ee8da20d21faf475a17d8756564bbad0

## Comparability Notes

- runner-level trial hard timeout watchdog was enabled

## Preview

- model: `deepseek-v4-pro`
- task_limit: `1`
- concurrency: `1`
- override_storage_mb: `<none>`
- job_path: `/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260720-222901`

## Provider Bridge

- status: `completed`
- exit_code: `Some(0)`
- job_path: `/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260720-222901`

## Agent Usage

- trials_with_metadata: `1`
- model_requests: `18`
- prompt_tokens: `116852`
- completion_tokens: `20959`
- total_tokens: `137811`
- tool_calls: `11`

## Agent Completion Evidence

- completed_trials: `1 / 1`
- recorded_outcomes: `11`
- external_tool_requests: `11`
- recorded_non_external_outcomes: `0`
- blockers: `<none>`
- final_stop_summaries: `Implementation completed and post-change verification passed.`

## Result

- run: `368896e8-b148-4413-b00f-cbb07ac9624c`
- dataset: `terminal-bench/terminal-bench-2-1`
- agent: `codefactory-headless`
- model: `Some("deepseek-v4-pro")`
- harbor_import_comparable: `true`
- trials: `1`
- pass_count: `0`
- mean_reward: `0.000`

## Trials

| Task | Reward | Failure class |
| --- | ---: | --- |
| `terminal-bench/circuit-fibsqrt` | `0` | `Some("verification")` |

## Verifier Environment Warnings

These warnings do not change Harbor rewards, but they mark local verifier runtime conditions that can weaken score interpretation.

| Trial | Category | Evidence |
| --- | --- | --- |
| `circuit-fibsqrt__Tm9wENw` | `emulated-browser-runtime` | `ERROR: unknown platform bitness` |

## Output Tail

```text

# Provider bridge attempt 1/3
   Compiling codefactory v1.51.6 (/Users/leo/Projects/CodeFactory-tb21-v1511/src-tauri)
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
209 | pub async fn mark_task_started(pool: &SqlitePool, id: &str) -> Result<(...
    |              ^^^^^^^^^^^^^^^^^

warning: `codefactory` (lib test) generated 4 warnings
    Finished `test` profile [unoptimized + debuginfo] target(s) in 12.96s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-77f4d9ffb1ceaaba)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=1 concurrency=1 trial_count=1 override_storage_mb=<none> job_path=/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260720-222901
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings has been running for over 60 seconds
provider_bridge_result status=completed exit_code=Some(0) job_path=/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260720-222901
provider_bridge_imported run=368896e8-b148-4413-b00f-cbb07ac9624c dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=true trials=1 mean_reward=0.000
provider_bridge_trial task=terminal-bench/circuit-fibsqrt reward=0 failure_class=Some("verification")
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 382 filtered out; finished in 507.54s


```
