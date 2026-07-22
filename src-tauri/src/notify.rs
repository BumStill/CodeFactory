// SPDX-License-Identifier: Apache-2.0
//! IM webhook notifications (WorkBuddy-gap P1): push the moments that need a
//! human back to the keyboard — a finished/failed autonomous task, a died
//! chat turn, a tool waiting for permission — to WeCom / Feishu / a generic
//! JSON endpoint. One-way by design in v1: no inbound control surface, no
//! secrets in the payload, fire-and-forget with log-only failures so a dead
//! webhook can never break the agent itself.

use serde_json::{json, Value};

use crate::config::settings::{ImWebhookFormat, Settings};

/// What happened, in the few shapes worth interrupting a human for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyEvent {
    TaskCompleted,
    TaskFailed,
    TurnError,
    PermissionWaiting,
}

impl NotifyEvent {
    fn label(self) -> &'static str {
        match self {
            NotifyEvent::TaskCompleted => "✅ 任务完成",
            NotifyEvent::TaskFailed => "❌ 任务失败",
            NotifyEvent::TurnError => "⚠️ 会话回合中断",
            NotifyEvent::PermissionWaiting => "⏸ 等待权限确认",
        }
    }
}

/// Build the provider-specific webhook payload. Pure — unit-tested per
/// format. `detail` is already user-facing text; never include tokens,
/// file contents, or command output here.
pub fn build_payload(format: ImWebhookFormat, event: NotifyEvent, detail: &str) -> Value {
    let title = event.label();
    match format {
        ImWebhookFormat::Wecom => json!({
            "msgtype": "markdown",
            "markdown": { "content": format!("**{title}**\n{detail}") }
        }),
        ImWebhookFormat::Feishu => json!({
            "msg_type": "text",
            "content": { "text": format!("{title}\n{detail}") }
        }),
        ImWebhookFormat::Generic => json!({
            "source": "codefactory",
            "event": match event {
                NotifyEvent::TaskCompleted => "task_completed",
                NotifyEvent::TaskFailed => "task_failed",
                NotifyEvent::TurnError => "turn_error",
                NotifyEvent::PermissionWaiting => "permission_waiting",
            },
            "title": title,
            "detail": detail,
        }),
    }
}

/// Whether notifications are configured at all.
pub fn enabled(settings: &Settings) -> bool {
    !settings.im_webhook_url.trim().is_empty()
}

/// Fire-and-forget send. Failures are logged and swallowed — a broken
/// webhook must never break or delay the agent.
pub fn send(settings: &Settings, event: NotifyEvent, detail: String) {
    if !enabled(settings) {
        return;
    }
    let url = settings.im_webhook_url.trim().to_string();
    let payload = build_payload(settings.im_webhook_format, event, &detail);
    tokio::spawn(async move {
        let client = reqwest::Client::new();
        match client
            .post(&url)
            .json(&payload)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            Ok(resp) if !resp.status().is_success() => {
                tracing::warn!("im webhook returned {}", resp.status());
            }
            Err(e) => tracing::warn!("im webhook send failed: {e}"),
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wecom_payload_is_markdown_shaped() {
        let payload = build_payload(
            ImWebhookFormat::Wecom,
            NotifyEvent::TaskCompleted,
            "修复滚动:全部 12 项测试通过",
        );
        assert_eq!(payload["msgtype"], "markdown");
        let content = payload["markdown"]["content"].as_str().unwrap();
        assert!(content.contains("任务完成"));
        assert!(content.contains("修复滚动"));
    }

    #[test]
    fn feishu_payload_is_text_shaped() {
        let payload = build_payload(
            ImWebhookFormat::Feishu,
            NotifyEvent::PermissionWaiting,
            "bash 命令等待批准",
        );
        assert_eq!(payload["msg_type"], "text");
        assert!(payload["content"]["text"]
            .as_str()
            .unwrap()
            .contains("等待权限确认"));
    }

    #[test]
    fn generic_payload_is_machine_readable() {
        let payload = build_payload(ImWebhookFormat::Generic, NotifyEvent::TurnError, "上下文超限");
        assert_eq!(payload["source"], "codefactory");
        assert_eq!(payload["event"], "turn_error");
        assert_eq!(payload["detail"], "上下文超限");
    }

    #[test]
    fn notifications_are_disabled_when_no_url_is_configured() {
        let settings = Settings::default();
        assert!(!enabled(&settings));
        let mut on = Settings::default();
        on.im_webhook_url = "https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key=x".into();
        assert!(enabled(&on));
    }
}
