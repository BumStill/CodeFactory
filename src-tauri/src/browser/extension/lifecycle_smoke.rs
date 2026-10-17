// SPDX-License-Identifier: Apache-2.0
//! Isolated process smoke. Never loads or publishes production pairing state.
use super::*;
use anyhow::{ensure, Context};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

type Socket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

async fn receive(socket: &mut Socket) -> anyhow::Result<Value> {
    let frame = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await?.context("socket ended")??;
    ensure!(frame.is_text(), "expected JSON frame");
    Ok(serde_json::from_str(frame.to_text()?)?)
}

async fn connect(port: u16, token: &str) -> anyhow::Result<Socket> {
    let stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
    let mut request = format!("ws://127.0.0.1:{port}").into_client_request()?;
    request.headers_mut().insert("Origin", "chrome-extension://abcdefghijklmnopabcdefghijklmnop".parse()?);
    let (mut socket, _) = tokio_tungstenite::client_async(request, stream).await?;
    socket.send(Message::Text(json!({"protocol_version": bridge::PROTOCOL_VERSION,
        "token": token, "extension_version": "synthetic"}).to_string().into())).await?;
    ensure!(receive(&mut socket).await?["ready"] == true, "authenticated ready");
    Ok(socket)
}

async fn command(bridge: &Arc<ExtensionBridge>, socket: &mut Socket, seen: &mut std::collections::HashSet<u64>) -> anyhow::Result<()> {
    let caller = Arc::clone(bridge);
    let call = tokio::spawn(async move { caller.list_tabs().await });
    let request = receive(socket).await?;
    ensure!(request["cmd"] == "list_tabs", "session command route");
    let id = request["id"].as_u64().context("command id")?;
    ensure!(seen.insert(id), "duplicate command id");
    socket.send(Message::Text(json!({"id": id, "ok": true,
        "data": [{"tab_id": 7, "title": "synthetic", "url": "https://example.invalid/", "active": true}]}).to_string().into())).await?;
    let tabs = call.await??;
    ensure!(tabs.len() == 1 && tabs[0].tab_id == 7, "same selected tab");
    Ok(())
}

async fn disconnected(bridge: &ExtensionBridge) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(2), async {
        while bridge.connected().await { tokio::task::yield_now().await; }
    }).await?;
    Ok(())
}

async fn probe() -> anyhow::Result<Value> {
    let bridge = Arc::new(ExtensionBridge {
        token: bridge::new_token(), port: Mutex::new(None), next_id: AtomicU64::new(1),
        next_connection_id: AtomicU64::new(1), pending: Arc::new(Mutex::new(HashMap::new())),
        outbound: Arc::new(Mutex::new(None)), connection_changed: Notify::new(),
    });
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let server = Arc::clone(&bridge);
    let listener_task = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (stream, _) = accepted?;
                    let server = Arc::clone(&server);
                    connections.spawn(async move { server.serve(stream).await; });
                }
                _ = connections.join_next(), if !connections.is_empty() => {}
            }
        }
        #[allow(unreachable_code)]
        Ok::<(), std::io::Error>(())
    });
    struct Abort(tokio::task::JoinHandle<std::io::Result<()>>);
    impl Drop for Abort { fn drop(&mut self) { self.0.abort(); } }
    let mut guard = Abort(listener_task);
    let mut seen = std::collections::HashSet::new();
    let mut a = connect(port, &bridge.token).await?;
    command(&bridge, &mut a, &mut seen).await?;
    let generation_a = bridge.outbound.lock().await.as_ref().context("active A")?.id;
    // No command is pending during idle; authenticated heartbeats preserve A.
    for tick in [0, 25, 50, 75, 100, 120] {
        a.send(Message::Text(json!({"heartbeat": true, "sent_at": tick}).to_string().into())).await?;
        ensure!(receive(&mut a).await?["heartbeat_ack"] == tick, "idle heartbeat ACK");
    }
    ensure!(bridge.outbound.lock().await.as_ref().context("idle active")?.id == generation_a, "idle generation");
    let caller = Arc::clone(&bridge);
    let stranded = tokio::spawn(async move { caller.list_tabs().await });
    let old = receive(&mut a).await?;
    ensure!(seen.insert(old["id"].as_u64().context("old command id")?), "duplicate old command");
    let mut b = connect(port, &bridge.token).await?;
    ensure!(stranded.await?.is_err(), "old generation must cancel");
    let generation_b = bridge.outbound.lock().await.as_ref().context("active B")?.id;
    ensure!(generation_b > generation_a, "generation monotonic");
    drop(a);
    command(&bridge, &mut b, &mut seen).await?;
    b.close(None).await?;
    drop(b);
    disconnected(&bridge).await?;
    let caller = Arc::clone(&bridge);
    let reconnect = tokio::spawn(async move { connect(port, &caller.token).await });
    ensure!(bridge.wait_until_connected(Duration::from_secs(2)).await, "automatic reconnect");
    let mut c = reconnect.await??;
    command(&bridge, &mut c, &mut seen).await?;
    let caller = Arc::clone(&bridge);
    let half_open = tokio::spawn(async move {
        caller.call_with_timeout(Command::ListTabs, Duration::from_millis(50)).await
    });
    let request = receive(&mut c).await?;
    ensure!(seen.insert(request["id"].as_u64().context("timeout id")?), "duplicate timeout command");
    ensure!(half_open.await?.is_err(), "half-open deadline");
    ensure!(!bridge.connected().await, "half-open evicted");
    drop(c);
    let mut d = connect(port, &bridge.token).await?;
    command(&bridge, &mut d, &mut seen).await?;
    d.close(None).await?;
    drop(d);
    disconnected(&bridge).await?;
    ensure!(bridge.pending.lock().await.is_empty(), "pending cleanup");
    ensure!(seen.len() == 6, "exact command ledger");
    guard.0.abort();
    ensure!((&mut guard.0).await.unwrap_err().is_cancelled(), "listener stopped");
    // A just-closed listener does not free its port at the same instant on every
    // platform: Windows keeps it reserved while the socket is torn down and
    // accepted connections sit in TIME_WAIT, so an immediate rebind fails with
    // WSAEADDRINUSE (10048). Poll the release instead of assuming Unix's
    // instantaneous close, and fail only when the port never comes back.
    let released = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            match TcpListener::bind(("127.0.0.1", port)).await {
                Ok(rebound) => return Ok(rebound),
                Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => return Err(error),
            }
        }
    })
    .await
    .context("listener did not release its port")?
    .context("port release probe")?;
    drop(released);
    Ok(json!({"ok": true, "scenario_id": "E2E-008", "old_generation_cancelled": true,
        "half_open_evicted": true, "same_session": true, "selected_tab": 7,
        "command_count": seen.len(), "duplicate_command_count": 0,
        "human_prompt_count": 0, "cleanup_ok": true,
        "protocol_version": bridge::PROTOCOL_VERSION,
        "app_version": env!("CARGO_PKG_VERSION"), "evidence_level": "synthetic_loopback_process"}))
}

pub async fn run() -> anyhow::Result<Value> {
    tokio::time::timeout(Duration::from_secs(15), probe()).await?
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn lifecycle_loopback_observes_generations_timeout_and_cleanup() {
        for _ in 0..3 {
            let receipt = super::run().await.expect("loopback lifecycle");
            assert_eq!(receipt["command_count"], 6);
            assert_eq!(receipt["cleanup_ok"], true);
            assert_eq!(receipt["same_session"], true);
        }
    }
}
