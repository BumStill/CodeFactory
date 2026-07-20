// SPDX-License-Identifier: Apache-2.0
//! `configure_git_remote` agent tool — conversational git-remote setup.
//!
//! The delivery chain's historical failure mode was a missing GitHub token:
//! the user had to leave the conversation, find the settings page, and fill a
//! form — so it never happened, and every delivery blocked at the PR step.
//! This tool inverts that: the AGENT performs every step (detect the repo,
//! create/update the remote entry, store the token in the OS keychain,
//! validate it against the GitHub API), and the user does exactly one thing —
//! paste the token into a secure prompt rendered by the UI.
//!
//! # Secret hygiene (non-negotiable)
//! The token value travels: secure UI prompt → oneshot channel → OS keychain
//! (`secrets::set_key`) → in-memory API client. It NEVER appears in chat
//! messages, tool results, stream events, the DB, logs, or the model context.

use std::time::Duration;

use serde_json::{json, Value};
use uuid::Uuid;

use super::{ExecCtx, ToolOutput};
use crate::config::settings::{GitProvider, GitRemoteConfig};
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, StreamEvent, ToolDefinition};

/// How long the user has to paste the token before the tool returns control
/// to the model (they can always ask to retry).
const SECRET_PROMPT_TIMEOUT: Duration = Duration::from_secs(300);

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "configure_git_remote".into(),
            description: "Set up (or repair) the GitHub credentials that power the delivery \
                chain (PR / CI / merge / release) — conversationally. Call this when the user \
                asks to configure git/GitHub access, or when delivery is blocked on a missing \
                token. You perform every step: the repo is detected from the working directory, \
                the remote entry is created or updated, and the token is validated against the \
                GitHub API and stored in the OS keychain. The USER does exactly one thing: paste \
                a GitHub token (repo scope) into a secure prompt the app pops up — the token \
                never appears in the conversation. Tell the user the prompt is coming and what \
                kind of token to prepare BEFORE calling this tool."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "repo": {
                        "type": "string",
                        "description": "Optional owner/repo override. Defaults to the working directory's origin remote."
                    }
                }
            }),
        },
    }
}

/// Create or update the GitHub entry for `owner_repo` in `remotes`, returning
/// the index of the entry and the keychain ref its token must be stored under.
/// Never touches the token value itself.
pub fn upsert_github_remote_entry(
    remotes: &mut Vec<GitRemoteConfig>,
    owner_repo: &str,
) -> (usize, String) {
    if let Some(index) = remotes.iter().position(|r| {
        matches!(r.provider, GitProvider::Github)
            && (r.default_repo.as_deref() == Some(owner_repo) || r.default_repo.is_none())
    }) {
        let token_ref = remotes[index]
            .token_ref
            .clone()
            .unwrap_or_else(|| format!("git-remote-{}", remotes[index].id));
        remotes[index].token_ref = Some(token_ref.clone());
        remotes[index].default_repo = Some(owner_repo.to_string());
        return (index, token_ref);
    }
    let id = Uuid::new_v4().to_string();
    let token_ref = format!("git-remote-{id}");
    remotes.push(GitRemoteConfig {
        id,
        name: "github".into(),
        provider: GitProvider::Github,
        base_url: "https://api.github.com".into(),
        token_ref: Some(token_ref.clone()),
        token: String::new(),
        default_repo: Some(owner_repo.to_string()),
    });
    (remotes.len() - 1, token_ref)
}

/// Model/user-facing success summary. MUST never contain the token.
pub fn describe_validated_token(owner_repo: &str, login: &str, can_push: bool) -> String {
    let push_note = if can_push {
        "具备 push/PR 权限"
    } else {
        "⚠️ 该 token 对此仓库没有 push 权限,交付链可能仍会在合并步骤受阻"
    };
    format!(
        "GitHub 远端配置完成:仓库 {owner_repo},token 归属账号 {login},{push_note}。\
         token 已存入系统钥匙串,不会出现在对话中。交付链(PR/CI/合并/发布)现在可用,\
         可以直接调用 deliver_changes 继续交付。"
    )
}

