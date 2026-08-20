// SPDX-License-Identifier: Apache-2.0
//! The extension backend: the browser the user already has open.
//!
//! Runs a loopback WebSocket listener the CodeFactory extension dials into, and
//! turns [`BrowserDriver`] calls into commands on that socket. The rules for who
//! may connect live in [`super::bridge`]; this module is the transport and the
//! request/reply correlation.
//!
//! Three deliberate properties:
//!
//!   * **Bound to 127.0.0.1.** Nothing is reachable off the machine, and the
//!     port is written to a file only the user can read.
//!   * **One extension at a time.** A second connection replaces the first
//!     rather than both racing to answer the same command, which would make
//!     replies non-deterministic.
//!   * **Every command has a deadline.** The browser can be closed, a tab can
//!     hang, the worker can be evicted mid-command — all of which look identical
//!     to "no reply", so an unanswered command fails instead of stalling a turn.
//!
//! ## Pairing survives a restart
//!
//! Both halves of the pairing used to be per-process: a fresh token every launch
//! and an ephemeral port. That quietly made pairing a chore the user repeated
//! after every restart, since yesterday's values no longer matched anything. Now
//! the token is persisted and reused, a preferred fixed port is tried first, and
//! [`super::extension_package`] stamps the live values into the extension's own
//! folder — so an extension paired once stays paired, and reconnects on its own
//! even when the port does change.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex, Notify};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::protocol::frame::{coding::CloseCode, CloseFrame};
use tokio_tungstenite::tungstenite::Message;

use super::bridge::{self, Command, Hello, Reply};
use crate::errors::{AppError, Result};

/// How long to wait for the extension to answer one command.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);
const SUPERSEDED_CLOSE_CODE: u16 = 4001;

fn superseded_close() -> Message {
    Message::Close(Some(CloseFrame {
        code: CloseCode::Library(SUPERSEDED_CLOSE_CODE),
        reason: "superseded".into(),
    }))
}

/// Port the bridge asks for before falling back to whatever the OS offers.
///
/// A stable port is what lets a manually paired extension — one installed from a
/// store, where the app cannot write into its package — keep working after a
/// restart. Falling back to an ephemeral port when it is taken keeps two
/// CodeFactory windows from fighting over it.
const PREFERRED_PORT: u16 = 47615;

/// A tab the user has open.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Tab {
    pub tab_id: i64,
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub active: bool,
}

/// Pairing details the Settings page shows the user.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Pairing {
    pub port: u16,
    pub token: String,
}

struct PendingCall {
    connection_id: u64,
    reply: oneshot::Sender<Reply>,
}

#[derive(Clone)]
struct ActiveConnection {
    id: u64,
    sender: futures::channel::mpsc::UnboundedSender<Message>,
}

type Pending = Arc<Mutex<HashMap<u64, PendingCall>>>;
type Outbound = Arc<Mutex<Option<ActiveConnection>>>;

/// The extension-backed browser.
pub struct ExtensionBridge {
    token: String,
    port: Mutex<Option<u16>>,
    next_id: AtomicU64,
    next_connection_id: AtomicU64,
    pending: Pending,
    outbound: Outbound,
    connection_changed: Notify,
}

impl ExtensionBridge {
    pub fn new() -> Self {
        Self {
            token: load_or_create_token(),
            port: Mutex::new(None),
            next_id: AtomicU64::new(1),
            next_connection_id: AtomicU64::new(1),
            pending: Arc::new(Mutex::new(HashMap::new())),
            outbound: Arc::new(Mutex::new(None)),
            connection_changed: Notify::new(),
        }
    }

