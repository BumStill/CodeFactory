// SPDX-License-Identifier: Apache-2.0
//! U34 acceptance: the local-only dispatch entry, exercised from *outside* the
//! crate through its public API exactly the way the CLI client uses it.
//!
//! These are the counter-example tests the spec calls the focus of acceptance
//! (CF-HDE-R2 / CF-HDE-R3), plus one end-to-end round trip proving a real socket
//! client is answered with the real protocol (CF-HDE-R1 / CF-HDE-R6).

#![cfg(unix)]

use codefactory_lib::headless_dispatch::{
    client, parse_request, socket, AuditEntry, AuditLog, DeliveryAuthorization, DispatchHost,
    ErrorBody, Request, Response, DISPATCH_SOURCE, PERMISSION_MODES,
};
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};

/// A host that records every request it was asked to execute and answers with
/// the operation name, so the test can prove a rejected request never reaches it.
struct RecordingHost {
    seen: Arc<Mutex<Vec<&'static str>>>,
    projects_that_exist: Vec<String>,
}

impl DispatchHost for RecordingHost {
    fn execute(&self, request: &Request) -> Result<serde_json::Value, ErrorBody> {
        self.seen
            .lock()
            .expect("host mutex")
            .push(codefactory_lib::headless_dispatch::operation_of(request));
        match request {
            // CF-HDE-R1/R2: a project that does not exist is refused by the host,
            // not silently turned into "create a project somewhere".
            Request::CreateAndSend { project, .. } => {
                if !self.projects_that_exist.iter().any(|known| known == project) {
                    return Err(ErrorBody::not_found("project not found".to_string()));
                }
                Ok(serde_json::json!({ "session_id": "created-1" }))
            }
            Request::Status { session_id } => Ok(serde_json::json!({
                "session_id": session_id,
                "objective_state": "running",
                "latest_reply": "working on it",
                "pr_number": 590,
            })),
            Request::ListApprovals {} => Ok(serde_json::json!([{ "approval_id": "a-1", "risk": "high" }])),
            _ => Ok(serde_json::json!({ "ok": true })),
        }
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    socket: std::path::PathBuf,
    audit: std::path::PathBuf,
    host: Arc<RecordingHost>,
    seen: Arc<Mutex<Vec<&'static str>>>,
}

fn harness() -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).expect("chmod");
    let socket_path = dir.path().join("dispatch.sock");
    let audit_path = dir.path().join("dispatch-audit.log");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let host = Arc::new(RecordingHost {
        seen: Arc::clone(&seen),
        projects_that_exist: vec!["/tmp/known-project".to_string()],
    });
    let listener = socket::bind(&socket_path).expect("private socket binds");
    let dyn_host: Arc<dyn DispatchHost> = host.clone();
    let audit = AuditLog::new(&audit_path);
    std::thread::spawn(move || {
        let _ = codefactory_lib::headless_dispatch::serve(listener, dyn_host, Some(audit));
    });
    Harness {
        _dir: dir,
        socket: socket_path,
        audit: audit_path,
        host,
        seen,
    }
}

fn send(h: &Harness, raw: &str) -> Response {
    client::request(&h.socket, raw).expect("the server answers on a real socket")
}

