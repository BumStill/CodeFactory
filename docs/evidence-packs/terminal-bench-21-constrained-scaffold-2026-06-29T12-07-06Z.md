# Terminal-Bench 2.1 Constrained Scaffold Evidence

- generated_at: `2026-06-29T12-07-06Z`
- evaluation_axis: `codefactory-agent-capability`
- evaluation_subject: `codefactory-headless`
- scope: `single-task canary`
- task: `terminal-bench/write-compressor`
- endpoint: `deepseek`
- model: `deepseek-v4-pro`
- changed_variable: `constrained implementation mode also runs after no-action recovery when decompressor context is already known`

## Before

- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T12-03-31Z.md`
- run: `234859fc-085f-4492-9083-c883a4a39d13`
- job_path: `.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-115942`
- task_count: `1`
- pass_count: `0 / 1`
- mean_reward: `0.000`
- failure_class: `environment`
- runtime: `228.60s`

## After

- report: `docs/evidence-packs/terminal-bench-21-regression-subset-2026-06-29T12-07-06Z.md`
- run: `5b1c540d-56ab-4be2-afcb-ee3521b013d6`
- job_path: `.codefactory/benchmark-jobs/cf-tb21-codefactory-provider-deepseek-20260629-120513`
- task_count: `1`
- pass_count: `0 / 1`
- mean_reward: `0.000`
- failure_class: `environment`
- runtime: `112.72s`

## Delta

- runtime: `228.60s` -> `112.72s` (`-115.88s`, about `50.7%` faster)
- score: unchanged at `0.000`
- failure attribution: stable `environment`, not `tool-use`
- behavior: latest trajectory contains `constrained-implementation-ok` and writes `/app/data.comp` through the CodeFactory scaffold instead of relying on further model probing.

## Interpretation

This is a real loop improvement, not a score improvement. The agent now reaches artifact-producing constrained implementation materially faster on `write-compressor`. The remaining zero reward is blocked by verifier environment/resource readiness, already classified as `environment/verifier-dependency-resource` by the importer path.

Next score-facing work should target the verifier resource preflight and/or run a canary task whose verifier does not fail on apt/cache dependency bootstrap, then continue constrained mode generalization beyond `write-compressor`.

