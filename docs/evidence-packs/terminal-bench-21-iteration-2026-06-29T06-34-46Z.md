# Terminal-Bench 2.1 Product Iteration Report

- generated_at: `2026-06-29T06-34-46Z`
- evaluation_axis: `codefactory-agent-capability`
- evaluation_subject: `codefactory-headless`
- scope: `canary`
- subset_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/docs/benchmark-subsets/terminal-bench-21-canary-subset-v1.json`
- endpoint: `deepseek`
- model: `<settings default>`
- hypothesis: `reduce repeated inspection and force earlier artifact implementation`
- target_failure_class: `tool-use`
- ran_command: `no`

## Baseline

- path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/docs/evidence-packs/terminal-bench-21-regression-subset-baseline-2026-06-28T15-41-50Z.md`
- run: `not available`
- pass_count: `4`
- trials: `18`
- mean_reward: `0.222222`

## Head

- path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T03-36-45Z.md`
- run: `e7d97f76-b1d1-4b08-beb7-08181a1f5a1e`
- pass_count: `0`
- trials: `18`
- mean_reward: `0.0`

## Delta

- pass_count: `4` -> `0` (`-4`)
- mean_reward: `0.222222` -> `0.000000` (`-0.222222`)

## Failure Class Counts

Baseline:
- `environment`: `2`
- `long-horizon`: `4`
- `pass`: `4`
- `tool-use`: `3`
- `verification`: `5`

Head:
- `environment`: `1`
- `policy`: `3`
- `tool-use`: `13`
- `verification`: `1`

## Next Improvement Queue

- P0: reduce repeated inspection by escalating to artifact implementation earlier.
- P0: add command preflight for missing files, wrong cwd, command-not-found, and obvious non-productive reads.
- P1: feed compact workspace inventory into the model before broad exploration.

## Command Output Tail

```text
not executed
```