/// Wait for the user's secret (or cancellation / timeout). Registered under
/// `request_id` in the shared pending-secrets map; the `provide_secret` tauri
/// command resolves it from the UI.
pub async fn await_secret(
    pending: &crate::PendingSecretMap,
    request_id: &str,
    timeout: Duration,
) -> Option<String> {
    let (sender, receiver) = tokio::sync::oneshot::channel::<Option<String>>();
    pending
        .lock()
        .await
        .insert(request_id.to_string(), sender);
    let outcome = tokio::time::timeout(timeout, receiver).await;
    // Whatever happened, the request is no longer active.
    pending.lock().await.remove(request_id);
    match outcome {
        Ok(Ok(secret)) => secret.filter(|s| !s.trim().is_empty()),
        _ => None,
    }
}

/// Validate a GitHub token against the live API: who is it, and can it push
/// to `owner_repo`. Returns (login, can_push).
async fn validate_github_token(
    base_url: &str,
    token: &str,
    owner_repo: &str,
) -> std::result::Result<(String, bool), String> {
    let client =
        crate::git_remote::client::RemoteGitClient::new(base_url, token, GitProvider::Github);
    let user = client.get("/user").await.map_err(|e| {
        format!("token 无效或无法访问 GitHub API(GET /user 失败): {e}")
    })?;
    let login = user
        .get("login")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let repo = client.get(&format!("/repos/{owner_repo}")).await.map_err(|e| {
        format!("token 有效(账号 {login}),但访问仓库 {owner_repo} 失败: {e}")
    })?;
    let can_push = repo
        .pointer("/permissions/push")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Ok((login, can_push))
}

