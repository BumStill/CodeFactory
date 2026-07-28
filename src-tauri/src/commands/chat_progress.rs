// SPDX-License-Identifier: Apache-2.0

use std::collections::HashMap;

use chrono::DateTime;
use serde::Serialize;
use tauri::State;

use crate::{errors::AppError, AppState};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DurationSampleSummary {
    pub sample_count: usize,
    pub p25_ms: i64,
    pub p75_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnTimingProfile {
    pub phases: HashMap<String, DurationSampleSummary>,
    pub build: Option<DurationSampleSummary>,
    pub external_job: Option<DurationSampleSummary>,
}

#[derive(sqlx::FromRow)]
struct PlanObservation {
    root_turn_id: String,
    plan_json: String,
    created_at: i64,
}

#[derive(sqlx::FromRow)]
struct ExternalJobObservation {
    started_at: String,
    completed_at: String,
}

#[derive(sqlx::FromRow)]
struct BashObservation {
    arguments: String,
    duration_ms: i64,
}

fn summarize(mut samples: Vec<i64>) -> Option<DurationSampleSummary> {
    samples.retain(|sample| *sample > 0);
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable();
    let last = samples.len() - 1;
    let p25_index = last / 4;
    let p75_index = (last * 3).div_ceil(4);
    Some(DurationSampleSummary {
        sample_count: samples.len(),
        p25_ms: samples[p25_index],
        p75_ms: samples[p75_index],
    })
}

fn phase_samples(mut rows: Vec<PlanObservation>) -> HashMap<String, Vec<i64>> {
    rows.sort_by_key(|row| row.created_at);
    let mut active: HashMap<(String, String), (String, i64)> = HashMap::new();
    let mut completed: HashMap<String, Vec<i64>> = HashMap::new();
    for row in rows {
        let Ok(steps) = serde_json::from_str::<Vec<codefactory_agent_loop::types::PlanStepEvent>>(
            &row.plan_json,
        ) else {
            continue;
        };
        let present_step_ids = steps
            .iter()
            .map(|step| step.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        active.retain(|(root_turn_id, step_id), _| {
            root_turn_id != &row.root_turn_id || present_step_ids.contains(step_id.as_str())
        });
        for step in steps {
            let key = (row.root_turn_id.clone(), step.id);
            match step.status.as_str() {
                "in_progress" => {
                    active.entry(key).or_insert((step.kind, row.created_at));
                }
                "completed" => {
                    if let Some((kind, started_at)) = active.remove(&key) {
                        let elapsed = row.created_at.saturating_sub(started_at);
                        if elapsed > 0 {
                            completed.entry(kind).or_default().push(elapsed);
                        }
                    }
                }
                _ => {
                    active.remove(&key);
                }
            }
        }
    }
    completed
}

fn external_job_duration(row: ExternalJobObservation) -> Option<i64> {
    let started = DateTime::parse_from_rfc3339(&row.started_at).ok()?;
    let completed = DateTime::parse_from_rfc3339(&row.completed_at).ok()?;
    let duration = completed.timestamp_millis() - started.timestamp_millis();
    (duration > 0).then_some(duration)
}

fn is_build_or_test_command(arguments: &str) -> bool {
    let command = serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|value| value.get("command")?.as_str().map(ToOwned::to_owned))
        .unwrap_or_default();
    command
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .any(|token| {
            matches!(
                token.to_ascii_lowercase().as_str(),
                "build" | "test" | "vitest" | "pytest" | "jest"
            )
        })
}

async fn load_build_test_samples(
    pool: &sqlx::SqlitePool,
    cwd: &str,
) -> Result<Vec<i64>, sqlx::Error> {
    let rows = sqlx::query_as::<_, BashObservation>(
        "SELECT tc.arguments, tc.duration_ms
         FROM tool_calls tc
         JOIN messages m ON m.id = tc.message_id
         JOIN sessions s ON s.id = m.session_id
         WHERE s.cwd = ?
           AND tc.tool_name = 'bash'
           AND tc.status = 'done'
           AND tc.duration_ms > 0
         ORDER BY tc.created_at DESC
         LIMIT 1000",
    )
    .bind(cwd)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter(|row| is_build_or_test_command(&row.arguments))
        .take(200)
        .map(|row| row.duration_ms)
        .collect())
}

