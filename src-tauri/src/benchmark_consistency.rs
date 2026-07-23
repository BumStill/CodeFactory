// SPDX-License-Identifier: Apache-2.0
//! P5-lite — cross-model consistency & failure-distribution report.
//!
//! A read-only aggregation over rows that already exist (`benchmark_runs` +
//! `benchmark_trials`): the first objective, model-independent picture of where
//! the agent fails and how consistently it behaves across models. No new run
//! path, no Harbor, no headless runner — just query + aggregate.
//!
//! The pure core here takes plain input structs (no sqlx) so it can be
//! exhaustively unit-tested; the tauri command loads the rows and calls in.
//!
//! Comparability (R29): only runs with `comparable = true` sharing the SAME
//! `dataset` + `dataset_version` are compared. Everything else is excluded and
//! explained in `comparability_note`.

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// A benchmark run with its trials, as read from the DB (sqlx-free).
#[derive(Debug, Clone)]
pub struct RunWithTrials {
    pub run_id: String,
    pub model: String,
    pub dataset: String,
    pub dataset_version: String,
    pub comparable: bool,
    pub trials: Vec<TrialRow>,
}

#[derive(Debug, Clone)]
pub struct TrialRow {
    pub task_name: String,
    pub reward: f64,
    pub failure_class: Option<String>,
}

