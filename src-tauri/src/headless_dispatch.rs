// SPDX-License-Identifier: Apache-2.0
//! U34 / CF-HDE-*: the local-only headless dispatch entry.
//!
//! ## Why this exists
//!
//! On macOS the whole window-level Accessibility surface (menu items, synthetic
//! clicks, keystrokes) stops working the moment the screen locks: the OS refuses
//! `AXUIElement` actions with "actions need the screen unlocked". The native
//! menu bar added for M7 is therefore *not* reachable from an overnight
//! orchestrator — an unattended run silently stalls until a human unlocks.
//!
//! This module is the missing input surface: a **local-only** request channel
//! that the running app accepts while the screen is locked and the window is on
//! another display. It is deliberately transport-only. It does not decide what a
//! task may do — it hands the request to the same machinery GUI input uses, so
//! there is no bypass channel (CF-HDE-R3).
//!
//! ## Threat model (short form; the PR carries the long form)
//!
//! *Who can call it.* Only a process running as the **same OS user** on the
//! **same machine**. The channel is a `0600` Unix domain socket inside the
//! app's private data directory. No TCP/UDP listener is ever opened, so there is
//! no address another machine could reach (CF-HDE-R2).
//!
//! *What it protects against.* Another local user, a socket someone else
//! replaced, a world/group-accessible socket, a group/world-writable parent
//! directory, a non-socket path, and a `tcp://` "address" are all rejected
//! before a single byte of the body is interpreted.
//!
//! *What it deliberately does NOT do.* It never infers delivery authorization
//! from the wording of a message. `delivery_authorized` is a required structured
//! boolean field; absent means the request does not even parse (M48).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Stable identifier for this entry point, used in the audit log and as the
/// session-visible source marker (CF-HDE-R4).
pub const DISPATCH_SOURCE: &str = "local_task_entry";

/// Permission modes the app accepts (mirrors `menu_spec::PERMISSION_MODES`).
pub const PERMISSION_MODES: [&str; 3] = ["safe", "standard", "trusted"];

/// Delivery directions a `send` may take (CF-HDE-R8). Mirrors the interface's
/// Enter (`steer`) / ⌘Enter (`queue`) semantics, and `desktopDispatch.ts`'s
/// `DISPATCH_SEND_MODES`. Absent means `steer` — the interface's unmodified
/// Enter. Accepting one more value here widens nothing: the destination is
/// still decided by the interface against the same turn-admission rules.
pub const SEND_MODES: [&str; 2] = ["steer", "queue"];

/// Largest request line we will read, so a hostile client cannot make the
/// server allocate unbounded memory over the socket.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;

// ---------------------------------------------------------------------------
// Protocol
// ---------------------------------------------------------------------------

/// One dispatch request. `deny_unknown_fields` is load-bearing: a typo in a
/// field name must fail loudly instead of silently dropping the field (which,
/// for `delivery_authorized`, would be a security-relevant silent default).
///
/// `delivery_authorized` intentionally has **no** serde default. A request that
/// does not state it explicitly does not deserialize at all.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// CF-HDE-R1: create a session in a named project and send the first message.
    CreateAndSend {
        project: String,
        message: String,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        permission_mode: Option<String>,
        delivery_authorized: bool,
    },
    /// CF-HDE-R1: send a message to an existing session.
    ///
    /// CF-HDE-R8: `mode` chooses the direction when a run is already in flight
    /// — `steer` interjects into it (the interface's Enter), `queue` defers to
    /// after it (the interface's ⌘Enter). Absent defaults to `steer`. When the
    /// session is idle both mean "start the next turn now".
    Send {
        session_id: String,
        message: String,
        #[serde(default)]
        mode: Option<String>,
        delivery_authorized: bool,
    },
    /// CF-HDE-R1: set the session's permission mode.
    SetPermission {
        session_id: String,
        permission_mode: String,
    },
    /// CF-HDE-R1: query objective state / latest reply summary / PR number.
    Status { session_id: String },
    /// CF-HDE-R6: pick or change the model (session-scoped, or the default).
    SetModel {
        #[serde(default)]
        session_id: Option<String>,
        model: String,
    },
    /// CF-HDE-R6 (M27): switch the session shown in the GUI.
    SwitchSession { session_id: String },
    /// CF-HDE-R6: stop the current run.
    Stop {
        #[serde(default)]
        session_id: Option<String>,
    },
    /// CF-HDE-R6: list pending approval requests.
    ListApprovals {},
    /// CF-HDE-R6: approve/deny one pending request. Approving still goes
    /// through the same permission rules — this is not a blanket grant.
    ResolveApproval { approval_id: String, approve: bool },
    /// CF-HDE-R6 (M42): move the main window back to the main display.
    FocusMainDisplay {},
}

/// Failure codes the interface may hand back through `Response::error`
/// (CF-HDE-R0 / R7). `not_found` is how a request whose `session_id` cannot be
/// located fails — never by silently falling back to whatever the interface is
/// showing; `delivery_failed` is how a `send` that could not actually start a
/// turn fails instead of reporting ok. Kept in one testable place so the
/// protocol's vocabulary cannot drift between the two languages.
pub const INTERFACE_ERROR_CODES: [&str; 4] =
    ["invalid_request", "denied", "not_found", "delivery_failed"];

/// A machine-readable failure. `code` is stable so a client can branch on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

impl ErrorBody {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_request".into(),
            message: message.into(),
        }
    }
    pub fn denied(message: impl Into<String>) -> Self {
        Self {
            code: "denied".into(),
            message: message.into(),
        }
    }
    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            code: "not_found".into(),
            message: message.into(),
        }
    }
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: "internal".into(),
            message: message.into(),
        }
    }
}

