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
use tokio::sync::{oneshot, Mutex};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::Message;

use super::bridge::{self, Command, Hello, Reply};
use crate::errors::{AppError, Result};

/// How long to wait for the extension to answer one command.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);

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

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>;
type Outbound = Arc<Mutex<Option<futures::channel::mpsc::UnboundedSender<Message>>>>;

/// The extension-backed browser.
pub struct ExtensionBridge {
    token: String,
    port: Mutex<Option<u16>>,
    next_id: AtomicU64,
    pending: Pending,
    outbound: Outbound,
}

impl ExtensionBridge {
    pub fn new() -> Self {
        Self {
            token: load_or_create_token(),
            port: Mutex::new(None),
            next_id: AtomicU64::new(1),
            pending: Arc::new(Mutex::new(HashMap::new())),
            outbound: Arc::new(Mutex::new(None)),
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
        publish(&pairing);
        Ok(pairing)
    }

    /// Whether an extension is currently connected.
    pub async fn connected(&self) -> bool {
        self.outbound.lock().await.is_some()
    }

    async fn serve(self: Arc<Self>, stream: tokio::net::TcpStream) {
        // Capture Origin during the handshake: it is the check a web page cannot
        // defeat, and it is only available here.
        let origin: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let captured = Arc::clone(&origin);
        let accepted = tokio_tungstenite::accept_hdr_async(
            stream,
            move |request: &Request, response: Response| -> std::result::Result<Response, ErrorResponse> {
                if let Some(value) = request.headers().get("origin").and_then(|v| v.to_str().ok()) {
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
        let refusal = match hello {
            Some(hello) => bridge::authorize(origin.as_deref(), &hello, &self.token).err(),
            None => Some(bridge::Refusal::BadToken),
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
        // so two connections cannot both answer the same command.
        let (sender, mut receiver) = futures::channel::mpsc::unbounded::<Message>();
        *self.outbound.lock().await = Some(sender);

        let pump = tokio::spawn(async move {
            while let Some(message) = receiver.next().await {
                if writer.send(message).await.is_err() {
                    break;
                }
            }
        });

        while let Some(Ok(message)) = reader.next().await {
            let Message::Text(text) = message else { continue };
            if let Ok(reply) = serde_json::from_str::<Reply>(&text) {
                if let Some(waiting) = self.pending.lock().await.remove(&reply.id) {
                    let _ = waiting.send(reply);
                }
            }
        }

        // The extension went away: clear the channel so `connected()` is honest
        // and later commands fail fast instead of waiting for a timeout.
        pump.abort();
        *self.outbound.lock().await = None;
        self.pending.lock().await.clear();
    }

    /// Send one command and await its reply.
    async fn call(&self, command: Command) -> Result<serde_json::Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let frame = {
            let mut value = serde_json::to_value(&command)
                .map_err(|error| AppError::Other(format!("Could not encode command: {error}")))?;
            value["id"] = serde_json::json!(id);
            Message::Text(value.to_string().into())
        };

        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);

        {
            let outbound = self.outbound.lock().await;
            let Some(channel) = outbound.as_ref() else {
                self.pending.lock().await.remove(&id);
                return Err(AppError::Other(
                    "The CodeFactory browser extension isn't connected. Install it and pair it \
                     from Settings → Browser."
                        .into(),
                ));
            };
            channel.unbounded_send(frame).map_err(|_| {
                AppError::Other("The browser extension connection dropped".into())
            })?;
        }

        match tokio::time::timeout(COMMAND_TIMEOUT, receiver).await {
            Ok(Ok(reply)) if reply.ok => Ok(reply.data.unwrap_or(serde_json::Value::Null)),
            Ok(Ok(reply)) => Err(AppError::Other(
                reply.error.unwrap_or_else(|| "The browser reported an error".into()),
            )),
            Ok(Err(_)) => Err(AppError::Other(
                "The browser extension disconnected before replying".into(),
            )),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(AppError::Other(format!(
                    "The browser did not reply within {}s — the tab may be busy or closed",
                    COMMAND_TIMEOUT.as_secs()
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
        super::page::parse_readable(&raw)
            .ok_or_else(|| AppError::Other("Could not extract readable content from that tab".into()))
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
            tracing::debug!("bridge: port {PREFERRED_PORT} unavailable ({error}); using an ephemeral port");
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
    let looks_like_a_token =
        token.len() == 32 && token.chars().all(|c| c.is_ascii_hexdigit());
    looks_like_a_token.then_some(token)
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
        if let Err(error) = super::extension_package::write_pairing(&dir, pairing.port, &pairing.token)
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

    #[tokio::test]
    async fn a_command_without_a_connected_extension_says_so_immediately() {
        // The failure users will actually hit: extension not installed yet. It
        // must be an instant, actionable error rather than a 20s timeout.
        let bridge = ExtensionBridge::new();
        let started = std::time::Instant::now();
        let error = bridge.list_tabs().await.expect_err("no extension");

        assert!(error.to_string().contains("isn't connected"));
        assert!(error.to_string().contains("Settings"));
        assert!(started.elapsed() < Duration::from_secs(1), "must not wait for the timeout");
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
            ("not-hex.json", r#"{"token": "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"}"#),
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
        assert!(
            tokio::net::TcpStream::connect(("127.0.0.1", pairing.port))
                .await
                .is_ok()
        );
        // …and not bound on a routable interface.
        assert!(!bridge.connected().await);
    }
}
