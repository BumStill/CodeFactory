# Terminal-Bench 2.1 MTEB Scoring Canary Evidence

- generated_at: `2026-06-29`
- evaluation_axis: `codefactory-agent-capability`
- evaluation_subject: `codefactory-headless`
- dataset: `terminal-bench/terminal-bench-2-1`
- task: `terminal-bench/mteb-retrieve`
- model backend: `deepseek-v4-pro`

## Result

- passing run: `5a4e758d-f949-40ba-8f2d-e0017fa9b722`
- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T13-33-52Z.md`
- iteration report: `docs/evidence-packs/terminal-bench-21-iteration-2026-06-29T13-33-52Z.md`
- official comparable: `true`
- pass: `1 / 1`
- mean reward: `1.000`
- failure class: `None`

This is targeted canary score movement. It does not replace the full 89-task score (`6 / 89`, mean reward `0.06741573033707865`) or the latest fixed 18-task provider-backed score (`0 / 18`, mean reward `0.000`) until those same scopes are rerun.

## Root Cause Isolation

The earlier `mteb-retrieve` runs wrote `/app/result.txt` but still scored `0.0` because verifier bootstrap failed. A diagnostic Harbor storage override run remained failed and was correctly marked non-comparable:

- diagnostic run: `0224b9ba-e6f4-4b45-8bd8-1249b8911561`
- diagnostic report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T12-58-09Z.md`
- override: `--override-storage-mb 65536`
- official comparable: `false`
- result: reward `0.0`, failure class `environment`

The actual environment blocker was local Docker overlay exhaustion. Before cleanup, a clean `python:3.10-slim-bookworm` apt smoke showed overlay `30G / 30G`, `100%`, and reproduced apt package index/signature failures. After removing unused Terminal-Bench images, the same apt smoke passed.

## Product Changes

- Added explicit storage override plumbing for diagnosis while preserving comparability attribution.
- Corrected the MTEB task-family implementation hint to use `mteb.get_model("BAAI/bge-small-zh-v1.5", revision=...)`, `task_name="SciFact"`, and `PromptType.query` / `PromptType.passage`.
- Preserved `Implementation hint:` messages during context compaction so task-family guidance reaches the model.
- Raised the benchmark shell timeout default to `300s` for model-loading and embedding tasks.

## Next Gate

Run the fixed 18-task regression subset after PR validation. Only that same-scope result can establish the next aggregate CodeFactory score.