    /// Start listening. Returns the pairing details to show the user.
    pub async fn start(self: &Arc<Self>) -> Result<Pairing> {
        if let Some(port) = *self.port.lock().await {
            return Ok(Pairing {
                port,
                token: self.token.clone(),
            });
        }

        let listener = bind_listener().await?;
        let port = listener
            .local_addr()
            .map_err(|error| AppError::Other(format!("Could not read the bridge port: {error}")))?
            .port();
        *self.port.lock().await = Some(port);
        let holds_stable_port = port == PREFERRED_PORT;

        let bridge = Arc::clone(self);
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let bridge = Arc::clone(&bridge);
                tokio::spawn(async move {
                    bridge.serve(stream).await;
                });
            }
        });

        let pairing = Pairing {
            port,
            token: self.token.clone(),
        };
        // Skipped under `cfg(test)` so the transport tests do not write into a
        // developer's real home; the publishing itself is covered by
        // `extension_package`'s own tests.
        #[cfg(not(test))]
        if may_publish(holds_stable_port).await {
            publish(&pairing);
        } else {
            tracing::info!(
                "bridge: listening on {port}; leaving the published pairing pointed at {PREFERRED_PORT}"
            );
        }
        #[cfg(test)]
        let _ = holds_stable_port;
        Ok(pairing)
    }

    /// Whether an extension is currently connected.
    pub async fn connected(&self) -> bool {
        self.outbound.lock().await.is_some()
    }

    /// Give an already-installed extension one bounded reconnect window.
    ///
    /// MV3 workers and sleeping browsers can disappear between two tool calls.
    /// Treating that short transport gap as a fresh pairing request made a
    /// healthy installation feel broken and discarded the caller's lease.
    pub async fn wait_until_connected(&self, deadline: Duration) -> bool {
        if self.connected().await {
            return true;
        }
        tokio::time::timeout(deadline, async {
            loop {
                let changed = self.connection_changed.notified();
                if self.connected().await {
                    return;
                }
                changed.await;
            }
        })
        .await
        .is_ok()
    }

    async fn serve(self: Arc<Self>, stream: tokio::net::TcpStream) {
        // Capture Origin during the handshake: it is the check a web page cannot
        // defeat, and it is only available here.
        let origin: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let captured = Arc::clone(&origin);
        let accepted = tokio_tungstenite::accept_hdr_async(
            stream,
            move |request: &Request,
                  response: Response|
                  -> std::result::Result<Response, ErrorResponse> {
                if let Some(value) = request
                    .headers()
                    .get("origin")
                    .and_then(|v| v.to_str().ok())
                {
                    if let Ok(mut slot) = captured.try_lock() {
                        *slot = Some(value.to_string());
                    }
                }
                Ok(response)
            },
        )
        .await;
        let Ok(websocket) = accepted else { return };
        let (mut writer, mut reader) = websocket.split();

        // First frame must be the handshake.
        let hello = match reader.next().await {
            Some(Ok(Message::Text(text))) => serde_json::from_str::<Hello>(&text).ok(),
            _ => None,
        };
        let origin = origin.lock().await.clone();
        let (refusal, standby_probe) = match hello.as_ref() {
            Some(hello) => (
                bridge::authorize(origin.as_deref(), hello, &self.token).err(),
                hello.standby_probe,
            ),
            None => (Some(bridge::Refusal::BadToken), false),
        };
        if let Some(refusal) = refusal {
            // Tell the extension why so it can stop retrying and prompt for
            // re-pairing, then close rather than leaving a half-open socket.
            let _ = writer
                .send(Message::Text(
                    serde_json::json!({"refused": true, "error": refusal.message()})
                        .to_string()
                        .into(),
                ))
                .await;
            let _ = writer.close().await;
            return;
        }

        // One extension at a time: replacing the channel drops the previous one,
        // so two connections cannot both answer the same command. The identity is
        // essential: the superseded socket can finish closing after this point and
        // must not clear the newer connection or its pending calls.
        let connection_id = self.next_connection_id.fetch_add(1, Ordering::Relaxed);
        let (sender, mut receiver) = futures::channel::mpsc::unbounded::<Message>();
        let registration = {
            let mut outbound = self.outbound.lock().await;
            if standby_probe && outbound.is_some() {
                None
            } else {
                Some(outbound.replace(ActiveConnection {
                    id: connection_id,
                    sender,
                }))
            }
        };
        let Some(superseded) = registration else {
            // A profile that already lost ownership may periodically probe for
            // vacancy, but must not evict the healthy winner and restart the
            // multi-profile takeover loop.
            let _ = writer.send(superseded_close()).await;
            return;
        };
        if let Some(superseded) = superseded {
            // Calls already sent to the old socket cannot be completed by the
            // replacement. Cancel them immediately so the browser-session
            // layer can replay the read-only command on this generation.
            let _ = superseded.sender.unbounded_send(superseded_close());
            self.clear_pending(superseded.id).await;
        }
        self.connection_changed.notify_waiters();

        // `open` only proves TCP/WebSocket establishment. This acknowledgment is
        // sent after origin/token/protocol authorization so the extension does not
        // flash "connected" before the bridge has actually accepted it.
        if writer
            .send(Message::Text(
                serde_json::json!({"ready": true}).to_string().into(),
            ))
            .await
            .is_err()
        {
            self.clear_connection(connection_id).await;
            return;
        }

        let pump = tokio::spawn(async move {
            while let Some(message) = receiver.next().await {
                if writer.send(message).await.is_err() {
                    break;
                }
            }
        });

        while let Some(Ok(message)) = reader.next().await {
            let Message::Text(text) = message else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            if value.get("heartbeat").and_then(serde_json::Value::as_bool) == Some(true) {
                let current = self.outbound.lock().await.clone();
                if let Some(current) = current.filter(|active| active.id == connection_id) {
                    if current
                        .sender
                        .unbounded_send(Message::Text(
                            serde_json::json!({
                                "heartbeat_ack": value.get("sent_at").cloned().unwrap_or_default()
                            })
                            .to_string()
                            .into(),
                        ))
                        .is_err()
                    {
                        break;
                    }
                }
                continue;
            }
            if let Ok(reply) = serde_json::from_value::<Reply>(value) {
                let waiting = {
                    let mut pending = self.pending.lock().await;
                    if pending
                        .get(&reply.id)
                        .is_some_and(|call| call.connection_id == connection_id)
                    {
                        pending.remove(&reply.id)
                    } else {
                        None
                    }
                };
                if let Some(waiting) = waiting {
                    let _ = waiting.reply.send(reply);
                }
            }
        }

        // The extension went away: clear the channel so `connected()` is honest
        // and later commands fail fast instead of waiting for a timeout.
        pump.abort();
        self.clear_connection(connection_id).await;
    }

    async fn clear_connection(&self, connection_id: u64) {
        let cleared_current = {
            let mut outbound = self.outbound.lock().await;
            if outbound
                .as_ref()
                .is_some_and(|active| active.id == connection_id)
            {
                outbound.take()
            } else {
                None
            }
        };
        if let Some(active) = &cleared_current {
            // Dropping SplitSink alone does not close the peer while the serve
            // task still owns SplitStream. Send an explicit close frame before
            // dropping the last sender so the extension's close handler can
            // reconnect inside the browser-session retry grace.
            let _ = active.sender.unbounded_send(Message::Close(None));
        }
        self.clear_pending(connection_id).await;
        if cleared_current.is_some() {
            self.connection_changed.notify_waiters();
        }
    }

    async fn clear_pending(&self, connection_id: u64) {
        self.pending
            .lock()
            .await
            .retain(|_, call| call.connection_id != connection_id);
    }

    /// Send one command and await its reply.
    async fn call(&self, command: Command) -> Result<serde_json::Value> {
        self.call_with_timeout(command, COMMAND_TIMEOUT).await
    }

    async fn call_with_timeout(
        &self,
        command: Command,
        command_timeout: Duration,
    ) -> Result<serde_json::Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let frame = {
            let mut value = serde_json::to_value(&command)
                .map_err(|error| AppError::Other(format!("Could not encode command: {error}")))?;
            value["id"] = serde_json::json!(id);
            Message::Text(value.to_string().into())
        };

        let (sender, receiver) = oneshot::channel();
        let (connection_id, send_failed) = {
            // Snapshotting the generation, registering its pending reply and
            // sending the frame are one outbound-locked interval. Otherwise B
            // can replace A after the snapshot but before the pending insert;
            // B then has nothing to cancel and the orphaned A call waits 20s.
            let outbound = self.outbound.lock().await;
            let active = outbound.as_ref().ok_or_else(|| {
                AppError::Other(
                    "The CodeFactory browser extension isn't connected. Install it and pair it \
                     from Settings → Browser."
                        .into(),
                )
            })?;
            let mut pending = self.pending.lock().await;
            pending.insert(
                id,
                PendingCall {
                    connection_id: active.id,
                    reply: sender,
                },
            );
            let send_failed = active.sender.unbounded_send(frame).is_err();
            if send_failed {
                pending.remove(&id);
            }
            (active.id, send_failed)
        };
        if send_failed {
            self.clear_connection(connection_id).await;
            return Err(AppError::Other(
                "The browser extension connection dropped".into(),
            ));
        }

        match tokio::time::timeout(command_timeout, receiver).await {
            Ok(Ok(reply)) if reply.ok => Ok(reply.data.unwrap_or(serde_json::Value::Null)),
            Ok(Ok(reply)) => Err(AppError::Other(
                reply
                    .error
                    .unwrap_or_else(|| "The browser reported an error".into()),
            )),
            Ok(Err(_)) => Err(AppError::Other(
                "The browser extension disconnected before replying".into(),
            )),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                // A socket that accepted a command but never answered is not
                // healthy enough to advertise as connected. Evict it now so
                // the extension reconnects and an attached read can perform
                // its single bounded transport retry instead of waiting for
                // the later heartbeat-staleness window.
                self.clear_connection(connection_id).await;
                Err(AppError::Other(format!(
                    "The browser did not reply within {}s — the tab may be busy or closed",
                    command_timeout.as_secs_f64()
                )))
            }
        }
    }

    /// Tabs the user has open.
    pub async fn list_tabs(&self) -> Result<Vec<Tab>> {
        let data = self.call(Command::ListTabs).await?;
        serde_json::from_value(data)
            .map_err(|error| AppError::Other(format!("Could not read the tab list: {error}")))
    }

    /// Readable content of a tab.
    pub async fn read(&self, tab_id: i64) -> Result<super::PageContent> {
        let data = self.call(Command::Read { tab_id }).await?;
        let raw = data.to_string();
        super::page::parse_readable(&raw).ok_or_else(|| {
            AppError::Other("Could not extract readable content from that tab".into())
        })
    }

    /// Search within a tab.
    pub async fn find(&self, tab_id: i64, query: &str) -> Result<Vec<String>> {
        let data = self
            .call(Command::Find {
                tab_id,
                query: query.to_string(),
            })
            .await?;
        Ok(super::page::parse_find(&data.to_string()))
    }
}

