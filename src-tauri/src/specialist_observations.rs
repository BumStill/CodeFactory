// SPDX-License-Identifier: Apache-2.0
//! Host-owned read-observe state only. No execution permission or autonomous agent.
use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};

#[async_trait::async_trait]
pub trait ReadSource: Send + Sync {
    async fn get(&self, path: &str) -> Result<Value>;
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Snapshot {
    pub repo: String,
    pub repo_id: i64,
    pub actor_id: i64,
    pub pr: i64,
    pub head: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Binding {
    pub id: String,
    pub project: String,
    pub remote_id: String,
    pub epoch: i64,
    pub credential_revision: String,
    pub role: String,
    #[serde(flatten)]
    pub target: Snapshot,
}
impl std::ops::Deref for Binding {
    type Target = Snapshot;
    fn deref(&self) -> &Snapshot {
        &self.target
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CiRun {
    pub id: i64,
    pub attempt: i64,
    pub workflow_id: i64,
    pub status: String,
    pub conclusion: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub observed_at: String,
    pub pm_state: String,
    pub runs: Vec<CiRun>,
}
#[derive(Serialize)]
pub struct RecordView {
    pub binding: Binding,
    pub active: bool,
    pub observations: Vec<Observation>,
}
pub fn digest(value: &impl Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn positive(v: &Value) -> Result<i64> {
    let n = v.as_i64().unwrap_or(0);
    ensure!(n > 0, "invalid numeric identity");
    Ok(n)
}
fn short(v: &Value) -> Result<String> {
    let s = v.as_str().unwrap_or("");
    ensure!(!s.is_empty() && s.len() <= 128, "invalid status");
    Ok(s.into())
}
pub fn validate_repo(repo: &str) -> Result<()> {
    let parts: Vec<_> = repo.split('/').collect();
    ensure!(
        parts.len() == 2
            && parts.iter().all(|p| !p.is_empty()
                && p.len() <= 100
                && *p != "."
                && *p != ".."
                && p.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))),
        "invalid repository"
    );
    Ok(())
}
pub async fn snapshot(source: &impl ReadSource, repo: &str, pr: i64) -> Result<Snapshot> {
    validate_repo(repo)?;
    ensure!((1..=i32::MAX as i64).contains(&pr), "invalid PR");
    let actor = source.get("/user").await?;
    let actor_id = positive(&actor["id"])?;
    let r = source.get(&format!("/repos/{repo}")).await?;
    let repo_id = positive(&r["id"])?;
    ensure!(r["permissions"]["pull"] == true, "read permission missing");
    let name = r["full_name"].as_str().unwrap_or("");
    ensure!(name.eq_ignore_ascii_case(repo), "repository changed");
    validate_repo(name)?;
    let p = source.get(&format!("/repos/{repo}/pulls/{pr}")).await?;
    ensure!(
        p["number"].as_i64() == Some(pr) && p["state"] == "open",
        "PR not open"
    );
    ensure!(
        p["base"]["repo"]["id"].as_i64() == Some(repo_id)
            && p["head"]["repo"]["id"].as_i64() == Some(repo_id),
        "fork or repository mismatch"
    );
    let head = p["head"]["sha"].as_str().unwrap_or("");
    ensure!(
        head.len() == 40
            && head
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
        "invalid head"
    );
    Ok(Snapshot {
        repo: name.into(),
        repo_id,
        actor_id,
        pr,
        head: head.into(),
    })
}
pub async fn observe(source: &impl ReadSource, b: &Binding) -> Result<Vec<CiRun>> {
    ensure!(
        snapshot(source, &b.repo, b.pr).await? == b.target,
        "binding identity or head changed"
    );
    let data = source
        .get(&format!(
            "/repos/{}/actions/runs?head_sha={}&per_page=100&page=1",
            b.repo, b.head
        ))
        .await?;
    let rows = data["workflow_runs"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("missing run list"))?;
    ensure!(
        rows.len() <= 100 && data["total_count"].as_u64() == Some(rows.len() as u64),
        "incomplete run list"
    );
    let mut runs = Vec::new();
    for r in rows {
        ensure!(
            r["head_sha"] == b.head
                && r["repository"]["id"].as_i64() == Some(b.repo_id)
                && r["head_repository"]["id"].as_i64() == Some(b.repo_id),
            "run source mismatch"
        );
        runs.push(CiRun {
            id: positive(&r["id"])?,
            attempt: positive(&r["run_attempt"])?,
            workflow_id: positive(&r["workflow_id"])?,
            status: short(&r["status"])?,
            conclusion: if r["conclusion"].is_null() {
                None
            } else {
                Some(short(&r["conclusion"])?)
            },
        });
    }
    runs.sort_by_key(|r| (r.id, r.attempt));
    ensure!(
        !runs.windows(2).any(|r| r[0].id == r[1].id),
        "duplicate runs"
    );
    ensure!(
        snapshot(source, &b.repo, b.pr).await? == b.target,
        "source changed during observation"
    );
    Ok(runs)
}
fn normalize_sql(sql: &str) -> String {
    sql.to_ascii_lowercase()
        .replace("if not exists", "")
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ';')
        .collect()
}
pub async fn ensure_schema(pool: &SqlitePool) -> Result<()> {
    let source = include_str!("../migrations/0027_specialist_observations.sql");
    let source = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("--"))
        .collect::<Vec<_>>()
        .join("\n");
    let statements: Vec<String> = source
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    let expected: Vec<String> = statements.iter().map(|s| normalize_sql(s)).collect();
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    // BEGIN IMMEDIATE reserves writes before any schema inspection.
    let had_marker: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_master WHERE lower(name)='specialist_schema'",
    )
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("CREATE TABLE IF NOT EXISTS specialist_schema(version INTEGER PRIMARY KEY CHECK(version=1))").execute(&mut *tx).await?;
    let existing: Vec<String> = sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE (lower(name) GLOB 'specialist_*' OR lower(tbl_name) GLOB 'specialist_*') AND lower(name) != 'specialist_schema' AND sql IS NOT NULL").fetch_all(&mut *tx).await?;
    ensure!(
        had_marker == 0 || !existing.is_empty(),
        "partial specialist schema"
    );
    if !existing.is_empty() {
        ensure!(
            existing.len() == expected.len()
                && existing
                    .iter()
                    .all(|sql| expected.contains(&normalize_sql(sql))),
            "unsupported specialist schema"
        );
    }
    let meta: String =
        sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE name='specialist_schema'")
            .fetch_one(&mut *tx)
            .await?;
    ensure!(
        normalize_sql(&meta)
            == normalize_sql(
                "CREATE TABLE specialist_schema(version INTEGER PRIMARY KEY CHECK(version=1))"
            ),
        "unsupported schema marker"
    );
    if had_marker != 0 {
        let versions: Vec<i64> = sqlx::query_scalar("SELECT version FROM specialist_schema")
            .fetch_all(&mut *tx)
            .await?;
        ensure!(versions == vec![1], "unsupported schema version");
    }
    sqlx::query("INSERT OR IGNORE INTO specialist_schema(version) VALUES(1)")
        .execute(&mut *tx)
        .await?;
    for statement in statements {
        sqlx::query(&statement).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}
pub async fn project_epoch(pool: &SqlitePool, project: &str) -> Result<i64> {
    sqlx::query("INSERT OR IGNORE INTO specialist_projects(project,epoch) VALUES(?,0)")
        .bind(project)
        .execute(pool)
        .await?;
    Ok(
        sqlx::query_scalar("SELECT epoch FROM specialist_projects WHERE project=?")
            .bind(project)
            .fetch_one(pool)
            .await?,
    )
}
async fn fence(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    project: &str,
    epoch: i64,
) -> Result<()> {
    let n = sqlx::query("UPDATE specialist_projects SET epoch=epoch WHERE project=? AND epoch=?")
        .bind(project)
        .bind(epoch)
        .execute(&mut **tx)
        .await?
        .rows_affected();
    ensure!(n == 1, "project authorization revoked");
    Ok(())
}
#[cfg(test)]
pub async fn save_binding(
    pool: &SqlitePool,
    project: &str,
    remote: &str,
    epoch: i64,
    snapshot: &Snapshot,
) -> Result<Binding> {
    save_binding_with_revision(pool, project, remote, epoch, snapshot, "").await
}
pub async fn save_binding_with_revision(
    pool: &SqlitePool,
    project: &str,
    remote: &str,
    epoch: i64,
    snapshot: &Snapshot,
    revision: &str,
) -> Result<Binding> {
    let id = digest(&(
        project,
        remote,
        epoch,
        snapshot,
        revision,
        "qa_read_pm_observe",
    ))?;
    let b = Binding {
        id,
        project: project.into(),
        remote_id: remote.into(),
        epoch,
        credential_revision: revision.into(),
        role: "qa_read_pm_observe".into(),
        target: snapshot.clone(),
    };
    let mut tx = pool.begin().await?;
    fence(&mut tx, project, epoch).await?;
    sqlx::query(
        "INSERT OR IGNORE INTO specialist_bindings(id,project,epoch,payload) VALUES(?,?,?,?)",
    )
    .bind(&b.id)
    .bind(project)
    .bind(epoch)
    .bind(serde_json::to_string(&b)?)
    .execute(&mut *tx)
    .await?;
    let stored: String = sqlx::query_scalar("SELECT payload FROM specialist_bindings WHERE id=?")
        .bind(&b.id)
        .fetch_one(&mut *tx)
        .await?;
    ensure!(
        serde_json::from_str::<Binding>(&stored)? == b,
        "binding conflict"
    );
    tx.commit().await?;
    Ok(b)
}
pub async fn load_binding(pool: &SqlitePool, project: &str, id: &str) -> Result<Binding> {
    let payload:Option<String>=sqlx::query_scalar("SELECT b.payload FROM specialist_bindings b JOIN specialist_projects p ON b.project=p.project AND b.epoch=p.epoch WHERE b.project=? AND b.id=?").bind(project).bind(id).fetch_optional(pool).await?;
    match payload {
        Some(p) => Ok(serde_json::from_str(&p)?),
        None => bail!("unknown or revoked binding"),
    }
}
pub async fn save_observation(
    pool: &SqlitePool,
    b: &Binding,
    runs: &[CiRun],
) -> Result<Observation> {
    let id = digest(&(&b.id, runs))?;
    let o = Observation {
        id,
        observed_at: chrono::Utc::now().to_rfc3339(),
        pm_state: "needs_execution_provenance".into(),
        runs: runs.to_vec(),
    };
    let mut tx = pool.begin().await?;
    fence(&mut tx, &b.project, b.epoch).await?;
    let stored: Option<String> =
        sqlx::query_scalar("SELECT payload FROM specialist_bindings WHERE id=? AND project=?")
            .bind(&b.id)
            .bind(&b.project)
            .fetch_optional(&mut *tx)
            .await?;
    ensure!(
        stored
            .as_deref()
            .map(serde_json::from_str::<Binding>)
            .transpose()?
            == Some(b.clone()),
        "binding changed"
    );
    sqlx::query(
        "INSERT OR IGNORE INTO specialist_observations(id,binding_id,payload) VALUES(?,?,?)",
    )
    .bind(&o.id)
    .bind(&b.id)
    .bind(serde_json::to_string(&o)?)
    .execute(&mut *tx)
    .await?;
    let saved: String = sqlx::query_scalar(
        "SELECT payload FROM specialist_observations WHERE id=? AND binding_id=?",
    )
    .bind(&o.id)
    .bind(&b.id)
    .fetch_one(&mut *tx)
    .await?;
    let saved: Observation = serde_json::from_str(&saved)?;
    ensure!(
        saved.id == o.id && saved.runs == o.runs && saved.pm_state == o.pm_state,
        "observation conflict"
    );
    tx.commit().await?;
    Ok(saved)
}
pub async fn stop(pool: &SqlitePool, project: &str) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT OR IGNORE INTO specialist_projects(project,epoch) VALUES(?,0)")
        .bind(project)
        .execute(&mut *tx)
        .await?;
    let changed = sqlx::query("UPDATE specialist_projects SET epoch=epoch+1 WHERE project=? AND epoch<9223372036854775807").bind(project).execute(&mut *tx).await?.rows_affected();
    ensure!(changed == 1, "project epoch exhausted");
    tx.commit().await?;
    Ok(())
}
pub async fn list(pool: &SqlitePool, project: &str) -> Result<Vec<RecordView>> {
    let rows=sqlx::query("SELECT b.payload,b.epoch=p.epoch AS active FROM specialist_bindings b JOIN specialist_projects p ON b.project=p.project WHERE b.project=? ORDER BY b.rowid DESC LIMIT 100").bind(project).fetch_all(pool).await?;
    let mut result = Vec::new();
    for row in rows {
        let binding: Binding = serde_json::from_str(row.try_get("payload")?)?;
        let obs:Vec<String>=sqlx::query_scalar("SELECT payload FROM specialist_observations WHERE binding_id=? ORDER BY rowid DESC LIMIT 20").bind(&binding.id).fetch_all(pool).await?;
        result.push(RecordView {
            binding,
            active: row.try_get("active")?,
            observations: obs
                .iter()
                .map(|s| serde_json::from_str(s))
                .collect::<std::result::Result<_, _>>()?,
        });
    }
    Ok(result)
}
