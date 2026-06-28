// SPDX-License-Identifier: Apache-2.0

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, SqlitePool};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

use crate::config::settings::{normalize_model_id, ApiStyle, Settings};
use crate::errors::{AppError, Result};
use crate::util::no_window::NoWindow;

const TERMINAL_BENCH_21_PROFILE_ID: &str = "terminal-bench-2.1";
const TERMINAL_BENCH_21_DATASET: &str = "terminal-bench/terminal-bench-2-1";
const CODEFACTORY_HARBOR_AGENT: &str = "codefactory_bench.agent:CodeFactoryAgent";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BenchmarkProfile {
    pub id: String,
    pub dataset: String,
    pub harness: String,
    pub official_url: String,
    pub leaderboard_url: String,
    pub comparable_constraints: Vec<String>,
    pub default_smoke_task_limit: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus {
    Ok,
    Missing,
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProbeCommandResult {
    pub binary: String,
    pub available: bool,
    pub stdout: String,
    pub stderr: String,
    pub detail: String,
}

impl ProbeCommandResult {
    pub fn missing(binary: &str) -> Self {
        Self {
            binary: binary.to_string(),
            available: false,
            stdout: String::new(),
            stderr: String::new(),
            detail: format!("{binary} is not available"),
        }
    }

    pub fn ok(binary: &str, stdout: impl Into<String>) -> Self {
        Self {
            binary: binary.to_string(),
            available: true,
            stdout: stdout.into(),
            stderr: String::new(),
            detail: format!("{binary} is available"),
        }
    }

    fn warning(binary: &str, stderr: impl Into<String>) -> Self {
        let stderr = stderr.into();
        Self {
            binary: binary.to_string(),
            available: false,
            stdout: String::new(),
            detail: if stderr.trim().is_empty() {
                format!("{binary} returned a non-zero status")
            } else {
                stderr.clone()
            },
            stderr,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BenchmarkProbeItem {
    pub id: String,
    pub label: String,
    pub status: ProbeStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkEnvironmentProbe {
    pub generated_at: String,
    pub profile: BenchmarkProfile,
    pub ready: bool,
    pub blockers: Vec<String>,
    pub items: Vec<BenchmarkProbeItem>,
    pub command_preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkRunRecord {
    pub id: String,
    pub benchmark_id: String,
    pub dataset: String,
    pub dataset_version: Option<String>,
    pub agent_name: String,
    pub agent_version: Option<String>,
    pub model: Option<String>,
    pub codefactory_version: Option<String>,
    pub codefactory_git_sha: Option<String>,
    pub policy_preset: String,
    pub harbor_version: Option<String>,
    pub command: String,
    pub job_path: String,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub comparable: bool,
    pub comparable_reason: Option<String>,
    pub missing_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkTrialRecord {
    pub id: String,
    pub run_id: String,
    pub task_name: String,
    pub category: Option<String>,
    pub difficulty: Option<String>,
    pub reward: f64,
    pub duration_ms: Option<i64>,
    pub error_kind: Option<String>,
    pub failure_class: Option<String>,
    pub trajectory_path: Option<String>,
    pub verifier_stdout_path: Option<String>,
    pub verifier_stderr_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportedBenchmarkRun {
    pub run: BenchmarkRunRecord,
    pub trials: Vec<BenchmarkTrialRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BenchmarkEnvVarPreview {
    pub name: String,
    pub value: String,
    pub secret: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BenchmarkProviderBridgeRequest {
    pub profile_id: String,
    pub endpoint_name: Option<String>,
    pub model: Option<String>,
    pub task_limit: Option<u32>,
    pub concurrency: Option<u32>,
    // Backwards-compatible alias for older clients. Harbor uses `-n` as
    // concurrency, not repeated trial count.
    pub trial_count: Option<u32>,
    pub job_root: Option<String>,
    pub job_name: Option<String>,
    pub adapter_root: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BenchmarkProviderBridgePreview {
    pub generated_at: String,
    pub profile: BenchmarkProfile,
    pub endpoint_name: String,
    pub base_url: String,
    pub api_style: String,
    pub model: String,
    pub key_ref: String,
    pub agent_import_path: String,
    pub task_limit: u32,
    pub concurrency: u32,
    pub trial_count: u32,
    pub job_root: String,
    pub job_name: String,
    pub job_path: String,
    pub adapter_root: String,
    pub env_preview: Vec<BenchmarkEnvVarPreview>,
    pub command_preview: String,
    pub authorization_phrase: String,
    pub ready: bool,
    pub blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StartBenchmarkProviderRunRequest {
    pub bridge: BenchmarkProviderBridgeRequest,
    pub authorization_phrase: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkProviderRunResult {
    pub preview: BenchmarkProviderBridgePreview,
    pub status: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub imported: Option<ImportedBenchmarkRun>,
}

#[derive(Debug, Clone)]
struct AuthorizedBenchmarkProviderLaunch {
    preview: BenchmarkProviderBridgePreview,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

impl AuthorizedBenchmarkProviderLaunch {
    #[cfg(test)]
    fn env_value(&self, name: &str) -> Option<&str> {
        self.env
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
}

struct BenchmarkProviderCommandOutput {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

pub fn terminal_bench_21_profile() -> BenchmarkProfile {
    BenchmarkProfile {
        id: TERMINAL_BENCH_21_PROFILE_ID.to_string(),
        dataset: TERMINAL_BENCH_21_DATASET.to_string(),
        harness: "harbor".to_string(),
        official_url: "https://www.tbench.ai/docs/run-terminal-bench-2-1".to_string(),
        leaderboard_url: "https://www.tbench.ai/leaderboard/terminal-bench/2.1".to_string(),
        comparable_constraints: vec![
            "dataset must remain terminal-bench/terminal-bench-2-1".to_string(),
            "timeout and resource settings must not be modified".to_string(),
            "agent, model, CodeFactory build, policy preset, and Harbor job path must be recorded"
                .to_string(),
        ],
        default_smoke_task_limit: 5,
    }
}

pub fn list_profiles() -> Vec<BenchmarkProfile> {
    vec![terminal_bench_21_profile()]
}

pub fn preview_provider_bridge(
    settings: &Settings,
    request: &BenchmarkProviderBridgeRequest,
) -> Result<BenchmarkProviderBridgePreview> {
    let profile = profile_by_id(&request.profile_id)?;
    let endpoint_name = request
        .endpoint_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&settings.default_endpoint)
        .to_string();
    let endpoint = settings
        .endpoints
        .get(&endpoint_name)
        .ok_or_else(|| AppError::Other(format!("Unknown endpoint: {endpoint_name}")))?;
    let base_url = endpoint.base_url.trim_end_matches('/').to_string();
    let requested_model = request
        .model
        .as_deref()
        .filter(|model| !model.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| settings.active_model_for(&endpoint_name));
    let model = normalize_model_id(requested_model.trim(), &base_url);
    let key_ref = endpoint
        .key_ref
        .clone()
        .unwrap_or_else(|| format!("codefactory.endpoint.{endpoint_name}"));
    let task_limit = request
        .task_limit
        .unwrap_or(profile.default_smoke_task_limit)
        .max(1);
    let concurrency = request.concurrency.or(request.trial_count).unwrap_or(4).max(1);
    let trial_count = 1;
    let job_root = request
        .job_root
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| default_benchmark_job_root().to_string_lossy().to_string());
    let job_name = request
        .job_name
        .as_deref()
        .filter(|name| !name.trim().is_empty())
        .map(sanitize_job_name)
        .unwrap_or_else(default_benchmark_job_name);
    let job_path = Path::new(&job_root)
        .join(&job_name)
        .to_string_lossy()
        .to_string();
    let adapter_root = request
        .adapter_root
        .as_deref()
        .filter(|path| !path.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .to_string_lossy()
                .to_string()
        });
    let mut blockers = Vec::new();
    if !matches!(endpoint.api_style, ApiStyle::Openai) {
        blockers.push(format!(
            "Benchmark provider bridge currently supports OpenAI-compatible chat/completions endpoints only; endpoint '{endpoint_name}' uses {}",
            api_style_label(&endpoint.api_style)
        ));
    }
    if model.trim().is_empty() {
        blockers.push(format!(
            "Endpoint '{endpoint_name}' has no active model for Terminal-Bench"
        ));
    }

    let env_preview = vec![
        BenchmarkEnvVarPreview {
            name: "CODEFACTORY_BENCH_API_KEY".to_string(),
            value: format!("<redacted:{key_ref}>"),
            secret: true,
        },
        BenchmarkEnvVarPreview {
            name: "CODEFACTORY_BENCH_BASE_URL".to_string(),
            value: base_url.clone(),
            secret: false,
        },
        BenchmarkEnvVarPreview {
            name: "CODEFACTORY_BENCH_MODEL".to_string(),
            value: model.clone(),
            secret: false,
        },
        BenchmarkEnvVarPreview {
            name: "CODEFACTORY_BENCH_REQUIRE_MODEL".to_string(),
            value: "1".to_string(),
            secret: false,
        },
    ];
    let args = harbor_run_args(
        &profile,
        &model,
        task_limit,
        concurrency,
        &job_root,
        &job_name,
    );
    let authorization_phrase = format!(
        "Run {} with endpoint {} model {}",
        profile.id, endpoint_name, model
    );
    let command_preview = command_preview(&env_preview, &args);

    Ok(BenchmarkProviderBridgePreview {
        generated_at: Utc::now().to_rfc3339(),
        profile,
        endpoint_name,
        base_url,
        api_style: api_style_label(&endpoint.api_style).to_string(),
        model,
        key_ref,
        agent_import_path: CODEFACTORY_HARBOR_AGENT.to_string(),
        task_limit,
        concurrency,
        trial_count,
        job_root,
        job_name,
        job_path,
        adapter_root,
        env_preview,
        command_preview,
        authorization_phrase,
        ready: blockers.is_empty(),
        blockers,
    })
}

pub async fn start_provider_benchmark_run(
    pool: &SqlitePool,
    settings: &Settings,
    request: StartBenchmarkProviderRunRequest,
) -> Result<BenchmarkProviderRunResult> {
    let launch = resolve_authorized_provider_launch(settings, &request, crate::secrets::get_key)?;
    let preview = launch.preview.clone();
    let job_path = preview.job_path.clone();
    let output = tokio::task::spawn_blocking(move || execute_provider_launch(launch))
        .await
        .map_err(|err| AppError::Other(format!("Benchmark Harbor worker failed: {err}")))??;
    let imported = if output.exit_code == Some(0) && Path::new(&job_path).is_dir() {
        Some(import_harbor_job(pool, Path::new(&job_path)).await?)
    } else {
        None
    };
    let status = if output.exit_code == Some(0) {
        "completed"
    } else {
        "failed"
    }
    .to_string();

    Ok(BenchmarkProviderRunResult {
        preview,
        status,
        exit_code: output.exit_code,
        stdout: output.stdout,
        stderr: output.stderr,
        imported,
    })
}

fn resolve_authorized_provider_launch<F>(
    settings: &Settings,
    request: &StartBenchmarkProviderRunRequest,
    mut secret_lookup: F,
) -> Result<AuthorizedBenchmarkProviderLaunch>
where
    F: FnMut(&str) -> Result<Option<String>>,
{
    let preview = preview_provider_bridge(settings, &request.bridge)?;
    if !preview.ready {
        return Err(AppError::Other(format!(
            "Benchmark provider bridge is not ready: {}",
            preview.blockers.join("; ")
        )));
    }
    if request.authorization_phrase.trim() != preview.authorization_phrase {
        return Err(AppError::Other(
            "Benchmark provider authorization phrase did not match".to_string(),
        ));
    }
    let api_key = secret_lookup(&preview.key_ref)?.ok_or_else(|| {
        AppError::Other(format!(
            "API key not found for benchmark provider key_ref '{}'",
            preview.key_ref
        ))
    })?;
    if api_key.trim().is_empty() {
        return Err(AppError::Other(format!(
            "API key is empty for benchmark provider key_ref '{}'",
            preview.key_ref
        )));
    }

    Ok(AuthorizedBenchmarkProviderLaunch {
        args: harbor_run_args(
            &preview.profile,
            &preview.model,
            preview.task_limit,
            preview.concurrency,
            &preview.job_root,
            &preview.job_name,
        ),
        env: vec![
            ("CODEFACTORY_BENCH_API_KEY".to_string(), api_key),
            (
                "CODEFACTORY_BENCH_BASE_URL".to_string(),
                preview.base_url.clone(),
            ),
            ("CODEFACTORY_BENCH_MODEL".to_string(), preview.model.clone()),
            (
                "CODEFACTORY_BENCH_REQUIRE_MODEL".to_string(),
                "1".to_string(),
            ),
        ],
        preview,
    })
}

fn profile_by_id(profile_id: &str) -> Result<BenchmarkProfile> {
    list_profiles()
        .into_iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| AppError::Other(format!("Unknown benchmark profile: {profile_id}")))
}

pub fn probe_environment(profile_id: &str) -> Result<BenchmarkEnvironmentProbe> {
    let harbor_binary = resolve_harbor_binary();
    let harbor = probe_command(&harbor_binary, &["--help"]);
    let docker = probe_command(Path::new("docker"), &["info"]);
    probe_environment_from_command_results(profile_id, harbor, docker)
}

pub fn probe_environment_from_command_results(
    profile_id: &str,
    harbor: ProbeCommandResult,
    docker: ProbeCommandResult,
) -> Result<BenchmarkEnvironmentProbe> {
    let profile = profile_by_id(profile_id)?;
    let mut blockers = Vec::new();
    let mut items = Vec::new();

    let harbor_status = if harbor.available {
        ProbeStatus::Ok
    } else {
        blockers.push("harbor CLI is required to run Terminal-Bench 2.1".to_string());
        ProbeStatus::Missing
    };
    items.push(BenchmarkProbeItem {
        id: "harbor".to_string(),
        label: "Harbor CLI".to_string(),
        status: harbor_status,
        detail: truncate_probe_detail(&harbor),
    });

    let docker_status = if docker.available {
        ProbeStatus::Ok
    } else {
        blockers
            .push("Docker must be installed and running for Harbor task containers".to_string());
        ProbeStatus::Missing
    };
    items.push(BenchmarkProbeItem {
        id: "docker".to_string(),
        label: "Docker".to_string(),
        status: docker_status,
        detail: truncate_probe_detail(&docker),
    });

    items.push(BenchmarkProbeItem {
        id: "dataset".to_string(),
        label: "Dataset".to_string(),
        status: ProbeStatus::Ok,
        detail: profile.dataset.clone(),
    });
    items.push(BenchmarkProbeItem {
        id: "policy".to_string(),
        label: "Policy preset".to_string(),
        status: ProbeStatus::Warning,
        detail: "benchmark-sandbox must be scoped to Harbor task containers only".to_string(),
    });

    let command_preview = format!(
        "harbor run -d {} --agent-import-path {} -m <model> -l {}",
        profile.dataset, CODEFACTORY_HARBOR_AGENT, profile.default_smoke_task_limit
    );

    Ok(BenchmarkEnvironmentProbe {
        generated_at: Utc::now().to_rfc3339(),
        profile,
        ready: blockers.is_empty(),
        blockers,
        items,
        command_preview,
    })
}

fn probe_command(binary: &Path, args: &[&str]) -> ProbeCommandResult {
    let binary_label = binary.to_string_lossy();
    match Command::new(binary).no_window().args(args).output() {
        Ok(output) if output.status.success() => ProbeCommandResult {
            binary: binary_label.to_string(),
            available: true,
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            detail: format!("{binary_label} is available"),
        },
        Ok(output) => ProbeCommandResult::warning(
            &binary_label,
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ),
        Err(_) => ProbeCommandResult::missing(&binary_label),
    }
}

fn execute_provider_launch(
    launch: AuthorizedBenchmarkProviderLaunch,
) -> Result<BenchmarkProviderCommandOutput> {
    fs::create_dir_all(&launch.preview.job_root)?;
    let adapter_root = PathBuf::from(&launch.preview.adapter_root);
    if !adapter_root.is_dir() {
        return Err(AppError::Other(format!(
            "Benchmark adapter root is not a directory: {}",
            adapter_root.display()
        )));
    }

    let mut command = Command::new(resolve_harbor_binary()).no_window();
    command.args(&launch.args).current_dir(&adapter_root);
    for (key, value) in &launch.env {
        command.env(key, value);
    }
    command.env("PYTHONPATH", pythonpath_with_adapter_root(&adapter_root)?);

    let output = command.output()?;
    Ok(BenchmarkProviderCommandOutput {
        exit_code: output.status.code(),
        stdout: tail_text(&String::from_utf8_lossy(&output.stdout), 12000),
        stderr: tail_text(&String::from_utf8_lossy(&output.stderr), 12000),
    })
}

fn resolve_harbor_binary() -> PathBuf {
    if let Some(path) = find_binary_on_path("harbor") {
        return path;
    }
    if let Some(home) = dirs::home_dir() {
        for candidate in [
            home.join(".local/bin/harbor"),
            home.join(".local/share/uv/tools/harbor/bin/harbor"),
        ] {
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from("harbor")
}

fn find_binary_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn pythonpath_with_adapter_root(adapter_root: &Path) -> Result<OsString> {
    let mut paths = vec![adapter_root.to_path_buf()];
    if let Some(existing) = std::env::var_os("PYTHONPATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths)
        .map_err(|err| AppError::Other(format!("Could not build PYTHONPATH: {err}")))
}

fn harbor_run_args(
    profile: &BenchmarkProfile,
    model: &str,
    task_limit: u32,
    concurrency: u32,
    job_root: &str,
    job_name: &str,
) -> Vec<String> {
    vec![
        "run".to_string(),
        "-d".to_string(),
        profile.dataset.clone(),
        "--agent-import-path".to_string(),
        CODEFACTORY_HARBOR_AGENT.to_string(),
        "-m".to_string(),
        model.to_string(),
        "-l".to_string(),
        task_limit.to_string(),
        "-n".to_string(),
        concurrency.to_string(),
        "-o".to_string(),
        job_root.to_string(),
        "--job-name".to_string(),
        job_name.to_string(),
        "-y".to_string(),
    ]
}

fn command_preview(env_preview: &[BenchmarkEnvVarPreview], args: &[String]) -> String {
    let env = env_preview
        .iter()
        .map(|item| format!("{}={}", item.name, shell_quote(&item.value)))
        .collect::<Vec<_>>()
        .join(" ");
    let command = std::iter::once("harbor".to_string())
        .chain(args.iter().cloned())
        .map(|part| shell_quote(&part))
        .collect::<Vec<_>>()
        .join(" ");
    format!("{env} {command}")
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '='))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn sanitize_job_name(name: &str) -> String {
    let sanitized = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if sanitized.is_empty() {
        default_benchmark_job_name()
    } else {
        sanitized
    }
}

fn default_benchmark_job_name() -> String {
    format!(
        "cf-tb21-codefactory-headless-{}",
        Utc::now().format("%Y%m%d-%H%M%S")
    )
}

fn default_benchmark_job_root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("CodeFactory")
        .join("benchmark-jobs")
}

fn api_style_label(style: &ApiStyle) -> &'static str {
    match style {
        ApiStyle::Openai => "openai",
        ApiStyle::Anthropic => "anthropic",
        ApiStyle::Chatgpt => "chatgpt",
    }
}

fn tail_text(text: &str, max_chars: usize) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let tail = text
        .chars()
        .skip(total.saturating_sub(max_chars))
        .collect::<String>();
    format!("[truncated to last {max_chars} chars]\n{tail}")
}

fn truncate_probe_detail(result: &ProbeCommandResult) -> String {
    let detail = if !result.detail.trim().is_empty() {
        result.detail.trim()
    } else if !result.stderr.trim().is_empty() {
        result.stderr.trim()
    } else {
        result.stdout.trim()
    };
    detail.chars().take(240).collect()
}

pub async fn ensure_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS benchmark_runs (
            id                   TEXT PRIMARY KEY,
            benchmark_id         TEXT NOT NULL,
            dataset              TEXT NOT NULL,
            dataset_version      TEXT,
            agent_name           TEXT NOT NULL,
            agent_version        TEXT,
            model                TEXT,
            codefactory_version  TEXT,
            codefactory_git_sha  TEXT,
            policy_preset        TEXT NOT NULL,
            harbor_version       TEXT,
            command              TEXT NOT NULL,
            job_path             TEXT NOT NULL,
            status               TEXT NOT NULL,
            started_at           TEXT NOT NULL,
            finished_at          TEXT,
            comparable           INTEGER NOT NULL DEFAULT 0,
            comparable_reason    TEXT,
            missing_files_json   TEXT NOT NULL DEFAULT '[]',
            created_at           TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS benchmark_trials (
            id                    TEXT PRIMARY KEY,
            run_id                TEXT NOT NULL,
            task_name             TEXT NOT NULL,
            category              TEXT,
            difficulty            TEXT,
            reward                REAL NOT NULL,
            duration_ms           INTEGER,
            error_kind            TEXT,
            failure_class         TEXT,
            trajectory_path       TEXT,
            verifier_stdout_path  TEXT,
            verifier_stderr_path  TEXT,
            created_at            TEXT NOT NULL,
            FOREIGN KEY(run_id) REFERENCES benchmark_runs(id) ON DELETE CASCADE
        )",
    )
    .execute(pool)
    .await?;

    for index in [
        "CREATE INDEX IF NOT EXISTS idx_benchmark_runs_benchmark ON benchmark_runs(benchmark_id)",
        "CREATE INDEX IF NOT EXISTS idx_benchmark_runs_created ON benchmark_runs(created_at)",
        "CREATE INDEX IF NOT EXISTS idx_benchmark_trials_run ON benchmark_trials(run_id)",
        "CREATE INDEX IF NOT EXISTS idx_benchmark_trials_failure ON benchmark_trials(failure_class)",
    ] {
        sqlx::query(index).execute(pool).await?;
    }
    Ok(())
}

pub async fn import_harbor_job(pool: &SqlitePool, job_path: &Path) -> Result<ImportedBenchmarkRun> {
    ensure_schema(pool).await?;

    if !job_path.is_dir() {
        return Err(AppError::Other(format!(
            "Harbor job path is not a directory: {}",
            job_path.display()
        )));
    }

    let profile = terminal_bench_21_profile();
    let mut missing_files = Vec::new();
    let config = read_json_file(job_path.join("config.json"), &mut missing_files)?;
    let result = read_json_file(job_path.join("result.json"), &mut missing_files)?;
    let trial_agent_info = first_trial_agent_info(job_path)?;
    let now = Utc::now().to_rfc3339();

    let dataset = json_string(&config, &["dataset"])
        .or_else(|| json_string(&config, &["dataset_name"]))
        .or_else(|| json_string(&config, &["datasets", "0", "name"]))
        .unwrap_or_else(|| profile.dataset.clone());
    let command = json_string(&config, &["command"]).unwrap_or_else(|| {
        format!(
            "harbor run -d {} --agent-import-path {} -m <model> -l {}",
            profile.dataset, CODEFACTORY_HARBOR_AGENT, profile.default_smoke_task_limit
        )
    });
    let agent_name = json_string(&config, &["agent"])
        .or_else(|| json_string(&config, &["agent_name"]))
        .or_else(|| json_string(&config, &["agents", "0", "name"]))
        .or_else(|| trial_agent_info.name.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let policy_preset = json_string(&config, &["metadata", "policy_preset"])
        .unwrap_or_else(|| "benchmark-sandbox".to_string());
    let status = if missing_files.is_empty() {
        json_string(&result, &["status"]).unwrap_or_else(|| "imported".to_string())
    } else {
        "partial_import".to_string()
    };
    let comparable = dataset == profile.dataset && !has_official_constraint_override(&config);
    let comparable_reason = if comparable {
        None
    } else if dataset != profile.dataset {
        Some(format!(
            "dataset mismatch: expected {}, got {dataset}",
            profile.dataset
        ))
    } else {
        Some("timeout/resource override marker found in job config".to_string())
    };

    let run = BenchmarkRunRecord {
        id: json_string(&result, &["id"])
            .or_else(|| json_string(&config, &["run_id"]))
            .unwrap_or_else(|| Uuid::new_v4().to_string()),
        benchmark_id: profile.id,
        dataset,
        dataset_version: json_string(&config, &["dataset_version"])
            .or_else(|| json_string(&config, &["datasets", "0", "version"]))
            .or_else(|| json_string(&config, &["datasets", "0", "ref"])),
        agent_name,
        agent_version: json_string(&config, &["agent_version"]).or(trial_agent_info.version),
        model: json_string(&config, &["model"])
            .or_else(|| json_string(&config, &["agents", "0", "model_name"]))
            .or(trial_agent_info.model),
        codefactory_version: json_string(&config, &["metadata", "codefactory_version"]),
        codefactory_git_sha: json_string(&config, &["metadata", "codefactory_git_sha"]),
        policy_preset,
        harbor_version: json_string(&config, &["harbor_version"]),
        command,
        job_path: job_path.to_string_lossy().to_string(),
        status,
        started_at: json_string(&config, &["started_at"]).unwrap_or_else(|| now.clone()),
        finished_at: json_string(&result, &["finished_at"]),
        comparable,
        comparable_reason,
        missing_files,
    };

    let trials = import_trials(&run.id, job_path)?;

    persist_run(pool, &run).await?;
    sqlx::query("DELETE FROM benchmark_trials WHERE run_id = ?")
        .bind(&run.id)
        .execute(pool)
        .await?;
    for trial in &trials {
        persist_trial(pool, trial).await?;
    }

    Ok(ImportedBenchmarkRun { run, trials })
}

fn read_json_file(path: PathBuf, missing_files: &mut Vec<String>) -> Result<Value> {
    if !path.exists() {
        missing_files.push(path.to_string_lossy().to_string());
        return Ok(Value::Object(Default::default()));
    }
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn import_trials(run_id: &str, job_path: &Path) -> Result<Vec<BenchmarkTrialRecord>> {
    let mut trials = Vec::new();
    for trial_dir in harbor_trial_dirs(job_path)? {
        let fallback_task_name = trial_dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| trial_dir.to_string_lossy().to_string());
        let mut missing = Vec::new();
        let config = read_json_file(trial_dir.join("config.json"), &mut missing)?;
        let result = read_json_file(trial_dir.join("result.json"), &mut missing)?;
        let task_name = json_string(&result, &["task_name"])
            .or_else(|| json_string(&config, &["task_name"]))
            .or_else(|| json_string(&config, &["task", "name"]))
            .or_else(|| json_string(&config, &["name"]))
            .unwrap_or(fallback_task_name);
        let reward = json_f64(&result, &["reward"])
            .or_else(|| json_f64(&result, &["result", "reward"]))
            .or_else(|| json_f64(&result, &["verifier_result", "rewards", "reward"]))
            .unwrap_or(0.0);
        let verifier_stdout_path = first_existing_path(
            &trial_dir,
            &[
                "verifier/test-stdout.txt",
                "verifier/stdout.txt",
                "test-stdout.txt",
            ],
        );
        let verifier_stderr_path = first_existing_path(
            &trial_dir,
            &[
                "verifier/test-stderr.txt",
                "verifier/stderr.txt",
                "test-stderr.txt",
            ],
        );
        let trajectory_path = first_existing_path(
            &trial_dir,
            &["agent/trajectory.json", "trajectory.json", "agent.log"],
        );
        let evidence = [
            read_optional_text(verifier_stdout_path.as_deref())?,
            read_optional_text(verifier_stderr_path.as_deref())?,
            json_string(&result, &["error"]).unwrap_or_default(),
            json_string(&result, &["exception_info", "exception_type"]).unwrap_or_default(),
            json_string(&result, &["exception_info", "exception_message"]).unwrap_or_default(),
            read_optional_text(first_existing_path(&trial_dir, &["exception.txt"]).as_deref())?,
        ]
        .join("\n");

        trials.push(BenchmarkTrialRecord {
            id: Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            task_name,
            category: json_string(&config, &["category"])
                .or_else(|| json_string(&result, &["source"])),
            difficulty: json_string(&config, &["difficulty"]),
            reward,
            duration_ms: json_i64(&result, &["duration_ms"])
                .or_else(|| json_i64(&result, &["duration"]))
                .or_else(|| duration_ms_from_result(&result)),
            error_kind: json_string(&result, &["error_kind"])
                .or_else(|| json_string(&result, &["exception_info", "type"]))
                .or_else(|| json_string(&result, &["exception_info", "exception_type"])),
            failure_class: classify_failure(reward, &evidence),
            trajectory_path: trajectory_path.map(|path| path.to_string_lossy().to_string()),
            verifier_stdout_path: verifier_stdout_path
                .map(|path| path.to_string_lossy().to_string()),
            verifier_stderr_path: verifier_stderr_path
                .map(|path| path.to_string_lossy().to_string()),
        });
    }

    Ok(trials)
}

#[derive(Default)]
struct TrialAgentInfo {
    name: Option<String>,
    version: Option<String>,
    model: Option<String>,
}

fn first_trial_agent_info(job_path: &Path) -> Result<TrialAgentInfo> {
    for trial_dir in harbor_trial_dirs(job_path)? {
        let result_path = trial_dir.join("result.json");
        if !result_path.exists() {
            continue;
        }
        let result: Value = serde_json::from_str(&fs::read_to_string(result_path)?)?;
        let info = TrialAgentInfo {
            name: json_string(&result, &["agent_info", "name"]),
            version: json_string(&result, &["agent_info", "version"]),
            model: json_string(&result, &["agent_info", "model_info", "name"]),
        };
        if info.name.is_some() || info.version.is_some() || info.model.is_some() {
            return Ok(info);
        }
    }
    Ok(TrialAgentInfo::default())
}

fn harbor_trial_dirs(job_path: &Path) -> Result<Vec<PathBuf>> {
    let trials_dir = if job_path.join("trials").is_dir() {
        job_path.join("trials")
    } else {
        job_path.to_path_buf()
    };
    if !trials_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut entries: Vec<_> = fs::read_dir(&trials_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .filter(|entry| entry.path().join("result.json").exists())
        .collect();
    entries.sort_by_key(|entry| entry.file_name());

    Ok(entries.into_iter().map(|entry| entry.path()).collect())
}

fn classify_failure(reward: f64, evidence: &str) -> Option<String> {
    if reward >= 1.0 {
        return None;
    }
    let text = evidence.to_lowercase();
    let class = if text.contains("permission")
        || text.contains("denied")
        || text.contains("not allowed")
        || text.contains("outside workspace")
    {
        "policy"
    } else if text.contains("model request failed")
        || text.contains("chat/completions")
        || text.contains("insufficient balance")
        || text.contains("payment required")
        || text.contains("rate limit")
        || text.contains("too many requests")
        || text.contains("invalid_api_key")
        || text.contains("http 401")
        || text.contains("http 402")
        || text.contains("http 403")
        || text.contains("http 429")
    {
        "model-provider"
    } else if text.contains("command not found")
        || text.contains("no such file")
        || text.contains("exit status 127")
        || text.contains("syntax error")
    {
        "tool-use"
    } else if text.contains("assert")
        || text.contains("expected")
        || text.contains("pytest")
        || text.contains("test failed")
    {
        "verification"
    } else if text.contains("timeout") || text.contains("timed out") {
        "long-horizon"
    } else if text.contains("docker")
        || text.contains("network")
        || text.contains("dependency")
        || text.contains("package")
    {
        "environment"
    } else if text.contains("context") || text.contains("instruction") || text.contains("readme") {
        "context"
    } else {
        "planning"
    };
    Some(class.to_string())
}

fn first_existing_path(base: &Path, candidates: &[&str]) -> Option<PathBuf> {
    candidates
        .iter()
        .map(|candidate| base.join(candidate))
        .find(|path| path.exists())
}

fn read_optional_text(path: Option<&Path>) -> Result<String> {
    match path {
        Some(path) => Ok(fs::read_to_string(path)?),
        None => Ok(String::new()),
    }
}

fn json_string(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for segment in path {
        current = if let Ok(index) = segment.parse::<usize>() {
            current.get(index)?
        } else {
            current.get(segment)?
        };
    }
    current
        .as_str()
        .map(str::to_string)
        .or_else(|| current.as_i64().map(|n| n.to_string()))
        .or_else(|| current.as_f64().map(|n| n.to_string()))
        .filter(|s| !s.trim().is_empty())
}

fn json_f64(value: &Value, path: &[&str]) -> Option<f64> {
    let mut current = value;
    for segment in path {
        current = if let Ok(index) = segment.parse::<usize>() {
            current.get(index)?
        } else {
            current.get(segment)?
        };
    }
    current
        .as_f64()
        .or_else(|| current.as_i64().map(|n| n as f64))
}

fn json_i64(value: &Value, path: &[&str]) -> Option<i64> {
    let mut current = value;
    for segment in path {
        current = if let Ok(index) = segment.parse::<usize>() {
            current.get(index)?
        } else {
            current.get(segment)?
        };
    }
    current.as_i64()
}

fn duration_ms_from_result(value: &Value) -> Option<i64> {
    let started = json_string(value, &["started_at"])?;
    let finished = json_string(value, &["finished_at"])?;
    let started = chrono::DateTime::parse_from_rfc3339(&started).ok()?;
    let finished = chrono::DateTime::parse_from_rfc3339(&finished).ok()?;
    Some((finished - started).num_milliseconds())
}

fn has_official_constraint_override(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.iter().any(|(key, value)| {
            let normalized = key.replace(['-', '_'], "").to_lowercase();
            matches!(
                normalized.as_str(),
                "timeoutoverride"
                    | "resourceoverride"
                    | "resourcesoverride"
                    | "modifiedtimeout"
                    | "modifiedresources"
                    | "comparableoverride"
            ) || has_official_constraint_override(value)
        }),
        Value::Array(items) => items.iter().any(has_official_constraint_override),
        _ => false,
    }
}

async fn persist_run(pool: &SqlitePool, run: &BenchmarkRunRecord) -> Result<()> {
    sqlx::query(
        "INSERT OR REPLACE INTO benchmark_runs (
            id, benchmark_id, dataset, dataset_version, agent_name, agent_version, model,
            codefactory_version, codefactory_git_sha, policy_preset, harbor_version, command,
            job_path, status, started_at, finished_at, comparable, comparable_reason,
            missing_files_json, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&run.id)
    .bind(&run.benchmark_id)
    .bind(&run.dataset)
    .bind(&run.dataset_version)
    .bind(&run.agent_name)
    .bind(&run.agent_version)
    .bind(&run.model)
    .bind(&run.codefactory_version)
    .bind(&run.codefactory_git_sha)
    .bind(&run.policy_preset)
    .bind(&run.harbor_version)
    .bind(&run.command)
    .bind(&run.job_path)
    .bind(&run.status)
    .bind(&run.started_at)
    .bind(&run.finished_at)
    .bind(if run.comparable { 1_i64 } else { 0_i64 })
    .bind(&run.comparable_reason)
    .bind(serde_json::to_string(&run.missing_files)?)
    .bind(Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

async fn persist_trial(pool: &SqlitePool, trial: &BenchmarkTrialRecord) -> Result<()> {
    sqlx::query(
        "INSERT INTO benchmark_trials (
            id, run_id, task_name, category, difficulty, reward, duration_ms, error_kind,
            failure_class, trajectory_path, verifier_stdout_path, verifier_stderr_path, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&trial.id)
    .bind(&trial.run_id)
    .bind(&trial.task_name)
    .bind(&trial.category)
    .bind(&trial.difficulty)
    .bind(trial.reward)
    .bind(trial.duration_ms)
    .bind(&trial.error_kind)
    .bind(&trial.failure_class)
    .bind(&trial.trajectory_path)
    .bind(&trial.verifier_stdout_path)
    .bind(&trial.verifier_stderr_path)
    .bind(Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

#[allow(dead_code)]
async fn list_runs(pool: &SqlitePool) -> Result<Vec<BenchmarkRunRecord>> {
    let rows = sqlx::query(
        "SELECT id, benchmark_id, dataset, dataset_version, agent_name, agent_version, model,
                codefactory_version, codefactory_git_sha, policy_preset, harbor_version, command,
                job_path, status, started_at, finished_at, comparable, comparable_reason,
                missing_files_json
         FROM benchmark_runs ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?;

    rows.into_iter().map(row_to_run).collect()
}

fn row_to_run(row: sqlx::sqlite::SqliteRow) -> Result<BenchmarkRunRecord> {
    let missing_files_json: String = row.try_get("missing_files_json")?;
    let missing_files = serde_json::from_str(&missing_files_json).unwrap_or_default();
    Ok(BenchmarkRunRecord {
        id: row.try_get("id")?,
        benchmark_id: row.try_get("benchmark_id")?,
        dataset: row.try_get("dataset")?,
        dataset_version: row.try_get("dataset_version")?,
        agent_name: row.try_get("agent_name")?,
        agent_version: row.try_get("agent_version")?,
        model: row.try_get("model")?,
        codefactory_version: row.try_get("codefactory_version")?,
        codefactory_git_sha: row.try_get("codefactory_git_sha")?,
        policy_preset: row.try_get("policy_preset")?,
        harbor_version: row.try_get("harbor_version")?,
        command: row.try_get("command")?,
        job_path: row.try_get("job_path")?,
        status: row.try_get("status")?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        comparable: row.try_get::<i64, _>("comparable")? == 1,
        comparable_reason: row.try_get("comparable_reason")?,
        missing_files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sqlx::sqlite::SqlitePoolOptions;
    use std::fs;
    use uuid::Uuid;

    fn temp_job_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cf-benchmark-job-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("create temp job dir");
        dir
    }

    #[test]
    fn terminal_bench_21_profile_is_locked_to_official_dataset() {
        let profile = terminal_bench_21_profile();

        assert_eq!(profile.id, "terminal-bench-2.1");
        assert_eq!(profile.dataset, "terminal-bench/terminal-bench-2-1");
        assert_eq!(profile.harness, "harbor");
        assert_eq!(
            profile.official_url,
            "https://www.tbench.ai/docs/run-terminal-bench-2-1"
        );
        assert_eq!(
            profile.leaderboard_url,
            "https://www.tbench.ai/leaderboard/terminal-bench/2.1"
        );
        assert!(
            profile
                .comparable_constraints
                .iter()
                .any(|item| item.contains("timeout") && item.contains("resource")),
            "profile must encode official comparability constraints"
        );
    }

    #[test]
    fn environment_probe_reports_missing_harbor_and_docker_as_blockers() {
        let probe = probe_environment_from_command_results(
            "terminal-bench-2.1",
            ProbeCommandResult::missing("harbor"),
            ProbeCommandResult::missing("docker"),
        )
        .expect("probe should support Terminal-Bench 2.1");

        assert!(!probe.ready);
        assert!(probe.blockers.iter().any(|item| item.contains("harbor")));
        assert!(probe.blockers.iter().any(|item| item.contains("Docker")));
        assert_eq!(
            probe.command_preview,
            "harbor run -d terminal-bench/terminal-bench-2-1 --agent-import-path codefactory_bench.agent:CodeFactoryAgent -m <model> -l 5"
        );

        let ready_probe = probe_environment_from_command_results(
            "terminal-bench-2.1",
            ProbeCommandResult::ok("harbor", "harbor help"),
            ProbeCommandResult::ok("docker", "Docker running"),
        )
        .expect("ready probe");
        assert!(ready_probe.ready);
    }

    #[tokio::test]
    async fn import_harbor_job_persists_run_trials_and_failure_classes() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        ensure_schema(&pool).await.expect("benchmark schema");

        let job_dir = temp_job_dir();
        fs::write(
            job_dir.join("config.json"),
            json!({
                "dataset": "terminal-bench/terminal-bench-2-1",
                "agent": "codefactory",
                "model": "gpt-5",
                "command": "harbor run -d terminal-bench/terminal-bench-2-1 --agent-import-path codefactory_bench.agent:CodeFactoryAgent -m gpt-5 -l 2",
                "metadata": {
                    "codefactory_version": "1.40.0",
                    "codefactory_git_sha": "abc123",
                    "policy_preset": "benchmark-sandbox"
                }
            })
            .to_string(),
        )
        .expect("write config");
        fs::write(
            job_dir.join("result.json"),
            json!({
                "status": "completed",
                "reward": 0.5,
                "finished_at": "2026-06-27T10:00:00Z"
            })
            .to_string(),
        )
        .expect("write result");

        let pass_dir = job_dir.join("trials").join("task-pass");
        fs::create_dir_all(&pass_dir).expect("create pass trial");
        fs::write(
            pass_dir.join("config.json"),
            json!({ "task_name": "task-pass", "category": "coding" }).to_string(),
        )
        .expect("write pass config");
        fs::write(
            pass_dir.join("result.json"),
            json!({ "reward": 1.0 }).to_string(),
        )
        .expect("write pass result");

        let fail_dir = job_dir.join("trials").join("task-fail");
        fs::create_dir_all(fail_dir.join("verifier")).expect("create fail trial");
        fs::write(
            fail_dir.join("config.json"),
            json!({ "task_name": "task-fail", "category": "shell" }).to_string(),
        )
        .expect("write fail config");
        fs::write(
            fail_dir.join("result.json"),
            json!({ "reward": 0.0, "duration_ms": 42000 }).to_string(),
        )
        .expect("write fail result");
        fs::write(
            fail_dir.join("verifier").join("test-stderr.txt"),
            "Permission denied while writing outside workspace",
        )
        .expect("write verifier stderr");

        let imported = import_harbor_job(&pool, &job_dir)
            .await
            .expect("import fake Harbor job");

        assert_eq!(imported.run.dataset, "terminal-bench/terminal-bench-2-1");
        assert!(imported.run.comparable);
        assert_eq!(imported.trials.len(), 2);
        let failed = imported
            .trials
            .iter()
            .find(|trial| trial.task_name == "task-fail")
            .expect("failed trial imported");
        assert_eq!(failed.reward, 0.0);
        assert_eq!(failed.failure_class.as_deref(), Some("policy"));

        let persisted_trials: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM benchmark_trials WHERE run_id = ?")
                .bind(&imported.run.id)
                .fetch_one(&pool)
                .await
                .expect("count persisted trials");
        assert_eq!(persisted_trials, 2);
    }

    #[tokio::test]
    async fn import_harbor_015_job_structure_without_trials_subdir() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        ensure_schema(&pool).await.expect("benchmark schema");

        let job_dir = temp_job_dir();
        fs::write(
            job_dir.join("config.json"),
            json!({
                "job_name": "cf-tb21-oracle-smoke",
                "n_concurrent_trials": 1,
                "agents": [
                    { "name": "oracle", "model_name": null }
                ],
                "datasets": [
                    {
                        "name": "terminal-bench/terminal-bench-2-1",
                        "ref": "sha256:dataset-ref",
                        "n_tasks": 1
                    }
                ]
            })
            .to_string(),
        )
        .expect("write job config");
        fs::write(
            job_dir.join("result.json"),
            json!({
                "id": "job-id",
                "started_at": "2026-06-27T03:13:08.536198Z",
                "finished_at": "2026-06-27T03:17:19.982607Z",
                "stats": {
                    "n_completed_trials": 1,
                    "n_errored_trials": 0
                }
            })
            .to_string(),
        )
        .expect("write job result");

        let trial_dir = job_dir.join("write-compressor__abc123");
        fs::create_dir_all(trial_dir.join("verifier")).expect("create trial");
        fs::write(
            trial_dir.join("config.json"),
            json!({
                "task": {
                    "name": "terminal-bench/write-compressor",
                    "source": "terminal-bench/terminal-bench-2-1"
                },
                "trial_name": "write-compressor__abc123",
                "agent": { "name": "oracle" }
            })
            .to_string(),
        )
        .expect("write trial config");
        fs::write(
            trial_dir.join("result.json"),
            json!({
                "id": "trial-id",
                "task_name": "terminal-bench/write-compressor",
                "trial_name": "write-compressor__abc123",
                "source": "terminal-bench/terminal-bench-2-1",
                "verifier_result": {
                    "rewards": { "reward": 1.0 }
                },
                "started_at": "2026-06-27T03:13:08.826518Z",
                "finished_at": "2026-06-27T03:17:19.980929Z"
            })
            .to_string(),
        )
        .expect("write trial result");
        fs::write(trial_dir.join("verifier").join("reward.txt"), "1.0").expect("write reward");
        fs::write(trial_dir.join("verifier").join("test-stdout.txt"), "passed")
            .expect("write stdout");

        let imported = import_harbor_job(&pool, &job_dir)
            .await
            .expect("import real Harbor layout");

        assert_eq!(imported.run.id, "job-id");
        assert_eq!(imported.run.dataset, "terminal-bench/terminal-bench-2-1");
        assert_eq!(imported.run.agent_name, "oracle");
        assert_eq!(
            imported.run.dataset_version.as_deref(),
            Some("sha256:dataset-ref")
        );
        assert!(imported.run.comparable);
        assert_eq!(imported.trials.len(), 1);
        assert_eq!(
            imported.trials[0].task_name,
            "terminal-bench/write-compressor"
        );
        assert_eq!(imported.trials[0].reward, 1.0);
        assert_eq!(imported.trials[0].failure_class, None);
        assert!(
            imported.trials[0].duration_ms.unwrap_or_default() > 0,
            "duration should be derived from Harbor started_at/finished_at"
        );
    }

    #[tokio::test]
    async fn import_harbor_custom_agent_identity_from_trial_agent_info() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        ensure_schema(&pool).await.expect("benchmark schema");

        let job_dir = temp_job_dir();
        fs::write(
            job_dir.join("config.json"),
            json!({
                "job_name": "cf-tb21-codefactory-baseline",
                "agents": [
                    {
                        "name": null,
                        "import_path": "codefactory_bench.agent:CodeFactoryAgent",
                        "model_name": null
                    }
                ],
                "datasets": [
                    {
                        "name": "terminal-bench/terminal-bench-2-1",
                        "ref": "sha256:dataset-ref",
                        "n_tasks": 1
                    }
                ]
            })
            .to_string(),
        )
        .expect("write job config");
        fs::write(
            job_dir.join("result.json"),
            json!({
                "id": "job-id",
                "started_at": "2026-06-27T03:42:47.040459Z",
                "finished_at": "2026-06-27T03:43:51.386251Z"
            })
            .to_string(),
        )
        .expect("write job result");

        let trial_dir = job_dir.join("write-compressor__abc123");
        fs::create_dir_all(trial_dir.join("verifier")).expect("create trial");
        fs::write(
            trial_dir.join("config.json"),
            json!({
                "task": {
                    "name": "terminal-bench/write-compressor",
                    "source": "terminal-bench/terminal-bench-2-1"
                },
                "trial_name": "write-compressor__abc123",
                "agent": {
                    "name": null,
                    "import_path": "codefactory_bench.agent:CodeFactoryAgent"
                }
            })
            .to_string(),
        )
        .expect("write trial config");
        fs::write(
            trial_dir.join("result.json"),
            json!({
                "id": "trial-id",
                "task_name": "terminal-bench/write-compressor",
                "source": "terminal-bench/terminal-bench-2-1",
                "agent_info": {
                    "name": "codefactory-headless-baseline",
                    "version": "1.40.0",
                    "model_info": null
                },
                "verifier_result": {
                    "rewards": { "reward": 0.0 }
                }
            })
            .to_string(),
        )
        .expect("write trial result");
        fs::write(trial_dir.join("verifier").join("reward.txt"), "0").expect("write reward");
        fs::write(trial_dir.join("verifier").join("test-stdout.txt"), "failed")
            .expect("write stdout");

        let imported = import_harbor_job(&pool, &job_dir)
            .await
            .expect("import custom Harbor agent layout");

        assert_eq!(imported.run.agent_name, "codefactory-headless-baseline");
        assert_eq!(imported.run.agent_version.as_deref(), Some("1.40.0"));
        assert_eq!(imported.trials.len(), 1);
        assert_eq!(imported.trials[0].reward, 0.0);
    }

    #[test]
    fn failure_classifier_separates_model_provider_errors_from_agent_planning() {
        let failure = classify_failure(
            0.0,
            r#"RuntimeError: model request failed: HTTP 402: {"error":{"message":"Insufficient Balance"}}"#,
        );

        assert_eq!(failure.as_deref(), Some("model-provider"));
    }

    #[test]
    fn provider_bridge_preview_uses_current_deepseek_without_exposing_secret() {
        let settings = settings_with_deepseek_endpoint();
        let preview = preview_provider_bridge(
            &settings,
            &BenchmarkProviderBridgeRequest {
                profile_id: TERMINAL_BENCH_21_PROFILE_ID.to_string(),
                endpoint_name: None,
                model: None,
                task_limit: Some(1),
                concurrency: Some(3),
                trial_count: Some(1),
                job_root: Some("/tmp/cf-bench".to_string()),
                job_name: Some("deepseek-smoke".to_string()),
                adapter_root: Some("/repo".to_string()),
            },
        )
        .expect("preview");

        assert_eq!(preview.endpoint_name, "deepseek");
        assert_eq!(preview.base_url, "https://api.deepseek.com");
        assert_eq!(preview.model, "deepseek-v4-flash");
        assert_eq!(preview.key_ref, "codefactory.endpoint.deepseek");
        assert_eq!(preview.concurrency, 3);
        assert_eq!(preview.trial_count, 1);
        assert!(preview.ready);
        assert!(preview
            .env_preview
            .iter()
            .any(|item| item.name == "CODEFACTORY_BENCH_API_KEY"
                && item.secret
                && item.value == "<redacted:codefactory.endpoint.deepseek>"));
        assert!(!preview.command_preview.contains("test-secret"));
        assert!(preview
            .command_preview
            .contains("CODEFACTORY_BENCH_API_KEY='<redacted:codefactory.endpoint.deepseek>'"));
        assert!(preview.command_preview.contains("-m deepseek-v4-flash"));
        assert!(preview.command_preview.contains("-n 3"));
    }

    #[test]
    fn provider_bridge_requires_authorization_before_secret_lookup() {
        let settings = settings_with_deepseek_endpoint();
        let request = StartBenchmarkProviderRunRequest {
            bridge: BenchmarkProviderBridgeRequest {
                profile_id: TERMINAL_BENCH_21_PROFILE_ID.to_string(),
                endpoint_name: None,
                model: None,
                task_limit: Some(1),
                concurrency: None,
                trial_count: Some(1),
                job_root: Some("/tmp/cf-bench".to_string()),
                job_name: Some("deepseek-smoke".to_string()),
                adapter_root: Some("/repo".to_string()),
            },
            authorization_phrase: "wrong".to_string(),
        };
        let err = resolve_authorized_provider_launch(&settings, &request, |_key_ref| {
            panic!("secret lookup must not run before authorization is valid");
        })
        .expect_err("invalid authorization should fail");

        assert!(err
            .to_string()
            .contains("Benchmark provider authorization phrase did not match"));
    }

    #[test]
    fn provider_bridge_authorized_launch_injects_secret_only_into_child_env() {
        let settings = settings_with_deepseek_endpoint();
        let bridge = BenchmarkProviderBridgeRequest {
            profile_id: TERMINAL_BENCH_21_PROFILE_ID.to_string(),
            endpoint_name: None,
            model: None,
            task_limit: Some(1),
            concurrency: None,
            trial_count: Some(1),
            job_root: Some("/tmp/cf-bench".to_string()),
            job_name: Some("deepseek-smoke".to_string()),
            adapter_root: Some("/repo".to_string()),
        };
        let preview = preview_provider_bridge(&settings, &bridge).expect("preview");
        let launch = resolve_authorized_provider_launch(
            &settings,
            &StartBenchmarkProviderRunRequest {
                bridge,
                authorization_phrase: preview.authorization_phrase.clone(),
            },
            |key_ref| {
                assert_eq!(key_ref, "codefactory.endpoint.deepseek");
                Ok(Some("test-secret".to_string()))
            },
        )
        .expect("authorized launch");

        assert_eq!(
            launch.env_value("CODEFACTORY_BENCH_API_KEY"),
            Some("test-secret")
        );
        assert_eq!(
            launch.env_value("CODEFACTORY_BENCH_BASE_URL"),
            Some("https://api.deepseek.com")
        );
        assert_eq!(
            launch.env_value("CODEFACTORY_BENCH_MODEL"),
            Some("deepseek-v4-flash")
        );
        assert!(!launch.preview.command_preview.contains("test-secret"));
        assert!(!launch
            .preview
            .env_preview
            .iter()
            .any(|item| item.value.contains("test-secret")));
    }

    fn settings_with_deepseek_endpoint() -> crate::config::settings::Settings {
        use std::collections::HashMap;

        let mut endpoints = HashMap::new();
        endpoints.insert(
            "deepseek".to_string(),
            crate::config::settings::Endpoint {
                base_url: "https://api.deepseek.com".to_string(),
                key_ref: Some("codefactory.endpoint.deepseek".to_string()),
                api_style: crate::config::settings::ApiStyle::Openai,
                custom_models: vec![],
                active_model: Some("deepseek/deepseek-v4-flash".to_string()),
            },
        );

        crate::config::settings::Settings {
            endpoints,
            default_endpoint: "deepseek".to_string(),
            default_model: "deepseek/deepseek-v4-flash".to_string(),
            ..crate::config::settings::Settings::default()
        }
    }

    #[tokio::test]
    #[ignore]
    async fn provider_bridge_runs_real_codefactory_endpoint_from_local_settings() {
        assert_eq!(
            std::env::var("CODEFACTORY_RUN_REAL_PROVIDER_BRIDGE").as_deref(),
            Ok("1"),
            "set CODEFACTORY_RUN_REAL_PROVIDER_BRIDGE=1 to run a real provider-backed Terminal-Bench smoke"
        );

        let repo_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("repo root")
            .to_path_buf();
        let endpoint_name =
            std::env::var("CODEFACTORY_BENCH_ENDPOINT").unwrap_or_else(|_| "deepseek".to_string());
        let model = std::env::var("CODEFACTORY_BENCH_MODEL_OVERRIDE").ok();
        let task_limit = env_u32("CODEFACTORY_BENCH_TASK_LIMIT", 1);
        let concurrency = env_u32(
            "CODEFACTORY_BENCH_CONCURRENCY",
            env_u32("CODEFACTORY_BENCH_TRIAL_COUNT", 4),
        );
        let job_root = std::env::var("CODEFACTORY_BENCH_JOB_ROOT").unwrap_or_else(|_| {
            repo_root
                .join(".codefactory/benchmark-jobs")
                .to_string_lossy()
                .to_string()
        });
        let job_name = format!(
            "cf-tb21-codefactory-provider-{}-{}",
            endpoint_name,
            Utc::now().format("%Y%m%d-%H%M%S")
        );
        let bridge = BenchmarkProviderBridgeRequest {
            profile_id: TERMINAL_BENCH_21_PROFILE_ID.to_string(),
            endpoint_name: Some(endpoint_name),
            model,
            task_limit: Some(task_limit),
            concurrency: Some(concurrency),
            trial_count: None,
            job_root: Some(job_root),
            job_name: Some(job_name),
            adapter_root: Some(repo_root.to_string_lossy().to_string()),
        };
        let settings = crate::config::settings::load();
        let preview = preview_provider_bridge(&settings, &bridge).expect("preview provider bridge");
        println!(
            "provider_bridge_preview endpoint={} base_url={} model={} key_ref={} agent={} task_limit={} concurrency={} trial_count={} job_path={}",
            preview.endpoint_name,
            preview.base_url,
            preview.model,
            preview.key_ref,
            preview.agent_import_path,
            preview.task_limit,
            preview.concurrency,
            preview.trial_count,
            preview.job_path
        );
        assert!(
            preview.ready,
            "provider bridge preview must be ready: {:?}",
            preview.blockers
        );

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        ensure_schema(&pool).await.expect("benchmark schema");

        let result = start_provider_benchmark_run(
            &pool,
            &settings,
            StartBenchmarkProviderRunRequest {
                bridge,
                authorization_phrase: preview.authorization_phrase.clone(),
            },
        )
        .await
        .expect("start provider benchmark run");

        println!(
            "provider_bridge_result status={} exit_code={:?} job_path={}",
            result.status, result.exit_code, result.preview.job_path
        );
        if result.status != "completed" {
            println!(
                "provider_bridge_stdout_tail:\n{}",
                redacted_log_tail(&result.stdout)
            );
            println!(
                "provider_bridge_stderr_tail:\n{}",
                redacted_log_tail(&result.stderr)
            );
        }
        assert_eq!(result.status, "completed", "Harbor provider run failed");

        let imported = result.imported.expect("completed run should be imported");
        let total_reward: f64 = imported.trials.iter().map(|trial| trial.reward).sum();
        let mean_reward = total_reward / imported.trials.len().max(1) as f64;
        println!(
            "provider_bridge_imported run={} dataset={} agent={} model={:?} comparable={} trials={} mean_reward={:.3}",
            imported.run.id,
            imported.run.dataset,
            imported.run.agent_name,
            imported.run.model,
            imported.run.comparable,
            imported.trials.len(),
            mean_reward
        );
        for trial in &imported.trials {
            println!(
                "provider_bridge_trial task={} reward={} failure_class={:?}",
                trial.task_name, trial.reward, trial.failure_class
            );
        }

        assert_eq!(imported.run.dataset, TERMINAL_BENCH_21_DATASET);
        assert_eq!(imported.run.agent_name, "codefactory-headless");
        assert!(
            !imported.trials.is_empty(),
            "real provider-backed Harbor job must include trial rows"
        );
    }

    fn env_u32(name: &str, default: u32) -> u32 {
        std::env::var(name)
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(default)
    }

    fn redacted_log_tail(text: &str) -> String {
        let filtered = text
            .lines()
            .filter(|line| {
                let lower = line.to_ascii_lowercase();
                !(lower.contains("authorization")
                    || lower.contains("api_key")
                    || lower.contains("bearer ")
                    || lower.contains("codefactory_bench_api_key"))
            })
            .collect::<Vec<_>>()
            .join("\n");
        tail_text(&filtered, 4000)
    }

    #[tokio::test]
    #[ignore]
    async fn import_harbor_job_from_env_path() {
        let job_path = std::env::var("CODEFACTORY_BENCHMARK_JOB_PATH")
            .expect("set CODEFACTORY_BENCHMARK_JOB_PATH to a Harbor job directory");
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("open in-memory db");
        ensure_schema(&pool).await.expect("benchmark schema");

        let imported = import_harbor_job(&pool, std::path::Path::new(&job_path))
            .await
            .expect("import Harbor job from env path");

        println!(
            "imported run={} dataset={} agent={} comparable={} trials={}",
            imported.run.id,
            imported.run.dataset,
            imported.run.agent_name,
            imported.run.comparable,
            imported.trials.len()
        );
        for trial in &imported.trials {
            println!(
                "trial={} reward={} failure_class={:?}",
                trial.task_name, trial.reward, trial.failure_class
            );
        }

        assert_eq!(imported.run.dataset, TERMINAL_BENCH_21_DATASET);
        assert!(
            !imported.trials.is_empty(),
            "real Harbor job import must include trial rows"
        );
    }
}