impl Default for ExtensionBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// Bind the listener, preferring a stable port over an ephemeral one.
///
/// The preferred port being taken is not an error: it usually means another
/// CodeFactory window already has it, and this one still needs a bridge. The
/// extension is told which port to use either way, so a fallback costs nothing.
async fn bind_listener() -> Result<TcpListener> {
    match TcpListener::bind(("127.0.0.1", PREFERRED_PORT)).await {
        Ok(listener) => Ok(listener),
        Err(error) => {
            tracing::debug!(
                "bridge: port {PREFERRED_PORT} unavailable ({error}); using an ephemeral port"
            );
            TcpListener::bind(("127.0.0.1", 0)).await.map_err(|error| {
                AppError::Other(format!("Could not open the bridge port: {error}"))
            })
        }
    }
}

/// Where the pairing details are persisted between runs.
fn pairing_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| {
        home.join(".codefactory")
            .join("browser")
            .join(bridge::BRIDGE_FILE)
    })
}

/// Re-use the token from the last run, or mint one and keep it.
///
/// A per-process token meant every restart silently un-paired the extension and
/// sent the user back to Settings to copy a new one — the single biggest source
/// of "why has it stopped working". The token stays a capability: it lives in an
/// owner-only file, and deleting that file revokes every paired extension.
fn load_or_create_token() -> String {
    if let Some(existing) = pairing_path().and_then(read_token) {
        return existing;
    }
    bridge::new_token()
}

