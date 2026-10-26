// SPDX-License-Identifier: Apache-2.0
//! Request-scoped preparation observations. Never stores messages or credentials.
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

tokio::task_local! {
    static STAGES: Arc<Mutex<BTreeMap<String, f64>>>;
}

pub fn record(stage: &str, started: Instant) {
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    tracing::debug!(stage, elapsed_ms, "first request preparation");
    let _ = STAGES.try_with(|stages| {
        stages
            .lock()
            .unwrap()
            .entry(stage.to_string())
            .or_insert(elapsed_ms);
    });
}

pub async fn observe<F: std::future::Future>(future: F) -> (F::Output, BTreeMap<String, f64>) {
    let stages = Arc::new(Mutex::new(BTreeMap::from([(
        "quota_refresh_lock".into(),
        0.0,
    )])));
    let output = STAGES.scope(stages.clone(), future).await;
    let snapshot = stages.lock().unwrap().clone();
    (output, snapshot)
}

/// Synthetic, local-only first POST measurement through production admission,
/// checkpoint, route gates, context loading and AgentLoop transport. Run in a
/// separate process with an empty HOME so no user skills or credentials load.
#[cfg(not(test))]
pub async fn measure() -> anyhow::Result<serde_json::Value> {
    use super::events::{CollectingEventSink, EventSink};
    use super::{AgentLoop, AgentMode};
    use crate::config::settings::{ApiStyle, Endpoint, Settings};
    use crate::mcp::McpManager;
    use crate::util::no_window::NoWindow;
    use anyhow::{anyhow, ensure};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;
    use tokio::sync::{Mutex as AsyncMutex, RwLock};

    let home = std::env::var("CODEFACTORY_LATENCY_FIXTURE_HOME")?;
    ensure!(
        std::env::var("HOME")? == home,
        "measurement requires isolated HOME"
    );
    ensure!(
        std::path::Path::new(&home).join("synthetic-only").is_file(),
        "missing synthetic HOME marker"
    );
    let root = tempfile::tempdir()?;
    let project = root.path().join("project");
    std::fs::create_dir(&project)?;
    for args in [
        vec!["init", "-q"],
        vec![
            "-c",
            "user.name=Synthetic",
            "-c",
            "user.email=synthetic@example.invalid",
            "commit",
            "--allow-empty",
            "-qm",
            "synthetic",
        ],
    ] {
        ensure!(
            std::process::Command::new("git")
                .no_window()
                .args(args)
                .current_dir(&project)
                .status()?
                .success(),
            "fixture git failed"
        );
    }
    std::fs::write(project.join("README.md"), "# Synthetic latency fixture\n")?;
    let db_url = format!("sqlite:{}", root.path().join("fixture.db").display());
    let pool = crate::storage::db::connect(&db_url).await?;
    let mut samples = Vec::new();
    for round in 0..5 {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let base_url = format!("http://{}", listener.local_addr()?);
        let (send, receive) = tokio::sync::oneshot::channel();
        let start = Instant::now();
        let server = std::thread::spawn(move || -> anyhow::Result<()> {
            let deadline = Instant::now() + Duration::from_secs(30);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        ensure!(Instant::now() < deadline, "first HTTP request timed out");
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => return Err(error.into()),
                }
            };
            let arrived = start.elapsed().as_secs_f64() * 1000.0;
            stream.set_nonblocking(false)?;
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            let mut bytes = Vec::new();
            let body = loop {
                let mut buffer = [0; 8192];
                let count = stream.read(&mut buffer)?;
                ensure!(count > 0, "incomplete HTTP request");
                bytes.extend_from_slice(&buffer[..count]);
                ensure!(bytes.len() < 4 * 1024 * 1024, "fixture request too large");
                if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                    let header = std::str::from_utf8(&bytes[..end])?;
                    ensure!(
                        header.starts_with("POST /chat/completions "),
                        "unexpected provider route"
                    );
                    let length: usize = header
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(str::trim)
                                .map(str::to_string)
                        })
                        .ok_or_else(|| anyhow!("missing content length"))?
                        .parse()?;
                    if bytes.len() >= end + 4 + length {
                        break serde_json::from_slice::<serde_json::Value>(
                            &bytes[end + 4..end + 4 + length],
                        )?;
                    }
                }
            };
            ensure!(body["model"] == "synthetic-model", "wrong model");
            ensure!(
                body["messages"]
                    .as_array()
                    .is_some_and(|messages| messages.iter().any(|m| m["role"] == "user")),
                "missing admitted user message"
            );
            ensure!(body["stream"] == true, "stream protocol changed");
            let response = "data: {\"choices\":[{\"delta\":{\"content\":\"synthetic\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response)?;
            let _ = send.send(arrived);
            Ok(())
        });
        let session = format!("synthetic-latency-{round}");
        sqlx::query("INSERT INTO sessions (id,title,cwd,model_id,permission_mode,created_at,updated_at) VALUES (?, 'Synthetic', ?, 'synthetic-model', 'trusted', 0, 0)")
            .bind(&session).bind(project.to_string_lossy().as_ref()).execute(&pool).await?;
        let mut settings = Settings::default();
        settings.endpoints.clear();
        settings.default_endpoint = "synthetic".into();
        settings.default_model = "synthetic-model".into();
        settings.endpoints.insert(
            "synthetic".into(),
            Endpoint {
                base_url: base_url.clone(),
                key_ref: None,
                api_style: ApiStyle::Openai,
                custom_models: Vec::new(),
                active_model: Some("synthetic-model".into()),
            },
        );
        let (result, mut stages) = observe(async {
            let started = Instant::now();
            crate::commands::chat::admit_headless_chat_turn(&pool, &session, "Explain the synthetic fixture; do not change files.").await?;
            record("admission", started);
            let started = Instant::now();
            super::checkpoint::create(&project, "synthetic").map_err(anyhow::Error::msg)?;
            record("workspace_checkpoint", started);
            let started = Instant::now();
            let (plan, _) = crate::commands::chat::resolve_route_plan(&settings, "synthetic-model", "fixed", false).await?;
            record("route_quota", started);
            let mut route = plan.candidates()[0].clone();
            // Only the fixture owns this inline secret; never access OS credentials.
            route.credential_ref = None;
            route.legacy_inline_api_key = Some("synthetic-key".into());
            let history = crate::storage::load_agent_history(&pool, &session).await?;
            let sink: Arc<dyn EventSink> = Arc::new(CollectingEventSink::new());
            let mut agent = AgentLoop::new_headless(sink, pool.clone(), session.clone(), "synthetic".into(), "synthetic-model".into(), base_url, "synthetic-key".into(), ApiStyle::Openai, project.clone(), Arc::new(RwLock::new(settings)), Arc::new(AsyncMutex::new(std::collections::HashMap::new())), Arc::new(McpManager::new()), None, AgentMode::Interactive)
                .with_failover_plan(super::failover::RouteCandidatePlan::new(route));
            tokio::select! {
                arrived = receive => Ok::<f64, anyhow::Error>(arrived?),
                outcome = agent.run(history) => Err(anyhow!("agent ended before first HTTP receipt: {outcome:?}")),
            }
        }).await;
        server
            .join()
            .map_err(|_| anyhow!("fixture server panicked"))??;
        stages.insert("admission_to_http".into(), result?);
        ensure!(
            stages.contains_key("skills_memory"),
            "skills/memory stage missing"
        );
        ensure!(
            stages.contains_key("provider_post"),
            "provider admission stage missing"
        );
        samples.push(stages);
    }
    crate::storage::db::close_and_release_files(pool).await;
    let mut median_ms = BTreeMap::new();
    for key in samples[0].keys() {
        let mut values: Vec<f64> = samples.iter().map(|sample| sample[key]).collect();
        values.sort_by(f64::total_cmp);
        median_ms.insert(key.clone(), values[2]);
    }
    Ok(serde_json::json!({"synthetic_only": true, "rounds": samples, "median_ms": median_ms}))
}
