// SPDX-License-Identifier: Apache-2.0
//! Desktop tool-execution backend (keystone slice 4.3).
//!
//! The in-process implementation of [`codefactory_agent_loop::tool::ToolBackend`]:
//! it builds a [`crate::tools::ExecCtx`] from the per-call [`ToolCtx`] plus its
//! own long-lived handles and runs the tool MCP-first / native-dispatch, exactly
//! as both provider loop bodies did inline before. Both loops now route through
//! one `execute`, so the duplicated dispatch block lives in a single place.
//!
//! This owns the `AppHandle` privately (under `#[cfg(not(test))]`, mirroring
//! `ExecCtx.app`), so the loop only ever calls it through the trait and the
//! unit-test EXE links no Tauri entrypoints (#166). It is constructed only in
//! `run_openai`/`run_anthropic`, which the test EXE dead-strips.

use codefactory_agent_core::ToolKind;
use codefactory_agent_loop::tool::{
    ToolBackend, ToolCtx, ToolError, ToolExecutionStatus, ToolInvocationResult,
};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::collections::HashSet;
use std::path::{Component, Path};

use crate::openrouter::types::{ToolCall, ToolDefinition};
#[cfg(test)]
use crate::util::no_window::NoWindow;

enum MutationAdmission {
    Unbound,
    Dispatch {
        receipt_id: Option<String>,
        browser_execution: Option<super::browser_recovery::BrowserExecutionPermit>,
    },
    Replay(ToolInvocationResult),
    Waiting(ToolInvocationResult),
}

#[derive(Debug, Clone)]
struct BrowserObservationPlan {
    action: super::browser_recovery::BrowserAction,
    session_id: String,
    observer_kind: super::browser_recovery::BrowserObserverKind,
    safe_locator_json: String,
    precondition_digest: Option<String>,
    expected_postcondition_digest: Option<String>,
}

#[derive(Debug, Clone)]
struct FileObservationPlan {
    safe_locator_json: String,
    precondition_digest: String,
    expected_postcondition_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObservationState {
    Applied,
    DefinitelyNotApplied,
    StillUnknown,
    Conflict,
}

impl ObservationState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::DefinitelyNotApplied => "definitely_not_applied",
            Self::StillUnknown => "still_unknown",
            Self::Conflict => "conflict",
        }
    }
}

struct FileObservation {
    state: ObservationState,
    observed_digest: Option<String>,
}

fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "null".into(),
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::String(value) => {
            serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
        }
        serde_json::Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        serde_json::Value::Object(values) => {
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            format!(
                "{{{}}}",
                entries
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into()),
                        canonical_json(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn opaque_digest(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update(part.as_bytes());
        digest.update([0]);
    }
    format!("sha256:{:x}", digest.finalize())
}

fn bytes_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn absent_file_digest() -> String {
    opaque_digest(&["file_content_sha256_v1", "absent"])
}

fn has_safe_relative_components(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

async fn prepare_file_observation(
    tool_name: &str,
    args: &serde_json::Value,
    cwd: &Path,
) -> Option<FileObservationPlan> {
    let requested = args.get("path")?.as_str()?;
    let workspace = cwd.canonicalize().ok()?;
    let resolved = match tool_name {
        "write_file" => {
            crate::tools::workspace_path::resolve_writable(&workspace, requested).ok()?
        }
        "edit_file" => {
            crate::tools::workspace_path::resolve_existing(&workspace, requested).ok()?
        }
        _ => return None,
    };
    let relative = resolved.strip_prefix(&workspace).ok()?;
    if !has_safe_relative_components(relative) {
        return None;
    }
    let relative = relative.to_str()?.replace('\\', "/");
    let before = match tokio::fs::read(&resolved).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(_) => return None,
    };
    let before_exists = resolved.exists();
    let precondition_digest = if before_exists {
        bytes_digest(&before)
    } else {
        absent_file_digest()
    };
    let expected = match tool_name {
        "write_file" => args.get("content")?.as_str()?.as_bytes().to_vec(),
        "edit_file" => {
            let original = String::from_utf8(before).ok()?;
            let old_string = args.get("old_string")?.as_str()?;
            let new_string = args.get("new_string")?.as_str()?;
            let replace_all = args
                .get("replace_all")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            if !replace_all {
                let count = original.matches(old_string).count();
                if count != 1 {
                    return None;
                }
            }
            if replace_all {
                original.replace(old_string, new_string).into_bytes()
            } else {
                original.replacen(old_string, new_string, 1).into_bytes()
            }
        }
        _ => return None,
    };
    Some(FileObservationPlan {
        safe_locator_json: serde_json::json!({
            "workspace_relative_path": relative,
        })
        .to_string(),
        precondition_digest,
        expected_postcondition_digest: bytes_digest(&expected),
    })
}

async fn observe_file_contract(
    cwd: &Path,
    safe_locator_json: &str,
    precondition_digest: &str,
    expected_postcondition_digest: &str,
) -> FileObservation {
    let relative = serde_json::from_str::<serde_json::Value>(safe_locator_json)
        .ok()
        .and_then(|locator| {
            locator
                .get("workspace_relative_path")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        });
    let Some(relative) = relative else {
        return FileObservation {
            state: ObservationState::StillUnknown,
            observed_digest: None,
        };
    };
    let relative_path = Path::new(&relative);
    if !has_safe_relative_components(relative_path) {
        return FileObservation {
            state: ObservationState::StillUnknown,
            observed_digest: None,
        };
    }
    let resolved = match crate::tools::workspace_path::resolve_writable(cwd, &relative) {
        Ok(path) => path,
        Err(_) => {
            return FileObservation {
                state: ObservationState::StillUnknown,
                observed_digest: None,
            }
        }
    };
    let digest = match tokio::fs::read(&resolved).await {
        Ok(bytes) => bytes_digest(&bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => absent_file_digest(),
        Err(_) => {
            return FileObservation {
                state: ObservationState::StillUnknown,
                observed_digest: None,
            }
        }
    };
    let state = if digest == expected_postcondition_digest {
        ObservationState::Applied
    } else if digest == precondition_digest {
        ObservationState::DefinitelyNotApplied
    } else {
        ObservationState::Conflict
    };
    FileObservation {
        state,
        observed_digest: Some(digest),
    }
}

fn desktop_command_and_kind(tool_name: &str, args: &serde_json::Value) -> (String, ToolKind) {
    let (command, typed_kind) =
        codefactory_agent_loop::policy::completion_command_and_kind(tool_name, args);
    let kind = match tool_name {
        // Shell and browser have argument-sensitive native classifiers. In
        // particular, browser probes remain probes while click/fill/screenshot
        // keep their mutation semantics.
        "bash" | "browser_session" => typed_kind,
        // This is an explicit read-only capability list. Every new native tool
        // defaults to Mutation until it receives an audited typed classifier.
        "read_file" | "glob" | "grep" | "kb_search" | "kb_get_chunk" | "read_pptx"
        | "skill_list" | "skill_search" | "read_xlsx" => ToolKind::ReadOnly,
        _ => ToolKind::Mutation,
    };
    (command, kind)
}

fn bash_has_explicit_external_mutation(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    let curl = lower
        .split(|character: char| character.is_whitespace() || matches!(character, ';' | '|' | '&'))
        .any(|word| word == "curl");
    curl && ([
        " -x post",
        " -xpost",
        " --request post",
        " -x put",
        " -xput",
        " --request put",
        " -x patch",
        " -xpatch",
        " --request patch",
        " -x delete",
        " -xdelete",
        " --request delete",
        " --data",
        " --json",
        " --form",
        " --upload-file",
        " -d ",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
        // curl short options are case-sensitive: `-F` uploads a form while
        // `-f` is the read-only fail-on-HTTP-error flag; likewise `-T` uploads.
        || command.contains(" -F ")
        || command.contains(" -T "))
}

const READ_ONLY_BASH_VERBS: &[&str] = &[
    "cd",
    "echo",
    "pwd",
    "ls",
    "rg",
    "grep",
    "find",
    "cat",
    "head",
    "tail",
    "sed -n",
    "stat",
    "wc",
    "du",
    "df",
    "which",
    "command -v",
    "git status",
    "git diff",
    "git log",
    "git show",
    "git rev-parse",
    "git ls-files",
    "git branch --show-current",
    "kubectl get",
    "kubectl describe",
    "kubectl logs",
    "kubectl version",
];

/// Redirects that discard output cannot mutate the workspace, and `2>&1` only
/// merges two streams. Strip them before segmentation so they neither read as
/// a write nor split a pipeline at their `&`. Longer patterns come first: a
/// leading `>/dev/null` rewrite would otherwise strand the `2` of `2>/dev/null`.
fn strip_discarded_redirects(command: &str) -> String {
    let mut stripped = command.to_string();
    for pattern in [
        "2>&1",
        "&>/dev/null",
        "&> /dev/null",
        "2>/dev/null",
        "2> /dev/null",
        "1>/dev/null",
        "1> /dev/null",
        ">/dev/null",
        "> /dev/null",
    ] {
        stripped = stripped.replace(pattern, " ");
    }
    stripped
}

/// A trailing `&` backgrounds the command, so its completion is unobservable
/// no matter how read-only the verb looks. `&&` is a sequencer, not a fork.
fn has_background_operator(command: &str) -> bool {
    let bytes = command.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'&' {
            if bytes.get(index + 1) == Some(&b'&') {
                index += 2;
                continue;
            }
            return true;
        }
        index += 1;
    }
    false
}

fn split_shell_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut characters = command.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\n' | ';' => segments.push(std::mem::take(&mut current)),
            '&' | '|' => {
                if characters.peek() == Some(&character) {
                    characters.next();
                }
                segments.push(std::mem::take(&mut current));
            }
            _ => current.push(character),
        }
    }
    segments.push(current);
    segments.retain(|segment| !segment.trim().is_empty());
    segments
}

/// Some whitelisted verbs carry flags that turn them into writers. `find` can
/// delete or exec, and `sed -n` still edits in place under `-i`. Keep these
/// per-verb: a blanket `-i` denylist would fence the very common `grep -i`.
fn read_only_verb_flags_are_safe(segment: &str) -> bool {
    if segment.starts_with("find") {
        return ![
            "-delete", "-exec", "-execdir", "-ok", "-okdir", "-fls", "-fprint",
        ]
        .iter()
        .any(|flag| segment.contains(flag));
    }
    if segment.starts_with("sed") {
        return !segment.contains("-i");
    }
    true
}

fn bash_segment_is_read_only(segment: &str) -> bool {
    // A redirect that survived stripping writes somewhere real.
    if segment.contains('>') || segment.contains('&') {
        return false;
    }
    let lower = segment.trim().to_ascii_lowercase();
    READ_ONLY_BASH_VERBS.iter().any(|verb| {
        (lower == *verb
            || lower
                .strip_prefix(verb)
                .is_some_and(|suffix| suffix.chars().next().is_some_and(char::is_whitespace)))
            && read_only_verb_flags_are_safe(&lower)
    })
}

fn bash_segment_is_strict_mode_prelude(segment: &str) -> bool {
    matches!(
        segment.trim().to_ascii_lowercase().as_str(),
        "set -e"
            | "set -u"
            | "set -eu"
            | "set -ue"
            | "set -o pipefail"
            | "set -euo pipefail"
            | "set -ueo pipefail"
    )
}

/// Agents routinely probe a repository with compound read-only pipelines such
/// as `cd repo && ls src 2>/dev/null | head -50`. Rejecting every command that
/// merely contains `&`, `>` or `;` pushed those probes into the mutation
/// branch, where bash can never supply the observation contract the receipt
/// gate demands, so the tool call settled as `Waiting` and stranded the turn.
/// Segment the command instead and keep it read-only only when *every* segment
/// is an explicitly read-only verb; anything unrecognized still fences it.
fn bash_is_explicit_read_only(command: &str) -> bool {
    // Command substitution can hide any verb inside a read-only looking shell.
    if command.contains('`') || command.contains("$(") {
        return false;
    }
    let normalized = strip_discarded_redirects(command);
    if has_background_operator(&normalized) {
        return false;
    }
    let segments = split_shell_segments(&normalized);
    !segments.is_empty()
        && segments.iter().enumerate().all(|(index, segment)| {
            bash_segment_is_read_only(segment)
                || (index == 0 && bash_segment_is_strict_mode_prelude(segment))
        })
}

/// Completion evidence and durable side-effect admission answer different
/// questions. A background service remains `BackgroundServiceStart`, a POST
/// probe may remain `RuntimeProbe`, and both still require an Objective-bound
/// receipt before dispatch.
fn native_requires_mutation_receipt(
    tool_name: &str,
    args: &serde_json::Value,
    completion_kind: &ToolKind,
) -> bool {
    match tool_name {
        "bash" => {
            let command = args
                .get("command")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if bash_has_explicit_external_mutation(command) {
                return true;
            }
            match completion_kind {
                ToolKind::Mutation | ToolKind::BackgroundServiceStart => true,
                ToolKind::Verification
                | ToolKind::RuntimeProbe
                | ToolKind::FunctionalProbe { .. } => false,
                ToolKind::ReadOnly => !bash_is_explicit_read_only(command),
            }
        }
        "browser_session" => !matches!(
            args.get("action").and_then(serde_json::Value::as_str),
            Some("snapshot" | "tabs" | "read" | "find")
        ),
        "read_file" | "glob" | "grep" | "kb_search" | "kb_get_chunk" | "read_pptx"
        | "skill_list" | "skill_search" | "read_xlsx" => false,
        // update_plan mutates only CodeFactory's transactional control-plane
        // projection. A rejected schema/plan revision is known not-applied;
        // treating it as an unobservable external effect creates a false
        // `unknown` receipt that poisons every later tool in the Objective.
        "update_plan" => false,
        _ => true,
    }
}

/// `click` and `press` can submit forms, publish content, place orders or
/// delete data. A DOM ref and a successful CDP send do not make either action
/// observable after a crash, so require the caller to name a deterministic
/// URL postcondition before the outer receipt is created. The raw URL remains
/// in the normalized tool call/ephemeral browser lease; the Browser recovery
/// contract persists only its digest.
fn browser_has_observation_contract(args: &serde_json::Value) -> bool {
    match args.get("action").and_then(serde_json::Value::as_str) {
        Some("click" | "press") => args
            .get("expected_url")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|url| {
                let Ok(url) = reqwest::Url::parse(url) else {
                    return false;
                };
                matches!(url.scheme(), "http" | "https")
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none()
            }),
        // `fill` is observed by the exact target/value digest; lifecycle and
        // file actions have deterministic resource/file observers.
        Some("open" | "attach" | "select_tab" | "close" | "fill" | "screenshot") => true,
        Some("snapshot" | "tabs") => true,
        _ => false,
    }
}

fn digest_hex(parts: &[&str]) -> String {
    opaque_digest(parts)
        .strip_prefix("sha256:")
        .expect("opaque digest prefix")
        .to_string()
}

