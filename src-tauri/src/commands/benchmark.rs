// SPDX-License-Identifier: Apache-2.0
use serde::Deserialize;
use tauri::State;

use crate::benchmark::{
    BenchmarkEnvironmentProbe, BenchmarkProfile, BenchmarkProviderBridgePreview,
    BenchmarkProviderBridgeRequest, BenchmarkProviderRunResult, ImportedBenchmarkRun,
    StartBenchmarkProviderRunRequest,
};
use crate::errors::AppError;
use crate::AppState;

#[derive(Debug, Clone, Deserialize)]
pub struct ImportBenchmarkResultsRequest {
    pub job_path: String,
}

#[tauri::command]
pub async fn list_benchmark_profiles() -> Result<Vec<BenchmarkProfile>, AppError> {
    Ok(crate::benchmark::list_profiles())
}

#[tauri::command]
pub async fn probe_benchmark_environment(
    profile_id: String,
) -> Result<BenchmarkEnvironmentProbe, AppError> {
    crate::benchmark::probe_environment(&profile_id)
}

#[tauri::command]
pub async fn preview_benchmark_provider_bridge(
    request: BenchmarkProviderBridgeRequest,
    state: State<'_, AppState>,
) -> Result<BenchmarkProviderBridgePreview, AppError> {
    let settings = state.settings.read().await;
    crate::benchmark::preview_provider_bridge(&settings, &request)
}

#[tauri::command]
pub async fn start_benchmark_provider_run(
    request: StartBenchmarkProviderRunRequest,
    state: State<'_, AppState>,
) -> Result<BenchmarkProviderRunResult, AppError> {
    let settings = state.settings.read().await.clone();
    let pool = state.db.read().await.clone();
    crate::benchmark::start_provider_benchmark_run(&pool, &settings, request).await
}

#[tauri::command]
pub async fn import_benchmark_results(
    request: ImportBenchmarkResultsRequest,
    state: State<'_, AppState>,
) -> Result<ImportedBenchmarkRun, AppError> {
    let pool = state.db.read().await;
    crate::benchmark::import_harbor_job(&pool, std::path::Path::new(&request.job_path)).await
}

#[derive(Debug, Deserialize)]
pub struct ConsistencyReportRequest {
    pub dataset: String,
    /// Optional; when omitted, the newest dataset_version present for `dataset`
    /// among comparable runs is used.
    pub dataset_version: Option<String>,
}

/// P5-lite: read-only cross-model consistency & failure-distribution report
/// over existing benchmark rows. No new run, no Harbor — pure aggregation.
#[tauri::command]
pub async fn benchmark_consistency_report(
    request: ConsistencyReportRequest,
    state: State<'_, AppState>,
) -> Result<crate::benchmark_consistency::ConsistencyReport, AppError> {
    use crate::benchmark_consistency::{build_report, RunWithTrials, TrialRow, DEFAULT_PASS_THRESHOLD};
    let pool = state.db.read().await.clone();

    // Resolve dataset_version: explicit, else the newest present for a
    // comparable run of this dataset.
    let version = match request.dataset_version {
        Some(v) => v,
        None => sqlx::query_scalar::<_, Option<String>>(
            "SELECT dataset_version FROM benchmark_runs \
             WHERE dataset = ? AND comparable = 1 AND dataset_version IS NOT NULL \
             ORDER BY started_at DESC LIMIT 1",
        )
        .bind(&request.dataset)
        .fetch_optional(&pool)
        .await
        .map_err(|e| AppError::Other(e.to_string()))?
        .flatten()
        .unwrap_or_default(),
    };

    let run_rows: Vec<(String, Option<String>, String, Option<String>, i64)> = sqlx::query_as(
        "SELECT id, model, dataset, dataset_version, comparable \
         FROM benchmark_runs WHERE dataset = ?",
    )
    .bind(&request.dataset)
    .fetch_all(&pool)
    .await
    .map_err(|e| AppError::Other(e.to_string()))?;

    let mut runs = Vec::with_capacity(run_rows.len());
    for (run_id, model, dataset, dataset_version, comparable) in run_rows {
        let trials: Vec<(String, f64, Option<String>)> = sqlx::query_as(
            "SELECT task_name, reward, failure_class FROM benchmark_trials WHERE run_id = ?",
        )
        .bind(&run_id)
        .fetch_all(&pool)
        .await
        .map_err(|e| AppError::Other(e.to_string()))?;
        runs.push(RunWithTrials {
            run_id,
            model: model.unwrap_or_else(|| "unknown".to_string()),
            dataset,
            dataset_version: dataset_version.unwrap_or_default(),
            comparable: comparable != 0,
            trials: trials
                .into_iter()
                .map(|(task_name, reward, failure_class)| TrialRow {
                    task_name,
                    reward,
                    failure_class,
                })
                .collect(),
        });
    }

    Ok(build_report(&request.dataset, &version, &runs, DEFAULT_PASS_THRESHOLD))
}