/// One dispatch response. Always exactly one JSON line back to the client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

impl Response {
    pub fn ok(result: serde_json::Value) -> Self {
        Self {
            ok: true,
            result: Some(result),
            error: None,
        }
    }
    pub fn failed(error: ErrorBody) -> Self {
        Self {
            ok: false,
            result: None,
            error: Some(error),
        }
    }
}

/// Parse and validate one raw request line.
///
/// Validation is fail-closed: anything the entry point cannot fully understand
/// is rejected here, before the host is asked to do anything.
pub fn parse_request(raw: &str) -> Result<Request, ErrorBody> {
    let request: Request = serde_json::from_str(raw)
        .map_err(|error| ErrorBody::invalid(format!("malformed dispatch request: {error}")))?;
    validate_request(&request)?;
    Ok(request)
}

fn require_non_empty(field: &str, value: &str) -> Result<(), ErrorBody> {
    if value.trim().is_empty() {
        return Err(ErrorBody::invalid(format!("{field} must not be empty")));
    }
    Ok(())
}

fn require_permission_mode(mode: &str) -> Result<(), ErrorBody> {
    if PERMISSION_MODES.contains(&mode) {
        Ok(())
    } else {
        Err(ErrorBody::invalid(format!(
            "permission_mode must be one of {}",
            PERMISSION_MODES.join(", ")
        )))
    }
}

fn require_send_mode(mode: &str) -> Result<(), ErrorBody> {
    if SEND_MODES.contains(&mode) {
        Ok(())
    } else {
        Err(ErrorBody::invalid(format!(
            "mode must be one of {}",
            SEND_MODES.join(", ")
        )))
    }
}

/// Field-level validation shared by the socket server and the CLI client, so a
/// bad request is refused before it reaches the app regardless of direction.
pub fn validate_request(request: &Request) -> Result<(), ErrorBody> {
    match request {
        Request::CreateAndSend {
            project,
            message,
            model,
            permission_mode,
            ..
        } => {
            require_non_empty("project", project)?;
            require_non_empty("message", message)?;
            if let Some(model) = model {
                require_non_empty("model", model)?;
            }
            if let Some(mode) = permission_mode {
                require_permission_mode(mode)?;
            }
        }
        Request::Send {
            session_id,
            message,
            mode,
            ..
        } => {
            require_non_empty("session_id", session_id)?;
            require_non_empty("message", message)?;
            if let Some(mode) = mode {
                require_send_mode(mode)?;
            }
        }
        Request::SetPermission {
            session_id,
            permission_mode,
        } => {
            require_non_empty("session_id", session_id)?;
            require_permission_mode(permission_mode)?;
        }
        Request::Status { session_id } => require_non_empty("session_id", session_id)?,
        Request::SetModel { session_id, model } => {
            require_non_empty("model", model)?;
            if let Some(session_id) = session_id {
                require_non_empty("session_id", session_id)?;
            }
        }
        Request::SwitchSession { session_id } => require_non_empty("session_id", session_id)?,
        Request::Stop { session_id } => {
            if let Some(session_id) = session_id {
                require_non_empty("session_id", session_id)?;
            }
        }
        Request::ListApprovals {} => {}
        Request::ResolveApproval { approval_id, .. } => {
            require_non_empty("approval_id", approval_id)?
        }
        Request::FocusMainDisplay {} => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Admission (CF-HDE-R3)
// ---------------------------------------------------------------------------

/// How delivery (PR / merge / release) was authorized for a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryAuthorization {
    /// The caller stated it in the structured `delivery_authorized` field.
    Explicit,
    /// The caller did not ask for delivery. The run may still do ordinary work.
    NotRequested,
}

/// The admission decision for a submitted message.
///
/// This is the *only* place delivery authorization is decided, and it reads one
/// thing: the structured boolean. The text of the message is never consulted —
/// "please merge the PR" arriving without the field is still `NotRequested`
/// (and, because the field is mandatory, such a request never even parses).
///
/// GUI input and this entry point both land here, which is what makes "same
/// input, identical admission" checkable rather than aspirational.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admission {
    pub delivery: DeliveryAuthorization,
    /// Messages from the entry point always carry this marker so the session can
    /// show where the message came from (CF-HDE-R4).
    pub source: &'static str,
}

pub fn admit(delivery_authorized: bool) -> Admission {
    Admission {
        delivery: if delivery_authorized {
            DeliveryAuthorization::Explicit
        } else {
            DeliveryAuthorization::NotRequested
        },
        source: DISPATCH_SOURCE,
    }
}

// ---------------------------------------------------------------------------
// Audit log (CF-HDE-R4)
// ---------------------------------------------------------------------------

/// One audit record: who / when / which session / the original text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// RFC 3339 timestamp of the dispatch.
    pub at: String,
    /// Always [`DISPATCH_SOURCE`]; the field exists so the log stays readable
    /// once other sources are added.
    pub source: String,
    pub operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_authorized: Option<bool>,
}

