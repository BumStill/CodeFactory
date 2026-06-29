# Terminal-Bench 2.1 Forced Transition Canary Evidence

- generated_at: `2026-06-29T08-01-36Z`
- evaluation_axis: `codefactory-agent-capability`
- evaluation_subject: `codefactory-headless`
- scope: `canary`
- subset: `terminal-bench-21-canary-subset-v1`
- endpoint: `deepseek`
- model: `deepseek-v4-pro`
- hypothesis: `tool-use P0 forced implementation transition after blocked inspection`
- runner_exit_code: `124`
- timeout: `360s`
- job_path: `/Users/leo/Projects/CodeFactory-terminal-bench-21-design/.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-075536`
- harbor_run_id: `0a749dd8-c926-4636-bde8-20ad6042b320`

## Result Boundary

This is not a complete canary score. The bounded iteration runner returned `124` after `360s`.

- total_trials: `4`
- completed_trials_at_timeout: `1`
- running_trials_at_timeout: `2`
- pending_trials_at_timeout: `1`
- completed_pass_count: `0`
- completed_mean_reward: `0.000`
- full_canary_score_available: `no`

## Partial Trial State

| Task | Reward/status | Evidence |
| --- | ---: | --- |
| `terminal-bench/write-compressor` | `0.0` | Completed. Trajectory had `2` implementation-required blocks, `3` artifact-required blocks, `3` forced-implementation prompts, and `1` auto-repair-ok. Auto-repair wrote `/app/data.comp` at `2476` bytes. Verifier failed while installing/running dependencies because `/var/cache/apt/archives` had insufficient free space and `curl` / `uvx` were unavailable, so this result should be attributed to environment/resource readiness rather than artifact content alone. |
| `terminal-bench/filter-js-from-html` | running at timeout | Trajectory had repeated nonzero/ok tool calls, `4` suppressed commands, and `4` repair-goal entries. |
| `terminal-bench/mteb-retrieve` | running at timeout | Early trajectory only; no scoring result at timeout. |
| `terminal-bench/count-dataset-tokens` | pending at timeout | No trajectory at timeout. |

## Product Conclusion

The forced implementation prompt is wired correctly and reaches real canary trajectories. It changes behavior observability but still does not reliably convert blocked exploration into a valid implementation.

The next product change should not be another natural-language reminder. It should add a constrained implementation mode after repeated `artifact-required` / `implementation-required` states:

- collect a compact workspace inventory once;
- generate or request a concrete artifact-producing shell command;
- reject further probe-only commands after the transition;
- if the model still probes, run a deterministic fallback scaffold for task families where CodeFactory already has a safe recipe;
- classify verifier dependency/resource failures such as apt cache exhaustion separately from agent tool-use failures.
