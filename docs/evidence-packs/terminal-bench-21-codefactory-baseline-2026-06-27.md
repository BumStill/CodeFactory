# Terminal-Bench 2.1 CodeFactory Baseline Evidence

## Scope

This evidence pack records the first real Terminal-Bench 2.1 Harbor run using a CodeFactory-owned custom agent adapter.

This is a baseline adapter run, not a full model-backed CodeFactory agent evaluation. The agent class is `codefactory_bench.agent:CodeFactoryAgent`; it runs in Harbor as `codefactory-headless-baseline`, records sandbox diagnostics, and does not load user credentials or call an LLM.

## Environment

- Date: 2026-06-27
- Harbor: 0.15.0
- Docker runtime: Colima + Docker Engine 29.5.2
- Docker Compose: 5.2.0
- Dataset: `terminal-bench/terminal-bench-2-1`
- Job path: `.codefactory/benchmark-jobs/cf-tb21-codefactory-baseline-20260627-1145`
- Agent import path: `codefactory_bench.agent:CodeFactoryAgent`
- Agent name: `codefactory-headless-baseline`
- Agent version: `1.40.0`
- Agent mode: `baseline-no-model`

## Command

```bash
PYTHONPATH=/Users/leo/Projects/CodeFactory-terminal-bench-21-design \
  harbor run \
    -d terminal-bench/terminal-bench-2-1 \
    --agent-import-path codefactory_bench.agent:CodeFactoryAgent \
    -l 1 \
    -n 1 \
    -o /Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs \
    --job-name cf-tb21-codefactory-baseline-20260627-1145 \
    -y
```

## Result

- Run id: `3bcbc381-e510-4317-8947-fbb5a1e64bcd`
- Task: `terminal-bench/write-compressor`
- Trial: `write-compressor__2tew5QP`
- Trials: 1
- Exceptions: 0
- Mean reward: 0.000
- Trial reward: 0.0
- Failure class after CodeFactory import: `verification`
- Total runtime: 1m 4s
- Agent execution: completed with `exec_return_code=0`
- Instruction hash: `c4fc3e73e44deeac95f1cd4a34a2d87e853b22ceec2b2824767f5f087b470bb2`

## CodeFactory Import Evidence

```bash
CODEFACTORY_BENCHMARK_JOB_PATH=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-baseline-20260627-1145 \
  cargo test benchmark::tests::import_harbor_job_from_env_path --lib -- --ignored --nocapture
```

Observed output:

```text
imported run=3bcbc381-e510-4317-8947-fbb5a1e64bcd dataset=terminal-bench/terminal-bench-2-1 agent=codefactory-headless-baseline comparable=true trials=1
trial=terminal-bench/write-compressor reward=0 failure_class=Some("verification")
```

## Boundary

This proves the first CodeFactory-owned Terminal-Bench 2.1 evaluation path: Harbor can import the CodeFactory adapter, run it in a task container, produce verifier reward, and CodeFactory can import and classify the result.

It does not prove product agent capability yet. The next required slice is a model-backed headless CodeFactory runner with `benchmark-sandbox` policy and trajectory/tool-call capture.
