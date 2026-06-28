# Terminal-Bench 2.1 CodeFactory DeepSeek Evidence - 2026-06-28

## Scope

- Branch: `codex/terminal-bench-21-design`
- PR: `#90`
- Benchmark: Terminal-Bench 2.1
- Dataset: `terminal-bench/terminal-bench-2-1`
- Evaluation subject: `codefactory-headless`
- Model backend: CodeFactory endpoint `deepseek`, model `deepseek-v4-pro`
- Product path: CodeFactory provider bridge loads local endpoint settings, prints only redacted preview data, injects the provider key into the Harbor child process, then imports the Harbor job artifact.

This is a CodeFactory agent evaluation using DeepSeek as the backend model. It must not be reported as a standalone DeepSeek benchmark result.

## Local Loop Hardening

Implemented and verified in the headless adapter:

- Compact chat payloads before model calls so large tool-call command bodies are summarized instead of repeatedly sent back to the model.
- Configurable tool output limit via `CODEFACTORY_BENCH_TOOL_OUTPUT_LIMIT`.
- Bounded model request timeout based on remaining agent wall-clock.
- Controlled handling for `TimeoutError`, `IncompleteRead`, and remote disconnects from the provider request path.
- Repair hints for segmentation faults, missing artifacts, missing tools, and timed-out commands.
- Repeated low-value inspection suppression for repeated read-only commands and repeated reads of the same file.
- Exact stdout verification hints extracted from task text such as `running <command> gives exactly <file>`.
- Phase reminders that push the model from inspection into implementation.

Local validation:

```text
...................
----------------------------------------------------------------------
Ran 19 tests in 1.539s

OK
```

Additional syntax check:

```bash
python3 -m py_compile codefactory_bench/agent.py tests/test_codefactory_bench_agent.py
```

## Controlled Timeout Run

Command:

```bash
CODEFACTORY_RUN_REAL_PROVIDER_BRIDGE=1 \
CODEFACTORY_BENCH_ENDPOINT=deepseek \
CODEFACTORY_BENCH_TASK_LIMIT=1 \
CODEFACTORY_BENCH_TRIAL_COUNT=1 \
CODEFACTORY_BENCH_MODEL_TIMEOUT_SEC=60 \
CODEFACTORY_BENCH_SHELL_TIMEOUT_SEC=45 \
CODEFACTORY_BENCH_MAX_STEPS=28 \
CODEFACTORY_BENCH_AGENT_WALL_TIMEOUT_SEC=720 \
CODEFACTORY_BENCH_TOOL_OUTPUT_LIMIT=20000 \
cargo test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings \
  --lib -- --ignored --nocapture
```

Result:

- Harbor job path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-044653`
- Run id: `5e08d50b-ca8e-4efa-94ab-68bc18918814`
- Task: `terminal-bench/write-compressor`
- Agent: `codefactory-headless`
- Model: `deepseek-v4-pro`
- Mean reward: `0.000`
- Harbor stats: `n_completed_trials=1`, `n_errored_trials=0`
- Failure class after import: `verification`
- Evidence: trajectory contained `role=model-error` with `model request timed out: The read operation timed out`; Harbor still completed and CodeFactory imported the result.

Observed output:

```text
provider_bridge_imported run=5e08d50b-ca8e-4efa-94ab-68bc18918814 dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=true trials=1 mean_reward=0.000
provider_bridge_trial task=terminal-bench/write-compressor reward=0 failure_class=Some("verification")
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok
```

## Repetition Suppression Run

Command: same provider bridge command with `CODEFACTORY_BENCH_MODEL_TIMEOUT_SEC=60`.

Result:

- Harbor job path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-050213`
- Run id: `5f1b77a2-650c-4115-851b-d868e4725b44`
- Task: `terminal-bench/write-compressor`
- Mean reward: `0.000`
- Harbor stats: `n_completed_trials=1`, `n_errored_trials=0`
- Failure class after import: `verification`
- Evidence: repeated `cat /app/decomp.c` was suppressed and the model moved to another command, but later hit a controlled model read timeout before creating `/app/data.comp`.

Observed output:

```text
provider_bridge_imported run=5f1b77a2-650c-4115-851b-d868e4725b44 dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=true trials=1 mean_reward=0.000
provider_bridge_trial task=terminal-bench/write-compressor reward=0 failure_class=Some("verification")
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok
```

## IncompleteRead Boundary

One intermediate run exposed an uncaught provider transport error:

- Harbor job path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-050745`
- Run id: `e833bcbb-04f2-41ec-a76d-edd818bc80ac`
- Exception stats before fix: `IncompleteRead`
- Imported failure class: `environment`

This was not a real task environment failure. The adapter now converts `IncompleteRead` and provider remote disconnects into controlled model request timeouts so Harbor can finish collecting artifacts.

## Latest Post-Fix Run

Command: same provider bridge command with `CODEFACTORY_BENCH_MODEL_TIMEOUT_SEC=60`.

Result:

- Harbor job path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-053114`
- Run id: `5d9246fe-4662-4e93-996d-5f3597d9e56e`
- Task: `terminal-bench/write-compressor`
- Agent: `codefactory-headless`
- Model: `deepseek-v4-pro`
- Mean reward: `0.000`
- Harbor stats: `n_completed_trials=1`, `n_errored_trials=0`
- Failure class after import: `verification`
- End-to-end test runtime: `181.39s`
- Evidence: missing-tool repair hint and early implementation reminder were appended before the next model request; model transport timeout was converted into a controlled `model-error`; Harbor still completed and CodeFactory imported the run.

Observed output:

```text
provider_bridge_result status=completed exit_code=Some(0) job_path=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260628-053114
provider_bridge_imported run=5d9246fe-4662-4e93-996d-5f3597d9e56e dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless model=Some("deepseek-v4-pro") comparable=true trials=1 mean_reward=0.000
provider_bridge_trial task=terminal-bench/write-compressor reward=0 failure_class=Some("verification")
test benchmark::tests::provider_bridge_runs_real_codefactory_endpoint_from_local_settings ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 164 filtered out; finished in 181.39s
```

Verifier evidence:

```text
reward.txt: 0
test_compressed_file_exists: failed because /app/data.comp does not exist
test_decompression_produces_original: failed because /app/data.comp does not exist
test_compression_size: failed because /app/data.comp does not exist
```

## Product Finding

The first CodeFactory-owned DeepSeek evaluation path is now operational and produces comparable imported results. The current score is still `0.000`; this is not a provider-balance issue. The dominant capability gap is the CodeFactory headless agent loop: natural-language reminders now reach the model, but they are not enough. The next slice should enforce implementation state mechanically so the agent starts producing/verifying the required artifact before the model transport timeout boundary.

## Next Slice

- Add a stronger implementation-first loop state after initial inspection.
- Consider provider/model ablation only as an attributed comparison: CodeFactory scaffold fixed, backend model varied.
- Persist evaluation axis and fixed/changed variables in the run schema/UI so these runs are not confused with raw model leaderboard scores.