fn browser_observation_plan(
    args: &serde_json::Value,
    receipt_id: &str,
    resource_id: &str,
    working_directory: &std::path::Path,
) -> Option<BrowserObservationPlan> {
    use super::browser_recovery::{BrowserAction, BrowserObserverKind};

    let action = args.get("action")?.as_str()?;
    let supplied_session = args.get("session_id").and_then(serde_json::Value::as_str);
    let session_id = match action {
        "open" | "attach" => format!(
            "codefactory-receipt-{}",
            digest_hex(&["browser_session", receipt_id, resource_id])
        ),
        _ => supplied_session?.to_string(),
    };
    let session_digest = digest_hex(&["browser_session_id", &session_id]);
    let mut locator = serde_json::Map::new();
    locator.insert("session_digest".into(), session_digest.into());

    let (action, observer_kind, precondition, expected) = match action {
        "click" => {
            let target = args.get("target")?.as_str()?;
            let expected_url = args.get("expected_url")?.as_str()?;
            locator.insert(
                "target_digest".into(),
                digest_hex(&["target", target]).into(),
            );
            (
                BrowserAction::Click,
                BrowserObserverKind::PageDigest,
                None,
                Some(digest_hex(&["page_url", expected_url])),
            )
        }
        "fill" => {
            let target = args.get("target")?.as_str()?;
            let text = args.get("text")?.as_str()?;
            locator.insert(
                "target_digest".into(),
                digest_hex(&["target", target]).into(),
            );
            (
                BrowserAction::Fill,
                BrowserObserverKind::ElementDigest,
                None,
                Some(digest_hex(&["fill_value", target, text])),
            )
        }
        "press" => {
            let key = args
                .get("text")
                .and_then(serde_json::Value::as_str)
                .or_else(|| args.get("target").and_then(serde_json::Value::as_str))?;
            let expected_url = args.get("expected_url")?.as_str()?;
            locator.insert(
                "focus_digest".into(),
                digest_hex(&["focus_key", key]).into(),
            );
            (
                BrowserAction::Press,
                BrowserObserverKind::PageDigest,
                None,
                Some(digest_hex(&["page_url", expected_url])),
            )
        }
        "open" => {
            let url = args.get("url")?.as_str()?;
            (
                BrowserAction::Open,
                BrowserObserverKind::PageDigest,
                None,
                Some(digest_hex(&["page_url", url])),
            )
        }
        "attach" => (
            BrowserAction::Attach,
            BrowserObserverKind::SessionPresence,
            None,
            Some(digest_hex(&["session_present", &session_id])),
        ),
        "select_tab" => {
            let target = args.get("target")?.as_str()?;
            locator.insert("tab_digest".into(), digest_hex(&["tab", target]).into());
            (
                BrowserAction::SelectTab,
                BrowserObserverKind::TabDigest,
                None,
                Some(digest_hex(&["selected_tab", &session_id, target])),
            )
        }
        "close" => (
            BrowserAction::Close,
            BrowserObserverKind::SessionPresence,
            Some(digest_hex(&["session_present", &session_id])),
            Some(digest_hex(&["session_absent", &session_id])),
        ),
        "screenshot" => {
            let path = args.get("path")?.as_str()?;
            locator.insert("path_digest".into(), digest_hex(&["path", path]).into());
            let screenshot_path = working_directory.join(path);
            let precondition = std::fs::read(screenshot_path).ok().map(|bytes| {
                use sha2::Digest;
                format!("{:x}", sha2::Sha256::digest(bytes))
            });
            (
                BrowserAction::Screenshot,
                BrowserObserverKind::WorkspaceFileSha256,
                precondition,
                None,
            )
        }
        _ => return None,
    };
    Some(BrowserObservationPlan {
        action,
        session_id,
        observer_kind,
        safe_locator_json: serde_json::Value::Object(locator).to_string(),
        precondition_digest: precondition,
        expected_postcondition_digest: expected,
    })
}

fn waiting_result(command: &str, kind: ToolKind, code: &str) -> ToolInvocationResult {
    let (content, next_action) = match code {
        "tool_observation_contract_missing" => (
            "该外部操作缺少可验证的观察契约，系统未执行；将改用可观察的专用工具或当前状态重新规划。",
            "replan_observable_tool",
        ),
        "mutation_permit_lost" => (
            "旧执行权已失效，操作未发出；系统将使用当前目标租约继续。",
            "resume_current_objective",
        ),
        _ => (
            "外部变更未再次发出；系统将核对持久化状态后自动继续。",
            "observe_only_reconcile",
        ),
    };
    ToolInvocationResult {
        content: content.into(),
        is_error: false,
        status: ToolExecutionStatus::Waiting,
        command: command.to_string(),
        kind,
        return_code: None,
        stdout: String::new(),
        stderr: String::new(),
        error: None,
        metadata: Some(serde_json::json!({
            "code": code,
            "recoverable": true,
            "next_action": next_action,
            "system_owned": true,
        })),
        next_working_directory: None,
        duration_ms: 0,
    }
}

fn replay_result(
    command: &str,
    kind: ToolKind,
    observation_state: Option<ObservationState>,
) -> ToolInvocationResult {
    ToolInvocationResult {
        content: "此前相同外部变更已由持久化回执确认完成；未重复执行。".into(),
        is_error: false,
        status: ToolExecutionStatus::Done,
        command: command.to_string(),
        kind,
        return_code: None,
        stdout: String::new(),
        stderr: String::new(),
        error: None,
        metadata: Some(serde_json::json!({
            "receipt_replayed": true,
            "observation_state": observation_state.map(ObservationState::as_str),
            "system_owned": true,
        })),
        next_working_directory: None,
        duration_ms: 0,
    }
}

fn invocation_from_output(
    output: crate::tools::ToolOutput,
    command: String,
    kind: ToolKind,
) -> ToolInvocationResult {
    ToolInvocationResult {
        content: output.content,
        is_error: output.is_error,
        status: match output.status {
            crate::tools::ToolExecutionStatus::Done => ToolExecutionStatus::Done,
            crate::tools::ToolExecutionStatus::Waiting => ToolExecutionStatus::Waiting,
            crate::tools::ToolExecutionStatus::Blocked => ToolExecutionStatus::Blocked,
            crate::tools::ToolExecutionStatus::Error => ToolExecutionStatus::Error,
        },
        command,
        kind,
        return_code: None,
        stdout: String::new(),
        stderr: String::new(),
        error: None,
        metadata: output.metadata,
        next_working_directory: None,
        duration_ms: 0,
    }
}

/// In-process tool backend for the desktop app. Holds the long-lived handles;
/// per-call context (cwd, session, task, knowledge scope) arrives via [`ToolCtx`].
pub(super) struct DesktopToolBackend {
    /// Owned privately so the loop never sees an `AppHandle`. Absent in the
    /// test config — this struct is constructed only in the (dead-stripped)
    /// provider loops, never in a `#[cfg(test)]` test.
    #[cfg(not(test))]
    pub(super) app: Option<tauri::AppHandle>,
    pub(super) db: sqlx::SqlitePool,
    pub(super) mcp_manager: std::sync::Arc<crate::mcp::McpManager>,
    pub(super) settings: std::sync::Arc<tokio::sync::RwLock<crate::config::settings::Settings>>,
    /// `ToolBackend::classify` is synchronous, so MCP discovery refreshes this
    /// conservative cache. Unknown/missing MCP annotations never downgrade a
    /// connected tool to read-only.
    pub(super) mcp_tool_names: std::sync::Arc<std::sync::RwLock<HashSet<String>>>,
}

impl DesktopToolBackend {
    async fn ensure_observation_schema(&self) -> Result<(), ToolError> {
        sqlx::raw_sql(include_str!(
            "../../migrations/0012_tool_observation_contracts.sql"
        ))
        .execute(&self.db)
        .await
        .map_err(|error| ToolError {
            message: format!("ensure tool observation schema: {error}"),
        })?;
        sqlx::raw_sql(include_str!(
            "../../migrations/0014_browser_recovery_contracts.sql"
        ))
        .execute(&self.db)
        .await
        .map_err(|error| ToolError {
            message: format!("ensure browser recovery schema: {error}"),
        })?;
        super::tool_recovery::ToolRecoveryStore::ensure_schema(&self.db)
            .await
            .map_err(|error| ToolError {
                message: format!("ensure generic tool recovery schema: {error}"),
            })?;
        Ok(())
    }

