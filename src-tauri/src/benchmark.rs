// SPDX-License-Identifier: Apache-2.0

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Row, SqlitePool};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

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
    pub default_smoke_k: u32,
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
        default_smoke_k: 5,
    }
}

pub fn list_profiles() -> Vec<BenchmarkProfile> {
    vec![terminal_bench_21_profile()]
}

fn profile_by_id(profile_id: &str) -> Result<BenchmarkProfile> {
    list_profiles()
        .into_iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| AppError::Other(format!("Unknown benchmark profile: {profile_id}")))
}

pub fn probe_environment(profile_id: &str) -> Result<BenchmarkEnvironmentProbe> {
    let harbor = probe_command("harbor", &["--help"]);
    let docker = probe_command("docker", &["info"]);
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
        "harbor run -d {} -a {} -m <model> -k {}",
        profile.dataset, CODEFACTORY_HARBOR_AGENT, profile.default_smoke_k
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

fn probe_command(binary: &str, args: &[&str]) -> ProbeCommandResult {
    match Command::new(binary).no_window().args(args).output() {
        Ok(output) if output.status.success() => ProbeCommandResult {
            binary: binary.to_string(),
            available: true,
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            detail: format!("{binary} is available"),
        },
        Ok(output) => ProbeCommandResult::warning(
            binary,
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ),
        Err(_) => ProbeCommandResult::missing(binary),
    }
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
    let now = Utc::now().to_rfc3339();

    let dataset = json_string(&config, &["dataset"])
        .or_else(|| json_string(&config, &["dataset_name"]))
        .unwrap_or_else(|| profile.dataset.clone());
    let command = json_string(&config, &["command"]).unwrap_or_else(|| {
        format!(
            "harbor run -d {} -a {} -m <model> -k {}",
            profile.dataset, CODEFACTORY_HARBOR_AGENT, profile.default_smoke_k
        )
    });
    let agent_name = json_string(&config, &["agent"])
        .or_else(|| json_string(&config, &["agent_name"]))
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
        dataset_version: json_string(&config, &["dataset_version"]),
        agent_name,
        agent_version: json_string(&config, &["agent_version"]),
        model: json_string(&config, &["model"]),
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
    let trials_dir = job_path.join("trials");
    if !trials_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut entries: Vec<_> = fs::read_dir(&trials_dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .collect();
    entries.sort_by_key(|entry| entry.file_name());

    let mut trials = Vec::new();
    for entry in entries {
        let trial_dir = entry.path();
        let fallback_task_name = entry.file_name().to_string_lossy().to_string();
        let mut missing = Vec::new();
        let config = read_json_file(trial_dir.join("config.json"), &mut missing)?;
        let result = read_json_file(trial_dir.join("result.json"), &mut missing)?;
        let task_name = json_string(&config, &["task_name"])
            .or_else(|| json_string(&config, &["name"]))
            .unwrap_or(fallback_task_name);
        let reward = json_f64(&result, &["reward"])
            .or_else(|| json_f64(&result, &["result", "reward"]))
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
        ]
        .join("\n");

        trials.push(BenchmarkTrialRecord {
            id: Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            task_name,
            category: json_string(&config, &["category"]),
            difficulty: json_string(&config, &["difficulty"]),
            reward,
            duration_ms: json_i64(&result, &["duration_ms"])
                .or_else(|| json_i64(&result, &["duration"])),
            error_kind: json_string(&result, &["error_kind"]),
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
        current = current.get(segment)?;
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
        current = current.get(segment)?;
    }
    current
        .as_f64()
        .or_else(|| current.as_i64().map(|n| n as f64))
}

fn json_i64(value: &Value, path: &[&str]) -> Option<i64> {
    let mut current = value;
    for segment in path {
        current = current.get(segment)?;
    }
    current.as_i64()
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
            "harbor run -d terminal-bench/terminal-bench-2-1 -a codefactory_bench.agent:CodeFactoryAgent -m <model> -k 5"
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
                "command": "harbor run -d terminal-bench/terminal-bench-2-1 -a codefactory_bench.agent:CodeFactoryAgent -m gpt-5 -k 2",
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
}
