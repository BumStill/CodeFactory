# Terminal-Bench 2.1 Product Iteration Report

- generated_at: `2026-06-29T13-25-36Z`
- evaluation_axis: `codefactory-agent-capability`
- evaluation_subject: `codefactory-headless`
- scope: `canary`
- subset_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-subsets/terminal-bench-21-canary-subset.json`
- endpoint: `deepseek`
- model: `<settings default>`
- override_storage_mb: `<none>`
- official_comparable: `yes`
- hypothesis: `retain MTEB implementation hint through context compaction`
- target_failure_class: `verification`
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

- comparable_delta: `no`
- reason: baseline and head have different trial counts; use this report as targeted canary evidence, not an aggregate score delta.

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

- P0: parse verifier/self-check output into a concrete repair_goal.
- P1: block final answers until the smallest available self-check has run after a candidate fix.

## Command Output Tail

```text

BENCHMARK_RUN_TIMEOUT: exceeded 600 seconds

```