    async fn mutation_preflight(
        &self,
        call: &ToolCall,
        args: &serde_json::Value,
        ctx: &ToolCtx,
        command: &str,
        kind: ToolKind,
        is_mcp_tool: bool,
    ) -> Result<MutationAdmission, ToolError> {
        self.ensure_observation_schema().await?;
        if call.function.name == "browser_session" && !browser_has_observation_contract(args) {
            return Ok(MutationAdmission::Waiting(waiting_result(
                command,
                kind,
                "browser_observation_contract_required",
            )));
        }
        let file_observation =
            prepare_file_observation(&call.function.name, args, &ctx.working_directory).await;
        let generic_observation = if is_mcp_tool {
            None
        } else {
            match super::tool_recovery::ToolRecoveryStore::new(self.db.clone())
                .prepare(
                    &call.function.name,
                    args,
                    &ctx.working_directory,
                    ctx.session_id.as_deref(),
                    ctx.root_turn_id.as_deref(),
                )
                .await
            {
                Ok(plan) => plan,
                Err(_) => None,
            }
        };
        let specialized = matches!(
            call.function.name.as_str(),
            "browser_session" | "deliver_changes"
        );
        let resource = if let Some(task_id) = ctx.task_id.as_deref() {
            Some(("task", "task_run", task_id, true))
        } else {
            ctx.root_turn_id
                .as_deref()
                .map(|root_turn_id| ("chat", "chat_root_turn", root_turn_id, false))
        };
        let Some((binding_domain, resource_kind, resource_id, is_task)) = resource else {
            return Ok(MutationAdmission::Waiting(waiting_result(
                command,
                kind,
                "objective_identity_missing",
            )));
        };

        let mut tx = self.db.begin().await.map_err(|error| ToolError {
            message: format!("begin mutation preflight: {error}"),
        })?;
        let objective_id = if is_task {
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT objective_id FROM task_runs WHERE id=?",
            )
            .bind(resource_id)
            .fetch_optional(&mut *tx)
            .await
        } else {
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT objective_id FROM chat_turn_state WHERE root_turn_id=?",
            )
            .bind(resource_id)
            .fetch_optional(&mut *tx)
            .await
        }
        .map_err(|error| ToolError {
            message: format!("resolve mutation objective: {error}"),
        })?
        .flatten()
        .ok_or_else(|| ToolError {
            message: format!(
                "mutation refused without an opaque Objective binding for {resource_kind}:{resource_id}"
            ),
        })?;

        let objective = sqlx::query(
            "SELECT revision, status, remediation_id
             FROM objectives WHERE id=?",
        )
        .bind(&objective_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| ToolError {
            message: format!("load mutation objective: {error}"),
        })?
        .ok_or_else(|| ToolError {
            message: format!("mutation Objective {objective_id} no longer exists"),
        })?;
        let revision: i64 = objective.get("revision");
        let objective_status: String = objective.get("status");
        let remediation_id: Option<String> = objective.get("remediation_id");

        let binding = sqlx::query(
            "SELECT id, resource_generation FROM objective_bindings
             WHERE objective_id=? AND domain=? AND resource_kind=? AND resource_id=?
             ORDER BY resource_generation DESC LIMIT 1",
        )
        .bind(&objective_id)
        .bind(binding_domain)
        .bind(resource_kind)
        .bind(resource_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| ToolError {
            message: format!("load mutation Objective binding: {error}"),
        })?
        .ok_or_else(|| ToolError {
            message: format!(
                "mutation refused because Objective {objective_id} has no authoritative {resource_kind} binding"
            ),
        })?;
        let binding_id: String = binding.get("id");
        let resource_generation: i64 = binding.get("resource_generation");
        let canonical_args = canonical_json(args);
        let cwd = ctx.working_directory.to_string_lossy();
        let generation = resource_generation.to_string();
        let action_fingerprint = opaque_digest(&[
            &call.function.name,
            &canonical_args,
            cwd.as_ref(),
            &binding_id,
            &generation,
        ]);

        let trajectory_session_id = ctx
            .trajectory_session_id
            .as_deref()
            .or(ctx.session_id.as_deref())
            .ok_or_else(|| ToolError {
                message: "objective-bound mutation is missing its trajectory session".into(),
            })?;
        let trace_id = crate::trajectory::trace_record_id(trajectory_session_id, &call.id);
        let attributed = sqlx::query(
            "UPDATE tool_calls
             SET objective_id=?, binding_id=?, action_signature=?, resource_generation=?
             WHERE id=?
               AND (objective_id IS NULL OR objective_id=?)
               AND (binding_id IS NULL OR binding_id=?)
               AND (action_signature IS NULL OR action_signature=?)
               AND (resource_generation IS NULL OR resource_generation=?)",
        )
        .bind(&objective_id)
        .bind(&binding_id)
        .bind(&action_fingerprint)
        .bind(resource_generation)
        .bind(&trace_id)
        .bind(&objective_id)
        .bind(&binding_id)
        .bind(&action_fingerprint)
        .bind(resource_generation)
        .execute(&mut *tx)
        .await
        .map_err(|error| ToolError {
            message: format!("persist mutation tool attribution: {error}"),
        })?;
        if attributed.rows_affected() != 1 {
            return Err(ToolError {
                message: format!(
                    "normalized tool call {trace_id} is missing or has conflicting Objective attribution"
                ),
            });
        }

        match (objective_status.as_str(), ctx.mutation_permit.as_ref()) {
            ("active", None) => {}
            ("waiting_system", Some(permit)) => {
                let permit_matches = permit.objective_id == objective_id
                    && remediation_id.as_deref() == Some(permit.remediation_id.as_str())
                    && permit.binding_id.as_deref() == Some(binding_id.as_str())
                    && permit.resource_generation == Some(resource_generation);
                if !permit_matches {
                    tx.commit().await.map_err(|error| ToolError {
                        message: format!("persist stale mutation attribution: {error}"),
                    })?;
                    return Ok(MutationAdmission::Waiting(waiting_result(
                        command,
                        kind,
                        "mutation_permit_lost",
                    )));
                }
                let now = chrono::Utc::now().timestamp_millis();
                let remediation = sqlx::query(
                    "UPDATE objective_remediations SET updated_at=updated_at
                     WHERE id=? AND objective_id=? AND binding_id=?
                       AND status='claimed' AND lease_owner=?
                       AND attempt_index=? AND lease_expires_at>?",
                )
                .bind(&permit.remediation_id)
                .bind(&objective_id)
                .bind(&binding_id)
                .bind(&permit.owner)
                .bind(permit.claim_epoch)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(|error| ToolError {
                    message: format!("validate mutation remediation permit: {error}"),
                })?;
                let objective_claim = sqlx::query(
                    "UPDATE objectives SET updated_at=updated_at
                     WHERE id=? AND revision=? AND status='waiting_system'
                       AND remediation_id=? AND lease_owner=? AND lease_expires_at>?",
                )
                .bind(&objective_id)
                .bind(revision)
                .bind(&permit.remediation_id)
                .bind(&permit.owner)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(|error| ToolError {
                    message: format!("validate mutation Objective permit: {error}"),
                })?;
                if remediation.rows_affected() != 1 || objective_claim.rows_affected() != 1 {
                    tx.commit().await.map_err(|error| ToolError {
                        message: format!("persist expired mutation attribution: {error}"),
                    })?;
                    return Ok(MutationAdmission::Waiting(waiting_result(
                        command,
                        kind,
                        "mutation_permit_lost",
                    )));
                }
            }
            ("waiting_system", None) | ("active", Some(_)) => {
                tx.commit().await.map_err(|error| ToolError {
                    message: format!("persist fenced mutation attribution: {error}"),
                })?;
                return Ok(MutationAdmission::Waiting(waiting_result(
                    command,
                    kind,
                    "mutation_permit_lost",
                )));
            }
            _ => {
                tx.commit().await.map_err(|error| ToolError {
                    message: format!("persist inactive mutation attribution: {error}"),
                })?;
                return Ok(MutationAdmission::Waiting(waiting_result(
                    command,
                    kind,
                    "objective_not_mutable",
                )));
            }
        }

        // DeliveryRun owns its own epoch, mutation rung and revision receipts.
        // Wrapping it in the generic ledger would turn a legitimate durable
        // Waiting result into `unknown` and block its observation loop. It
        // still receives the exact Objective attribution and permit check above.
        if call.function.name == "deliver_changes" {
            mark_side_effect_started_in_tx(
                &mut tx,
                &objective_id,
                revision,
                &binding_id,
                resource_generation,
            )
            .await?;
            tx.commit().await.map_err(|error| ToolError {
                message: format!("commit delivery Objective attribution: {error}"),
            })?;
            return Ok(MutationAdmission::Dispatch {
                receipt_id: None,
                browser_execution: None,
            });
        }

        // Provider call ids change across forced reprompts and process resume.
        // Durable idempotency is the Objective-bound action itself, not one
        // transport response's ephemeral identifier.
        let idempotency_key =
            opaque_digest(&[&objective_id, &action_fingerprint, &binding_id, &generation]);
        if let Some(existing) = sqlx::query(
            "SELECT id, status, summary_json FROM side_effect_receipts
             WHERE objective_id=? AND action_fingerprint=? AND idempotency_key=?
             ORDER BY observed_at DESC LIMIT 1",
        )
        .bind(&objective_id)
        .bind(&action_fingerprint)
        .bind(&idempotency_key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| ToolError {
            message: format!("load mutation replay receipt: {error}"),
        })? {
            let existing_receipt_id: String = existing.get("id");
            let status: String = existing.get("status");
            sqlx::query(
                "INSERT OR IGNORE INTO tool_recovery_call_links
                 (receipt_id, tool_call_id, created_at) VALUES (?, ?, ?)",
            )
            .bind(&existing_receipt_id)
            .bind(&trace_id)
            .bind(chrono::Utc::now().timestamp_millis())
            .execute(&mut *tx)
            .await
            .map_err(|error| ToolError {
                message: format!("link replayed Tool call to durable receipt: {error}"),
            })?;
            let exact_link: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM tool_recovery_call_links
                 WHERE receipt_id=? AND tool_call_id=?",
            )
            .bind(&existing_receipt_id)
            .bind(&trace_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|error| ToolError {
                message: format!("verify replayed Tool call link: {error}"),
            })?;
            if exact_link != 1 {
                return Err(ToolError {
                    message: "provider Tool call identity is already linked to another receipt"
                        .into(),
                });
            }
            if matches!(status.as_str(), "committed" | "reconciled") {
                let summary_json: Option<String> = existing.get("summary_json");
                let summary = summary_json
                    .as_deref()
                    .and_then(|summary| serde_json::from_str::<serde_json::Value>(summary).ok())
                    .ok_or_else(|| ToolError {
                        message: "committed mutation receipt has no valid replay summary".into(),
                    })?;
                if summary.get("status").and_then(|value| value.as_str()) != Some("done") {
                    return Err(ToolError {
                        message: "committed mutation receipt has an invalid status".into(),
                    });
                }
                let replay = replay_result(command, kind, None);
                tx.commit().await.map_err(|error| ToolError {
                    message: format!("commit mutation replay attribution: {error}"),
                })?;
                return Ok(MutationAdmission::Replay(replay));
            }
            if matches!(status.as_str(), "started" | "unknown") {
                let browser_store =
                    super::browser_recovery::BrowserRecoveryStore::new(self.db.clone());
                let browser_contract: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM browser_recovery_contracts WHERE receipt_id=?",
                )
                .bind(&existing_receipt_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|error| ToolError {
                    message: format!("load browser replay contract: {error}"),
                })?;
                if browser_contract == 1 {
                    tx.commit().await.map_err(|error| ToolError {
                        message: format!("commit browser replay attribution: {error}"),
                    })?;
                    let disposition = browser_store
                        .disposition(&existing_receipt_id)
                        .await
                        .map_err(|error| ToolError {
                            message: format!("inspect browser recovery disposition: {error}"),
                        })?;
                    use super::browser_recovery::BrowserRecoveryDisposition as Disposition;
                    match disposition {
                        Disposition::AwaitingSettlement | Disposition::ObservedApplied => {
                            browser_store
                                .settle(&existing_receipt_id, chrono::Utc::now().timestamp_millis())
                                .await
                                .map_err(|error| ToolError {
                                    message: format!("settle observed browser receipt: {error}"),
                                })?;
                            return Ok(MutationAdmission::Replay(replay_result(
                                command,
                                kind,
                                Some(ObservationState::Applied),
                            )));
                        }
                        Disposition::SettledCommitted | Disposition::SettledReconciled => {
                            return Ok(MutationAdmission::Replay(replay_result(
                                command,
                                kind,
                                Some(ObservationState::Applied),
                            )));
                        }
                        Disposition::Prepared
                        | Disposition::ReplayableExactGeneration
                        | Disposition::ReplayableDigestCas
                            if ctx.mutation_permit.is_some() =>
                        {
                            let operation = browser_store
                                .operation_permit(&existing_receipt_id)
                                .await
                                .map_err(|error| ToolError {
                                    message: format!("load browser retry permit: {error}"),
                                })?;
                            return Ok(MutationAdmission::Dispatch {
                                receipt_id: Some(existing_receipt_id),
                                browser_execution: Some(
                                    super::browser_recovery::BrowserExecutionPermit {
                                        operation,
                                        recovery: ctx.mutation_permit.clone(),
                                    },
                                ),
                            });
                        }
                        Disposition::Conflict => {
                            return Ok(MutationAdmission::Waiting(waiting_result(
                                command,
                                kind,
                                "browser_observation_conflict",
                            )));
                        }
                        _ => {
                            return Ok(MutationAdmission::Waiting(waiting_result(
                                command,
                                kind,
                                "browser_external_state_uncertain",
                            )));
                        }
                    }
                }
                if let Some(contract) = sqlx::query(
                    "SELECT safe_locator_json, precondition_digest,
                            expected_postcondition_digest, last_dispatch_epoch
                     FROM side_effect_observation_contracts WHERE receipt_id=?",
                )
                .bind(&existing_receipt_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|error| ToolError {
                    message: format!("load file observation contract: {error}"),
                })? {
                    let safe_locator_json: String = contract.get("safe_locator_json");
                    let precondition_digest: String = contract.get("precondition_digest");
                    let expected_postcondition_digest: String =
                        contract.get("expected_postcondition_digest");
                    let observation = observe_file_contract(
                        &ctx.working_directory,
                        &safe_locator_json,
                        &precondition_digest,
                        &expected_postcondition_digest,
                    )
                    .await;
                    let now = chrono::Utc::now().timestamp_millis();
                    sqlx::query(
                        "UPDATE side_effect_observation_contracts
                         SET state=?, observed_digest=?,
                             observation_count=observation_count+1, observed_at=?
                         WHERE receipt_id=?",
                    )
                    .bind(observation.state.as_str())
                    .bind(&observation.observed_digest)
                    .bind(now)
                    .bind(&existing_receipt_id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|error| ToolError {
                        message: format!("persist file observation: {error}"),
                    })?;

                    match observation.state {
                        ObservationState::Applied => {
                            let summary = serde_json::json!({"status": "done"}).to_string();
                            sqlx::query(
                                "UPDATE side_effect_receipts
                                 SET status='reconciled', summary_json=?, observed_at=?
                                 WHERE id=? AND status IN ('started','unknown')",
                            )
                            .bind(summary)
                            .bind(now)
                            .bind(&existing_receipt_id)
                            .execute(&mut *tx)
                            .await
                            .map_err(|error| ToolError {
                                message: format!("reconcile applied file receipt: {error}"),
                            })?;
                            tx.commit().await.map_err(|error| ToolError {
                                message: format!("commit applied file observation: {error}"),
                            })?;
                            return Ok(MutationAdmission::Replay(replay_result(
                                command,
                                kind,
                                Some(ObservationState::Applied),
                            )));
                        }
                        ObservationState::StillUnknown => {
                            tx.commit().await.map_err(|error| ToolError {
                                message: format!("commit unknown file observation: {error}"),
                            })?;
                            return Ok(MutationAdmission::Waiting(waiting_result(
                                command,
                                kind,
                                "external_state_uncertain",
                            )));
                        }
                        ObservationState::Conflict => {
                            tx.commit().await.map_err(|error| ToolError {
                                message: format!("commit conflicting file observation: {error}"),
                            })?;
                            return Ok(MutationAdmission::Waiting(waiting_result(
                                command,
                                kind,
                                "tool_observation_conflict",
                            )));
                        }
                        ObservationState::DefinitelyNotApplied => {
                            let Some(permit) = ctx.mutation_permit.as_ref() else {
                                tx.commit().await.map_err(|error| ToolError {
                                    message: format!(
                                        "commit not-applied file observation without permit: {error}"
                                    ),
                                })?;
                                return Ok(MutationAdmission::Waiting(waiting_result(
                                    command,
                                    kind,
                                    "external_state_uncertain",
                                )));
                            };
                            let other_uncertain: i64 = sqlx::query_scalar(
                                "SELECT COUNT(*) FROM side_effect_receipts
                                 WHERE objective_id=? AND binding_id=? AND id<>?
                                   AND status IN ('started','unknown')",
                            )
                            .bind(&objective_id)
                            .bind(&binding_id)
                            .bind(&existing_receipt_id)
                            .fetch_one(&mut *tx)
                            .await
                            .map_err(|error| ToolError {
                                message: format!("inspect competing mutation receipts: {error}"),
                            })?;
                            if other_uncertain > 0 {
                                tx.commit().await.map_err(|error| ToolError {
                                    message: format!(
                                        "commit competing mutation observation: {error}"
                                    ),
                                })?;
                                return Ok(MutationAdmission::Waiting(waiting_result(
                                    command,
                                    kind,
                                    "external_state_uncertain",
                                )));
                            }
                            let admitted = sqlx::query(
                                "UPDATE side_effect_observation_contracts
                                 SET last_dispatch_epoch=?, observed_at=?
                                 WHERE receipt_id=? AND state='definitely_not_applied'
                                   AND last_dispatch_epoch<?",
                            )
                            .bind(permit.claim_epoch)
                            .bind(now)
                            .bind(&existing_receipt_id)
                            .bind(permit.claim_epoch)
                            .execute(&mut *tx)
                            .await
                            .map_err(|error| ToolError {
                                message: format!("admit observed file retry: {error}"),
                            })?;
                            if admitted.rows_affected() != 1 {
                                tx.commit().await.map_err(|error| ToolError {
                                    message: format!("commit fenced file retry: {error}"),
                                })?;
                                return Ok(MutationAdmission::Waiting(waiting_result(
                                    command,
                                    kind,
                                    "external_state_uncertain",
                                )));
                            }
                            tx.commit().await.map_err(|error| ToolError {
                                message: format!("commit observed file retry: {error}"),
                            })?;
                            return Ok(MutationAdmission::Dispatch {
                                receipt_id: Some(existing_receipt_id),
                                browser_execution: None,
                            });
                        }
                    }
                }
                if let Some(contract) = sqlx::query(
                    "SELECT state, dispatch_claim_epoch
                     FROM tool_recovery_contracts WHERE receipt_id=?",
                )
                .bind(&existing_receipt_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|error| ToolError {
                    message: format!("load generic Tool recovery contract: {error}"),
                })? {
                    let state: String = contract.get("state");
                    let dispatch_claim_epoch: i64 = contract.get("dispatch_claim_epoch");
                    let Some(permit) = ctx.mutation_permit.as_ref() else {
                        tx.commit().await.map_err(|error| ToolError {
                            message: format!("commit unowned Tool recovery observation: {error}"),
                        })?;
                        return Ok(MutationAdmission::Waiting(waiting_result(
                            command,
                            kind,
                            "external_state_uncertain",
                        )));
                    };
                    if state == "observed_unchanged" && dispatch_claim_epoch == permit.claim_epoch {
                        let now = chrono::Utc::now().timestamp_millis();
                        let admitted = sqlx::query(
                            "UPDATE tool_recovery_contracts
                             SET state='dispatching', dispatch_generation=dispatch_generation+1,
                                 dispatch_started_at=?, updated_at=?
                             WHERE receipt_id=? AND state='observed_unchanged'
                               AND dispatch_owner=? AND dispatch_claim_epoch=?",
                        )
                        .bind(now)
                        .bind(now)
                        .bind(&existing_receipt_id)
                        .bind(&permit.owner)
                        .bind(permit.claim_epoch)
                        .execute(&mut *tx)
                        .await
                        .map_err(|error| ToolError {
                            message: format!("admit exact Tool retry: {error}"),
                        })?;
                        if admitted.rows_affected() == 1 {
                            tx.commit().await.map_err(|error| ToolError {
                                message: format!("commit exact Tool retry: {error}"),
                            })?;
                            return Ok(MutationAdmission::Dispatch {
                                receipt_id: Some(existing_receipt_id),
                                browser_execution: None,
                            });
                        }
                    }
                    tx.commit().await.map_err(|error| ToolError {
                        message: format!("commit fenced Tool retry: {error}"),
                    })?;
                    return Ok(MutationAdmission::Waiting(waiting_result(
                        command,
                        kind,
                        "external_state_uncertain",
                    )));
                }
            }
        }

        let uncertain: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM side_effect_receipts
             WHERE objective_id=? AND binding_id=?
               AND status IN ('started','unknown')",
        )
        .bind(&objective_id)
        .bind(&binding_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(|error| ToolError {
            message: format!("inspect uncertain mutation receipts: {error}"),
        })?;
        if uncertain > 0 {
            tx.commit().await.map_err(|error| ToolError {
                message: format!("commit uncertain mutation attribution: {error}"),
            })?;
            return Ok(MutationAdmission::Waiting(waiting_result(
                command,
                kind,
                "external_state_uncertain",
            )));
        }

        // Existing receipts are authoritative even when the original
        // precondition (for example edit_file.old_text) can no longer be
        // reconstructed after the effect. Only a genuinely new dispatch must
        // present an observer before side_effect_started is persisted.
        if is_mcp_tool
            || (!specialized && file_observation.is_none() && generic_observation.is_none())
        {
            tx.commit().await.map_err(|error| ToolError {
                message: format!("commit missing Tool observer attribution: {error}"),
            })?;
            return Ok(MutationAdmission::Waiting(waiting_result(
                command,
                kind,
                "tool_observation_contract_missing",
            )));
        }

        mark_side_effect_started_in_tx(
            &mut tx,
            &objective_id,
            revision,
            &binding_id,
            resource_generation,
        )
        .await?;

        // The primary key is deliberately deterministic. It gives every new
        // writer a database-enforced cross-revision collision even on schemas
        // whose older composite UNIQUE constraint still included `revision`.
        let receipt_id = opaque_digest(&["side_effect_receipt", &idempotency_key]);
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, binding_id, revision, action_fingerprint,
              idempotency_key, status, created_at, observed_at)
             VALUES (?, ?, ?, ?, ?, ?, 'started', ?, ?)",
        )
        .bind(&receipt_id)
        .bind(&objective_id)
        .bind(&binding_id)
        .bind(revision)
        .bind(&action_fingerprint)
        .bind(&idempotency_key)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|error| ToolError {
            message: format!("persist started mutation receipt: {error}"),
        })?;
        sqlx::query(
            "INSERT INTO tool_recovery_call_links (receipt_id, tool_call_id, created_at)
             VALUES (?, ?, ?)",
        )
        .bind(&receipt_id)
        .bind(&trace_id)
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|error| ToolError {
            message: format!("persist Tool receipt call link: {error}"),
        })?;
        let browser_execution = if call.function.name == "browser_session" {
            let plan =
                browser_observation_plan(args, &receipt_id, resource_id, &ctx.working_directory)
                    .ok_or_else(|| ToolError {
                        message: "browser mutation lacks a durable observation plan".into(),
                    })?;
            let operation = super::browser_recovery::BrowserRecoveryStore::create_prepared_in_tx(
                &mut tx,
                super::browser_recovery::BrowserPreparedOperation {
                    receipt_id: receipt_id.clone(),
                    objective_id: objective_id.clone(),
                    objective_revision: revision,
                    binding_id: binding_id.clone(),
                    resource_generation,
                    action_fingerprint: action_fingerprint.clone(),
                    tool_call_id: trace_id.clone(),
                    action: plan.action,
                    session_id: plan.session_id,
                    session_generation: 1,
                    observer_kind: plan.observer_kind,
                    safe_locator_json: plan.safe_locator_json,
                    precondition_digest: plan.precondition_digest,
                    expected_postcondition_digest: plan.expected_postcondition_digest,
                    now,
                },
            )
            .await
            .map_err(|error| ToolError {
                message: format!("persist browser recovery contract: {error}"),
            })?;
            Some(super::browser_recovery::BrowserExecutionPermit {
                operation,
                recovery: ctx.mutation_permit.clone(),
            })
        } else {
            None
        };
        if let Some(observation) = file_observation {
            let dispatch_epoch = ctx
                .mutation_permit
                .as_ref()
                .map(|permit| permit.claim_epoch)
                .unwrap_or(0);
            sqlx::query(
                "INSERT INTO side_effect_observation_contracts
                 (receipt_id, objective_id, binding_id, action_fingerprint,
                  operation_domain, observer_kind, safe_locator_json,
                  precondition_digest, expected_postcondition_digest, state,
                  last_dispatch_epoch, observation_count, created_at, observed_at)
                 VALUES (?, ?, ?, ?, 'tool_file', 'file_content_sha256_v1', ?, ?, ?,
                         'definitely_not_applied', ?, 0, ?, ?)",
            )
            .bind(&receipt_id)
            .bind(&objective_id)
            .bind(&binding_id)
            .bind(&action_fingerprint)
            .bind(observation.safe_locator_json)
            .bind(observation.precondition_digest)
            .bind(observation.expected_postcondition_digest)
            .bind(dispatch_epoch)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|error| ToolError {
                message: format!("persist file observation contract: {error}"),
            })?;
        } else if let Some(observation) = generic_observation {
            super::tool_recovery::ToolRecoveryStore::create_contract_in_tx(
                &mut tx,
                &receipt_id,
                &objective_id,
                revision,
                &binding_id,
                resource_generation,
                &action_fingerprint,
                &trace_id,
                observation,
                ctx.mutation_permit
                    .as_ref()
                    .map(|permit| permit.owner.as_str()),
                ctx.mutation_permit
                    .as_ref()
                    .map(|permit| permit.claim_epoch)
                    .unwrap_or(0),
                now,
            )
            .await
            .map_err(|error| ToolError {
                message: format!("persist Tool recovery contract: {error}"),
            })?;
        }
        tx.commit().await.map_err(|error| ToolError {
            message: format!("commit started mutation receipt: {error}"),
        })?;
        Ok(MutationAdmission::Dispatch {
            receipt_id: Some(receipt_id),
            browser_execution,
        })
    }

    async fn settle_mutation_receipt(
        &self,
        receipt_id: &str,
        result: Option<&ToolInvocationResult>,
        ctx: &ToolCtx,
    ) -> Result<(), ToolError> {
        let succeeded = result
            .is_some_and(|result| result.status == ToolExecutionStatus::Done && !result.is_error);
        if super::tool_recovery::ToolRecoveryStore::new(self.db.clone())
            .settle_foreground(receipt_id, &ctx.working_directory, succeeded)
            .await
            .map_err(|error| ToolError {
                message: format!("settle generic Tool recovery contract: {error}"),
            })?
        {
            return Ok(());
        }
        let browser_contract: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM browser_recovery_contracts WHERE receipt_id=?",
        )
        .bind(receipt_id)
        .fetch_one(&self.db)
        .await
        .map_err(|error| ToolError {
            message: format!("load settling browser contract: {error}"),
        })?;
        if browser_contract == 1 {
            let succeeded = result.is_some_and(|result| {
                result.status == ToolExecutionStatus::Done && !result.is_error
            });
            if succeeded {
                let settlement =
                    super::browser_recovery::BrowserRecoveryStore::new(self.db.clone())
                        .settle(receipt_id, chrono::Utc::now().timestamp_millis())
                        .await
                        .map_err(|error| ToolError {
                            message: format!("settle browser recovery contract: {error}"),
                        })?;
                if !matches!(
                    settlement,
                    super::browser_recovery::BrowserSettlement::Committed
                        | super::browser_recovery::BrowserSettlement::Reconciled
                ) {
                    return Err(ToolError {
                        message: format!(
                            "browser action returned success without durable settlement: {settlement:?}"
                        ),
                    });
                }
                return Ok(());
            }
            sqlx::query(
                "UPDATE side_effect_receipts SET status='unknown', observed_at=?
                 WHERE id=? AND status IN ('started','unknown')",
            )
            .bind(chrono::Utc::now().timestamp_millis())
            .bind(receipt_id)
            .execute(&self.db)
            .await
            .map_err(|error| ToolError {
                message: format!("persist browser mutation uncertainty: {error}"),
            })?;
            return Ok(());
        }
        let contract = sqlx::query(
            "SELECT safe_locator_json, precondition_digest, expected_postcondition_digest
             FROM side_effect_observation_contracts WHERE receipt_id=?",
        )
        .bind(receipt_id)
        .fetch_optional(&self.db)
        .await
        .map_err(|error| ToolError {
            message: format!("load settling file observation contract: {error}"),
        })?;
        let mut post_dispatch_observation = None;
        if let Some(contract) = contract {
            let safe_locator_json: String = contract.get("safe_locator_json");
            let precondition_digest: String = contract.get("precondition_digest");
            let expected_postcondition_digest: String =
                contract.get("expected_postcondition_digest");
            let observation = observe_file_contract(
                &ctx.working_directory,
                &safe_locator_json,
                &precondition_digest,
                &expected_postcondition_digest,
            )
            .await;
            sqlx::query(
                "UPDATE side_effect_observation_contracts
                 SET state=?, observed_digest=?,
                     observation_count=observation_count+1, observed_at=?
                 WHERE receipt_id=?",
            )
            .bind(observation.state.as_str())
            .bind(&observation.observed_digest)
            .bind(chrono::Utc::now().timestamp_millis())
            .bind(receipt_id)
            .execute(&self.db)
            .await
            .map_err(|error| ToolError {
                message: format!("persist settling file observation: {error}"),
            })?;
            post_dispatch_observation = Some(observation.state);
        }
        let succeeded = result
            .is_some_and(|result| result.status == ToolExecutionStatus::Done && !result.is_error);
        let observation_confirms_success =
            post_dispatch_observation.is_none_or(|state| state == ObservationState::Applied);
        let (status, summary_json) = if succeeded && observation_confirms_success {
            let summary = serde_json::json!({
                "status": "done",
            });
            ("committed", Some(summary.to_string()))
        } else {
            ("unknown", None)
        };
        let updated = sqlx::query(
            "UPDATE side_effect_receipts
             SET status=?, summary_json=?, observed_at=?
             WHERE id=? AND status IN ('started','unknown')",
        )
        .bind(status)
        .bind(summary_json)
        .bind(chrono::Utc::now().timestamp_millis())
        .bind(receipt_id)
        .execute(&self.db)
        .await
        .map_err(|error| ToolError {
            message: format!("persist mutation receipt outcome: {error}"),
        })?;
        if updated.rows_affected() != 1 {
            return Err(ToolError {
                message: format!("mutation receipt {receipt_id} changed before settlement"),
            });
        }
        Ok(())
    }
}

