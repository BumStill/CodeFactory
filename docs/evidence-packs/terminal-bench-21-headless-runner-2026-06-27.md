# Terminal-Bench 2.1 Headless Runner Evidence

## Scope

This evidence pack records the first model-backed CodeFactory headless runner implementation for the Harbor adapter.

The first real provider-backed launch now exists in `docs/evidence-packs/terminal-bench-21-codefactory-provider-deepseek-2026-06-27.md`. That launch reached the configured DeepSeek endpoint through the CodeFactory provider bridge, but the endpoint returned `HTTP 402 Insufficient Balance`; therefore it is a provider-availability result, not a meaningful task-solving score.

## Adapter Contract

- Harbor import path: `codefactory_bench.agent:CodeFactoryAgent`
- Agent name for new runs: `codefactory-headless`
- No-model mode: `baseline-no-model`
- Model-backed mode: `model-backed`
- Required explicit model env:
  - `CODEFACTORY_BENCH_API_KEY`
  - `CODEFACTORY_BENCH_MODEL` or Harbor `-m <model>`
  - Optional `CODEFACTORY_BENCH_BASE_URL`, defaulting to `https://api.openai.com/v1`

The adapter does not read CodeFactory desktop settings, macOS keychain entries, generic provider env vars, or user credentials.

## Implemented Behavior

- OpenAI-compatible chat-completions request loop.
- Single headless tool: `run_shell`, executed through Harbor `BaseEnvironment.exec`.
- `benchmark-sandbox` command gate before execution.
- Hard denies for obvious destructive commands, Harbor solution/test/verifier paths, credential paths, provider secret names, and network/exfiltration tools unless explicitly allowed by `CODEFACTORY_BENCH_ALLOW_NETWORK=1`.
- Trajectory output at `agent/trajectory.json` and `agent/trajectory.jsonl`.
- Final output at `agent/final.txt`.
- Metadata records mode, model, instruction hash, tool-call count, and max steps without logging API keys.

## Local Verification

```bash
PYTHONPATH=/Users/leo/Projects/CodeFactory-terminal-bench-21-design \
  /Users/leo/.local/share/uv/tools/harbor/bin/python \
  tests/test_codefactory_bench_agent.py
```

Observed output:

```text
....
----------------------------------------------------------------------
Ran 4 tests in 1.021s

OK
```

Covered scenarios:

- Harbor import path is stable.
- No-model mode still writes diagnostics and metadata.
- Model-backed mode can call a fake OpenAI-compatible server, receive a `run_shell` tool call, execute it in the fake Harbor environment, and write trajectory.
- `benchmark-sandbox` denies a fake model's `curl http://example.com` command before any environment execution.

## Real Harbor No-Model Smoke After Runner Upgrade

This smoke verifies the upgraded adapter still runs through Harbor and imports back into CodeFactory when explicit model env is absent. It is not a model-backed capability score.

```bash
PYTHONPATH=/Users/leo/Projects/CodeFactory-terminal-bench-21-design \
  harbor run \
    -d terminal-bench/terminal-bench-2-1 \
    --agent-import-path codefactory_bench.agent:CodeFactoryAgent \
    -l 1 \
    -n 1 \
    -o /Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs \
    --job-name cf-tb21-codefactory-headless-nomodel-20260627-1205 \
    -y
```

Result:

- Run id: `19e42aa8-9e97-4f3b-8965-21993f081ae5`
- Task: `terminal-bench/write-compressor`
- Trial: `write-compressor__q3TbaVy`
- Agent: `codefactory-headless`
- Mode: `baseline-no-model`
- Trials: 1
- Exceptions: 0
- Mean reward: 0.000
- Trial reward: 0.0
- Total runtime: 1m 0s

Import evidence:

```text
imported run=19e42aa8-9e97-4f3b-8965-21993f081ae5 dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless comparable=true trials=1
trial=terminal-bench/write-compressor reward=0 failure_class=Some("verification")
```

## Current Blocker For Real Model-Backed Score

The explicit benchmark env vars can now be supplied through the CodeFactory provider bridge without printing raw keys. The current blocker is provider account state:

- Endpoint: `deepseek`
- Model: `deepseek-v4-pro`
- Error: `HTTP 402 Insufficient Balance`

Rerun after funding the endpoint or selecting another configured OpenAI-compatible endpoint.