impl AuditEntry {
    /// Build the "request arrived" record for `request` at `now`.
    pub fn for_request(request: &Request, now: chrono::DateTime<chrono::Utc>) -> Self {
        let mut entry = Self {
            at: now.to_rfc3339(),
            source: DISPATCH_SOURCE.to_string(),
            operation: operation_of(request).to_string(),
            session_id: None,
            project: None,
            message: None,
            delivery_authorized: None,
        };
        match request {
            Request::CreateAndSend {
                project,
                message,
                delivery_authorized,
                ..
            } => {
                entry.project = Some(project.clone());
                entry.message = Some(message.clone());
                entry.delivery_authorized = Some(*delivery_authorized);
            }
            Request::Send {
                session_id,
                message,
                delivery_authorized,
                ..
            } => {
                entry.session_id = Some(session_id.clone());
                entry.message = Some(message.clone());
                entry.delivery_authorized = Some(*delivery_authorized);
            }
            Request::SetPermission { session_id, .. }
            | Request::Status { session_id }
            | Request::SwitchSession { session_id } => {
                entry.session_id = Some(session_id.clone());
            }
            Request::SetModel { session_id, .. } | Request::Stop { session_id } => {
                entry.session_id = session_id.clone();
            }
            Request::ListApprovals {} | Request::FocusMainDisplay {} => {}
            Request::ResolveApproval { approval_id, .. } => {
                entry.message = Some(format!("approval_id={approval_id}"));
            }
        }
        entry
    }
}

/// The stable operation name for a request.
pub fn operation_of(request: &Request) -> &'static str {
    match request {
        Request::CreateAndSend { .. } => "create_and_send",
        Request::Send { .. } => "send",
        Request::SetPermission { .. } => "set_permission",
        Request::Status { .. } => "status",
        Request::SetModel { .. } => "set_model",
        Request::SwitchSession { .. } => "switch_session",
        Request::Stop { .. } => "stop",
        Request::ListApprovals {} => "list_approvals",
        Request::ResolveApproval { .. } => "resolve_approval",
        Request::FocusMainDisplay {} => "focus_main_display",
    }
}

/// Append-only JSON-lines audit log. The file is created `0600`.
#[derive(Debug, Clone)]
pub struct AuditLog {
    path: PathBuf,
}

impl AuditLog {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Append one record. Never rewrites: the log is the evidence trail.
    pub fn append(&self, entry: &AuditEntry) -> std::io::Result<()> {
        use std::io::Write;
        let line = serde_json::to_string(entry).unwrap_or_else(|_| "{}".to_string());
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&self.path)?;
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        file.flush()
    }
}

// ---------------------------------------------------------------------------
// Host
// ---------------------------------------------------------------------------

/// What the app can actually do with a request.
///
/// The server owns transport and security; the host owns behaviour. The shipping
/// implementation forwards to the same code paths the GUI uses; tests supply a
/// recording host.
pub trait DispatchHost: Send + Sync + 'static {
    fn execute(&self, request: &Request) -> Result<serde_json::Value, ErrorBody>;
}

