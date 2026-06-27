// SPDX-License-Identifier: Apache-2.0
use serde::Deserialize;
use tauri::State;

use crate::benchmark::{BenchmarkEnvironmentProbe, BenchmarkProfile, ImportedBenchmarkRun};
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
pub async fn import_benchmark_results(
    request: ImportBenchmarkResultsRequest,
    state: State<'_, AppState>,
) -> Result<ImportedBenchmarkRun, AppError> {
    let pool = state.db.read().await;
    crate::benchmark::import_harbor_job(&pool, std::path::Path::new(&request.job_path)).await
}
