# Terminal-Bench 2.1 First Smoke Evidence

## Scope

This evidence pack records the first real Terminal-Bench 2.1 Harbor smoke run for the CodeFactory benchmark evaluation slice.

This is an oracle smoke baseline. It verifies Harbor, Docker, dataset resolution, task container execution, verifier output, Harbor artifacts, and CodeFactory result import. It does not evaluate CodeFactory as the agent yet.

## Environment

- Date: 2026-06-27
- Harbor: 0.15.0
- Docker runtime: Colima + Docker Engine 29.5.2
- Docker Compose: 5.2.0
- Dataset: `terminal-bench/terminal-bench-2-1`
- Job path: `.codefactory/benchmark-jobs/cf-tb21-oracle-smoke-20260627-1116`

## Command

```bash
harbor run \
  -d terminal-bench/terminal-bench-2-1 \
  -a oracle \
  -l 1 \
  -n 1 \
  -o /Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs \
  --job-name cf-tb21-oracle-smoke-20260627-1116 \
  -y
```

## Result

- Run id: `1e7185f0-68b1-4c74-b45b-bfbc3373010b`
- Task: `terminal-bench/write-compressor`
- Trial: `write-compressor__bTMbpuD`
- Trials: 1
- Exceptions: 0
- Mean reward: 1.000
- Trial reward: 1.0
- Total runtime: 4m 11s

## CodeFactory Import Evidence

```bash
CODEFACTORY_BENCHMARK_JOB_PATH=/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-oracle-smoke-20260627-1116 \
  cargo test benchmark::tests::import_harbor_job_from_env_path --lib -- --ignored --nocapture
```

Observed output:

```text
imported run=1e7185f0-68b1-4c74-b45b-bfbc3373010b dataset=terminal-bench/terminal-bench-2-1 agent=oracle comparable=true trials=1
trial=terminal-bench/write-compressor reward=1 failure_class=None
```

## Boundary

The first real smoke is complete for the evaluation infrastructure. The next missing step is CodeFactory-as-agent evaluation: implement the Harbor custom agent adapter, headless runner, and `benchmark-sandbox` policy, then rerun a smoke job with the CodeFactory adapter instead of `oracle`.