/// The one and only peer check: the kernel-reported peer uid must equal the
/// current user's uid. Pulled out as a pure function so the counter-example
/// "another local user" is testable without a second account.
pub fn authorize_peer(peer_uid: u32, current_uid: u32) -> Result<(), ErrorBody> {
    if peer_uid != current_uid {
        return Err(ErrorBody::denied(
            "request came from a different user",
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Socket security (Unix only)
// ---------------------------------------------------------------------------

#[cfg(unix)]
pub mod socket {
    use super::{ErrorBody, MAX_REQUEST_BYTES};
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::fd::{AsRawFd, RawFd};
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};

    pub fn current_uid() -> u32 {
        // SAFETY: `geteuid` takes no arguments and cannot fail.
        unsafe { libc::geteuid() }
    }

    /// Read the peer's uid from the connected socket itself.
    ///
    /// This is the credential that matters: it comes from the kernel, not from
    /// anything either side claims.
    #[cfg(target_os = "macos")]
    pub fn peer_uid(fd: RawFd) -> Result<u32, ErrorBody> {
        // SAFETY: `xucred` is plain-old-data; the kernel fills it in.
        let mut cred: libc::xucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::xucred>() as libc::socklen_t;
        // SAFETY: valid fd, valid out-pointer, correct struct length.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_LOCAL,
                libc::LOCAL_PEERCRED,
                &mut cred as *mut libc::xucred as *mut libc::c_void,
                &mut len,
            )
        };
        if rc != 0 {
            return Err(ErrorBody::denied(format!(
                "could not read peer credentials: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(cred.cr_uid)
    }

    #[cfg(not(target_os = "macos"))]
    pub fn peer_uid(fd: RawFd) -> Result<u32, ErrorBody> {
        // SAFETY: `ucred` is plain-old-data; the kernel fills it in.
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: valid fd, valid out-pointer, correct struct length.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut libc::ucred as *mut libc::c_void,
                &mut len,
            )
        };
        if rc != 0 {
            return Err(ErrorBody::denied(format!(
                "could not read peer credentials: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(cred.uid)
    }

    pub fn peer_uid_of(stream: &UnixStream) -> Result<u32, ErrorBody> {
        peer_uid(stream.as_raw_fd())
    }

    /// The endpoint must be a real socket, owned by the current user, with no
    /// group/world permission bits at all.
    pub fn validate_socket(path: &Path, peer_uid: u32, current_uid: u32) -> Result<(), ErrorBody> {
        let metadata = std::fs::symlink_metadata(path).map_err(|error| {
            ErrorBody::denied(format!("dispatch endpoint is not usable: {error}"))
        })?;
        if !metadata.file_type().is_socket() {
            return Err(ErrorBody::denied(
                "dispatch endpoint is not a Unix socket".to_string(),
            ));
        }
        if metadata.uid() != current_uid {
            return Err(ErrorBody::denied(
                "dispatch socket is not owned by the current user".to_string(),
            ));
        }
        if metadata.mode() & 0o077 != 0 {
            return Err(ErrorBody::denied(
                "dispatch socket is reachable by other users".to_string(),
            ));
        }
        if peer_uid != current_uid {
            return Err(ErrorBody::denied(
                "request came from a different user".to_string(),
            ));
        }
        Ok(())
    }

    /// The directory holding the socket must not be group/world writable —
    /// otherwise another user could swap the socket out from under us.
    pub fn validate_socket_dir(dir: &Path, current_uid: u32) -> Result<(), ErrorBody> {
        let metadata = std::fs::symlink_metadata(dir)
            .map_err(|error| ErrorBody::denied(format!("dispatch directory unusable: {error}")))?;
        if !metadata.is_dir() {
            return Err(ErrorBody::denied(
                "dispatch directory is not a directory".to_string(),
            ));
        }
        if metadata.uid() != current_uid {
            return Err(ErrorBody::denied(
                "dispatch directory is not owned by the current user".to_string(),
            ));
        }
        if metadata.mode() & 0o022 != 0 {
            return Err(ErrorBody::denied(
                "dispatch directory is writable by other users".to_string(),
            ));
        }
        Ok(())
    }

    /// Remove a stale socket, but never clobber something we did not create.
    pub fn prepare_socket(path: &Path) -> Result<(), ErrorBody> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => {
                let owned = metadata.uid() == current_uid();
                let private = metadata.mode() & 0o077 == 0;
                if !metadata.file_type().is_socket() || !owned || !private {
                    return Err(ErrorBody::denied(
                        "refusing to replace a dispatch endpoint we do not own".to_string(),
                    ));
                }
                std::fs::remove_file(path)
                    .map_err(|error| ErrorBody::denied(format!("stale socket: {error}")))
            }
            Err(_) => Ok(()),
        }
    }

    /// Create the socket with mode `0600` before anyone can connect.
    pub fn bind(path: &Path) -> Result<UnixListener, ErrorBody> {
        let uid = current_uid();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|error| ErrorBody::denied(format!("dispatch dir: {error}")))?;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            validate_socket_dir(dir, uid)?;
        }
        prepare_socket(path)?;
        let listener = bind_private(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| ErrorBody::denied(format!("socket mode: {error}")))?;
        validate_socket(path, uid, uid)?;
        Ok(listener)
    }

    fn bind_private(path: &Path) -> Result<UnixListener, ErrorBody> {
        // SAFETY: `umask` only changes this process's file-creation mask.
        let previous = unsafe { libc::umask(0o177) };
        let listener = UnixListener::bind(path);
        // SAFETY: restore the caller's mask immediately.
        unsafe { libc::umask(previous) };
        listener.map_err(|error| ErrorBody::denied(format!("dispatch bind failed: {error}")))
    }

    /// Read one request line, refusing anything larger than the cap.
    pub fn read_line_capped(reader: &mut BufReader<UnixStream>) -> Result<String, ErrorBody> {
        let mut line = String::new();
        let read = reader
            .take(MAX_REQUEST_BYTES as u64 + 1)
            .read_line(&mut line)
            .map_err(|error| ErrorBody::invalid(format!("could not read request: {error}")))?;
        if read == 0 {
            return Err(ErrorBody::invalid("empty request".to_string()));
        }
        if line.len() > MAX_REQUEST_BYTES {
            return Err(ErrorBody::invalid(format!(
                "request exceeds {MAX_REQUEST_BYTES} bytes"
            )));
        }
        Ok(line)
    }

    pub fn write_line(stream: &mut UnixStream, body: &str) -> std::io::Result<()> {
        stream.write_all(body.as_bytes())?;
        stream.write_all(b"\n")?;
        stream.flush()
    }

    /// Default socket path: inside the app's private per-user data directory.
    pub fn default_socket_path() -> Option<PathBuf> {
        let base = if let Ok(dir) = std::env::var("CODEFACTORY_DISPATCH_DIR") {
            PathBuf::from(dir)
        } else {
            let home = std::env::var("HOME").ok()?;
            if cfg!(target_os = "macos") {
                PathBuf::from(home).join("Library/Application Support/com.codefactory.app")
            } else {
                PathBuf::from(home).join(".config/com.codefactory.app")
            }
        };
        Some(base.join("dispatch.sock"))
    }

    /// Default audit log path, next to the socket.
    pub fn default_audit_path() -> Option<PathBuf> {
        default_socket_path().map(|path| path.with_file_name("dispatch-audit.log"))
    }
}

// ---------------------------------------------------------------------------
// Server (Unix only)
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod server {
    use super::socket;
    use super::{AuditEntry, AuditLog, DispatchHost, ErrorBody, Response};
    use std::io::BufReader;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::Path;
    use std::sync::Arc;

    /// Answer one connection: authenticate, parse, audit, execute, reply.
    ///
    /// The peer check happens **before** the body is read, so a foreign user
    /// gets a denial without the server doing any work on their behalf.
    fn handle_connection(
        mut stream: UnixStream,
        host: &Arc<dyn DispatchHost>,
        audit: Option<&AuditLog>,
        current_uid: u32,
    ) {
        let peer = match socket::peer_uid_of(&stream) {
            Ok(uid) => uid,
            Err(error) => {
                reply(&mut stream, Response::failed(error));
                return;
            }
        };
        if let Err(error) = super::authorize_peer(peer, current_uid) {
            reply(&mut stream, Response::failed(error));
            return;
        }

        let mut reader = BufReader::new(match stream.try_clone() {
            Ok(clone) => clone,
            Err(error) => {
                reply(
                    &mut stream,
                    Response::failed(ErrorBody::internal(format!(
                        "connection clone failed: {error}"
                    ))),
                );
                return;
            }
        });

        let response = match socket::read_line_capped(&mut reader) {
            Ok(line) => match super::parse_request(line.trim()) {
                Ok(request) => {
                    if let Some(audit) = audit {
                        let entry = AuditEntry::for_request(&request, chrono::Utc::now());
                        let _ = audit.append(&entry);
                    }
                    match host.execute(&request) {
                        Ok(result) => Response::ok(result),
                        Err(error) => Response::failed(error),
                    }
                }
                Err(error) => Response::failed(error),
            },
            Err(error) => Response::failed(error),
        };
        reply(&mut stream, response);
    }

    fn reply(stream: &mut UnixStream, response: Response) {
        let body = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
        let _ = socket::write_line(stream, &body);
    }

    /// Serve requests until the process exits. Blocking; call from a dedicated
    /// thread. Never opens a network listener.
    pub fn serve(
        listener: UnixListener,
        host: Arc<dyn DispatchHost>,
        audit: Option<AuditLog>,
    ) -> std::io::Result<()> {
        let current_uid = socket::current_uid();
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let host = Arc::clone(&host);
                    let audit = audit.clone();
                    std::thread::spawn(move || {
                        handle_connection(stream, &host, audit.as_ref(), current_uid)
                    });
                }
                Err(error) => tracing::warn!("dispatch accept failed: {error}"),
            }
        }
        Ok(())
    }

    /// Bind the socket and serve it on a background thread.
    pub fn spawn(
        path: &Path,
        host: Arc<dyn DispatchHost>,
        audit: Option<AuditLog>,
    ) -> Result<(), ErrorBody> {
        let listener = socket::bind(path)?;
        std::thread::Builder::new()
            .name("headless-dispatch".to_string())
            .spawn(move || {
                if let Err(error) = serve(listener, host, audit) {
                    tracing::warn!("headless dispatch server stopped: {error}");
                }
            })
            .map_err(|error| ErrorBody::internal(format!("dispatch thread: {error}")))?;
        Ok(())
    }
}