async fn mark_side_effect_started_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    objective_id: &str,
    revision: i64,
    binding_id: &str,
    resource_generation: i64,
) -> Result<(), ToolError> {
    let objective_started =
        sqlx::query("UPDATE objectives SET side_effect_started=1 WHERE id=? AND revision=?")
            .bind(objective_id)
            .bind(revision)
            .execute(&mut **tx)
            .await
            .map_err(|error| ToolError {
                message: format!("mark Objective side effect started: {error}"),
            })?;
    let binding_started = sqlx::query(
        "UPDATE objective_bindings SET side_effect_started=1, updated_at=?
         WHERE id=? AND objective_id=? AND resource_generation=?",
    )
    .bind(chrono::Utc::now().timestamp_millis())
    .bind(binding_id)
    .bind(objective_id)
    .bind(resource_generation)
    .execute(&mut **tx)
    .await
    .map_err(|error| ToolError {
        message: format!("mark Objective binding side effect started: {error}"),
    })?;
    if objective_started.rows_affected() != 1 || binding_started.rows_affected() != 1 {
        return Err(ToolError {
            message: "Objective identity changed before mutation dispatch".into(),
        });
    }
    Ok(())
}

#[async_trait::async_trait]
impl ToolBackend for DesktopToolBackend {
    async fn list_schemas(&self) -> Vec<ToolDefinition> {
        // Desktop surface = native tools + every connected MCP tool. (The
        // anonymous KB-tool strip stays in the loop for now — it depends on the
        // run's anonymous flag; folded in when the loop moves in slice 4.6.)
        let mut defs = crate::tools::all_definitions();
        let mcp_tools = self.mcp_manager.list_all_tools().await;
        if let Ok(mut names) = self.mcp_tool_names.write() {
            names.clear();
            names.extend(mcp_tools.iter().map(|tool| tool.name.clone()));
        }
        for mcp_tool in &mcp_tools {
            defs.push(super::mcp_tool_to_definition(mcp_tool));
        }
        defs
    }

    async fn execute(
        &self,
        call: &ToolCall,
        args: &serde_json::Value,
        ctx: &ToolCtx,
    ) -> Result<ToolInvocationResult, ToolError> {
        let mcp_server = self.mcp_manager.find_tool_server(&call.function.name).await;
        let is_mcp_tool = mcp_server.is_some()
            || self
                .mcp_tool_names
                .read()
                .is_ok_and(|names| names.contains(&call.function.name));
        let (command, native_kind) = desktop_command_and_kind(&call.function.name, args);
        let kind = if is_mcp_tool {
            ToolKind::Mutation
        } else {
            native_kind
        };
        let native_tool_known = crate::tools::all_definitions()
            .iter()
            .any(|definition| definition.function.name == call.function.name);
        let requires_receipt = is_mcp_tool
            || (native_tool_known
                && native_requires_mutation_receipt(&call.function.name, args, &kind));
        let admission = if requires_receipt {
            self.mutation_preflight(call, args, ctx, &command, kind.clone(), is_mcp_tool)
                .await?
        } else {
            MutationAdmission::Unbound
        };
        let (receipt_id, browser_execution) = match admission {
            MutationAdmission::Replay(result) | MutationAdmission::Waiting(result) => {
                return Ok(result)
            }
            MutationAdmission::Unbound => (None, None),
            MutationAdmission::Dispatch {
                receipt_id,
                browser_execution,
            } => (receipt_id, browser_execution),
        };

        let exec_ctx = crate::tools::ExecCtx {
            cwd: ctx.working_directory.clone(),
            #[cfg(not(test))]
            app: self.app.clone(),
            db: Some(self.db.clone()),
            session_id: ctx.session_id.clone(),
            root_turn_id: ctx.root_turn_id.clone(),
            task_id: ctx.task_id.clone(),
            outer_receipt_id: receipt_id.clone(),
            mutation_permit: browser_execution
                .as_ref()
                .and_then(|execution| execution.recovery.clone()),
            knowledge_library_ids: ctx.knowledge_library_ids.clone(),
            settings: Some(self.settings.read().await.clone()),
        };

        // MCP-first, then native dispatch — precedence and the `Unknown tool`
        // sentinel are preserved. An MCP error becomes an `is_error` result the
        // model sees; a native-dispatch `Err` is FATAL and aborts the turn.
        let output = if let Some(server_id) = mcp_server {
            match self
                .mcp_manager
                .call_tool(&server_id, &call.function.name, args.clone())
                .await
            {
                Ok(text) => crate::tools::ToolOutput::ok(text),
                Err(e) => crate::tools::ToolOutput::err(format!("MCP error: {e}")),
            }
        } else {
            match crate::tools::dispatch(&call.function.name, args.clone(), &exec_ctx).await {
                Ok(output) => output,
                Err(error) => {
                    if let Some(receipt_id) = receipt_id.as_deref() {
                        self.settle_mutation_receipt(receipt_id, None, ctx).await?;
                    }
                    return Err(ToolError {
                        message: error.to_string(),
                    });
                }
            }
        };

        let result = invocation_from_output(output, command, kind);
        if let Some(receipt_id) = receipt_id.as_deref() {
            self.settle_mutation_receipt(receipt_id, Some(&result), ctx)
                .await?;
        }
        Ok(result)
    }

    fn classify(&self, call: &ToolCall, args: &serde_json::Value) -> (String, ToolKind) {
        if self
            .mcp_tool_names
            .read()
            .is_ok_and(|names| names.contains(&call.function.name))
        {
            (format!("mcp:{}", call.function.name), ToolKind::Mutation)
        } else {
            desktop_command_and_kind(&call.function.name, args)
        }
    }
}

#[cfg(test)]
mod tests {
    //! In `#[cfg(test)]` builds `DesktopToolBackend` has NO `app` field, so
    //! these construct the REAL backend with no `AppHandle` — that headless
    //! constructibility is the whole point of the seam, and it keeps the
    //! unit-test EXE clear of Tauri entrypoints (#166; McpManager/Settings/pool
    //! own no `AppHandle`). This locks the contract the loop relies on: the full
    //! native tool surface runs through `execute`, MCP-first with a native
    //! fallback, and an unknown tool is a clean `is_error` result, not a fatal
    //! `Err`.
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    const TEST_SESSION_ID: &str = "session-tool-fencing";
    const TEST_ROOT_TURN_ID: &str = "root-tool-fencing";
    const TEST_OBJECTIVE_ID: &str = "5cf0bf25-2ed8-4cad-a775-f55cd16f0830";
    const TEST_BINDING_ID: &str = "binding-tool-fencing";

