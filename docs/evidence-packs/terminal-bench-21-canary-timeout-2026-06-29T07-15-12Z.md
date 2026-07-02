# Terminal-Bench 2.1 Canary Timeout Evidence

- generated_at: `2026-06-29T07-15-12Z`
- evaluation_axis: `codefactory-agent-capability`
- evaluation_subject: `codefactory-headless`
- scope: `canary`
- subset: `terminal-bench-21-canary-subset-v1`
- endpoint: `deepseek`
- model: `deepseek-v4-pro`
- hypothesis: `tool-use P0 hard artifact gate and semantic failure detection with bounded runner timeout`
- runner_exit_code: `124`
- timeout: `360s`
- job_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-070912`
- harbor_run_id: `461a2164-fd0c-4f2b-b6ab-b2626243b6b2`

## Result Boundary

This is not a complete canary score. The iteration runner timed out and returned a reportable `124` instead of hanging indefinitely.

- total_trials: `4`
- completed_trials_at_timeout: `2`
- running_trials_at_timeout: `2`
- completed_pass_count: `0`
- completed_mean_reward: `0.000`
- full_canary_score_available: `no`

## Completed Trials At Timeout

| Task | Reward | Observed behavior |
| --- | ---: | --- |
| `terminal-bench/write-compressor` | `0.0` | Trajectory triggered `implementation-required` and `artifact-required`, but the model did not switch into a valid artifact-producing strategy before timeout. |
| `terminal-bench/filter-js-from-html` | `0.0` | Trajectory had bounded execution and repeated-command suppression, but no final successful implementation. |

## Comparison Point

The immediately previous complete canary run is:

- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T06-58-36Z.md`
- run: `77e98d56-2638-4b0c-a941-a84b542d51ff`
- pass_count: `0 / 4`
- mean_reward: `0.000`
- failure_class: `tool-use` for all four tasks

## Product Conclusion

The first tool-use P0 product iteration improved observability and enforcement, but did not improve the canary score. The hard gate can stop repeated inspection, and semantic failure detection can identify a failed command hidden behind `return_code=0`, but the agent still lacks a reliable policy transition from blocked/probed state into a concrete candidate implementation.

Next product work should focus on a strategy-level loop:

- turn `implementation-required` / `artifact-required` into a concrete forced implementation plan, not only a blocked tool result;
- summarize available files, executables, expected artifacts, and verifier hints before asking for the next tool call;
- add a max-blocks escape hatch that injects a minimal implementation scaffold or asks the model to write one directly;
- keep the bounded runner timeout as a permanent evaluation safety rail.