#[cfg(unix)]
pub use server::{serve, spawn};

// ---------------------------------------------------------------------------
// Client (Unix only)
// ---------------------------------------------------------------------------

#[cfg(unix)]
pub mod client {
    use super::socket;
    use super::{ErrorBody, Response};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::path::Path;

    /// Send one raw request line and return the raw response line.
    pub fn request_raw(socket_path: &Path, request_json: &str) -> Result<String, ErrorBody> {
        let mut stream = UnixStream::connect(socket_path).map_err(|error| {
            ErrorBody::internal(format!(
                "CodeFactory is not accepting local tasks ({}): {error}",
                socket_path.display()
            ))
        })?;
        stream
            .write_all(request_json.trim().as_bytes())
            .and_then(|()| stream.write_all(b"\n"))
            .and_then(|()| stream.flush())
            .map_err(|error| ErrorBody::internal(format!("could not send request: {error}")))?;
        let mut reader = BufReader::new(
            stream
                .try_clone()
                .map_err(|error| ErrorBody::internal(format!("connection lost: {error}")))?,
        );
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|error| ErrorBody::internal(format!("could not read reply: {error}")))?;
        if line.trim().is_empty() {
            return Err(ErrorBody::internal("the app closed the connection".to_string()));
        }
        Ok(line.trim().to_string())
    }

    /// Send a request and parse the structured reply.
    pub fn request(socket_path: &Path, request_json: &str) -> Result<Response, ErrorBody> {
        let raw = request_raw(socket_path, request_json)?;
        serde_json::from_str(&raw)
            .map_err(|error| ErrorBody::internal(format!("malformed reply: {error}")))
    }

    /// Convenience: does the endpoint exist and look private?
    pub fn endpoint_ready(socket_path: &Path) -> Result<(), ErrorBody> {
        let uid = socket::current_uid();
        socket::validate_socket(socket_path, uid, uid)
    }
}

/// CLI entry for the dispatch client. Returns `true` when this process was a
/// dispatch client (and the caller should not start the GUI).
///
/// Usage:
///   CodeFactory --dispatch '<json request>'
///   CodeFactory --dispatch -            (read the request JSON from stdin)
#[cfg(unix)]
pub fn run_client_cli() -> bool {
    let args: Vec<String> = std::env::args().collect();
    let Some(flag) = args.get(1).map(String::as_str) else {
        return false;
    };
    if flag != "--dispatch" {
        return false;
    }
    let payload = match args.get(2).map(String::as_str) {
        Some("-") | None => {
            use std::io::Read;
            let mut buffer = String::new();
            if let Err(error) = std::io::stdin().read_to_string(&mut buffer) {
                eprintln!("could not read the request from stdin: {error}");
                std::process::exit(2);
            }
            buffer
        }
        Some(raw) => raw.to_string(),
    };
    let Some(socket_path) = socket::default_socket_path() else {
        eprintln!("could not resolve the local task entry point path");
        std::process::exit(2);
    };
    match client::request(&socket_path, &payload) {
        Ok(response) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&response).unwrap_or_default()
            );
            if response.ok {
                std::process::exit(0);
            } else {
                std::process::exit(1);
            }
        }
        Err(error) => {
            eprintln!("{}: {}", error.code, error.message);
            std::process::exit(2);
        }
    }
}

// ---------------------------------------------------------------------------
// Bridge into the running app (the shipping host)
// ---------------------------------------------------------------------------

/// Event the app's interface listens on (`src/lib/desktopDispatch.ts`).
pub const DISPATCH_REQUEST_EVENT: &str = "dispatch:request";