/// Read a previously stored token, rejecting anything that is not one.
///
/// A corrupted or hand-edited file must not become the expected token: an empty
/// or short value there would weaken the check the token exists to make.
fn read_token(path: PathBuf) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let stored: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let token = stored.get("token")?.as_str()?.trim().to_string();
    let looks_like_a_token = token.len() == 32 && token.chars().all(|c| c.is_ascii_hexdigit());
    looks_like_a_token.then_some(token)
}

/// Whether this instance may repoint the extension at itself.
///
/// The pairing file and the extension's stamped copy are shared by every
/// CodeFactory on the machine, and the last writer won. When a second instance
/// starts — a dev build beside the installed app is the ordinary case — it finds
/// [`PREFERRED_PORT`] taken, falls back to an ephemeral port, and published
/// that. From then on the extension dialled a socket owned by the short-lived
/// process; when that process exited, the pairing pointed at nothing. An
/// extension paired weeks earlier stopped being recognised with no visible
/// cause, and the installed app sitting on the stable port was never found
/// again.
///
/// So the instance holding the stable port owns the pairing. A fallback
/// instance publishes only when nothing is actually serving — which is what
/// bootstraps the very first launch, and what recovers the file if the stable
/// port is squatted by something that never accepts a connection.
#[cfg_attr(test, allow(dead_code))]
async fn may_publish(holds_stable_port: bool) -> bool {
    if holds_stable_port {
        return publish_decision(true, true, None);
    }
    let stable_port_live = port_is_listening(PREFERRED_PORT).await;
    let published_alive = match pairing_path().and_then(read_published_port) {
        Some(port) => Some(port_is_listening(port).await),
        None => None,
    };
    publish_decision(false, stable_port_live, published_alive)
}