#[tauri::command]
pub async fn get_turn_timing_profile(
    cwd: String,
    state: State<'_, AppState>,
) -> Result<TurnTimingProfile, AppError> {
    let pool = state.db.read().await;
    let plan_rows = sqlx::query_as::<_, PlanObservation>(
        "SELECT e.root_turn_id, e.plan_json, e.created_at
         FROM chat_plan_events e
         JOIN sessions s ON s.id = e.session_id
         WHERE s.cwd = ?
         ORDER BY e.created_at DESC
         LIMIT 4000",
    )
    .bind(&cwd)
    .fetch_all(&*pool)
    .await?;
    let phases = phase_samples(plan_rows)
        .into_iter()
        .filter_map(|(kind, samples)| summarize(samples).map(|summary| (kind, summary)))
        .collect();

    let build_samples = load_build_test_samples(&pool, &cwd).await?;

    let external_rows = sqlx::query_as::<_, ExternalJobObservation>(
        "SELECT started_at, completed_at
         FROM task_runs
         WHERE cwd = ?
           AND status = 'completed'
           AND started_at IS NOT NULL
           AND completed_at IS NOT NULL
         ORDER BY completed_at DESC
         LIMIT 200",
    )
    .bind(&cwd)
    .fetch_all(&*pool)
    .await?;
    let external_samples = external_rows
        .into_iter()
        .filter_map(external_job_duration)
        .collect();

    Ok(TurnTimingProfile {
        phases,
        build: summarize(build_samples),
        external_job: summarize(external_samples),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_summary_is_deterministic_and_keeps_source_count() {
        assert_eq!(
            summarize(vec![9_000, 1_000, 5_000, 3_000]),
            Some(DurationSampleSummary {
                sample_count: 4,
                p25_ms: 1_000,
                p75_ms: 9_000,
            }),
        );
        assert_eq!(summarize(Vec::new()), None);
    }

    #[test]
    fn phase_duration_requires_observed_start_and_completion() {
        let steps = |status: &str| {
            serde_json::json!([
                {
                    "id": "verify",
                    "title": "验证",
                    "kind": "verification",
                    "status": status,
                    "external_job_id": null
                }
            ])
            .to_string()
        };
        let samples = phase_samples(vec![
            PlanObservation {
                root_turn_id: "root".into(),
                plan_json: steps("pending"),
                created_at: 100,
            },
            PlanObservation {
                root_turn_id: "root".into(),
                plan_json: steps("in_progress"),
                created_at: 200,
            },
            PlanObservation {
                root_turn_id: "root".into(),
                plan_json: steps("completed"),
                created_at: 1_200,
            },
        ]);
        assert_eq!(samples.get("verification"), Some(&vec![1_000]));
    }

    #[test]
    fn phase_duration_is_stable_when_recent_rows_arrive_newest_first() {
        let steps = |status: &str| {
            serde_json::json!([
                {
                    "id": "verify",
                    "title": "验证",
                    "kind": "verification",
                    "status": status,
                    "external_job_id": null
                }
            ])
            .to_string()
        };
        let samples = phase_samples(vec![
            PlanObservation {
                root_turn_id: "root".into(),
                plan_json: steps("completed"),
                created_at: 1_200,
            },
            PlanObservation {
                root_turn_id: "root".into(),
                plan_json: steps("in_progress"),
                created_at: 200,
            },
        ]);
        assert_eq!(samples.get("verification"), Some(&vec![1_000]));
    }

    #[test]
    fn phase_duration_discards_a_reverted_active_interval() {
        let steps = |status: &str| {
            serde_json::json!([
                {
                    "id": "verify",
                    "title": "验证",
                    "kind": "verification",
                    "status": status,
                    "external_job_id": null
                }
            ])
            .to_string()
        };
        let samples = phase_samples(vec![
            PlanObservation {
                root_turn_id: "root".into(),
                plan_json: steps("in_progress"),
                created_at: 100,
            },
            PlanObservation {
                root_turn_id: "root".into(),
                plan_json: steps("pending"),
                created_at: 200,
            },
            PlanObservation {
                root_turn_id: "root".into(),
                plan_json: steps("in_progress"),
                created_at: 1_000,
            },
            PlanObservation {
                root_turn_id: "root".into(),
                plan_json: steps("completed"),
                created_at: 1_500,
            },
        ]);
        assert_eq!(samples.get("verification"), Some(&vec![500]));
    }

    #[tokio::test]
    async fn build_history_includes_successful_build_and_test_commands_only() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query("CREATE TABLE sessions (id TEXT PRIMARY KEY, cwd TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE messages (id TEXT PRIMARY KEY, session_id TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE tool_calls (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                tool_name TEXT NOT NULL,
                arguments TEXT NOT NULL,
                status TEXT NOT NULL,
                duration_ms INTEGER,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO sessions VALUES ('s', '/project')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO messages VALUES ('m', 's')")
            .execute(&pool)
            .await
            .unwrap();
        for (id, command, status, duration) in [
            ("build", "pnpm build", "done", 100_i64),
            ("test", "pnpm test", "done", 200_i64),
            ("lint", "pnpm lint", "done", 300_i64),
            ("failed", "cargo test", "error", 400_i64),
            ("false-positive", "echo latest", "done", 500_i64),
        ] {
            sqlx::query(
                "INSERT INTO tool_calls
                 (id, message_id, tool_name, arguments, status, duration_ms, created_at)
                 VALUES (?, 'm', 'bash', ?, ?, ?, 1)",
            )
            .bind(id)
            .bind(serde_json::json!({ "command": command }).to_string())
            .bind(status)
            .bind(duration)
            .execute(&pool)
            .await
            .unwrap();
        }

        let mut samples = load_build_test_samples(&pool, "/project").await.unwrap();
        samples.sort_unstable();
        assert_eq!(samples, vec![100, 200]);
    }

    #[test]
    fn invalid_external_job_timestamps_are_not_estimated() {
        assert_eq!(
            external_job_duration(ExternalJobObservation {
                started_at: "invalid".into(),
                completed_at: chrono::Utc::now().to_rfc3339(),
            }),
            None,
        );
    }
}
