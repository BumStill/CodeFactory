# Terminal-Bench 2.1 Regression Subset Evidence

- generated_at: `2026-07-21T00-45-00Z`
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
- agent binary sha256: dc8cafe28a14893fcf977c0dde7f5828557ff2e8d62be4bd4c2b60fc5428d21d

## Comparability Notes

- runner-level trial hard timeout watchdog was enabled
- watchdog stopped one or more stale trial containers

## Preview

- model: `deepseek-v4-pro`
- task_limit: `1`
- concurrency: `1`
- override_storage_mb: `<none>`
- job_path: `/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260721-002231`

## Provider Bridge

- status: `completed`
- exit_code: `Some(0)`
- job_path: `/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260721-002231`

## Agent Usage

- trials_with_metadata: `1`
- model_requests: `80`
- prompt_tokens: `614840`
- completion_tokens: `71512`
- total_tokens: `686352`
- tool_calls: `69`

## Agent Completion Evidence

- completed_trials: `0 / 1`
- recorded_outcomes: `69`
- external_tool_requests: `69`
- recorded_non_external_outcomes: `0`
- blockers: `the requested output or return value requires a later machine-checked assertion that exits nonzero on mismatch; printing expected and actual values is diagnostic evidence, not verification; background services require a recorded PID, pidfile, or process handle; background services require an explicit log destination; background services require a later successful bounded functional probe`
- final_stop_summaries: `I cannot make progress — every command I run, including the most basic ('echo', 'touch', 'printf', 'true'), returns only 'service "main" is not running' with exit code 1. This began mid-session (after step 22 of the prior history) and persists across all subsequent attempts. The sandbox's command-execution proxy/service is down and requires infrastructure-level restart before I can read '/app/sim.c', generate '/app/gates.txt', compile, run, or verify anything.`

## Result

- run: `d6ad2b52-412c-48d4-8a6e-3f4ed80e9920`
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
| `terminal-bench/circuit-fibsqrt` | `0` | `Some("environment")` |

## Watchdog Interventions

The regression runner stopped stale trial containers so the remaining matrix could finish.

| Trial | Elapsed sec | Action | Containers |
| --- | ---: | --- | --- |
| `circuit-fibsqrt__RZF5wxe` | `900` | `docker-stop` | `circuit-fibsqrt__rzf5wxe-main-1` |

## Output Tail

```text

# Provider bridge attempt 1/3
   Compiling codefactory v1.51.7 (/Users/leo/Projects/CodeFactory-tb21-v1511/src-tauri)
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
    Finished `test` profile [unoptimized + debuginfo] target(s) in 13.32s
     Running unittests src/lib.rs (target/debug/deps/codefactory_lib-bee8aa29a650f25d)

running 1 test
provider_bridge_preview endpoint=deepseek base_url=https://api.deepseek.com model=deepseek-v4-pro key_ref=codefactory.endpoint.deepseek agent=codefactory_bench.agent:CodeFactoryAgent task_limit=1 concurrency=1 trial_count=1 override_storage_mb=<none> job_path=/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260721-002231
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings has been running for over 60 seconds
provider_bridge_result status=completed exit_code=Some(0) job_path=/Users/leo/Projects/CodeFactory-tb21-v1511/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260721-002231
provider_bridge_imported run=d6ad2b52-412c-48d4-8a6e-3f4ed80e9920 dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=true trials=1 mean_reward=0.000
provider_bridge_trial task=terminal-bench/circuit-fibsqrt reward=0 failure_class=Some("environment")
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 384 filtered out; finished in 1348.95s


```