/// A trial passes when its reward clears the threshold. Terminal-Bench rewards
/// are 1.0 pass / 0.0 fail, so 1.0 is the default.
pub const DEFAULT_PASS_THRESHOLD: f64 = 1.0;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ModelSummary {
    pub model: String,
    pub run_id: String,
    pub total: usize,
    pub passed: usize,
    pub pass_rate: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PairwiseConsistency {
    pub model_a: String,
    pub model_b: String,
    /// |passed_A ∩ passed_B| / |passed_A ∪ passed_B|. 1.0 when both pass-sets
    /// are empty (vacuously identical).
    pub jaccard: f64,
    pub both_passed: usize,
    pub a_only: usize,
    pub b_only: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DivergentTask {
    pub task_name: String,
    /// model -> reward, for every model that ran this task.
    pub per_model: BTreeMap<String, f64>,
    /// max(reward) - min(reward) across models.
    pub spread: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FailureBucket {
    pub model: String,
    pub failure_class: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConsistencyReport {
    pub dataset: String,
    pub dataset_version: String,
    pub models: Vec<ModelSummary>,
    pub pairwise: Vec<PairwiseConsistency>,
    pub divergent_tasks: Vec<DivergentTask>,
    pub failure_distribution: Vec<FailureBucket>,
    /// Human note on what was excluded and why (non-comparable / mismatched
    /// dataset+version). Empty when nothing was excluded.
    pub comparability_note: String,
}

fn passed_set(run: &RunWithTrials, threshold: f64) -> BTreeSet<String> {
    run.trials
        .iter()
        .filter(|t| t.reward >= threshold)
        .map(|t| t.task_name.clone())
        .collect()
}

/// Jaccard of two sets; two empty sets are vacuously identical (1.0).
pub fn jaccard(a: &BTreeSet<String>, b: &BTreeSet<String>) -> f64 {
    let inter = a.intersection(b).count();
    let union = a.union(b).count();
    if union == 0 {
        1.0
    } else {
        inter as f64 / union as f64
    }
}

/// Build the report for one (dataset, dataset_version). Non-comparable runs and
/// runs from other datasets/versions are excluded; the reason is recorded.
/// When two runs share a model on the same slice, the most-trials run wins
/// (a re-run supersedes a partial one) — deterministic, no silent double-count.
pub fn build_report(
    dataset: &str,
    dataset_version: &str,
    runs: &[RunWithTrials],
    pass_threshold: f64,
) -> ConsistencyReport {
    let mut excluded_non_comparable = 0usize;
    let mut excluded_other_slice = 0usize;

    // Pick one run per model on this comparable slice (most trials wins).
    let mut by_model: BTreeMap<String, &RunWithTrials> = BTreeMap::new();
    for run in runs {
        if run.dataset != dataset || run.dataset_version != dataset_version {
            excluded_other_slice += 1;
            continue;
        }
        if !run.comparable {
            excluded_non_comparable += 1;
            continue;
        }
        by_model
            .entry(run.model.clone())
            .and_modify(|existing| {
                if run.trials.len() > existing.trials.len() {
                    *existing = run;
                }
            })
            .or_insert(run);
    }

    // Per-model summaries.
    let mut models: Vec<ModelSummary> = by_model
        .values()
        .map(|run| {
            let passed = run.trials.iter().filter(|t| t.reward >= pass_threshold).count();
            let total = run.trials.len();
            ModelSummary {
                model: run.model.clone(),
                run_id: run.run_id.clone(),
                total,
                passed,
                pass_rate: if total == 0 { 0.0 } else { passed as f64 / total as f64 },
            }
        })
        .collect();
    models.sort_by(|a, b| a.model.cmp(&b.model));

    // Pairwise Jaccard over pass-sets.
    let model_names: Vec<String> = models.iter().map(|m| m.model.clone()).collect();
    let pass_sets: BTreeMap<String, BTreeSet<String>> = by_model
        .iter()
        .map(|(m, run)| (m.clone(), passed_set(run, pass_threshold)))
        .collect();
    let mut pairwise = Vec::new();
    for i in 0..model_names.len() {
        for j in (i + 1)..model_names.len() {
            let a = &pass_sets[&model_names[i]];
            let b = &pass_sets[&model_names[j]];
            pairwise.push(PairwiseConsistency {
                model_a: model_names[i].clone(),
                model_b: model_names[j].clone(),
                jaccard: jaccard(a, b),
                both_passed: a.intersection(b).count(),
                a_only: a.difference(b).count(),
                b_only: b.difference(a).count(),
            });
        }
    }

    // Divergent tasks: a task where some model passed and another failed.
    // reward-per-model, spread, sorted by spread desc then name.
    let mut task_rewards: BTreeMap<String, BTreeMap<String, f64>> = BTreeMap::new();
    for (model, run) in &by_model {
        for t in &run.trials {
            task_rewards
                .entry(t.task_name.clone())
                .or_default()
                .insert(model.clone(), t.reward);
        }
    }
    let mut divergent_tasks: Vec<DivergentTask> = task_rewards
        .into_iter()
        .filter_map(|(task_name, per_model)| {
            let any_pass = per_model.values().any(|r| *r >= pass_threshold);
            let any_fail = per_model.values().any(|r| *r < pass_threshold);
            if per_model.len() < 2 || !(any_pass && any_fail) {
                return None;
            }
            let max = per_model.values().cloned().fold(f64::NEG_INFINITY, f64::max);
            let min = per_model.values().cloned().fold(f64::INFINITY, f64::min);
            Some(DivergentTask {
                task_name,
                spread: max - min,
                per_model,
            })
        })
        .collect();
    divergent_tasks.sort_by(|a, b| {
        b.spread
            .partial_cmp(&a.spread)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.task_name.cmp(&b.task_name))
    });

    // Failure distribution per model (failure_class; None → "unclassified").
    let mut failure_counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (model, run) in &by_model {
        for t in &run.trials {
            if t.reward >= pass_threshold {
                continue;
            }
            let class = t
                .failure_class
                .clone()
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| "unclassified".to_string());
            *failure_counts.entry((model.clone(), class)).or_insert(0) += 1;
        }
    }
    let failure_distribution: Vec<FailureBucket> = failure_counts
        .into_iter()
        .map(|((model, failure_class), count)| FailureBucket {
            model,
            failure_class,
            count,
        })
        .collect();

    let mut notes = Vec::new();
    if excluded_non_comparable > 0 {
        notes.push(format!(
            "{excluded_non_comparable} 个 run 因未标记 comparable(R29)被排除"
        ));
    }
    if excluded_other_slice > 0 {
        notes.push(format!(
            "{excluded_other_slice} 个 run 属于其它 dataset/version 被排除"
        ));
    }

    ConsistencyReport {
        dataset: dataset.to_string(),
        dataset_version: dataset_version.to_string(),
        models,
        pairwise,
        divergent_tasks,
        failure_distribution,
        comparability_note: notes.join(";"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(id: &str, model: &str, comparable: bool, trials: &[(&str, f64, Option<&str>)]) -> RunWithTrials {
        RunWithTrials {
            run_id: id.into(),
            model: model.into(),
            dataset: "terminal-bench".into(),
            dataset_version: "2.1".into(),
            comparable,
            trials: trials
                .iter()
                .map(|(t, r, f)| TrialRow {
                    task_name: (*t).into(),
                    reward: *r,
                    failure_class: f.map(|s| s.to_string()),
                })
                .collect(),
        }
    }

    #[test]
    fn jaccard_handles_overlap_disjoint_and_empty() {
        let a: BTreeSet<String> = ["x", "y", "z"].iter().map(|s| s.to_string()).collect();
        let b: BTreeSet<String> = ["y", "z", "w"].iter().map(|s| s.to_string()).collect();
        assert!((jaccard(&a, &b) - 0.5).abs() < 1e-9); // {y,z} / {x,y,z,w}
        let empty: BTreeSet<String> = BTreeSet::new();
        assert_eq!(jaccard(&empty, &empty), 1.0); // vacuously identical
        assert_eq!(jaccard(&a, &empty), 0.0);
    }

    #[test]
    fn per_model_pass_rate_and_pairwise_jaccard() {
        let runs = vec![
            run("r1", "gpt-5.6", true, &[("t1", 1.0, None), ("t2", 1.0, None), ("t3", 0.0, Some("timeout"))]),
            run("r2", "deepseek-v4", true, &[("t1", 1.0, None), ("t2", 0.0, Some("wrong")), ("t3", 0.0, Some("timeout"))]),
        ];
        let rep = build_report("terminal-bench", "2.1", &runs, DEFAULT_PASS_THRESHOLD);
        assert_eq!(rep.models.len(), 2);
        let gpt = rep.models.iter().find(|m| m.model == "gpt-5.6").unwrap();
        assert_eq!((gpt.passed, gpt.total), (2, 3));
        assert!((gpt.pass_rate - 2.0 / 3.0).abs() < 1e-9);
        // pass-sets: gpt {t1,t2}, deepseek {t1} → jaccard 1/2.
        // Pairs use sorted model order → model_a = deepseek-v4, model_b = gpt-5.6.
        assert_eq!(rep.pairwise.len(), 1);
        let p = &rep.pairwise[0];
        assert_eq!((p.model_a.as_str(), p.model_b.as_str()), ("deepseek-v4", "gpt-5.6"));
        assert!((p.jaccard - 0.5).abs() < 1e-9);
        assert_eq!((p.both_passed, p.a_only, p.b_only), (1, 0, 1));
    }

    #[test]
    fn divergent_tasks_only_lists_pass_fail_splits_sorted_by_spread() {
        let runs = vec![
            run("r1", "a", true, &[("t1", 1.0, None), ("t2", 1.0, None), ("t3", 0.4, None)]),
            run("r2", "b", true, &[("t1", 1.0, None), ("t2", 0.0, Some("x")), ("t3", 1.0, None)]),
        ];
        let rep = build_report("terminal-bench", "2.1", &runs, DEFAULT_PASS_THRESHOLD);
        // t1 both pass → not divergent. t2 a-pass/b-fail (spread 1.0). t3
        // a-fail(0.4)/b-pass(1.0) (spread 0.6). Sorted by spread desc.
        let names: Vec<&str> = rep.divergent_tasks.iter().map(|d| d.task_name.as_str()).collect();
        assert_eq!(names, vec!["t2", "t3"]);
        assert!((rep.divergent_tasks[0].spread - 1.0).abs() < 1e-9);
        assert_eq!(rep.divergent_tasks[0].per_model["a"], 1.0);
        assert_eq!(rep.divergent_tasks[0].per_model["b"], 0.0);
    }

    #[test]
    fn non_comparable_and_other_slice_runs_are_excluded_with_a_note() {
        let mut other = run("r3", "c", true, &[("t1", 1.0, None)]);
        other.dataset_version = "2.0".into();
        let runs = vec![
            run("r1", "a", true, &[("t1", 1.0, None)]),
            run("r2", "b", false, &[("t1", 0.0, Some("x"))]), // not comparable
            other,                                             // other version
        ];
        let rep = build_report("terminal-bench", "2.1", &runs, DEFAULT_PASS_THRESHOLD);
        assert_eq!(rep.models.len(), 1);
        assert_eq!(rep.models[0].model, "a");
        assert!(rep.comparability_note.contains("comparable"));
        assert!(rep.comparability_note.contains("dataset/version"));
    }

    #[test]
    fn failure_distribution_buckets_by_class_with_unclassified_fallback() {
        let runs = vec![run(
            "r1",
            "a",
            true,
            &[("t1", 0.0, Some("timeout")), ("t2", 0.0, Some("timeout")), ("t3", 0.0, None), ("t4", 1.0, None)],
        )];
        let rep = build_report("terminal-bench", "2.1", &runs, DEFAULT_PASS_THRESHOLD);
        let timeout = rep.failure_distribution.iter().find(|b| b.failure_class == "timeout").unwrap();
        assert_eq!(timeout.count, 2);
        let unclassified = rep.failure_distribution.iter().find(|b| b.failure_class == "unclassified").unwrap();
        assert_eq!(unclassified.count, 1); // t3 failed with no class; t4 passed, excluded
    }

    #[test]
    fn duplicate_model_runs_pick_the_one_with_more_trials() {
        let runs = vec![
            run("partial", "a", true, &[("t1", 1.0, None)]),
            run("full", "a", true, &[("t1", 1.0, None), ("t2", 1.0, None)]),
        ];
        let rep = build_report("terminal-bench", "2.1", &runs, DEFAULT_PASS_THRESHOLD);
        assert_eq!(rep.models.len(), 1);
        assert_eq!(rep.models[0].run_id, "full");
        assert_eq!(rep.models[0].total, 2);
    }

    #[test]
    fn empty_input_yields_an_empty_report_not_a_panic() {
        let rep = build_report("terminal-bench", "2.1", &[], DEFAULT_PASS_THRESHOLD);
        assert!(rep.models.is_empty());
        assert!(rep.pairwise.is_empty());
        assert!(rep.divergent_tasks.is_empty());
    }
}
