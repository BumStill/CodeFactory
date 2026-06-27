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