/// The rule itself, separated from the probes so it can be stated as a table.
///
/// `published_alive` is `None` when no pairing has ever been written — the
/// first launch, which must bootstrap the file even from a fallback port.
fn publish_decision(
    holds_stable_port: bool,
    stable_port_live: bool,
    published_alive: Option<bool>,
) -> bool {
    if holds_stable_port {
        return true;
    }
    if stable_port_live {
        return false;
    }
    !published_alive.unwrap_or(false)
}

/// The port the published pairing currently names.
#[cfg_attr(test, allow(dead_code))]
fn read_published_port(path: PathBuf) -> Option<u16> {
    let raw = std::fs::read_to_string(path).ok()?;
    let stored: serde_json::Value = serde_json::from_str(&raw).ok()?;
    u16::try_from(stored.get("port")?.as_u64()?)
        .ok()
        .filter(|port| *port > 0)
}

/// Whether anything on loopback accepts a connection there.
///
/// Bounded, because "port taken but nobody accepting" is exactly the state this
/// has to distinguish, and a blocking connect against it would hold up startup.
#[cfg_attr(test, allow(dead_code))]
async fn port_is_listening(port: u16) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_millis(300),
            tokio::net::TcpStream::connect(("127.0.0.1", port)),
        )
        .await,
        Ok(Ok(_))
    )
}

/// Make the live pairing available to both the Settings page and the extension.
///
/// Best effort on purpose: the bridge is perfectly usable with a hand-pasted
/// pairing, so a read-only home directory must not stop it from starting.
#[cfg_attr(test, allow(dead_code))]
fn publish(pairing: &Pairing) {
    write_pairing_file(pairing);
    // The step that removes the copy-and-paste: stamp the live port and token
    // into the extension's own folder, if one has been prepared.
    if let Some(dir) = super::extension_package::existing_dir() {
        if let Err(error) =
            super::extension_package::write_pairing(&dir, pairing.port, &pairing.token)
        {
            tracing::warn!("bridge: could not update the extension pairing file: {error}");
        }
    }
}