pub async fn execute(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    // ── Resolve collaboration context ───────────────────────────────────────
    let Some(app) = ctx.app.clone() else {
        return Ok(ToolOutput::err(
            "此上下文无法进行交互式配置(缺少 UI 通道);请让用户在设置→远程仓库中手动配置。",
        ));
    };
    let Some(pending_secrets) = ctx.pending_secrets.clone() else {
        return Ok(ToolOutput::err(
            "此上下文无法进行交互式配置(缺少安全输入通道);请让用户在设置→远程仓库中手动配置。",
        ));
    };
    let Some(settings_state) = ctx.settings_state.clone() else {
        return Ok(ToolOutput::err(
            "此上下文无法写入设置;请让用户在设置→远程仓库中手动配置。",
        ));
    };

    // ── Detect the repo ─────────────────────────────────────────────────────
    let owner_repo = args
        .get("repo")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| crate::agent::delivery::origin_owner_repo(&ctx.cwd));
    let Some(owner_repo) = owner_repo else {
        return Ok(ToolOutput::err(
            "无法从当前目录的 origin 推断 GitHub 仓库(owner/repo);请让用户确认仓库地址,\
             或在参数 repo 中显式提供 owner/repo。",
        ));
    };

    // ── Ask the UI for the token (secure prompt; value never enters chat) ──
    let request_id = Uuid::new_v4().to_string();
    let event_name = ctx
        .session_id
        .as_deref()
        .map(|sid| format!("stream:{sid}"))
        .unwrap_or_else(|| "stream:unknown".into());
    use tauri::Emitter;
    app.emit(
        &event_name,
        StreamEvent::SecretRequest {
            request_id: request_id.clone(),
            purpose: format!("为 {owner_repo} 配置 GitHub 访问令牌(repo 权限)"),
            hint: "在 github.com → Settings → Developer settings → Personal access tokens 创建,\
                   勾选 repo 权限后粘贴到此处"
                .into(),
        },
    )
    .ok();

    let Some(token) = await_secret(&pending_secrets, &request_id, SECRET_PROMPT_TIMEOUT).await
    else {
        return Ok(ToolOutput::err(
            "用户未在安全输入框中提供 token(取消或超时)。告诉用户:准备好 GitHub token \
             (repo 权限)后再让你重新配置即可;不要重复自动重试。",
        ));
    };

    // ── Validate against the live API before persisting ─────────────────────
    let (login, can_push) =
        match validate_github_token("https://api.github.com", &token, &owner_repo).await {
            Ok(ok) => ok,
            Err(e) => {
                return Ok(ToolOutput::err(format!(
                    "token 验证失败,未保存:{e}。请告诉用户检查 token 权限(需要 repo 范围)\
                     后重试 configure_git_remote。"
                )));
            }
        };

    // ── Persist: entry in settings, token in the OS keychain ────────────────
    {
        let mut settings = settings_state.write().await;
        let (_, token_ref) = upsert_github_remote_entry(&mut settings.git_remotes, &owner_repo);
        if let Err(e) = crate::secrets::set_key(&token_ref, &token) {
            return Ok(ToolOutput::err(format!(
                "token 验证通过(账号 {login}),但写入系统钥匙串失败:{e}。请让用户在设置→\
                 远程仓库中手动完成配置。"
            )));
        }
        if let Err(e) = crate::config::settings::save(&settings) {
            return Ok(ToolOutput::err(format!(
                "token 已入钥匙串,但保存设置失败:{e}。请让用户重启应用或在设置中检查远程仓库配置。"
            )));
        }
    }

    Ok(ToolOutput::ok(describe_validated_token(
        &owner_repo,
        &login,
        can_push,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    #[test]
    fn upsert_creates_a_github_entry_with_keychain_ref() {
        let mut remotes = Vec::new();
        let (index, token_ref) = upsert_github_remote_entry(&mut remotes, "BumStill/CodeFactory");
        assert_eq!(remotes.len(), 1);
        let entry = &remotes[index];
        assert!(matches!(entry.provider, GitProvider::Github));
        assert_eq!(entry.base_url, "https://api.github.com");
        assert_eq!(entry.default_repo.as_deref(), Some("BumStill/CodeFactory"));
        assert_eq!(entry.token_ref.as_deref(), Some(token_ref.as_str()));
        assert!(entry.token.is_empty(), "token value must never live inline");
    }

    #[test]
    fn upsert_reuses_an_existing_github_entry() {
        let mut remotes = Vec::new();
        let (first, ref_a) = upsert_github_remote_entry(&mut remotes, "BumStill/CodeFactory");
        let (second, ref_b) = upsert_github_remote_entry(&mut remotes, "BumStill/CodeFactory");
        assert_eq!(first, second);
        assert_eq!(ref_a, ref_b);
        assert_eq!(remotes.len(), 1);
    }

    #[test]
    fn validated_summary_never_contains_a_token_placeholder() {
        let summary = describe_validated_token("BumStill/CodeFactory", "BumStill", true);
        assert!(summary.contains("BumStill/CodeFactory"));
        assert!(summary.contains("钥匙串"));
        assert!(summary.contains("deliver_changes"));
        let warned = describe_validated_token("BumStill/CodeFactory", "BumStill", false);
        assert!(warned.contains("没有 push 权限"));
    }

    #[tokio::test]
    async fn await_secret_times_out_to_none_and_cleans_up() {
        let pending: crate::PendingSecretMap = Arc::new(Mutex::new(HashMap::new()));
        let got = await_secret(&pending, "req-1", Duration::from_millis(20)).await;
        assert!(got.is_none());
        assert!(pending.lock().await.is_empty(), "timed-out request must be unregistered");
    }

    #[tokio::test]
    async fn await_secret_delivers_the_provided_value() {
        let pending: crate::PendingSecretMap = Arc::new(Mutex::new(HashMap::new()));
        let pending_clone = pending.clone();
        let wait = tokio::spawn(async move {
            await_secret(&pending_clone, "req-2", Duration::from_secs(5)).await
        });
        // Wait until the request registers, then resolve it like the tauri
        // command would.
        for _ in 0..100 {
            if let Some(sender) = pending.lock().await.remove("req-2") {
                sender.send(Some("ghp_secret".into())).unwrap();
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(wait.await.unwrap().as_deref(), Some("ghp_secret"));
    }

    #[tokio::test]
    async fn await_secret_treats_cancellation_as_none() {
        let pending: crate::PendingSecretMap = Arc::new(Mutex::new(HashMap::new()));
        let pending_clone = pending.clone();
        let wait = tokio::spawn(async move {
            await_secret(&pending_clone, "req-3", Duration::from_secs(5)).await
        });
        for _ in 0..100 {
            if let Some(sender) = pending.lock().await.remove("req-3") {
                sender.send(None).unwrap();
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(wait.await.unwrap().is_none());
    }
}
