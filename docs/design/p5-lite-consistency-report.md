# P5-lite — Cross-Model Consistency & Failure-Distribution Report

The recommended first move from the adversarial review (see
`product-capability-adversarial-review-2026-07.md`). A **read-only** aggregation
over data that already exists (`benchmark_runs` + `benchmark_trials`), producing
the first objective, model-independent picture of *where the agent actually
fails and how consistently it behaves across models* — the ruler that turns the
founder's five hypotheses into evidence.

## Scope (v1)

- No new run path, no Harbor invocation, no headless runner. Pure query +
  aggregation over rows already imported by the Terminal-Bench harness.
- Honors the **R29 comparability gate**: only runs with `comparable = 1` and the
  same `dataset` + `dataset_version` are compared to each other.

## Inputs (existing schema)

- `benchmark_runs`: `id, dataset, dataset_version, model, comparable, status, …`
- `benchmark_trials`: `run_id, task_name, reward, failure_class, error_kind, …`

## Aggregation core (pure, exhaustively unit-tested)

1. **pass predicate** — a trial passes when `reward >= pass_threshold`
   (default 1.0; Terminal-Bench rewards are 1.0 pass / 0.0 fail).
2. **pass-set Jaccard** — for a pair of models over the same comparable
   dataset+version, `|passed_A ∩ passed_B| / |passed_A ∪ passed_B|`.
3. **reward spread** — per task, `max(reward) - min(reward)` across models
   (magnitude of cross-model disagreement).
4. **divergent tasks** — tasks a model passed AND another failed, with each
   model's reward. This is the actionable inconsistency list.
5. **failure distribution** — counts by `failure_class` / `error_kind`, overall
   and per-model.
6. **per-model pass rate** — passed / total on the comparable slice.

## Output (`ConsistencyReport`)

```
{
  dataset, dataset_version,
  models: [ { model, run_id, total, passed, pass_rate } ],
  pairwise: [ { model_a, model_b, jaccard, both_passed, a_only, b_only } ],
  divergent_tasks: [ { task_name, per_model: { model: reward } } ],   // capped, sorted by spread
  failure_distribution: [ { model, failure_class, count } ],
  comparability_note,   // why any runs were excluded
}
```

## Surface

- Tauri command `benchmark_consistency_report(dataset, dataset_version?)`.
- A compact read-only panel on the Benchmarks page (no new page).

## Explicitly out of scope (deferred per the review)

- Any live run / A/B / auto-activation.
- Task-type coverage as a scored suite (use the static capability matrix instead).
- The headless product-agent runner (the keystone — separate charter).