    async fn backend() -> DesktopToolBackend {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        DesktopToolBackend {
            db,
            mcp_manager: std::sync::Arc::new(crate::mcp::McpManager::new()),
            settings: std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::config::settings::Settings::default(),
            )),
            mcp_tool_names: std::sync::Arc::new(std::sync::RwLock::new(HashSet::new())),
        }
    }

    /// Materialize only the persisted identities that a mutation permit may
    /// trust. The failure-first tests below deliberately exercise the real
    /// backend seam: no test-only dispatcher or fake receipt store can make a
    /// duplicate external launch look safe.
    async fn objective_backend(waiting_with_foreign_lease: bool) -> DesktopToolBackend {
        let backend = backend().await;
        sqlx::raw_sql(include_str!(
            "../../migrations/0007_unified_objective_control_plane.sql"
        ))
        .execute(&backend.db)
        .await
        .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE sessions (
                 id TEXT PRIMARY KEY,
                 cwd TEXT NOT NULL
             );
             CREATE TABLE messages (
                 id TEXT PRIMARY KEY,
                 session_id TEXT NOT NULL,
                 role TEXT NOT NULL,
                 content TEXT NOT NULL,
                 created_at INTEGER NOT NULL
             );
             CREATE TABLE task_runs (
                 id TEXT PRIMARY KEY,
                 cwd TEXT NOT NULL
             );
             CREATE TABLE chat_turn_state (
                 root_turn_id TEXT PRIMARY KEY,
                 session_id TEXT NOT NULL,
                 objective_id TEXT,
                 revision INTEGER NOT NULL
             );
             CREATE TABLE tool_calls (
                 id TEXT PRIMARY KEY,
                 message_id TEXT NOT NULL,
                 tool_name TEXT NOT NULL,
                 arguments TEXT NOT NULL DEFAULT '{}',
                 result TEXT,
                 metadata TEXT,
                 status TEXT NOT NULL DEFAULT 'pending',
                 error TEXT,
                 duration_ms INTEGER,
                 created_at INTEGER NOT NULL,
                 objective_id TEXT,
                 binding_id TEXT,
                 action_signature TEXT,
                 resource_generation INTEGER
             );",
        )
        .execute(&backend.db)
        .await
        .unwrap();

        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query("INSERT INTO sessions (id, cwd) VALUES (?, '.')")
            .bind(TEST_SESSION_ID)
            .execute(&backend.db)
            .await
            .unwrap();
        for (id, role) in [
            (TEST_ROOT_TURN_ID, "user"),
            ("assistant-message", "assistant"),
        ] {
            sqlx::query(
                "INSERT INTO messages (id, session_id, role, content, created_at)
                 VALUES (?, ?, ?, '{}', ?)",
            )
            .bind(id)
            .bind(TEST_SESSION_ID)
            .bind(role)
            .bind(now)
            .execute(&backend.db)
            .await
            .unwrap();
        }
        let status = if waiting_with_foreign_lease {
            "waiting_system"
        } else {
            "active"
        };
        let decision_type = if waiting_with_foreign_lease {
            "waiting"
        } else {
            "continue"
        };
        let remediation_id = waiting_with_foreign_lease.then_some("remediation-tool-fencing");
        let lease_owner = waiting_with_foreign_lease.then_some("replacement-supervisor");
        let lease_expires_at = waiting_with_foreign_lease.then_some(now + 60_000);
        sqlx::query(
            "INSERT INTO objectives
             (id, revision, kind, session_id, root_turn_id, status, decision_type,
              domain, autonomous_completion, requested_acceptance, requires_user_action,
              recovery_owner, remediation_id, lease_owner, lease_expires_at,
              created_surface, created_at, updated_at)
             VALUES (?, 1, 'local_mutation', ?, ?, ?, ?, 'tool', 1,
                     'validated_change', 0, 'objective-supervisor', ?, ?, ?,
                     'test', ?, ?)",
        )
        .bind(TEST_OBJECTIVE_ID)
        .bind(TEST_SESSION_ID)
        .bind(TEST_ROOT_TURN_ID)
        .bind(status)
        .bind(decision_type)
        .bind(remediation_id)
        .bind(lease_owner)
        .bind(lease_expires_at)
        .bind(now)
        .bind(now)
        .execute(&backend.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, created_at, updated_at)
             VALUES (?, ?, 'chat', 'chat_root_turn', ?, 1,
                     'sha256:test-binding', ?, ?)",
        )
        .bind(TEST_BINDING_ID)
        .bind(TEST_OBJECTIVE_ID)
        .bind(TEST_ROOT_TURN_ID)
        .bind(now)
        .bind(now)
        .execute(&backend.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, session_id, objective_id, revision)
             VALUES (?, ?, ?, 1)",
        )
        .bind(TEST_ROOT_TURN_ID)
        .bind(TEST_SESSION_ID)
        .bind(TEST_OBJECTIVE_ID)
        .execute(&backend.db)
        .await
        .unwrap();
        if waiting_with_foreign_lease {
            sqlx::query(
                "INSERT INTO objective_remediations
                 (id, objective_id, binding_id, domain, status, failure_code,
                  failure_signature, strategy, approach_index, attempt_index,
                  next_observation_at, lease_owner, lease_expires_at,
                  created_at, updated_at)
                 VALUES ('remediation-tool-fencing', ?, ?, 'tool', 'claimed',
                         'external_state_uncertain', 'sha256:test-failure',
                         'observe_then_resume', 0, 2, ?,
                         'replacement-supervisor', ?, ?, ?)",
            )
            .bind(TEST_OBJECTIVE_ID)
            .bind(TEST_BINDING_ID)
            .bind(now)
            .bind(now + 60_000)
            .bind(now)
            .bind(now)
            .execute(&backend.db)
            .await
            .unwrap();
        }
        backend
    }

    fn call_with_args(id: &str, name: &str, args: &serde_json::Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            r#type: "function".into(),
            function: crate::openrouter::types::FunctionCall {
                name: name.into(),
                arguments: args.to_string(),
            },
        }
    }

    fn append_once_args() -> serde_json::Value {
        if cfg!(windows) {
            serde_json::json!({"command": "Add-Content -Path effect.log -Value once"})
        } else {
            serde_json::json!({"command": "printf 'once\\n' >> effect.log"})
        }
    }

    fn objective_ctx(dir: &std::path::Path) -> ToolCtx {
        ToolCtx {
            working_directory: dir.to_path_buf(),
            session_id: Some(TEST_SESSION_ID.into()),
            root_turn_id: Some(TEST_ROOT_TURN_ID.into()),
            trajectory_session_id: Some(TEST_SESSION_ID.into()),
            ..Default::default()
        }
    }

    fn current_permit(claim_epoch: i64) -> codefactory_agent_loop::tool::MutationPermit {
        codefactory_agent_loop::tool::MutationPermit {
            objective_id: TEST_OBJECTIVE_ID.into(),
            remediation_id: "remediation-tool-fencing".into(),
            owner: "replacement-supervisor".into(),
            claim_epoch,
            binding_id: Some(TEST_BINDING_ID.into()),
            resource_generation: Some(1),
        }
    }

    async fn register_tool_call(
        backend: &DesktopToolBackend,
        tool_call: &ToolCall,
        args: &serde_json::Value,
    ) {
        sqlx::query(
            "INSERT INTO tool_calls
             (id, message_id, tool_name, arguments, status, created_at)
             VALUES (?, 'assistant-message', ?, ?, 'pending', ?)",
        )
        .bind(crate::trajectory::trace_record_id(
            TEST_SESSION_ID,
            &tool_call.id,
        ))
        .bind(&tool_call.function.name)
        .bind(args.to_string())
        .bind(chrono::Utc::now().timestamp_millis())
        .execute(&backend.db)
        .await
        .unwrap();
    }

    async fn prime_file_receipt_without_dispatch(
        backend: &DesktopToolBackend,
        ctx: &ToolCtx,
        call_id: &str,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> String {
        let tool_call = call_with_args(call_id, tool_name, args);
        register_tool_call(backend, &tool_call, args).await;
        let (command, kind) = backend.classify(&tool_call, args);
        match backend
            .mutation_preflight(&tool_call, args, ctx, &command, kind, false)
            .await
            .expect("write-ahead mutation receipt")
        {
            MutationAdmission::Dispatch {
                receipt_id: Some(receipt_id),
                ..
            } => receipt_id,
            _ => panic!("a fresh observable file mutation must be admitted exactly once"),
        }
    }

    async fn claim_file_recovery(backend: &DesktopToolBackend, claim_epoch: i64) {
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "UPDATE objectives
             SET status='waiting_system', decision_type='waiting',
                 remediation_id='remediation-tool-fencing',
                 lease_owner='replacement-supervisor', lease_expires_at=?, updated_at=?
             WHERE id=?",
        )
        .bind(now + 60_000)
        .bind(now)
        .bind(TEST_OBJECTIVE_ID)
        .execute(&backend.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objective_remediations
             (id, objective_id, binding_id, domain, status, failure_code,
              failure_signature, strategy, approach_index, attempt_index,
              next_observation_at, lease_owner, lease_expires_at,
              created_at, updated_at)
             VALUES ('remediation-tool-fencing', ?, ?, 'tool', 'claimed',
                     'external_state_uncertain', 'sha256:file-observation-test',
                     'observe_then_resume', 0, ?, ?,
                     'replacement-supervisor', ?, ?, ?)",
        )
        .bind(TEST_OBJECTIVE_ID)
        .bind(TEST_BINDING_ID)
        .bind(claim_epoch)
        .bind(now)
        .bind(now + 60_000)
        .bind(now)
        .bind(now)
        .execute(&backend.db)
        .await
        .unwrap();
    }

    async fn claimed_tool_recovery(
        backend: &DesktopToolBackend,
        claim_epoch: i64,
    ) -> super::super::objective::ClaimedRemediation {
        super::super::objective::ClaimedRemediation {
            objective: super::super::objective::ObjectiveStore::new(backend.db.clone())
                .get(TEST_OBJECTIVE_ID)
                .await
                .unwrap()
                .unwrap(),
            remediation_id: "remediation-tool-fencing".into(),
            domain: super::super::objective::RecoveryDomain::Tool,
            failure_code: "external_state_uncertain".into(),
            claim_epoch,
            binding_id: Some(TEST_BINDING_ID.into()),
            resource_generation: Some(1),
        }
    }

    async fn assert_objective_bound_call_starts_receipt(
        tool_name: &str,
        call_id: &str,
        args: &serde_json::Value,
    ) {
        let backend = objective_backend(false).await;
        let call = call_with_args(call_id, tool_name, args);
        register_tool_call(&backend, &call, args).await;
        let (command, kind) = backend.classify(&call, args);
        assert!(
            native_requires_mutation_receipt(tool_name, args, &kind),
            "{tool_name} with {args} must enter the durable mutation fence"
        );
        let admission = backend
            .mutation_preflight(
                &call,
                args,
                &objective_ctx(std::path::Path::new(".")),
                &command,
                kind,
                false,
            )
            .await
            .expect("mutation preflight must persist its dispatch fence");
        let receipt_id = match admission {
            MutationAdmission::Dispatch {
                receipt_id: Some(receipt_id),
                ..
            } => receipt_id,
            _ => panic!("{tool_name} must not dispatch without a generic started receipt"),
        };
        let status: String =
            sqlx::query_scalar("SELECT status FROM side_effect_receipts WHERE id=?")
                .bind(receipt_id)
                .fetch_one(&backend.db)
                .await
                .unwrap();
        assert_eq!(status, "started");
    }

    fn call(name: &str) -> ToolCall {
        ToolCall {
            id: "t".into(),
            r#type: "function".into(),
            function: crate::openrouter::types::FunctionCall {
                name: name.into(),
                arguments: "{}".into(),
            },
        }
    }

    #[tokio::test]
    async fn desktop_backend_runs_the_native_surface_headless() {
        let dir = tempfile::tempdir().unwrap();
        let backend = objective_backend(false).await;
        let ctx = objective_ctx(dir.path());
        let write_args = serde_json::json!({ "path": "n.txt", "content": "hello backend" });
        let write_call = call_with_args("headless-native-write", "write_file", &write_args);
        register_tool_call(&backend, &write_call, &write_args).await;

        let out = backend
            .execute(&write_call, &write_args, &ctx)
            .await
            .expect("write is not fatal");
        assert!(!out.is_error, "write via backend: {}", out.content);

        let out = backend
            .execute(
                &call("read_file"),
                &serde_json::json!({ "path": "n.txt" }),
                &ctx,
            )
            .await
            .expect("read is not fatal");
        assert!(!out.is_error && out.content.contains("hello backend"));
    }

    #[tokio::test]
    async fn unknown_tool_is_an_is_error_result_not_a_fatal_err() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        let ctx = objective_ctx(dir.path());
        let unknown = call("no_such_tool");
        let args = serde_json::json!({});
        register_tool_call(&backend, &unknown, &args).await;
        let out = backend
            .execute(&unknown, &args, &ctx)
            .await
            .expect("unknown tool returns a result, never aborts the turn");
        assert!(out.is_error);
        assert!(out.content.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn cached_mcp_tool_is_mutation_without_calling_list_schemas_first() {
        let backend = backend().await;
        backend
            .mcp_tool_names
            .write()
            .unwrap()
            .insert("mcp_without_annotations".into());
        let (command, kind) = backend.classify(
            &call("mcp_without_annotations"),
            &serde_json::json!({"query": "read-looking but unannotated"}),
        );
        assert_eq!(command, "mcp:mcp_without_annotations");
        assert_eq!(kind, ToolKind::Mutation);
    }

    #[tokio::test]
    async fn every_native_tool_outside_the_read_only_whitelist_defaults_to_mutation() {
        let backend = backend().await;
        let read_only = [
            "read_file",
            "glob",
            "grep",
            "kb_search",
            "kb_get_chunk",
            "read_pptx",
            "skill_list",
            "skill_search",
            "read_xlsx",
        ];
        for definition in crate::tools::all_definitions() {
            let name = definition.function.name;
            if name == "bash" || name == "browser_session" {
                continue;
            }
            let (_, kind) = backend.classify(&call(&name), &serde_json::json!({}));
            if read_only.contains(&name.as_str()) {
                assert_eq!(
                    kind,
                    ToolKind::ReadOnly,
                    "{name} read-only contract drifted"
                );
            } else {
                assert_eq!(
                    kind,
                    ToolKind::Mutation,
                    "{name} must be fenced until explicitly audited read-only"
                );
            }
        }
        assert_eq!(
            backend
                .classify(
                    &call("format_pptx"),
                    &serde_json::json!({"path": "deck.pptx"})
                )
                .1,
            ToolKind::Mutation
        );
        assert_eq!(
            backend
                .classify(&call("update_plan"), &serde_json::json!({"steps": []}))
                .1,
            ToolKind::Mutation
        );
    }

    /// The 2026-08-13 freeze started with `cd repo && ls src 2>/dev/null |
    /// head -50` — a wholly read-only probe. The old whitelist rejected any
    /// command containing `&`, `>` or `;`, so the probe fell through to the
    /// mutation branch, demanded an observation contract bash cannot supply,
    /// and settled `Waiting` with the rest of the batch cancelled.
    #[test]
    fn compound_read_only_pipelines_never_demand_an_observation_contract() {
        for command in [
            "cd /repo && ls src",
            "set -euo pipefail; cd /repo; git diff --check; git status --short",
            "cd /repo && ls src && echo \"---\" && ls src/components 2>/dev/null | head -50",
            "ls src 2>/dev/null | head -50",
            "git status; git diff",
            "rg pattern src | head -20",
            "cat notes.txt 2>&1 | wc -l",
            "grep -i needle haystack.txt",
        ] {
            assert!(
                bash_is_explicit_read_only(command),
                "{command} only reads and must not be fenced behind a receipt"
            );
        }
    }

    #[test]
    fn update_plan_is_transactional_control_state_not_an_external_side_effect() {
        let args = serde_json::json!({
            "steps": [
                {"id":"inspect","title":"Inspect","kind":"analysis","status":"completed"},
                {"id":"verify","title":"Verify","kind":"verification","status":"in_progress"}
            ]
        });
        assert!(
            !native_requires_mutation_receipt("update_plan", &args, &ToolKind::Mutation),
            "a rejected plan validation must not create an unknown external-side-effect receipt"
        );
    }

    #[tokio::test]
    async fn invalid_plan_revision_never_poison_receipts_or_the_next_tool() {
        let backend = objective_backend(false).await;
        sqlx::query(
            "CREATE TABLE chat_plan_events (
               id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
               root_turn_id TEXT NOT NULL, revision INTEGER NOT NULL,
               plan_json TEXT NOT NULL, explanation TEXT,
               waiting_reason TEXT, next_action_owner TEXT NOT NULL,
               change_reason TEXT, created_at INTEGER NOT NULL
             )",
        )
        .execute(&backend.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_plan_events
             (id, session_id, root_turn_id, revision, plan_json,
              next_action_owner, created_at)
             VALUES ('plan-1', ?, ?, 1, ?, 'system', ?)",
        )
        .bind(TEST_SESSION_ID)
        .bind(TEST_ROOT_TURN_ID)
        .bind(serde_json::json!([
            {"id":"inspect","title":"Inspect","kind":"analysis","status":"in_progress","external_job_id":null},
            {"id":"verify","title":"Verify","kind":"verification","status":"pending","external_job_id":null}
        ]).to_string())
        .bind(chrono::Utc::now().timestamp_millis())
        .execute(&backend.db)
        .await
        .unwrap();

        let args = serde_json::json!({
            "steps": [
                {"id":"inspect","title":"Inspect","kind":"analysis","status":"completed"},
                {"id":"deliver","title":"Deliver","kind":"delivery","status":"in_progress"}
            ]
        });
        let call = call_with_args("invalid-plan-revision", "update_plan", &args);
        register_tool_call(&backend, &call, &args).await;
        let output = backend
            .execute(&call, &args, &objective_ctx(std::path::Path::new(".")))
            .await
            .expect("plan rejection is a normal tool result");
        assert!(output.is_error);
        assert!(output.content.contains("change_reason is required"));

        let receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM side_effect_receipts WHERE objective_id=?",
        )
        .bind(TEST_OBJECTIVE_ID)
        .fetch_one(&backend.db)
        .await
        .unwrap();
        let plan_revisions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM chat_plan_events WHERE session_id=? AND root_turn_id=?",
        )
        .bind(TEST_SESSION_ID)
        .bind(TEST_ROOT_TURN_ID)
        .fetch_one(&backend.db)
        .await
        .unwrap();
        let side_effect_started: i64 =
            sqlx::query_scalar("SELECT side_effect_started FROM objectives WHERE id=?")
                .bind(TEST_OBJECTIVE_ID)
                .fetch_one(&backend.db)
                .await
                .unwrap();
        assert_eq!(receipts, 0);
        assert_eq!(plan_revisions, 1);
        assert_eq!(side_effect_started, 0);

        let audit_args = serde_json::json!({
            "command": "set -euo pipefail; git diff --check; git status --short"
        });
        let audit_call = call_with_args("read-only-after-invalid-plan", "bash", &audit_args);
        register_tool_call(&backend, &audit_call, &audit_args).await;
        let audit = backend
            .execute(&audit_call, &audit_args, &objective_ctx(std::path::Path::new(".")))
            .await
            .expect("read-only audit dispatches after a rejected plan");
        assert_eq!(audit.status, ToolExecutionStatus::Done);
        assert!(!audit.content.contains("external_state_uncertain"));
    }

    /// Widening the whitelist to pipelines must not widen it to writers: one
    /// non-read-only segment fences the whole command. `find -exec` and
    /// `sed -i` matter most — segmentation on `;` would otherwise hand
    /// `find . -exec rm {} \;` a read-only verdict the old check refused.
    #[test]
    fn one_mutating_segment_fences_the_whole_command() {
        for command in [
            "ls src > out.txt",
            "ls src >> out.txt",
            "cd /repo && rm -rf build",
            "ls | tee out.txt",
            "ls | xargs rm",
            "echo hi > file",
            "find . -name '*.tmp' -delete",
            "find . -type f -exec rm {} \\;",
            "sed -n -i.bak 's/a/b/' file",
            "ls $(rm -rf /tmp/x)",
            "ls `whoami`",
            "ls &",
            "npm run build",
            "curl -X POST https://example.com",
        ] {
            assert!(
                !bash_is_explicit_read_only(command),
                "{command} can mutate external state and must stay fenced"
            );
        }
    }

    #[tokio::test]
    async fn native_mutation_without_observer_fails_before_receipt_or_dispatch() {
        let backend = objective_backend(false).await;
        let args = serde_json::json!({
            "command": "curl -X POST https://example.invalid/hooks -d secret"
        });
        let call = call_with_args("bash-without-observer", "bash", &args);
        register_tool_call(&backend, &call, &args).await;
        let (command, kind) = backend.classify(&call, &args);

        let admission = backend
            .mutation_preflight(
                &call,
                &args,
                &objective_ctx(std::path::Path::new(".")),
                &command,
                kind,
                false,
            )
            .await
            .unwrap();

        let MutationAdmission::Waiting(outcome) = admission else {
            panic!("an unobservable native mutation must fail before dispatch");
        };
        assert_eq!(outcome.status, ToolExecutionStatus::Waiting);
        assert_eq!(
            outcome
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("code"))
                .and_then(serde_json::Value::as_str),
            Some("tool_observation_contract_missing")
        );
        let receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM side_effect_receipts")
            .fetch_one(&backend.db)
            .await
            .unwrap();
        assert_eq!(receipts, 0);
        let side_effect_started: i64 =
            sqlx::query_scalar("SELECT side_effect_started FROM objectives WHERE id=?")
                .bind(TEST_OBJECTIVE_ID)
                .fetch_one(&backend.db)
                .await
                .unwrap();
        assert_eq!(side_effect_started, 0);
    }

    #[tokio::test]
    async fn powershell_local_file_mutation_gets_a_workspace_observer() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        sqlx::query("UPDATE sessions SET cwd=? WHERE id=?")
            .bind(dir.path().to_string_lossy().as_ref())
            .bind(TEST_SESSION_ID)
            .execute(&backend.db)
            .await
            .unwrap();
        let args = serde_json::json!({
            "command": "Add-Content -Path effect.log -Value once"
        });
        let call = call_with_args("powershell-local-mutation", "bash", &args);
        register_tool_call(&backend, &call, &args).await;
        let (command, kind) = backend.classify(&call, &args);
        let admission = backend
            .mutation_preflight(
                &call,
                &args,
                &objective_ctx(dir.path()),
                &command,
                kind,
                false,
            )
            .await
            .unwrap();
        assert!(matches!(admission, MutationAdmission::Dispatch { .. }));
        let resource_kind: String =
            sqlx::query_scalar("SELECT resource_kind FROM tool_recovery_contracts")
                .fetch_one(&backend.db)
                .await
                .unwrap();
        assert_eq!(resource_kind, "workspace_file");
    }

    #[tokio::test]
    async fn append_retry_contract_rejects_absolute_and_compound_external_commands() {
        for (call_id, command) in [
            (
                "powershell-absolute-append",
                "Add-Content -Path C:\\Temp\\effect.log -Value once",
            ),
            (
                "powershell-compound-external",
                "Add-Content -Path effect.log -Value once; Invoke-WebRequest https://example.invalid/hook",
            ),
            (
                "powershell-nested-external",
                "Add-Content -Path effect.log -Value (Invoke-WebRequest https://example.invalid/hook)",
            ),
            (
                "shell-compound-external",
                "printf 'once\\n' >> effect.log && curl -X POST https://example.invalid/hook",
            ),
            ("shell-parent-escape", "rm ../outside.txt"),
            ("shell-absolute-path", "rm /tmp/outside.txt"),
        ] {
            let backend = objective_backend(false).await;
            let dir = tempfile::tempdir().unwrap();
            let args = serde_json::json!({"command": command});
            let call = call_with_args(call_id, "bash", &args);
            register_tool_call(&backend, &call, &args).await;
            let (classified, kind) = backend.classify(&call, &args);
            let admission = backend
                .mutation_preflight(
                    &call,
                    &args,
                    &objective_ctx(dir.path()),
                    &classified,
                    kind,
                    false,
                )
                .await
                .unwrap();
            assert!(
                matches!(admission, MutationAdmission::Waiting(_)),
                "{call_id} unexpectedly reached dispatch"
            );
            let receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM side_effect_receipts")
                .fetch_one(&backend.db)
                .await
                .unwrap();
            assert_eq!(receipts, 0, "{call_id} must fail before dispatch");
        }
    }

    #[tokio::test]
    async fn skill_mutation_has_a_redacted_tree_observer_before_dispatch() {
        let backend = objective_backend(false).await;
        let args = serde_json::json!({"name": "workspace-maintenance"});
        let call = call_with_args("skill-create-observed", "skill_create", &args);
        register_tool_call(&backend, &call, &args).await;
        let (command, kind) = backend.classify(&call, &args);
        let admission = backend
            .mutation_preflight(
                &call,
                &args,
                &objective_ctx(std::path::Path::new(".")),
                &command,
                kind,
                false,
            )
            .await
            .unwrap();
        let MutationAdmission::Dispatch {
            receipt_id: Some(receipt_id),
            ..
        } = admission
        else {
            panic!("a skill mutation must have a durable observer");
        };
        let (kind, locator): (String, String) = sqlx::query_as(
            "SELECT resource_kind, safe_locator_json FROM tool_recovery_contracts WHERE receipt_id=?",
        )
        .bind(receipt_id)
        .fetch_one(&backend.db)
        .await
        .unwrap();
        assert_eq!(kind, "user_skills");
        assert_eq!(locator, "{}");
        assert!(!locator.contains("workspace-maintenance"));
    }

    #[tokio::test]
    async fn crash_before_generic_dispatch_grants_one_exact_retry_to_the_new_claim() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        sqlx::query("UPDATE sessions SET cwd=? WHERE id=?")
            .bind(dir.path().to_string_lossy().as_ref())
            .bind(TEST_SESSION_ID)
            .execute(&backend.db)
            .await
            .unwrap();
        let args = serde_json::json!({"path": "report.docx", "blocks": []});
        let call = call_with_args("docx-before-crash", "write_docx", &args);
        register_tool_call(&backend, &call, &args).await;
        let (command, kind) = backend.classify(&call, &args);
        let first = backend
            .mutation_preflight(
                &call,
                &args,
                &objective_ctx(dir.path()),
                &command,
                kind.clone(),
                false,
            )
            .await
            .unwrap();
        assert!(matches!(first, MutationAdmission::Dispatch { .. }));

        claim_file_recovery(&backend, 2).await;
        let claim = claimed_tool_recovery(&backend, 2).await;
        let permit = current_permit(2);
        let recovery = super::super::tool_recovery::ToolRecoveryStore::new(backend.db.clone());
        assert_eq!(
            recovery.reconcile_claimed(&claim, &permit).await.unwrap(),
            super::super::tool_recovery::ToolRecoveryDisposition::RetryExact
        );
        assert_eq!(
            recovery.reconcile_claimed(&claim, &permit).await.unwrap(),
            super::super::tool_recovery::ToolRecoveryDisposition::RetryExact,
            "a crash after reconciliation must preserve the same decision"
        );

        let retry = call_with_args("docx-after-takeover", "write_docx", &args);
        register_tool_call(&backend, &retry, &args).await;
        let mut retry_ctx = objective_ctx(dir.path());
        retry_ctx.mutation_permit = Some(permit);
        let admitted = backend
            .mutation_preflight(&retry, &args, &retry_ctx, &command, kind, false)
            .await
            .unwrap();
        assert!(matches!(admitted, MutationAdmission::Dispatch { .. }));
        let second = backend
            .mutation_preflight(
                &retry,
                &args,
                &retry_ctx,
                &command,
                ToolKind::Mutation,
                false,
            )
            .await
            .unwrap();
        assert!(matches!(second, MutationAdmission::Waiting(_)));
    }

    #[tokio::test]
    async fn crash_after_generic_effect_reconciles_and_never_replays_the_old_action() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        sqlx::query("UPDATE sessions SET cwd=? WHERE id=?")
            .bind(dir.path().to_string_lossy().as_ref())
            .bind(TEST_SESSION_ID)
            .execute(&backend.db)
            .await
            .unwrap();
        let args = serde_json::json!({"path": "report.docx", "blocks": []});
        let call = call_with_args("docx-effect-before-crash", "write_docx", &args);
        register_tool_call(&backend, &call, &args).await;
        let (command, kind) = backend.classify(&call, &args);
        let admission = backend
            .mutation_preflight(
                &call,
                &args,
                &objective_ctx(dir.path()),
                &command,
                kind,
                false,
            )
            .await
            .unwrap();
        assert!(matches!(admission, MutationAdmission::Dispatch { .. }));
        std::fs::write(dir.path().join("report.docx"), b"complete-after-crash").unwrap();

        claim_file_recovery(&backend, 2).await;
        let claim = claimed_tool_recovery(&backend, 2).await;
        let permit = current_permit(2);
        let recovery = super::super::tool_recovery::ToolRecoveryStore::new(backend.db.clone());
        for _ in 0..2 {
            assert_eq!(
                recovery.reconcile_claimed(&claim, &permit).await.unwrap(),
                super::super::tool_recovery::ToolRecoveryDisposition::ReplanCurrentState
            );
        }
        let receipt_status: String =
            sqlx::query_scalar("SELECT status FROM side_effect_receipts WHERE objective_id=?")
                .bind(TEST_OBJECTIVE_ID)
                .fetch_one(&backend.db)
                .await
                .unwrap();
        assert_eq!(receipt_status, "reconciled");
        assert_eq!(
            std::fs::read(dir.path().join("report.docx")).unwrap(),
            b"complete-after-crash"
        );
        let reconciliations: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM tool_recovery_reconciliations WHERE remediation_id='remediation-tool-fencing'",
        )
        .fetch_one(&backend.db)
        .await
        .unwrap();
        assert_eq!(reconciliations, 1);
    }

    #[tokio::test]
    async fn replacement_after_retry_success_adopts_terminal_receipt_without_replay() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        sqlx::query("UPDATE sessions SET cwd=? WHERE id=?")
            .bind(dir.path().to_string_lossy().as_ref())
            .bind(TEST_SESSION_ID)
            .execute(&backend.db)
            .await
            .unwrap();
        let args = serde_json::json!({"path": "report.docx", "blocks": []});
        let first = call_with_args("docx-first-dispatch", "write_docx", &args);
        register_tool_call(&backend, &first, &args).await;
        let (command, kind) = backend.classify(&first, &args);
        let MutationAdmission::Dispatch {
            receipt_id: Some(receipt_id),
            ..
        } = backend
            .mutation_preflight(
                &first,
                &args,
                &objective_ctx(dir.path()),
                &command,
                kind.clone(),
                false,
            )
            .await
            .unwrap()
        else {
            panic!("first dispatch must write its receipt")
        };

        claim_file_recovery(&backend, 2).await;
        let claim2 = claimed_tool_recovery(&backend, 2).await;
        let permit2 = current_permit(2);
        let recovery = super::super::tool_recovery::ToolRecoveryStore::new(backend.db.clone());
        assert_eq!(
            recovery.reconcile_claimed(&claim2, &permit2).await.unwrap(),
            super::super::tool_recovery::ToolRecoveryDisposition::RetryExact
        );
        let retry = call_with_args("docx-exact-retry", "write_docx", &args);
        register_tool_call(&backend, &retry, &args).await;
        let mut retry_ctx = objective_ctx(dir.path());
        retry_ctx.mutation_permit = Some(permit2);
        assert!(matches!(
            backend
                .mutation_preflight(&retry, &args, &retry_ctx, &command, kind, false)
                .await
                .unwrap(),
            MutationAdmission::Dispatch { .. }
        ));
        std::fs::write(dir.path().join("report.docx"), b"one-complete-retry").unwrap();
        assert!(recovery
            .settle_foreground(&receipt_id, dir.path(), true)
            .await
            .unwrap());
        let settled_statuses: Vec<String> =
            sqlx::query_scalar("SELECT status FROM tool_calls WHERE id IN (?, ?) ORDER BY id")
                .bind(crate::trajectory::trace_record_id(
                    TEST_SESSION_ID,
                    &first.id,
                ))
                .bind(crate::trajectory::trace_record_id(
                    TEST_SESSION_ID,
                    &retry.id,
                ))
                .fetch_all(&backend.db)
                .await
                .unwrap();
        assert_eq!(settled_statuses, vec!["done", "done"]);

        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "UPDATE objective_remediations SET attempt_index=3, lease_expires_at=?
             WHERE id='remediation-tool-fencing'",
        )
        .bind(now + 60_000)
        .execute(&backend.db)
        .await
        .unwrap();
        sqlx::query("UPDATE objectives SET lease_expires_at=? WHERE id=?")
            .bind(now + 60_000)
            .bind(TEST_OBJECTIVE_ID)
            .execute(&backend.db)
            .await
            .unwrap();
        let claim3 = claimed_tool_recovery(&backend, 3).await;
        assert_eq!(
            recovery
                .reconcile_claimed(&claim3, &current_permit(3))
                .await
                .unwrap(),
            super::super::tool_recovery::ToolRecoveryDisposition::ReplanCurrentState
        );
        assert_eq!(
            std::fs::read(dir.path().join("report.docx")).unwrap(),
            b"one-complete-retry"
        );
        let statuses: Vec<String> =
            sqlx::query_scalar("SELECT status FROM tool_calls WHERE id IN (?, ?) ORDER BY id")
                .bind(crate::trajectory::trace_record_id(
                    TEST_SESSION_ID,
                    &first.id,
                ))
                .bind(crate::trajectory::trace_record_id(
                    TEST_SESSION_ID,
                    &retry.id,
                ))
                .fetch_all(&backend.db)
                .await
                .unwrap();
        assert_eq!(statuses, vec!["done", "done"]);
    }

    #[tokio::test]
    async fn workspace_observer_detects_tracked_deletion_and_index_only_changes() {
        for (call_id, command, prepare, mutate) in [
            (
                "bash-remove-tracked",
                "rm tracked.txt",
                None,
                "rm tracked.txt",
            ),
            (
                "bash-stage-only",
                "git add tracked.txt",
                Some("changed before staging"),
                "git add tracked.txt",
            ),
        ] {
            let backend = objective_backend(false).await;
            let dir = tempfile::tempdir().unwrap();
            let run = |args: &[&str]| {
                let output = std::process::Command::new("git")
                    .no_window()
                    .args(args)
                    .current_dir(dir.path())
                    .output()
                    .unwrap();
                assert!(output.status.success(), "git {:?} failed", args);
            };
            run(&["init", "-q"]);
            std::fs::write(dir.path().join("tracked.txt"), b"original").unwrap();
            run(&["add", "tracked.txt"]);
            run(&[
                "-c",
                "user.name=CodeFactory Test",
                "-c",
                "user.email=codefactory@example.invalid",
                "commit",
                "-qm",
                "fixture",
            ]);
            if let Some(contents) = prepare {
                std::fs::write(dir.path().join("tracked.txt"), contents).unwrap();
            }
            sqlx::query("UPDATE sessions SET cwd=? WHERE id=?")
                .bind(dir.path().to_string_lossy().as_ref())
                .bind(TEST_SESSION_ID)
                .execute(&backend.db)
                .await
                .unwrap();
            let args = serde_json::json!({"command": command});
            let call = call_with_args(call_id, "bash", &args);
            register_tool_call(&backend, &call, &args).await;
            let (classified, kind) = backend.classify(&call, &args);
            assert!(matches!(
                backend
                    .mutation_preflight(
                        &call,
                        &args,
                        &objective_ctx(dir.path()),
                        &classified,
                        kind,
                        false,
                    )
                    .await
                    .unwrap(),
                MutationAdmission::Dispatch { .. }
            ));
            let effect = std::process::Command::new("sh")
                .no_window()
                .args(["-c", mutate])
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(effect.status.success());

            claim_file_recovery(&backend, 2).await;
            let claim = claimed_tool_recovery(&backend, 2).await;
            assert_eq!(
                super::super::tool_recovery::ToolRecoveryStore::new(backend.db.clone())
                    .reconcile_claimed(&claim, &current_permit(2))
                    .await
                    .unwrap(),
                super::super::tool_recovery::ToolRecoveryDisposition::ReplanCurrentState,
                "{call_id} must be observed as already applied"
            );
        }
    }

    #[tokio::test]
    async fn explicit_external_bash_mutations_fail_before_started_receipt() {
        for (call_id, command) in [
            (
                "bash-curl-post",
                "curl -X POST https://example.invalid/hooks -d '{\"ok\":true}'",
            ),
            ("bash-kubectl-apply", "kubectl apply -f deployment.yaml"),
            (
                "bash-nohup",
                "nohup sh -c 'touch launched.marker' >/dev/null 2>&1 &",
            ),
        ] {
            let backend = objective_backend(false).await;
            let args = serde_json::json!({"command": command});
            let call = call_with_args(call_id, "bash", &args);
            register_tool_call(&backend, &call, &args).await;
            let (command, kind) = backend.classify(&call, &args);
            let admission = backend
                .mutation_preflight(
                    &call,
                    &args,
                    &objective_ctx(std::path::Path::new(".")),
                    &command,
                    kind,
                    false,
                )
                .await
                .unwrap();
            assert!(matches!(admission, MutationAdmission::Waiting(_)));
            let receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM side_effect_receipts")
                .fetch_one(&backend.db)
                .await
                .unwrap();
            assert_eq!(receipts, 0, "{call_id} must fail before a started receipt");
        }
    }

    #[tokio::test]
    async fn unknown_executable_mutation_fails_closed_without_a_named_observer() {
        let backend = objective_backend(false).await;
        let args = serde_json::json!({"command": "lsmalware --perform-side-effect"});
        let call = call_with_args("bash-read-prefix-lookalike", "bash", &args);
        register_tool_call(&backend, &call, &args).await;
        let outcome = backend
            .execute(&call, &args, &objective_ctx(std::path::Path::new(".")))
            .await
            .unwrap();
        assert_eq!(outcome.status, ToolExecutionStatus::Waiting);
        assert_eq!(
            outcome
                .metadata
                .and_then(|value| value.get("code").cloned()),
            Some(serde_json::json!("tool_observation_contract_missing"))
        );
    }

    #[tokio::test]
    async fn unobservable_background_mutation_fails_closed_before_dispatch_or_started_receipt() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("must-not-launch.txt");
        let command = if cfg!(windows) {
            "Start-Process powershell -ArgumentList '-Command Set-Content must-not-launch.txt launched'"
        } else {
            "nohup sh -c 'printf launched > must-not-launch.txt' >/dev/null 2>&1 &"
        };
        let args = serde_json::json!({"command": command});
        let tool_call = call_with_args("unobservable-background", "bash", &args);
        register_tool_call(&backend, &tool_call, &args).await;

        let out = backend
            .execute(&tool_call, &args, &objective_ctx(dir.path()))
            .await
            .expect("missing observer is a system-owned wait");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(out.status, ToolExecutionStatus::Waiting);
        assert_eq!(
            out.metadata
                .as_ref()
                .and_then(|metadata| metadata.get("code"))
                .and_then(serde_json::Value::as_str),
            Some("tool_observation_contract_missing")
        );
        assert!(!marker.exists(), "the background process must never launch");
        let receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM side_effect_receipts")
            .fetch_one(&backend.db)
            .await
            .unwrap();
        assert_eq!(receipts, 0, "fail-closed is before a started receipt");
        let side_effect_started: i64 =
            sqlx::query_scalar("SELECT side_effect_started FROM objectives WHERE id=?")
                .bind(TEST_OBJECTIVE_ID)
                .fetch_one(&backend.db)
                .await
                .unwrap();
        assert_eq!(side_effect_started, 0);
    }

    #[tokio::test]
    async fn undeclared_mcp_mutation_fails_closed_before_native_fallback_or_receipt() {
        let backend = objective_backend(false).await;
        backend
            .mcp_tool_names
            .write()
            .unwrap()
            .insert("mcp_without_observer".into());
        let dir = tempfile::tempdir().unwrap();
        let args = serde_json::json!({"secret": "must-not-be-persisted"});
        let tool_call = call_with_args("mcp-observer-missing", "mcp_without_observer", &args);
        register_tool_call(&backend, &tool_call, &args).await;

        let out = backend
            .execute(&tool_call, &args, &objective_ctx(dir.path()))
            .await
            .expect("undeclared MCP mutation is held by the system");

        assert_eq!(out.status, ToolExecutionStatus::Waiting);
        assert_eq!(
            out.metadata
                .as_ref()
                .and_then(|metadata| metadata.get("code"))
                .and_then(serde_json::Value::as_str),
            Some("tool_observation_contract_missing")
        );
        let receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM side_effect_receipts")
            .fetch_one(&backend.db)
            .await
            .unwrap();
        assert_eq!(receipts, 0);
    }

    #[tokio::test]
    async fn restarted_edit_observer_reconciles_applied_without_replaying() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("state.txt"), "before\n").unwrap();
        let args = serde_json::json!({
            "path": "state.txt",
            "old_string": "before",
            "new_string": "after"
        });
        let ctx = objective_ctx(dir.path());
        prime_file_receipt_without_dispatch(
            &backend,
            &ctx,
            "edit-before-applied-crash",
            "edit_file",
            &args,
        )
        .await;
        // Simulate the exact crash window: the filesystem mutation committed,
        // but the old future never settled its receipt.
        std::fs::write(dir.path().join("state.txt"), "after\n").unwrap();
        claim_file_recovery(&backend, 2).await;
        let resumed = call_with_args("edit-after-applied-crash", "edit_file", &args);
        register_tool_call(&backend, &resumed, &args).await;
        let mut recovery_ctx = ctx.clone();
        recovery_ctx.mutation_permit = Some(current_permit(2));

        let out = backend
            .execute(&resumed, &args, &recovery_ctx)
            .await
            .unwrap();

        assert_eq!(out.status, ToolExecutionStatus::Done);
        assert!(
            !out.is_error,
            "an applied edit is replayed from its receipt"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("state.txt")).unwrap(),
            "after\n"
        );
        let (receipt_status, observation_state): (String, String) = sqlx::query_as(
            "SELECT r.status, o.state
             FROM side_effect_receipts r
             JOIN side_effect_observation_contracts o ON o.receipt_id=r.id",
        )
        .fetch_one(&backend.db)
        .await
        .unwrap();
        assert_eq!(receipt_status, "reconciled");
        assert_eq!(observation_state, "applied");
    }

    #[tokio::test]
    async fn restarted_write_observer_reconciles_applied_with_redacted_contract() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        let secret_content = "private-content-must-not-enter-contract\n";
        let args = serde_json::json!({
            "path": "nested/state.txt",
            "content": secret_content
        });
        std::fs::create_dir_all(dir.path().join("nested")).unwrap();
        let ctx = objective_ctx(dir.path());
        prime_file_receipt_without_dispatch(
            &backend,
            &ctx,
            "write-before-applied-crash",
            "write_file",
            &args,
        )
        .await;
        std::fs::write(dir.path().join("nested/state.txt"), secret_content).unwrap();
        claim_file_recovery(&backend, 2).await;
        let resumed = call_with_args("write-after-applied-crash", "write_file", &args);
        register_tool_call(&backend, &resumed, &args).await;
        let mut recovery_ctx = ctx.clone();
        recovery_ctx.mutation_permit = Some(current_permit(2));

        let out = backend
            .execute(&resumed, &args, &recovery_ctx)
            .await
            .unwrap();

        assert_eq!(out.status, ToolExecutionStatus::Done);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("nested/state.txt")).unwrap(),
            secret_content
        );
        let (locator, before_digest, expected_digest, observed_digest, state): (
            String,
            String,
            String,
            Option<String>,
            String,
        ) = sqlx::query_as(
            "SELECT safe_locator_json, precondition_digest,
                    expected_postcondition_digest, observed_digest, state
             FROM side_effect_observation_contracts",
        )
        .fetch_one(&backend.db)
        .await
        .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&locator).unwrap(),
            serde_json::json!({"workspace_relative_path": "nested/state.txt"})
        );
        let workspace_path = dir.path().to_string_lossy().into_owned();
        for persisted in [&locator, &before_digest, &expected_digest] {
            assert!(!persisted.contains(secret_content));
            assert!(!persisted.contains(workspace_path.as_str()));
        }
        assert!(before_digest.starts_with("sha256:"));
        assert!(expected_digest.starts_with("sha256:"));
        assert_eq!(observed_digest.as_deref(), Some(expected_digest.as_str()));
        assert_eq!(state, "applied");
    }

    #[tokio::test]
    async fn restarted_edit_observer_retries_definitely_not_applied_once_with_new_permit() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("state.txt"), "before\n").unwrap();
        let args = serde_json::json!({
            "path": "state.txt",
            "old_string": "before",
            "new_string": "after"
        });
        let ctx = objective_ctx(dir.path());
        prime_file_receipt_without_dispatch(
            &backend,
            &ctx,
            "edit-before-not-applied-crash",
            "edit_file",
            &args,
        )
        .await;
        claim_file_recovery(&backend, 2).await;
        let mut recovery_ctx = ctx.clone();
        recovery_ctx.mutation_permit = Some(current_permit(2));

        for call_id in ["edit-not-applied-resume", "edit-same-permit-replay"] {
            let resumed = call_with_args(call_id, "edit_file", &args);
            register_tool_call(&backend, &resumed, &args).await;
            let out = backend
                .execute(&resumed, &args, &recovery_ctx)
                .await
                .unwrap();
            assert_eq!(out.status, ToolExecutionStatus::Done);
            assert!(!out.is_error);
        }

        assert_eq!(
            std::fs::read_to_string(dir.path().join("state.txt")).unwrap(),
            "after\n"
        );
        let (receipts, last_dispatch_epoch, state): (i64, i64, String) = sqlx::query_as(
            "SELECT COUNT(*), MAX(o.last_dispatch_epoch), MAX(o.state)
             FROM side_effect_receipts r
             JOIN side_effect_observation_contracts o ON o.receipt_id=r.id",
        )
        .fetch_one(&backend.db)
        .await
        .unwrap();
        assert_eq!(receipts, 1);
        assert_eq!(last_dispatch_epoch, 2);
        assert_eq!(state, "applied");
    }

    #[tokio::test]
    async fn restarted_edit_observer_detects_conflict_and_never_executes() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("state.txt"), "before\n").unwrap();
        let args = serde_json::json!({
            "path": "state.txt",
            "old_string": "before",
            "new_string": "after"
        });
        let ctx = objective_ctx(dir.path());
        prime_file_receipt_without_dispatch(
            &backend,
            &ctx,
            "edit-before-conflict-crash",
            "edit_file",
            &args,
        )
        .await;
        std::fs::write(dir.path().join("state.txt"), "third-party\n").unwrap();
        claim_file_recovery(&backend, 2).await;
        let resumed = call_with_args("edit-after-conflict-crash", "edit_file", &args);
        register_tool_call(&backend, &resumed, &args).await;
        let mut recovery_ctx = ctx.clone();
        recovery_ctx.mutation_permit = Some(current_permit(2));

        let out = backend
            .execute(&resumed, &args, &recovery_ctx)
            .await
            .unwrap();

        assert_eq!(out.status, ToolExecutionStatus::Waiting);
        assert_eq!(
            out.metadata
                .as_ref()
                .and_then(|metadata| metadata.get("code"))
                .and_then(serde_json::Value::as_str),
            Some("tool_observation_conflict")
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("state.txt")).unwrap(),
            "third-party\n"
        );
        let state: String =
            sqlx::query_scalar("SELECT state FROM side_effect_observation_contracts")
                .fetch_one(&backend.db)
                .await
                .unwrap();
        assert_eq!(state, "conflict");
    }

    #[tokio::test]
    async fn restarted_file_observer_persists_still_unknown_when_locator_cannot_be_read() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        let args = serde_json::json!({"path": "state.txt", "content": "expected\n"});
        let ctx = objective_ctx(&cwd);
        prime_file_receipt_without_dispatch(
            &backend,
            &ctx,
            "write-before-unreadable-crash",
            "write_file",
            &args,
        )
        .await;
        claim_file_recovery(&backend, 2).await;
        drop(dir);
        let resumed = call_with_args("write-after-unreadable-crash", "write_file", &args);
        register_tool_call(&backend, &resumed, &args).await;
        let mut recovery_ctx = ctx;
        recovery_ctx.mutation_permit = Some(current_permit(2));

        let out = backend
            .execute(&resumed, &args, &recovery_ctx)
            .await
            .unwrap();

        assert_eq!(out.status, ToolExecutionStatus::Waiting);
        assert_eq!(
            out.metadata
                .as_ref()
                .and_then(|metadata| metadata.get("code"))
                .and_then(serde_json::Value::as_str),
            Some("external_state_uncertain")
        );
        let state: String =
            sqlx::query_scalar("SELECT state FROM side_effect_observation_contracts")
                .fetch_one(&backend.db)
                .await
                .unwrap();
        assert_eq!(state, "still_unknown");
    }

    #[tokio::test]
    async fn browser_open_close_and_select_tab_require_mutation_receipts() {
        for (action, args) in [
            (
                "open",
                serde_json::json!({"action": "open", "url": "https://example.invalid"}),
            ),
            (
                "close",
                serde_json::json!({"action": "close", "session_id": "codefactory-existing"}),
            ),
            (
                "select_tab",
                serde_json::json!({
                    "action": "select_tab",
                    "session_id": "codefactory-existing",
                    "target": "tab-1"
                }),
            ),
            ("attach", serde_json::json!({"action": "attach"})),
        ] {
            assert_objective_bound_call_starts_receipt(
                "browser_session",
                &format!("browser-{action}"),
                &args,
            )
            .await;
        }
    }

    #[test]
    fn browser_read_actions_never_require_a_mutation_receipt() {
        for action in ["tabs", "read", "find", "snapshot"] {
            let args = serde_json::json!({"action": action});
            assert!(!native_requires_mutation_receipt(
                "browser_session",
                &args,
                &ToolKind::ReadOnly,
            ));
        }
    }

    #[tokio::test]
    async fn unobservable_browser_act_is_fenced_before_native_dispatch() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        let args = serde_json::json!({
            "action": "click",
            "session_id": "codefactory-never-dispatch",
            "target": "ref_1"
        });
        let call = call_with_args(
            "browser-click-without-postcondition",
            "browser_session",
            &args,
        );
        register_tool_call(&backend, &call, &args).await;

        let output = backend
            .execute(&call, &args, &objective_ctx(dir.path()))
            .await
            .unwrap();

        assert_eq!(output.status, ToolExecutionStatus::Waiting);
        assert_eq!(
            output
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("code"))
                .and_then(serde_json::Value::as_str),
            Some("browser_observation_contract_required"),
        );
        let receipt_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM side_effect_receipts")
            .fetch_one(&backend.db)
            .await
            .unwrap();
        assert_eq!(receipt_count, 0, "no dispatchable receipt may be created");
    }

    #[tokio::test]
    async fn mutation_without_root_or_task_is_fenced_before_native_dispatch() {
        let backend = backend().await;
        let dir = tempfile::tempdir().unwrap();
        let args = serde_json::json!({"path": "must-not-exist.txt", "content": "side effect"});
        let call = call_with_args("unbound-native-mutation", "write_file", &args);
        let outcome = backend
            .execute(
                &call,
                &args,
                &ToolCtx {
                    working_directory: dir.path().to_path_buf(),
                    ..Default::default()
                },
            )
            .await;

        assert!(
            outcome.is_err()
                || outcome
                    .as_ref()
                    .is_ok_and(|result| { matches!(result.status, ToolExecutionStatus::Waiting) }),
            "an identity-free mutation must be rejected or held for system reconciliation"
        );
        assert!(
            !dir.path().join("must-not-exist.txt").exists(),
            "an identity-free mutation must have zero native dispatches"
        );
    }

    #[tokio::test]
    async fn mutation_without_opaque_objective_is_fenced_before_native_dispatch() {
        let backend = backend().await;
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&backend.db)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, objective_id)
             VALUES ('unbound-root', 'unbound-session', NULL)",
        )
        .execute(&backend.db)
        .await
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let args = serde_json::json!({"path": "must-not-exist.txt", "content": "side effect"});
        let call = call_with_args("opaque-objective-missing", "write_file", &args);
        let outcome = backend
            .execute(
                &call,
                &args,
                &ToolCtx {
                    working_directory: dir.path().to_path_buf(),
                    session_id: Some("unbound-session".into()),
                    root_turn_id: Some("unbound-root".into()),
                    trajectory_session_id: Some("unbound-session".into()),
                    ..Default::default()
                },
            )
            .await;

        assert!(
            outcome.is_err(),
            "missing opaque Objective must fail closed"
        );
        assert!(!dir.path().join("must-not-exist.txt").exists());
    }

    #[tokio::test]
    async fn waiting_tool_outcome_is_persisted_as_waiting_not_trajectory_error() {
        let backend = objective_backend(false).await;
        let args = append_once_args();
        let call = call_with_args("waiting-trajectory", "bash", &args);
        register_tool_call(&backend, &call, &args).await;

        crate::trajectory::record_terminal_tool_outcome(
            &backend.db,
            TEST_SESSION_ID,
            &call.id,
            "waiting",
            Some("system-owned observation pending"),
            None,
            7,
        )
        .await
        .expect("Waiting is durable lifecycle state, not a trajectory write error");

        let (status, error): (String, Option<String>) =
            sqlx::query_as("SELECT status, error FROM tool_calls WHERE id=?")
                .bind(crate::trajectory::trace_record_id(
                    TEST_SESSION_ID,
                    &call.id,
                ))
                .fetch_one(&backend.db)
                .await
                .unwrap();
        assert_eq!(status, "waiting");
        assert_eq!(error, None);
    }

    #[tokio::test]
    async fn objective_bound_mutation_records_receipt_and_tool_attribution() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        let args = append_once_args();
        let tool_call = call_with_args("mutation-attribution", "bash", &args);
        register_tool_call(&backend, &tool_call, &args).await;
        let now = chrono::Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, created_at, updated_at)
             VALUES ('delivery-domain-collision', ?, 'delivery', 'chat_root_turn', ?, 2,
                     'sha256:other-domain', ?, ?)",
        )
        .bind(TEST_OBJECTIVE_ID)
        .bind(TEST_ROOT_TURN_ID)
        .bind(now)
        .bind(now)
        .execute(&backend.db)
        .await
        .unwrap();

        let out = backend
            .execute(&tool_call, &args, &objective_ctx(dir.path()))
            .await
            .expect("the permitted mutation itself is not fatal");
        assert!(!out.is_error, "mutation output: {}", out.content);

        let attribution: (Option<String>, Option<String>, Option<String>, Option<i64>) =
            sqlx::query_as(
                "SELECT objective_id, binding_id, action_signature, resource_generation
             FROM tool_calls WHERE id=?",
            )
            .bind(crate::trajectory::trace_record_id(
                TEST_SESSION_ID,
                &tool_call.id,
            ))
            .fetch_one(&backend.db)
            .await
            .unwrap();
        assert_eq!(attribution.0.as_deref(), Some(TEST_OBJECTIVE_ID));
        assert_eq!(attribution.1.as_deref(), Some(TEST_BINDING_ID));
        assert!(
            attribution
                .2
                .as_deref()
                .is_some_and(|signature| signature.starts_with("sha256:")),
            "a mutation must carry a canonical, opaque action signature"
        );
        assert_eq!(attribution.3, Some(1));

        let receipt: (String, String, String) = sqlx::query_as(
            "SELECT objective_id, status, action_fingerprint
             FROM side_effect_receipts",
        )
        .fetch_one(&backend.db)
        .await
        .expect("a receipt must be durable before success is returned");
        assert_eq!(receipt.0, TEST_OBJECTIVE_ID);
        assert_eq!(receipt.1, "committed");
        assert_eq!(Some(receipt.2.as_str()), attribution.2.as_deref());
    }

    #[tokio::test]
    async fn forced_reprompt_reuses_one_committed_receipt_across_provider_call_ids() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        let args = append_once_args();
        let ctx = objective_ctx(dir.path());

        for provider_call_id in ["mutation-before-reprompt", "mutation-after-reprompt"] {
            let tool_call = call_with_args(provider_call_id, "bash", &args);
            register_tool_call(&backend, &tool_call, &args).await;
            let out = backend
                .execute(&tool_call, &args, &ctx)
                .await
                .expect("receipt replay is a normal tool result");
            assert!(!out.is_error, "mutation/replay output: {}", out.content);
        }

        let content = std::fs::read_to_string(dir.path().join("effect.log")).unwrap();
        assert_eq!(
            content.lines().collect::<Vec<_>>(),
            vec!["once"],
            "the same durable tool call must launch its side effect at most once"
        );
        let receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM side_effect_receipts
             WHERE objective_id=? AND status='committed'",
        )
        .bind(TEST_OBJECTIVE_ID)
        .fetch_one(&backend.db)
        .await
        .unwrap();
        assert_eq!(receipts, 1);
        let summary: String = sqlx::query_scalar(
            "SELECT summary_json FROM side_effect_receipts WHERE objective_id=?",
        )
        .bind(TEST_OBJECTIVE_ID)
        .fetch_one(&backend.db)
        .await
        .unwrap();
        assert!(
            !summary.contains("once"),
            "receipt summaries store no raw output"
        );
    }

    #[tokio::test]
    async fn uncertain_prior_mutation_forces_observe_only_instead_of_relaunch() {
        let backend = objective_backend(false).await;
        let dir = tempfile::tempdir().unwrap();
        let args = append_once_args();
        let first = call_with_args("mutation-before-crash", "bash", &args);
        register_tool_call(&backend, &first, &args).await;
        let ctx = objective_ctx(dir.path());
        backend
            .execute(&first, &args, &ctx)
            .await
            .expect("first mutation completes");

        let changed = sqlx::query(
            "UPDATE side_effect_receipts SET status='unknown'
             WHERE objective_id=? AND status='committed'",
        )
        .bind(TEST_OBJECTIVE_ID)
        .execute(&backend.db)
        .await
        .unwrap();
        assert_eq!(
            changed.rows_affected(),
            1,
            "the first mutation must have established its receipt fence"
        );

        // A resumed model may receive a fresh provider call id for the same
        // action. The unknown fingerprint, not that ephemeral id, is the fence.
        let resumed = call_with_args("mutation-after-takeover", "bash", &args);
        register_tool_call(&backend, &resumed, &args).await;
        let out = backend
            .execute(&resumed, &args, &ctx)
            .await
            .expect("uncertainty is a system-owned waiting result, not a fatal/user handoff");
        assert!(matches!(out.status, ToolExecutionStatus::Waiting));
        assert_eq!(
            out.metadata
                .as_ref()
                .and_then(|metadata| metadata.get("code"))
                .and_then(serde_json::Value::as_str),
            Some("external_state_uncertain")
        );
        let content = std::fs::read_to_string(dir.path().join("effect.log")).unwrap();
        assert_eq!(content.lines().collect::<Vec<_>>(), vec!["once"]);
    }

    #[tokio::test]
    async fn waiting_objective_without_its_current_mutation_permit_cannot_launch() {
        let backend = objective_backend(true).await;
        let dir = tempfile::tempdir().unwrap();
        let args = append_once_args();
        let stale = call_with_args("mutation-from-stale-runner", "bash", &args);
        register_tool_call(&backend, &stale, &args).await;

        // This context identifies the Objective but carries no owner/epoch
        // permit. The durable row is already claimed by a replacement owner.
        let out = backend
            .execute(&stale, &args, &objective_ctx(dir.path()))
            .await
            .expect("a fenced mutation becomes system-owned waiting");
        assert!(matches!(out.status, ToolExecutionStatus::Waiting));
        assert_eq!(
            out.metadata
                .as_ref()
                .and_then(|metadata| metadata.get("code"))
                .and_then(serde_json::Value::as_str),
            Some("mutation_permit_lost")
        );
        assert!(
            !dir.path().join("effect.log").exists(),
            "the stale runner must be fenced before external dispatch"
        );
    }

    #[tokio::test]
    async fn waiting_objective_with_current_permit_executes_and_commits_receipt() {
        let backend = objective_backend(true).await;
        let dir = tempfile::tempdir().unwrap();
        let args = append_once_args();
        let call = call_with_args("mutation-current-permit", "bash", &args);
        register_tool_call(&backend, &call, &args).await;
        let mut ctx = objective_ctx(dir.path());
        ctx.mutation_permit = Some(current_permit(2));

        let out = backend.execute(&call, &args, &ctx).await.unwrap();
        assert_eq!(out.status, ToolExecutionStatus::Done);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("effect.log"))
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            vec!["once"]
        );
        let status: String =
            sqlx::query_scalar("SELECT status FROM side_effect_receipts WHERE objective_id=?")
                .bind(TEST_OBJECTIVE_ID)
                .fetch_one(&backend.db)
                .await
                .unwrap();
        assert_eq!(status, "committed");
    }

    #[tokio::test]
    async fn stale_epoch_is_fenced_even_when_owner_string_is_unchanged() {
        let backend = objective_backend(true).await;
        let dir = tempfile::tempdir().unwrap();
        let args = append_once_args();
        let call = call_with_args("mutation-stale-same-owner", "bash", &args);
        register_tool_call(&backend, &call, &args).await;
        let mut ctx = objective_ctx(dir.path());
        ctx.mutation_permit = Some(current_permit(1));

        let out = backend.execute(&call, &args, &ctx).await.unwrap();
        assert_eq!(out.status, ToolExecutionStatus::Waiting);
        assert_eq!(
            out.metadata
                .as_ref()
                .and_then(|metadata| metadata.get("code"))
                .and_then(serde_json::Value::as_str),
            Some("mutation_permit_lost")
        );
        assert!(!dir.path().join("effect.log").exists());
        let receipts: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM side_effect_receipts WHERE objective_id=?")
                .bind(TEST_OBJECTIVE_ID)
                .fetch_one(&backend.db)
                .await
                .unwrap();
        assert_eq!(receipts, 0);
    }

    #[tokio::test]
    async fn expired_current_permit_is_fenced_before_dispatch() {
        let backend = objective_backend(true).await;
        let expired = chrono::Utc::now().timestamp_millis() - 1;
        sqlx::query("UPDATE objective_remediations SET lease_expires_at=?")
            .bind(expired)
            .execute(&backend.db)
            .await
            .unwrap();
        sqlx::query("UPDATE objectives SET lease_expires_at=? WHERE id=?")
            .bind(expired)
            .bind(TEST_OBJECTIVE_ID)
            .execute(&backend.db)
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let args = append_once_args();
        let call = call_with_args("mutation-expired-permit", "bash", &args);
        register_tool_call(&backend, &call, &args).await;
        let mut ctx = objective_ctx(dir.path());
        ctx.mutation_permit = Some(current_permit(2));

        let out = backend.execute(&call, &args, &ctx).await.unwrap();
        assert_eq!(out.status, ToolExecutionStatus::Waiting);
        assert_eq!(
            out.metadata
                .as_ref()
                .and_then(|metadata| metadata.get("code"))
                .and_then(serde_json::Value::as_str),
            Some("mutation_permit_lost")
        );
        assert!(!dir.path().join("effect.log").exists());
    }
}
