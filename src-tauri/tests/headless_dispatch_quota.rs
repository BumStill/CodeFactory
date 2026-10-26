// SPDX-License-Identifier: Apache-2.0
//! CF-QUOTA-R2: public protocol + real local socket + audit, with synthetic usage.
#![cfg(unix)]
use codefactory_lib::headless_dispatch::{
    self as dispatch, client, socket, AuditLog, DispatchHost, ErrorBody, Request,
};
use std::{
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
};

struct QuotaHost(Mutex<(u8, u8)>);
impl DispatchHost for QuotaHost {
    fn execute(&self, request: &Request) -> Result<serde_json::Value, ErrorBody> {
        let mut caps = self.0.lock().unwrap();
        match request {
            Request::GetQuota {} => {}
            Request::SetQuotaCap {
                endpoint,
                five_hour_percent,
                weekly_percent,
            } => {
                if endpoint != "synthetic-subscription" {
                    return Err(ErrorBody::not_found("unknown endpoint"));
                }
                if let Some(value) = five_hour_percent {
                    caps.0 = *value;
                }
                if let Some(value) = weekly_percent {
                    caps.1 = *value;
                }
            }
            _ => return Err(ErrorBody::invalid("unexpected operation")),
        }
        Ok(serde_json::json!([{
            "endpoint": "synthetic-subscription", "source": "本地估算",
            "five_hour_percent": 37, "weekly_percent": 12,
            "five_hour_cap_percent": caps.0, "weekly_cap_percent": caps.1,
            "five_hour_resets_at_ms": 1900000000000_i64, "weekly_resets_at_ms": null,
        }]))
    }
}

#[test]
fn quota_protocol_rejects_invalid_caps_and_unknown_fields() {
    for raw in [
        r#"{"operation":"get_quota","endpoint":"x"}"#,
        r#"{"operation":"set_quota_cap","endpoint":"x"}"#,
        r#"{"operation":"set_quota_cap","endpoint":"","weekly_percent":50}"#,
        r#"{"operation":"set_quota_cap","endpoint":"x","weekly_percent":0}"#,
        r#"{"operation":"set_quota_cap","endpoint":"x","weekly_percent":101}"#,
        r#"{"operation":"set_quota_cap","endpoint":"x","weekly_percent":2.5}"#,
        r#"{"operation":"set_quota_cap","endpoint":"x","weekly_percent":"60"}"#,
        r#"{"operation":"set_quota_cap","endpoint":"x","weekly_percent":null,"five_hour_percent":60}"#,
        r#"{"operation":"set_quota_cap","endpoint":"x","weekly_percent":60,"typo":true}"#,
    ] {
        assert_eq!(
            dispatch::parse_request(raw).unwrap_err().code,
            "invalid_request",
            "{raw}"
        );
    }
    for percent in 1..=100 {
        let raw =
            format!(r#"{{"operation":"set_quota_cap","endpoint":"x","weekly_percent":{percent}}}"#);
        let request = dispatch::parse_request(&raw).unwrap();
        assert_eq!(dispatch::operation_of(&request), "set_quota_cap");
    }
}

#[test]
fn quota_socket_round_trip_preserves_partial_caps_and_audits_updates() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = dir.path().join("dispatch.sock");
    let audit = dir.path().join("dispatch-audit.log");
    let listener = socket::bind(&path).unwrap();
    let log = AuditLog::new(&audit);
    let host: Arc<dyn DispatchHost> = Arc::new(QuotaHost(Mutex::new((80, 80))));
    std::thread::spawn(move || {
        let _ = dispatch::serve(listener, host, Some(log));
    });
    let initial = client::request(&path, r#"{"operation":"get_quota"}"#).unwrap();
    assert!(initial.ok);
    let initial = initial.result.unwrap();
    assert_eq!(initial[0]["five_hour_percent"], 37);
    assert_eq!(initial[0]["five_hour_resets_at_ms"], 1900000000000_i64);
    assert_eq!(initial[0]["five_hour_cap_percent"], 80);
    let update = client::request(
        &path,
        r#"{"operation":"set_quota_cap","endpoint":"synthetic-subscription","weekly_percent":55}"#,
    )
    .unwrap();
    assert!(update.ok);
    let after = client::request(&path, r#"{"operation":"get_quota"}"#)
        .unwrap()
        .result
        .unwrap();
    assert_eq!(after[0]["five_hour_cap_percent"], 80);
    assert_eq!(after[0]["weekly_cap_percent"], 55);
    let log = std::fs::read_to_string(audit).unwrap();
    assert!(log.contains("set_quota_cap"));
    assert!(log.contains("synthetic-subscription"));
    assert!(log.contains("55"));
    assert!(!log.contains("key_ref"));
}