/// Persist the port and token for the Settings page, owner-readable only.
///
/// The token is a capability: anything that can read it can drive the bridge, so
/// the file is not world-readable even though it lives under the user's home.
fn write_pairing_file(pairing: &Pairing) {
    let Some(path) = pairing_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let body = serde_json::json!({
        "port": pairing.port,
        "token": pairing.token,
        "protocol_version": bridge::PROTOCOL_VERSION,
    });
    if std::fs::write(&path, body.to_string()).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_EXTENSION_ORIGIN: &str = "chrome-extension://abcdefghijklmnopabcdefghijklmnop";

    async fn dial_test_extension(
        pairing: &Pairing,
        standby_probe: bool,
    ) -> tokio_tungstenite::WebSocketStream<tokio::net::TcpStream> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        let stream = tokio::net::TcpStream::connect(("127.0.0.1", pairing.port))
            .await
            .expect("connect to bridge");
        let mut request = format!("ws://127.0.0.1:{}", pairing.port)
            .into_client_request()
            .expect("websocket request");
        request.headers_mut().insert(
            "Origin",
            TEST_EXTENSION_ORIGIN.parse().expect("extension origin"),
        );
        let (mut socket, _) = tokio_tungstenite::client_async(request, stream)
            .await
            .expect("websocket handshake");
        socket
            .send(Message::Text(
                serde_json::json!({
                    "protocol_version": bridge::PROTOCOL_VERSION,
                    "token": pairing.token,
                    "extension_version": "test",
                    "standby_probe": standby_probe,
                })
                .to_string()
                .into(),
            ))
            .await
            .expect("send extension hello");
        socket
    }

    async fn connect_test_extension(
        bridge: &ExtensionBridge,
        pairing: &Pairing,
    ) -> tokio_tungstenite::WebSocketStream<tokio::net::TcpStream> {
        let mut socket = dial_test_extension(pairing, false).await;
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let Some(Ok(Message::Text(text))) = socket.next().await else {
                    panic!("socket closed before authenticated ready");
                };
                let value: serde_json::Value = serde_json::from_str(&text).expect("bridge json");
                if value.get("ready").and_then(serde_json::Value::as_bool) == Some(true) {
                    break;
                }
            }
        })
        .await
        .expect("authenticated ready deadline");
        assert!(bridge.connected().await, "bridge observes extension");
        socket
    }

    #[tokio::test]
    async fn a_command_without_a_connected_extension_says_so_immediately() {
        // The failure users will actually hit: extension not installed yet. It
        // must be an instant, actionable error rather than a 20s timeout.
        let bridge = ExtensionBridge::new();
        let started = std::time::Instant::now();
        let error = bridge.list_tabs().await.expect_err("no extension");

        assert!(error.to_string().contains("isn't connected"));
        assert!(error.to_string().contains("Settings"));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "must not wait for the timeout"
        );
    }

    #[tokio::test]
    async fn no_pending_command_is_left_behind_when_there_is_nobody_to_send_to() {
        let bridge = ExtensionBridge::new();
        let _ = bridge.read(1).await;
        assert!(
            bridge.pending.lock().await.is_empty(),
            "a command that was never sent must not stay pending"
        );
    }

    /// The failure this fixes, as a table. A dev build started beside the
    /// installed app takes an ephemeral port; publishing it repointed the
    /// extension at a process that was about to exit, and the app on the stable
    /// port was never found again — "I installed it before and now it isn't
    /// recognised". The stable-port holder owns the pairing; a fallback
    /// instance writes only when nothing at all is serving.
    #[test]
    fn only_the_stable_port_holder_owns_the_published_pairing() {
        assert!(
            publish_decision(true, true, Some(true)),
            "the instance on the stable port always publishes its own pairing"
        );
        assert!(
            !publish_decision(false, true, Some(true)),
            "a fallback instance must not repoint a live pairing at itself"
        );
        assert!(
            !publish_decision(false, true, None),
            "the stable-port holder will publish; a fallback must not race it"
        );
        assert!(
            publish_decision(false, false, None),
            "first launch bootstraps the pairing file even from a fallback port"
        );
        assert!(
            publish_decision(false, false, Some(false)),
            "a recorded pairing nobody answers on is repaired, not preserved"
        );
        assert!(
            !publish_decision(false, false, Some(true)),
            "something is serving the recorded pairing; leave it alone"
        );
    }

    #[test]
    fn a_published_port_is_read_back_and_a_damaged_one_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("good.json");
        std::fs::write(&good, serde_json::json!({"port": 47615}).to_string()).unwrap();
        assert_eq!(read_published_port(good), Some(47615));

        for (name, body) in [
            ("missing-port.json", r#"{"token":"x"}"#),
            ("zero.json", r#"{"port": 0}"#),
            ("out-of-range.json", r#"{"port": 70000}"#),
            ("garbage.json", "not json"),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, body).unwrap();
            assert_eq!(read_published_port(path), None, "{name} must be rejected");
        }
    }

    #[test]
    fn a_stored_token_is_reused_so_pairing_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bridge.json");
        std::fs::write(
            &path,
            serde_json::json!({"port": 1, "token": "0123456789abcdef0123456789abcdef"}).to_string(),
        )
        .unwrap();

        assert_eq!(
            read_token(path).as_deref(),
            Some("0123456789abcdef0123456789abcdef")
        );
    }

    #[test]
    fn a_damaged_pairing_file_mints_a_new_token_instead_of_weakening_the_check() {
        let dir = tempfile::tempdir().unwrap();
        for (name, body) in [
            ("empty.json", ""),
            ("blank-token.json", r#"{"token": ""}"#),
            ("short.json", r#"{"token": "abc"}"#),
            (
                "not-hex.json",
                r#"{"token": "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"}"#,
            ),
            ("wrong-shape.json", r#"{"token": 12345}"#),
            ("garbage.json", "not json at all"),
        ] {
            let path = dir.path().join(name);
            std::fs::write(&path, body).unwrap();
            assert_eq!(read_token(path), None, "{name} must be rejected");
        }
    }

    #[tokio::test]
    async fn starting_twice_reuses_the_same_port_and_token() {
        // Settings can be opened repeatedly; each visit must show the same
        // pairing details rather than silently invalidating the extension.
        let bridge = Arc::new(ExtensionBridge::new());
        let first = bridge.start().await.expect("start");
        let second = bridge.start().await.expect("start again");

        assert_eq!(first.port, second.port);
        assert_eq!(first.token, second.token);
        assert!(first.port > 0);
    }

    #[tokio::test]
    async fn the_listener_is_loopback_only() {
        let bridge = Arc::new(ExtensionBridge::new());
        let pairing = bridge.start().await.expect("start");

        // Reachable on loopback…
        assert!(tokio::net::TcpStream::connect(("127.0.0.1", pairing.port))
            .await
            .is_ok());
        // …and not bound on a routable interface.
        assert!(!bridge.connected().await);
    }

    #[tokio::test]
    async fn an_old_connection_closing_cannot_clear_the_new_extension() {
        let bridge = Arc::new(ExtensionBridge::new());
        let pairing = bridge.start().await.expect("start");

        let mut first = connect_test_extension(&bridge, &pairing).await;
        let mut second = connect_test_extension(&bridge, &pairing).await;

        let (command_seen, wait_for_command) = oneshot::channel();
        let (allow_reply, wait_to_reply) = oneshot::channel();
        let responder = tokio::spawn(async move {
            while let Some(Ok(Message::Text(text))) = second.next().await {
                let Ok(request) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                if request.get("cmd").and_then(|value| value.as_str()) != Some("list_tabs") {
                    continue;
                }
                let _ = command_seen.send(());
                let _ = wait_to_reply.await;
                second
                    .send(Message::Text(
                        serde_json::json!({
                            "id": request["id"],
                            "ok": true,
                            "data": [{
                                "tab_id": 7,
                                "title": "Synthetic page",
                                "url": "https://example.test/",
                                "active": true
                            }]
                        })
                        .to_string()
                        .into(),
                    ))
                    .await
                    .expect("reply on current extension");
                return second;
            }
            panic!("current extension closed before receiving the command");
        });

        let call = {
            let bridge = Arc::clone(&bridge);
            tokio::spawn(async move { bridge.list_tabs().await })
        };
        wait_for_command
            .await
            .expect("command reached current extension");

        first.close(None).await.expect("close old extension");
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(
            bridge.connected().await,
            "a superseded socket must not clear the newer live extension"
        );
        allow_reply.send(()).expect("allow current reply");
        let tabs = call
            .await
            .expect("browser call task")
            .expect("new extension answers commands");
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].tab_id, 7);
        let mut second = responder.await.expect("extension responder");
        second.close(None).await.expect("close current extension");
    }

    #[tokio::test]
    async fn a_superseded_profiles_standby_probe_cannot_evict_the_winner() {
        let bridge = Arc::new(ExtensionBridge::new());
        let pairing = bridge.start().await.expect("start");
        let mut winner = connect_test_extension(&bridge, &pairing).await;

        let mut standby = dial_test_extension(&pairing, true).await;
        let close = tokio::time::timeout(Duration::from_secs(1), standby.next())
            .await
            .expect("standby probe response deadline")
            .expect("standby close frame")
            .expect("valid standby close frame");
        let Message::Close(Some(frame)) = close else {
            panic!("standby probe must receive a reasoned close");
        };
        assert_eq!(u16::from(frame.code), SUPERSEDED_CLOSE_CODE);
        assert_eq!(frame.reason, "superseded");
        assert!(bridge.connected().await, "winner remains current");

        winner.close(None).await.expect("close winner");
        tokio::time::timeout(Duration::from_secs(1), async {
            while bridge.connected().await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("winner close observed");

        let mut recovery = dial_test_extension(&pairing, true).await;
        let ready = tokio::time::timeout(Duration::from_secs(1), recovery.next())
            .await
            .expect("vacant bridge response deadline")
            .expect("ready frame")
            .expect("valid ready frame");
        let Message::Text(ready) = ready else {
            panic!("vacant bridge must accept the standby probe");
        };
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&ready).expect("ready json")["ready"],
            true
        );
        assert!(bridge.connected().await);
        recovery.close(None).await.expect("close recovery");
    }

    #[tokio::test]
    async fn a_new_connection_cancels_a_command_stranded_on_the_old_generation() {
        let bridge = Arc::new(ExtensionBridge::new());
        let pairing = bridge.start().await.expect("start");
        let mut first = connect_test_extension(&bridge, &pairing).await;

        let (command_seen, wait_for_command) = oneshot::channel();
        let stranded = tokio::spawn(async move {
            while let Some(Ok(Message::Text(text))) = first.next().await {
                let Ok(request) = serde_json::from_str::<serde_json::Value>(&text) else {
                    continue;
                };
                if request.get("cmd").and_then(|value| value.as_str()) == Some("list_tabs") {
                    let _ = command_seen.send(());
                    return first;
                }
            }
            panic!("old extension closed before receiving the command");
        });
        let call = {
            let bridge = Arc::clone(&bridge);
            tokio::spawn(async move { bridge.list_tabs().await })
        };
        wait_for_command
            .await
            .expect("command reached old extension");

        let mut current = connect_test_extension(&bridge, &pairing).await;
        let error = tokio::time::timeout(Duration::from_secs(1), call)
            .await
            .expect("superseding must cancel promptly")
            .expect("browser call task")
            .expect_err("old generation cannot complete on the new socket");
        assert!(error.to_string().contains("disconnected before replying"));
        assert!(bridge.connected().await);

        let mut first = stranded.await.expect("stranded extension task");
        first.close(None).await.expect("close old extension");
        current.close(None).await.expect("close current extension");
    }

    #[tokio::test]
    async fn registering_a_pending_call_is_atomic_with_its_generation_snapshot() {
        let bridge = Arc::new(ExtensionBridge::new());
        let pairing = bridge.start().await.expect("start");
        let mut first = connect_test_extension(&bridge, &pairing).await;

        // Hold pending so `call` can reach the critical lock boundary. A correct
        // implementation keeps outbound locked while it waits; the old TOCTOU
        // implementation cloned A, released outbound, then waited here.
        let pending_guard = bridge.pending.lock().await;
        let call = {
            let bridge = Arc::clone(&bridge);
            tokio::spawn(async move { bridge.list_tabs().await })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if bridge.outbound.try_lock().is_err() {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("call must hold outbound until pending registration completes");

        let replacement = {
            let bridge = Arc::clone(&bridge);
            let pairing = pairing.clone();
            tokio::spawn(async move { connect_test_extension(&bridge, &pairing).await })
        };
        drop(pending_guard);
        let mut current = replacement.await.expect("replacement connection");
        let error = tokio::time::timeout(Duration::from_secs(1), call)
            .await
            .expect("replacement must cancel A's registered call")
            .expect("browser call task")
            .expect_err("superseded generation cannot complete the call");
        assert!(error.to_string().contains("disconnected before replying"));

        first.close(None).await.expect("close old extension");
        current.close(None).await.expect("close current extension");
    }

    #[tokio::test]
    async fn an_authorized_heartbeat_is_acknowledged_on_the_same_generation() {
        let bridge = Arc::new(ExtensionBridge::new());
        let pairing = bridge.start().await.expect("start");
        let mut socket = connect_test_extension(&bridge, &pairing).await;
        socket
            .send(Message::Text(
                serde_json::json!({"heartbeat": true, "sent_at": 1234})
                    .to_string()
                    .into(),
            ))
            .await
            .expect("send heartbeat");

        let ack = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let Some(Ok(Message::Text(text))) = socket.next().await else {
                    panic!("socket closed before heartbeat ack");
                };
                let value: serde_json::Value = serde_json::from_str(&text).expect("json frame");
                if value.get("heartbeat_ack").is_some() {
                    return value;
                }
            }
        })
        .await
        .expect("heartbeat ack deadline");
        assert_eq!(ack["heartbeat_ack"], 1234);
    }

    #[tokio::test]
    async fn a_command_timeout_evicts_the_half_open_connection() {
        let bridge = Arc::new(ExtensionBridge::new());
        let pairing = bridge.start().await.expect("start");
        let mut socket = connect_test_extension(&bridge, &pairing).await;
        let bridge_for_extension = Arc::clone(&bridge);
        let pairing_for_extension = pairing.clone();
        let (close_seen, wait_for_close) = oneshot::channel();
        let (allow_reconnect, wait_to_reconnect) = oneshot::channel();
        let silent_extension = tokio::spawn(async move {
            while let Some(Ok(Message::Text(text))) = socket.next().await {
                let request: serde_json::Value = serde_json::from_str(&text).expect("command json");
                if request.get("cmd").and_then(|value| value.as_str()) == Some("list_tabs") {
                    let close = tokio::time::timeout(Duration::from_secs(1), socket.next())
                        .await
                        .expect("bridge must close a timed-out socket")
                        .expect("close frame")
                        .expect("valid close frame");
                    assert!(matches!(close, Message::Close(_)));
                    close_seen.send(()).expect("report close frame");
                    wait_to_reconnect.await.expect("allow reconnect");
                    return connect_test_extension(&bridge_for_extension, &pairing_for_extension)
                        .await;
                }
            }
            panic!("socket closed before receiving command");
        });

        let error = bridge
            .call_with_timeout(Command::ListTabs, Duration::from_millis(50))
            .await
            .expect_err("silent extension must time out");
        assert!(error.to_string().contains("did not reply within"));
        assert!(
            !bridge.connected().await,
            "a timed-out socket is half-open and must no longer be advertised"
        );
        wait_for_close.await.expect("client observed close frame");
        allow_reconnect.send(()).expect("allow reconnect");
        assert!(
            bridge.wait_until_connected(Duration::from_secs(6)).await,
            "the close frame must let the extension reconnect inside the retry grace"
        );
        let mut replacement = silent_extension.await.expect("extension reconnect task");
        replacement.close(None).await.expect("close replacement");
    }
}
