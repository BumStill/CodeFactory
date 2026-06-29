# Terminal-Bench 2.1 Product Iteration Report

- generated_at: `2026-06-29T07-15-12Z`
- evaluation_axis: `codefactory-agent-capability`
- evaluation_subject: `codefactory-headless`
- scope: `canary`
- subset_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/docs/benchmark-subsets/terminal-bench-21-canary-subset-v1.json`
- endpoint: `deepseek`
- model: `<settings default>`
- hypothesis: `tool-use P0 hard artifact gate and semantic failure detection with bounded runner timeout`
- target_failure_class: `tool-use`
- ran_command: `yes`
- exit_code: `124`

## Baseline

- path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/docs/evidence-packs/terminal-bench-21-regression-subset-baseline-2026-06-28T15-41-50Z.md`
- run: `not available`
- pass_count: `4`
- trials: `18`
- mean_reward: `0.222222`

## Head

- path: `not available`
- run: `not available`
- pass_count: `unknown`
- trials: `unknown`
- mean_reward: `unknown`

## Delta

- pass_count: `unknown`
- mean_reward: `unknown`

## Failure Class Counts

Baseline:
- `environment`: `2`
- `long-horizon`: `4`
- `pass`: `4`
- `tool-use`: `3`
- `verification`: `5`

Head:
- no trial failure table available

## Next Improvement Queue

- P0: reduce repeated inspection by escalating to artifact implementation earlier.
- P0: add command preflight for missing files, wrong cwd, command-not-found, and obvious non-productive reads.
- P1: feed compact workspace inventory into the model before broad exploration.

## Command Output Tail

```text

BENCHMARK_RUN_TIMEOUT: exceeded 360 seconds

```
