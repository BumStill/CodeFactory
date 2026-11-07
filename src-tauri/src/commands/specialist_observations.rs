// SPDX-License-Identifier: Apache-2.0
//! Explicit, project-scoped read-observe authorization. Never dispatches code or CI.
use crate::specialist_observations::{
    self as service, Binding, Observation, ReadSource, RecordView,
};
use crate::{
    config::settings::{self, GitProvider, Settings},
    git_remote::client::RemoteGitClient,
    AppState,
};
use anyhow::{ensure, Result};
use std::time::Duration;
use tauri::State;

fn project_key(project: &str) -> Result<String> {
    ensure!(
        !project.is_empty() && project.len() <= 4096,
        "invalid project"
    );
    let path = std::fs::canonicalize(project)?;
    ensure!(path.is_dir(), "project directory required");
    path.into_os_string()
        .into_string()
        .map_err(|_| anyhow::anyhow!("non-Unicode project"))
}
fn material(settings: &Settings, id: &str) -> Result<(String, String)> {
    let cfg = settings
        .git_remotes
        .iter()
        .find(|r| r.id == id)
        .ok_or_else(|| anyhow::anyhow!("connection removed"))?;
    ensure!(
        cfg.provider == GitProvider::Github
            && cfg.base_url.trim_end_matches('/') == "https://api.github.com",
        "GitHub.com connection required"
    );
    let token = settings::resolve_git_remote_token(cfg)?;
    ensure!(!token.is_empty(), "credential missing");
    // Change detector only; not authentication or an execution capability.
    let revision =
        service::digest(&(cfg, &token, crate::specialist_credential_fence::revision()?))?;
    Ok((token, revision))
}
struct Source {
    client: RemoteGitClient,
}
#[async_trait::async_trait]
impl ReadSource for Source {
    async fn get(&self, path: &str) -> Result<serde_json::Value> {
        self.client
            .get_bounded(path, 2 * 1024 * 1024)
            .await
            .map_err(|_| anyhow::anyhow!("provider read failed"))
    }
}
async fn source(state: &AppState, id: &str) -> Result<(Source, String, String)> {
    let epoch = crate::specialist_credential_fence::revision()?;
    let (token, revision) = material(&*state.settings.read().await, id)?;
    let mut client = RemoteGitClient::new("https://api.github.com", &token, GitProvider::Github);
    client.inner = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()?;
    ensure!(
        epoch == crate::specialist_credential_fence::revision()?,
        "credential changed during read"
    );
    Ok((Source { client }, revision, epoch))
}
fn safe_error(_: impl std::fmt::Display) -> String {
    "只读取证未完成：项目、账户、连接权限或 head 已改变，或来源/存储不可用；请检查后重新显式绑定。"
        .into()
}

#[tauri::command]
pub async fn bind_specialist_project(
    project: String,
    remote_id: String,
    repo: String,
    pr: i64,
    expected_epoch: i64,
    state: State<'_, AppState>,
) -> Result<Binding, String> {
    let project = project_key(&project).map_err(safe_error)?;
    let pool = state.db.read().await.clone();
    let epoch = service::project_epoch(&pool, &project)
        .await
        .map_err(safe_error)?;
    if epoch != expected_epoch {
        return Err(safe_error("stale explicit authorization"));
    }
    let (source, revision, credential_epoch) =
        source(&state, &remote_id).await.map_err(safe_error)?;
    let snapshot = tokio::time::timeout(Duration::from_secs(45), async {
        let first = service::snapshot(&source, &repo, pr).await?;
        ensure!(
            service::snapshot(&source, &repo, pr).await? == first,
            "source changed while binding"
        );
        Ok::<_, anyhow::Error>(first)
    })
    .await
    .map_err(safe_error)?
    .map_err(safe_error)?;
    // Settings lock protects config; the shared secret fence protects credential mutation.
    let settings = state.settings.clone().read_owned().await;
    ensure_revision(&settings, &remote_id, &revision).map_err(safe_error)?;
    crate::specialist_credential_fence::commit(credential_epoch, async move {
        let result = service::save_binding_with_revision(
            &pool, &project, &remote_id, epoch, &snapshot, &revision,
        )
        .await;
        drop(settings); // Commit owns the guard even if the IPC future is cancelled.
        result
    })
    .await
    .map_err(safe_error)
}
fn ensure_revision(settings: &Settings, id: &str, expected: &str) -> Result<()> {
    ensure!(
        material(settings, id)?.1 == expected,
        "credential configuration changed"
    );
    Ok(())
}
#[tauri::command]
pub async fn query_specialist_observation(
    project: String,
    binding_id: String,
    state: State<'_, AppState>,
) -> Result<Observation, String> {
    let project = project_key(&project).map_err(safe_error)?;
    let pool = state.db.read().await.clone();
    let b = service::load_binding(&pool, &project, &binding_id)
        .await
        .map_err(safe_error)?;
    let (source, revision, credential_epoch) =
        source(&state, &b.remote_id).await.map_err(safe_error)?;
    if revision != b.credential_revision {
        return Err(safe_error("changed credential"));
    }
    let data = tokio::time::timeout(Duration::from_secs(45), service::observe(&source, &b))
        .await
        .map_err(safe_error)?
        .map_err(safe_error)?;
    let settings = state.settings.clone().read_owned().await;
    ensure_revision(&settings, &b.remote_id, &revision).map_err(safe_error)?;
    crate::specialist_credential_fence::commit(credential_epoch, async move {
        let result = service::save_observation(&pool, &b, &data).await;
        drop(settings);
        result
    })
    .await
    .map_err(safe_error)
}
#[tauri::command]
pub async fn stop_specialist_observations(
    project: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let project = project_key(&project).map_err(safe_error)?;
    let pool = state.db.read().await.clone();
    service::stop(&pool, &project).await.map_err(safe_error)
}
#[derive(serde::Serialize)]
pub struct ProjectObservations {
    pub epoch: i64,
    pub records: Vec<RecordView>,
}

#[tauri::command]
pub async fn list_specialist_observations(
    project: String,
    state: State<'_, AppState>,
) -> Result<ProjectObservations, String> {
    let project = project_key(&project).map_err(safe_error)?;
    let pool = state.db.read().await.clone();
    let mut rows = service::list(&pool, &project).await.map_err(safe_error)?;
    let epoch = service::project_epoch(&pool, &project)
        .await
        .map_err(safe_error)?;
    let settings = state.settings.read().await;
    for row in &mut rows {
        row.active = row.binding.epoch == epoch
            && ensure_revision(
                &settings,
                &row.binding.remote_id,
                &row.binding.credential_revision,
            )
            .is_ok();
    }
    Ok(ProjectObservations {
        epoch,
        records: rows,
    })
}