#[test]
fn normal_path_creates_sends_sets_permission_and_reads_state() {
    let h = harness();

    // CF-HDE-R1: create a session in a named project and send the first message.
    let created = send(
        &h,
        r#"{"operation":"create_and_send","project":"/tmp/known-project","message":"fix #590","delivery_authorized":true}"#,
    );
    assert!(created.ok, "create_and_send: {:?}", created.error);
    assert_eq!(created.result.unwrap()["session_id"], "created-1");

    // CF-HDE-R1: set the permission mode (trusted, so an unattended run proceeds).
    assert!(send(
        &h,
        r#"{"operation":"set_permission","session_id":"s-1","permission_mode":"trusted"}"#
    )
    .ok);

    // CF-HDE-R1: query objective state / latest reply / PR number.
    let status = send(&h, r#"{"operation":"status","session_id":"s-1"}"#);
    assert!(status.ok);
    let status = status.result.unwrap();
    assert_eq!(status["objective_state"], "running");
    assert_eq!(status["pr_number"], 590);

    // CF-HDE-R1: send a follow-up to an existing session.
    assert!(send(
        &h,
        r#"{"operation":"send","session_id":"s-1","message":"continue","delivery_authorized":false}"#
    )
    .ok);
}

#[test]
fn every_r6_capability_round_trips_over_the_real_socket() {
    let h = harness();
    // CF-HDE-R6: model pick/change, switch session, stop, approvals, display.
    for raw in [
        r#"{"operation":"set_model","session_id":"s-1","model":"deepseek/v4-pro"}"#,
        r#"{"operation":"set_model","model":"deepseek/v4-pro"}"#,
        r#"{"operation":"switch_session","session_id":"s-2"}"#,
        r#"{"operation":"stop"}"#,
        r#"{"operation":"list_approvals"}"#,
        r#"{"operation":"focus_main_display"}"#,
    ] {
        let response = send(&h, raw);
        assert!(response.ok, "{raw} -> {:?}", response.error);
    }

    // CF-HDE-R6: an approval is resolved one request at a time — never blanket.
    let listed = send(&h, r#"{"operation":"list_approvals"}"#);
    assert_eq!(listed.result.unwrap()[0]["approval_id"], "a-1");
    assert!(send(
        &h,
        r#"{"operation":"resolve_approval","approval_id":"a-1","approve":false}"#
    )
    .ok);
}

#[test]
fn counter_example_a_request_without_delivery_authorization_never_executes() {
    let h = harness();
    // The message *asks* for delivery. Without the structured field it is not a
    // request at all — and the host must never see it (CF-HDE-R3, M48).
    let refused = send(
        &h,
        r#"{"operation":"send","session_id":"s-1","message":"open a PR and merge it"}"#,
    );
    assert!(!refused.ok);
    assert_eq!(refused.error.unwrap().code, "invalid_request");
    assert!(
        h.seen.lock().unwrap().is_empty(),
        "an unparseable request must not reach the host"
    );
}

#[test]
fn counter_example_nonexistent_project_is_refused_by_the_host() {
    let h = harness();
    let refused = send(
        &h,
        r#"{"operation":"create_and_send","project":"/definitely/not/a/project","message":"work","delivery_authorized":false}"#,
    );
    assert!(!refused.ok);
    assert_eq!(refused.error.unwrap().code, "not_found");
}

#[test]
fn counter_example_unknown_operation_and_network_transport_are_refused() {
    let h = harness();
    for raw in [
        r#"{"operation":"exec","cmd":"id"}"#,
        r#"{"operation":"tcp://127.0.0.1:1234"}"#,
        r#"{"operation":"send","session_id":"s-1","message":"hi","delivery_authorized":true,"unexpected":1}"#,
    ] {
        let refused = send(&h, raw);
        assert!(!refused.ok, "{raw} must be refused");
        assert_eq!(refused.error.unwrap().code, "invalid_request");
    }
    assert!(h.seen.lock().unwrap().is_empty());
}

#[test]
fn counter_example_another_user_or_a_public_endpoint_is_refused() {
    let uid = socket::current_uid();
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("dispatch.sock");
    let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

    // A different peer uid is rejected even though the endpoint itself is fine.
    assert!(socket::validate_socket(&path, uid + 1, uid).is_err());
    // A socket reachable by another user is rejected.
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(socket::validate_socket(&path, uid, uid).is_err());
    // So is a plain file masquerading as the endpoint.
    let impostor = dir.path().join("dispatch.log");
    std::fs::write(&impostor, "x").unwrap();
    assert!(socket::validate_socket(&impostor, uid, uid).is_err());
}

#[test]
fn counter_example_no_network_listener_is_in_use() {
    // The protocol has no transport other than a Unix socket: a request whose
    // "operation" is an address cannot parse, and the client only ever dials a
    // filesystem path. This asserts the *shape*, since a real TCP port would
    // show up as a parsed operation.
    assert!(parse_request(r#"{"operation":"http://0.0.0.0:8080/task"}"#).is_err());
    assert!(client::endpoint_ready(std::path::Path::new("/tmp/definitely-absent.sock")).is_err());
}

#[test]
fn audit_log_records_source_time_session_and_original_text() {
    let h = harness();
    assert!(send(
        &h,
        r#"{"operation":"send","session_id":"s-9","message":"the original text","delivery_authorized":true}"#
    )
    .ok);
    let body = std::fs::read_to_string(&h.audit).expect("audit log written");
    let entry: AuditEntry = serde_json::from_str(body.lines().next().unwrap()).unwrap();
    assert_eq!(entry.source, DISPATCH_SOURCE);
    assert_eq!(entry.session_id.as_deref(), Some("s-9"));
    assert_eq!(entry.message.as_deref(), Some("the original text"));
    assert_eq!(entry.delivery_authorized, Some(true));
    assert!(!entry.at.is_empty(), "audit entries are timestamped");
}

#[test]
fn admission_is_identical_whether_delivery_was_requested_or_not() {
    // CF-HDE-R3: the entry point decides delivery from the structured field
    // only, which is the same rule GUI input is held to. Identical input,
    // identical admission.
    let from_entry = parse_request(
        r#"{"operation":"send","session_id":"s-1","message":"ship it","delivery_authorized":true}"#,
    )
    .unwrap();
    let Request::Send {
        delivery_authorized, ..
    } = from_entry
    else {
        panic!("expected a send request");
    };
    assert!(delivery_authorized);
    assert_eq!(
        codefactory_lib::headless_dispatch::admit(delivery_authorized).delivery,
        DeliveryAuthorization::Explicit
    );
    assert_eq!(
        codefactory_lib::headless_dispatch::admit(false).delivery,
        DeliveryAuthorization::NotRequested
    );
    // Every permission mode the menu offers is accepted by the entry point, so
    // "set the permission mode" is not a reduced version of the GUI capability.
    for mode in PERMISSION_MODES {
        assert!(parse_request(&format!(
            r#"{{"operation":"set_permission","session_id":"s-1","permission_mode":"{mode}"}}"#
        ))
        .is_ok());
    }

    // The recording host is unused here; keep the harness alive for symmetry.
    let _ = harness();
}