#[cfg(unix)]
pub mod bridge {
    use super::{
        operation_of, AuditLog, DispatchHost, ErrorBody, Request, Response,
        DISPATCH_REQUEST_EVENT,
    };
    use std::collections::HashMap;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tauri::{AppHandle, Emitter, Manager, Wry};

    /// Forwards requests to the interface and waits for the reply.
    ///
    /// The behaviour is deliberately *not* re-implemented here: the request
    /// travels to the same handlers the GUI uses and comes back as one response,
    /// which is how CF-HDE-R3 ("identical admission") stays true by
    /// construction instead of by review.
    pub struct DispatchBridge {
        app: AppHandle<Wry>,
        pending: Mutex<HashMap<String, Sender<Response>>>,
        timeout: Duration,
    }

    impl DispatchBridge {
        pub fn new(app: AppHandle<Wry>) -> Self {
            let timeout = std::env::var("CODEFACTORY_DISPATCH_TIMEOUT_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .map(Duration::from_millis)
                .unwrap_or_else(|| Duration::from_secs(120));
            Self {
                app,
                pending: Mutex::new(HashMap::new()),
                timeout,
            }
        }

        /// Called by the `dispatch_reply` command with the interface's answer.
        /// Returns `false` when the request already timed out (a late reply is
        /// dropped, never applied twice).
        pub fn settle(&self, request_id: &str, response: Response) -> bool {
            let sender = self
                .pending
                .lock()
                .ok()
                .and_then(|mut map| map.remove(request_id));
            match sender {
                Some(sender) => sender.send(response).is_ok(),
                None => false,
            }
        }

        fn ask_interface(&self, request: &Request) -> Result<serde_json::Value, ErrorBody> {
            let request_id = uuid::Uuid::new_v4().to_string();
            let (tx, rx): (Sender<Response>, Receiver<Response>) = channel();
            self.pending
                .lock()
                .map_err(|_| ErrorBody::internal("dispatch bridge is poisoned"))?
                .insert(request_id.clone(), tx);
            let payload = serde_json::json!({
                "request_id": request_id,
                "operation": operation_of(request),
                "request": request,
            });
            if let Err(error) = self.app.emit(DISPATCH_REQUEST_EVENT, payload) {
                self.settle(&request_id, Response::failed(ErrorBody::internal("dropped")));
                return Err(ErrorBody::internal(format!(
                    "could not reach the app interface: {error}"
                )));
            }
            match rx.recv_timeout(self.timeout) {
                Ok(response) if response.ok => Ok(response.result.unwrap_or(serde_json::Value::Null)),
                Ok(response) => Err(response
                    .error
                    .unwrap_or_else(|| ErrorBody::internal("the request was refused"))),
                Err(_) => Err(ErrorBody::internal(format!(
                    "the app interface did not answer within {:?}",
                    self.timeout
                ))),
            }
        }
    }

    impl DispatchHost for DispatchBridge {
        fn execute(&self, request: &Request) -> Result<serde_json::Value, ErrorBody> {
            match request {
                // Window placement has no interface-side equivalent, so it is
                // done natively — and it does not need the screen unlocked.
                Request::FocusMainDisplay {} => super::focus_main_display(&self.app),
                _ => self.ask_interface(request),
            }
        }
    }

    /// Bind the private socket and serve it on a background thread.
    pub fn start(app: &AppHandle<Wry>) -> Result<Arc<DispatchBridge>, ErrorBody> {
        let Some(path) = super::socket::default_socket_path() else {
            return Err(ErrorBody::internal(
                "could not resolve the local task entry point path",
            ));
        };
        let audit = super::socket::default_audit_path().map(AuditLog::new);
        let bridge = Arc::new(DispatchBridge::new(app.clone()));
        let host: Arc<dyn DispatchHost> = bridge.clone();
        super::spawn(&path, host, audit)?;
        tracing::info!("local task entry listening on {}", path.display());
        Ok(bridge)
    }
}

/// Move the main window back onto the main display. Native, so it works while
/// the screen is locked.
#[cfg(unix)]
pub fn focus_main_display(app: &tauri::AppHandle<tauri::Wry>) -> Result<serde_json::Value, ErrorBody> {
    use tauri::Manager;
    let window = app
        .get_window("main")
        .ok_or_else(|| ErrorBody::not_found("the app has no main window"))?;
    let monitor = window
        .primary_monitor()
        .map_err(|error| ErrorBody::internal(format!("could not read displays: {error}")))?
        .ok_or_else(|| ErrorBody::not_found("no main display is available"))?;
    let monitor_position = monitor.position();
    let monitor_size = monitor.size();
    let window_size = window
        .outer_size()
        .map_err(|error| ErrorBody::internal(format!("could not read the window size: {error}")))?;

    // Centre the window on the main display, never off its top-left corner.
    let x = monitor_position.x
        + ((monitor_size.width as i32 - window_size.width as i32).max(0) / 2);
    let y = monitor_position.y + 40;

    let already_there = window
        .current_monitor()
        .ok()
        .flatten()
        .zip(window.outer_position().ok())
        .is_some_and(|(current, position)| {
            current.position() == monitor_position
                && position.x >= monitor_position.x
                && position.y >= monitor_position.y
        });

    if !already_there {
        window
            .set_position(tauri::PhysicalPosition::new(x, y))
            .map_err(|error| ErrorBody::internal(format!("could not move the window: {error}")))?;
    }
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();

    Ok(serde_json::json!({
        "moved": !already_there,
        "display": { "x": monitor_position.x, "y": monitor_position.y },
    }))
}

/// Non-Unix platforms have no private per-user socket in this change, so the
/// entry point reports that plainly instead of pretending to work.
#[cfg(not(unix))]
pub mod bridge {
    use super::{ErrorBody, Response};
    use std::sync::Arc;

