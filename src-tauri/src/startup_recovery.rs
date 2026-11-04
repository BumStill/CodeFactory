// SPDX-License-Identifier: Apache-2.0
//! M233: keep historical recovery off the native window event loop.
use std::{path::PathBuf, time::{Duration, Instant}};

pub fn spawn<F>(data_dir: PathBuf, work: F)
where F: FnOnce() -> Result<(), Box<dyn std::error::Error>> + Send + 'static {
    spawn_worker(data_dir, work);
}

fn spawn_worker<F>(data_dir: PathBuf, work: F) -> tauri::async_runtime::JoinHandle<()>
where F: FnOnce() -> Result<(), Box<dyn std::error::Error>> + Send + 'static {
    // A dedicated blocking worker also isolates legacy synchronous filesystem
    // probes from Tokio's async workers. Admission stays inside `work`, after
    // settlement; never start a recovery supervisor from the setup thread.
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        record(&data_dir, "recovery", "started", Duration::ZERO);
        match work() {
            Ok(()) => record(&data_dir, "recovery", "completed", started.elapsed()),
            Err(error) => {
                record(&data_dir, "recovery", "failed", started.elapsed());
                tracing::error!(%error, "startup recovery failed; recovery admission remains closed");
            }
        }
    })
}

pub fn record(dir: &std::path::Path, step: &str, status: &str, elapsed: Duration) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("startup.log")) {
        let _ = writeln!(file, "{} step={step} status={status} elapsed_ms={}", chrono::Local::now().to_rfc3339(), elapsed.as_millis());
    }
    tracing::info!(step, status, elapsed_ms = elapsed.as_millis(), "startup stage");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::no_window::NoWindow;

    #[tokio::test]
    async fn m233_slow_history_worker_leaves_command_queries_responsive_and_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        // Synthetic durable history, never the user's database.
        for index in 0..320 {
            sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at) VALUES (?, 'synthetic', '/synthetic', 'synthetic', 1, 1)")
                .bind(format!("m233-{index}")).execute(&pool).await.unwrap();
        }
        let worker_pool = pool.clone();
        // A synthetic git executable avoids touching PATH or a user's repos.
        // Its public process API is deliberately slow, on both Windows and Mac.
        let fake_source = dir.path().join("slow_git.rs");
        std::fs::write(&fake_source, "fn main() { std::thread::sleep(std::time::Duration::from_millis(10)); }").unwrap();
        let fake_git = dir.path().join(if cfg!(windows) { "git.exe" } else { "git" });
        let compiled = std::process::Command::new("rustc").no_window().arg(&fake_source).arg("-o").arg(&fake_git).output().unwrap();
        assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));
        for index in 0..320 { std::fs::create_dir(dir.path().join(format!("workspace-{index}"))).unwrap(); }
        let workspace_root = dir.path().to_path_buf();
        let started = Instant::now();
        let handle = spawn_worker(dir.path().into(), move || {
            tauri::async_runtime::block_on(async {
                let sessions: Vec<String> = sqlx::query_scalar("SELECT id FROM sessions WHERE id LIKE 'm233-%'")
                    .fetch_all(&worker_pool).await?;
                for (index, _session) in sessions.into_iter().enumerate() {
                    let mut command = std::process::Command::new(&fake_git).no_window();
                    command.current_dir(workspace_root.join(format!("workspace-{index}")))
                        .args(["status", "--porcelain=v1", "--untracked-files=all"]);
                    crate::agent::delivery::bounded_delivery_output(&mut command).unwrap();
                }
                sqlx::query("UPDATE sessions SET title='settled' WHERE id LIKE 'm233-%'")
                    .execute(&worker_pool).await?;
                Ok::<(), sqlx::Error>(())
            })?;
            Ok(())
        });
        assert!(started.elapsed() < Duration::from_secs(2));
        let count: i64 = tokio::time::timeout(Duration::from_secs(2),
            sqlx::query_scalar("SELECT count(*) FROM sessions WHERE title='synthetic'").fetch_one(&pool)
        ).await.unwrap().unwrap();
        assert_eq!(count, 320, "commands run before slow recovery finishes");
        let command_ms = started.elapsed().as_millis();
        handle.await.unwrap();
        eprintln!("M233 synthetic history=320 setup_return_and_query_ms={command_ms} background_total_ms={}", started.elapsed().as_millis());
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE title='settled'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(count, 320);
        let log = std::fs::read_to_string(dir.path().join("startup.log")).unwrap();
        assert!(log.contains("step=recovery status=completed elapsed_ms="));
    }

    #[tokio::test]
    async fn m233_startup_schema_defers_fts_without_losing_upgrade_data() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite:{}", dir.path().join("startup.db").display());
        let pool = crate::storage::db::connect_startup(&url).await.unwrap();
        let exists: i64 = sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE name='knowledge_fts'")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(exists, 0, "FTS backfill is not in the native setup connection");
        crate::knowledge::ensure_schema(&pool).await.unwrap();
        let exists: i64 = sqlx::query_scalar("SELECT count(*) FROM sqlite_master WHERE name='knowledge_fts'")
            .fetch_one(&pool).await.unwrap();
        let library = crate::knowledge::KnowledgeLibrary {
            id: "m233-unreachable".into(), name: "synthetic".into(),
            root_path: dir.path().join("unreachable").to_string_lossy().into(),
            enabled: true, created_at: "synthetic".into(), last_scan_at: None, scan_status: "idle".into(),
        };
        sqlx::query("INSERT INTO knowledge_libraries(id,name,root_path,created_at) VALUES (?,?,?,?)")
            .bind(&library.id).bind(&library.name).bind(&library.root_path).bind(&library.created_at).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO knowledge_documents(id,library_id,path,kind,hash,mtime,size,status,updated_at) VALUES ('m233-document',?,'synthetic.docx','docx','h',1,1,'indexed','synthetic')")
            .bind(&library.id).execute(&pool).await.unwrap();
        assert!(crate::knowledge::scan_library(&pool, &library.id).await.is_err());
        let retained: i64 = sqlx::query_scalar("SELECT count(*) FROM knowledge_documents WHERE id='m233-document'").fetch_one(&pool).await.unwrap();
        assert_eq!(retained, 1, "an unreachable root must not erase the existing index");
        assert_eq!(exists, 1);
    }
}
