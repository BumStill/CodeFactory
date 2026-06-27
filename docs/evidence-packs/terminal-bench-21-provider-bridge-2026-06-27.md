# Terminal-Bench 2.1 Provider Bridge Evidence Pack

## Scope

- Date: 2026-06-27
- Branch: `codex/terminal-bench-21-design`
- PR: `#90`
- Feature: explicitly authorized CodeFactory provider bridge for Terminal-Bench 2.1

## Implemented Behavior

- `preview_benchmark_provider_bridge(request)` resolves the current or specified CodeFactory endpoint/model into a benchmark launch preview.
- The preview includes dataset, agent import path, task limit, trial count, job path, adapter root, redacted env, command preview, and an authorization phrase.
- Preview does not read the OS credential store and does not return raw API keys.
- `start_benchmark_provider_run(request)` requires the exact authorization phrase before reading the endpoint key from the OS credential store.
- After authorization, the provider key is injected only into the Harbor child process env as `CODEFACTORY_BENCH_API_KEY`.
- Raw keys are not included in command preview, Harbor args, returned stdout/stderr metadata, SQLite run command, or evidence pack content.
- Direct provider model ids are normalized with existing CodeFactory semantics. For example, DeepSeek direct API receives `deepseek-v4-flash` rather than an OpenRouter-routed `deepseek/deepseek-v4-flash`.

## Validation

Command:

```bash
cargo test provider_bridge --lib
```

Result:

```text
running 3 tests
test benchmark::tests::provider_bridge_preview_uses_current_deepseek_without_exposing_secret ... ok
test benchmark::tests::provider_bridge_requires_authorization_before_secret_lookup ... ok
test benchmark::tests::provider_bridge_authorized_launch_injects_secret_only_into_child_env ... ok

test result: ok. 3 passed; 0 failed
```

## Security Assertions

- Failed authorization returns before secret lookup.
- Successful authorization looks up only the endpoint `key_ref`.
- The raw key appears only in the in-memory child process env vector used to spawn Harbor.
- The frontend receives only redacted env values.

## Current Boundary

- This verifies the product/backend contract for using the current provider in a benchmark run.
- It does not yet produce a real DeepSeek-backed Terminal-Bench score.
- Real scoring still requires launching `start_benchmark_provider_run` from the CodeFactory app/runtime after explicit user authorization, then importing the Harbor job result.