    /// Stand-in for the Unix bridge: nothing can be forwarded because there is
    /// no private per-user endpoint to accept a request on this platform.
    #[derive(Debug)]
    pub struct DispatchBridge;

    impl DispatchBridge {
        pub fn settle(&self, _request_id: &str, _response: Response) -> bool {
            false
        }
    }

    pub fn start(_app: &tauri::AppHandle<tauri::Wry>) -> Result<Arc<DispatchBridge>, ErrorBody> {
        Err(ErrorBody::internal(
            "the local task entry point is not available on this platform yet",
        ))
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn send_json(message: &str, delivery: Option<bool>) -> String {
        match delivery {
            Some(value) => format!(
                r#"{{"operation":"send","session_id":"s-1","message":"{message}","delivery_authorized":{value}}}"#
            ),
            None => format!(r#"{{"operation":"send","session_id":"s-1","message":"{message}"}}"#),
        }
    }

    #[test]
    fn delivery_authorization_is_required_and_never_inferred_from_wording() {
        // The text explicitly asks for a merge, and the request still fails:
        // only the structured field can authorize delivery (M48).
        let raw = r#"{"operation":"send","session_id":"s-1","message":"please open a PR and merge it"}"#;
        assert!(parse_request(raw).is_err());
        let request = parse_request(&send_json("merge it", Some(false)))
            .expect("the explicit field is accepted as data");
        assert!(matches!(
            request,
            Request::Send {
                delivery_authorized: false,
                ..
            }
        ));
        assert_eq!(admit(false).delivery, DeliveryAuthorization::NotRequested);
        assert_eq!(admit(true).delivery, DeliveryAuthorization::Explicit);
        assert_eq!(admit(false).source, DISPATCH_SOURCE);
    }

    #[test]
    fn a_typo_in_a_field_name_is_rejected_instead_of_silently_dropped() {
        let raw = r#"{"operation":"send","session_id":"s-1","message":"hi","delivery_authorized":true,"delivery_authrized":true}"#;
        assert!(parse_request(raw).is_err());
    }

    #[test]
    fn unknown_operations_are_rejected() {
        assert!(parse_request(r#"{"operation":"run_shell","command":"rm -rf /"}"#).is_err());
        // A transport address is not an operation.
        assert!(parse_request(r#"{"operation":"tcp://127.0.0.1:1234"}"#).is_err());
        assert!(parse_request("").is_err());
        assert!(parse_request("not json at all").is_err());
    }

    #[test]
    fn send_mode_is_a_closed_set_and_defaults_to_steer() {
        // CF-HDE-R8: `mode` is optional (absent = the interface's plain Enter,
        // i.e. steer), and an unknown direction is refused rather than guessed —
        // guessing wrong means the user thinks they interjected and they queued.
        let absent = parse_request(&send_json("go", Some(false)))
            .expect("send without mode stays legal");
        assert!(matches!(absent, Request::Send { mode: None, .. }));
        for mode in SEND_MODES {
            let raw = format!(
                r#"{{"operation":"send","session_id":"s-1","message":"go","delivery_authorized":false,"mode":"{mode}"}}"#
            );
            let request = parse_request(&raw).expect("a known mode is accepted");
            assert!(matches!(request, Request::Send { mode: Some(_), .. }));
        }
        let bogus = r#"{"operation":"send","session_id":"s-1","message":"go","delivery_authorized":false,"mode":"shout"}"#;
        let error = parse_request(bogus).expect_err("an unknown direction must be refused");
        assert_eq!(error.code, "invalid_request");
    }

    #[test]
    fn interface_error_codes_are_stable_and_round_trip() {
        // CF-HDE-R0 / R7: the interface reports these codes back over the same
        // Response envelope, so they must deserialize here byte-for-byte.
        for code in INTERFACE_ERROR_CODES {
            let raw = format!(r#"{{"code":"{code}","message":"x"}}"#);
            let body: ErrorBody = serde_json::from_str(&raw).expect("a known code round-trips");
            assert_eq!(body.code, code);
        }
        assert_eq!(ErrorBody::not_found("x").code, "not_found");
        assert_eq!(ErrorBody::internal("x").code, "internal");
    }

    #[test]
    fn field_validation_is_fail_closed() {
        assert!(parse_request(&send_json("   ", Some(false))).is_err());
        assert!(parse_request(r#"{"operation":"send","session_id":"","message":"hi","delivery_authorized":false}"#).is_err());
        assert!(parse_request(
            r#"{"operation":"set_permission","session_id":"s-1","permission_mode":"godmode"}"#
        )
        .is_err());
        assert!(parse_request(r#"{"operation":"set_model","model":""}"#).is_err());
        assert!(parse_request(r#"{"operation":"resolve_approval","approval_id":"","approve":true}"#).is_err());
        for mode in PERMISSION_MODES {
            assert!(parse_request(&format!(
                r#"{{"operation":"set_permission","session_id":"s-1","permission_mode":"{mode}"}}"#
            ))
            .is_ok());
        }
    }

    #[test]
    fn every_r6_capability_has_a_parseable_request() {
        // CF-HDE-R6: model pick/change, switch session, stop, approvals,
        // main-display move — all reachable from the background entry.
        for raw in [
            r#"{"operation":"create_and_send","project":"/tmp/p","message":"go","model":"deepseek/v4","permission_mode":"trusted","delivery_authorized":true}"#,
            r#"{"operation":"set_model","session_id":"s-1","model":"deepseek/v4"}"#,
            r#"{"operation":"set_model","model":"deepseek/v4"}"#,
            r#"{"operation":"switch_session","session_id":"s-1"}"#,
            r#"{"operation":"stop","session_id":"s-1"}"#,
            r#"{"operation":"stop"}"#,
            r#"{"operation":"list_approvals"}"#,
            r#"{"operation":"resolve_approval","approval_id":"a-1","approve":true}"#,
            r#"{"operation":"focus_main_display"}"#,
        ] {
            parse_request(raw).unwrap_or_else(|error| panic!("{raw} -> {}", error.message));
        }
    }

    #[test]
    fn audit_records_who_when_session_and_the_original_text() {
        let request = parse_request(&send_json("fix #590", Some(true))).unwrap();
        let entry = AuditEntry::for_request(&request, chrono::Utc::now());
        assert_eq!(entry.source, DISPATCH_SOURCE);
        assert_eq!(entry.operation, "send");
        assert_eq!(entry.session_id.as_deref(), Some("s-1"));
        assert_eq!(entry.message.as_deref(), Some("fix #590"));
        assert_eq!(entry.delivery_authorized, Some(true));
        assert!(!entry.at.is_empty());
    }

    #[test]
    fn audit_log_is_append_only_json_lines_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dispatch-audit.log");
        let log = AuditLog::new(&path);
        let request = parse_request(&send_json("one", Some(false))).unwrap();
        log.append(&AuditEntry::for_request(&request, chrono::Utc::now()))
            .unwrap();
        log.append(&AuditEntry::for_request(&request, chrono::Utc::now()))
            .unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body.lines().count(), 2);
        for line in body.lines() {
            let parsed: AuditEntry = serde_json::from_str(line).unwrap();
            assert_eq!(parsed.source, DISPATCH_SOURCE);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
                0
            );
        }
    }

    #[test]
    fn another_local_user_is_rejected() {
        assert!(authorize_peer(1000, 1001).is_err());
        assert!(authorize_peer(1001, 1001).is_ok());
    }
}

#[cfg(all(test, unix))]
mod unix_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};

    fn private_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        dir
    }

    #[test]
    fn a_world_readable_or_non_socket_endpoint_is_rejected() {
        let uid = socket::current_uid();
        let dir = private_dir();
        let path = dir.path().join("dispatch.sock");

        // A regular file is not an endpoint.
        std::fs::write(&path, "not a socket").unwrap();
        assert!(socket::validate_socket(&path, uid, uid).is_err());

        // A socket reachable by other users is refused too.
        std::fs::remove_file(&path).unwrap();
        let _listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(socket::validate_socket(&path, uid, uid).is_err());

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(socket::validate_socket(&path, uid, uid).is_ok());
    }

    #[test]
    fn a_group_writable_directory_is_rejected() {
        let uid = socket::current_uid();
        let dir = tempfile::tempdir().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(socket::validate_socket_dir(dir.path(), uid).is_err());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(socket::validate_socket_dir(dir.path(), uid).is_ok());
    }

    #[test]
    fn bind_creates_a_private_socket_and_refuses_to_clobber_foreign_files() {
        let dir = private_dir();
        let path = dir.path().join("dispatch.sock");
        let listener = socket::bind(&path).expect("a fresh private socket binds");
        drop(listener);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
            0,
            "socket must not be group/world accessible"
        );

        // Something that is not our socket must never be replaced.
        let other = dir.path().join("other.sock");
        std::fs::write(&other, "important").unwrap();
        assert!(socket::prepare_socket(&other).is_err());
        assert!(other.exists());
    }

    #[test]
    fn the_kernel_reports_the_connected_peers_uid() {
        // Two ends of one socket pair are, by construction, the same user.
        // This is the credential the server trusts.
        let (_client, server) = UnixStream::pair().unwrap();
        assert_eq!(socket::peer_uid_of(&server).unwrap(), socket::current_uid());
    }

    #[test]
    fn server_answers_a_real_socket_client_and_writes_the_audit_log() {
        struct Recording;
        impl DispatchHost for Recording {
            fn execute(&self, request: &Request) -> Result<serde_json::Value, ErrorBody> {
                Ok(serde_json::json!({ "operation": operation_of(request) }))
            }
        }

        let dir = private_dir();
        let path = dir.path().join("dispatch.sock");
        let audit_path = dir.path().join("dispatch-audit.log");
        let listener = socket::bind(&path).unwrap();
        let host: std::sync::Arc<dyn DispatchHost> = std::sync::Arc::new(Recording);
        let audit = AuditLog::new(&audit_path);
        std::thread::spawn(move || {
            let _ = server::serve(listener, host, Some(audit));
        });

        let response = client::request(
            &path,
            r#"{"operation":"status","session_id":"s-1"}"#,
        )
        .expect("a real client reaches the server");
        assert!(response.ok);
        assert_eq!(
            response.result.unwrap()["operation"],
            serde_json::json!("status")
        );

        // A request the entry point cannot understand is refused, and the
        // refusal never reaches the host as an executed action.
        let bad = client::request(&path, r#"{"operation":"exec","cmd":"id"}"#).unwrap();
        assert!(!bad.ok);
        assert_eq!(bad.error.unwrap().code, "invalid_request");

        let logged = std::fs::read_to_string(&audit_path).unwrap();
        assert!(
            logged.contains("status") && !logged.contains("\"operation\":\"exec\""),
            "only parsed requests are audited: {logged}"
        );
    }
}

