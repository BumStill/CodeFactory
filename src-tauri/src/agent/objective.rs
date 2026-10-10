// SPDX-License-Identifier: Apache-2.0
//! Durable, cross-domain business-objective control plane.
//!
//! Turn, task, tool and delivery rows remain compatibility projections. This
//! module owns the additive truth tables and the typed transitions that decide
//! whether work remains system-owned, needs genuinely new user input, or has
//! enough evidence to complete.

use anyhow::{anyhow, bail, Context};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};
use uuid::Uuid;

macro_rules! string_enum {
    ($name:ident { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $value),+ }
            }

            fn parse(value: &str) -> anyhow::Result<Self> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    _ => bail!("unknown {} value: {value}", stringify!($name)),
                }
            }
        }
    };
}

string_enum!(ObjectiveKind {
    Informational => "informational",
    LocalMutation => "local_mutation",
    Delivery => "delivery",
    Live => "live",
    LegacyOrphan => "legacy_orphan",
});

const fn objective_kind_rank(kind: ObjectiveKind) -> u8 {
    match kind {
        ObjectiveKind::LegacyOrphan => 0,
        ObjectiveKind::Informational => 1,
        ObjectiveKind::LocalMutation => 2,
        ObjectiveKind::Delivery => 3,
        ObjectiveKind::Live => 4,
    }
}

pub fn current_process_instance() -> String {
    format!(
        "{}:{}",
        std::process::id(),
        crate::storage::db::current_process_start_token()
            .unwrap_or_else(|| "unknown-process-start".into())
    )
}

string_enum!(ObjectiveStatus {
    Active => "active",
    WaitingSystem => "waiting_system",
    WaitingCoreInput => "waiting_core_input",
    WaitingAuthorization => "waiting_authorization",
    WaitingBusinessDecision => "waiting_business_decision",
    Completed => "completed",
    Cancelled => "cancelled",
    // The honest failure terminal: every system-owned route was tried and none
    // worked. It sits beside `completed`/`cancelled` as a real terminal state,
    // never a "waiting for a future capability" limbo the user cannot act on.
    Failed => "failed",
    LegacyOrphan => "legacy_orphan",
});

/// Reason recorded when system-owned recovery gave up. Reused verbatim on the
/// transport turn (`chat_turn_state.terminal_reason`) so a settled turn and the
/// objective that produced it name the same thing.
pub const TECHNICAL_RECOVERY_EXHAUSTED: &str = "technical_recovery_exhausted";

/// U21 (2026-10-07). The provider episode fence, in the one case U21 keeps:
/// the last model request started a tool or other external mutation whose
/// receipt is still unresolved, so nothing proves the request safe to replay
/// yet. Unlike an interrupted stream (which U21 settles as replay-safe), the
/// tool side may still reconcile that receipt, so this keeps the ordinary
/// bounded ladder; its own code only lets the failed terminal say why.
pub const PROVIDER_EPISODE_UNRECONCILED: &str = "provider_episode_unreconciled";

/// U18/R3: the remediation's durable turn identity disagrees with the
/// Objective's live turn and reconciliation cannot decide which turn to resume.
/// Replaying the same call is guaranteed to produce the same answer, so this is
/// settled as a terminal failure on the first occurrence instead of being
/// retried until the recovery budget runs out.
pub const CHAT_IDENTITY_UNRECONCILABLE: &str = "chat_identity_unreconcilable";

/// An Objective that stopped without deciding anything: not a provider fault,
/// not a tool fault — the turn simply ended and left the row `active`, owning
/// no lease, no remediation and no scheduled observation.
pub const OBJECTIVE_PROGRESS_STALLED: &str = "objective_progress_stalled";

/// How long an `active` Objective may go without a heartbeat before it is
/// treated as abandoned. Deliberately generous: a healthy turn running a ten-
/// minute build still publishes activity, so nothing legitimate comes close.
pub const STALLED_ACTIVE_OBJECTIVE_MS: i64 = 30 * 60 * 1000;
pub const OBJECTIVE_INCIDENT_CONTROLLER: &str = "objective-incident-controller";

/// Version of the durable Chat recovery contract, not the application build.
/// Bump only when Chat can safely resume a class of incidents that the previous
/// contract had to park. Other domains own independent revisions below.
/// Merely restarting or installing an unrelated build must never buy another
/// recovery budget.
pub const RECOVERY_CAPABILITY_REVISION: i64 = 1;

fn recovery_capability_contract(domain: RecoveryDomain) -> (i64, bool, &'static str) {
    match domain {
        RecoveryDomain::Chat => (
            RECOVERY_CAPABILITY_REVISION,
            true,
            "chat-v1:exact-root-binding:reconcile-side-effects:resume-same-objective",
        ),
        RecoveryDomain::Context => (1, false, "context-v1:parked-reactivation-disabled"),
        RecoveryDomain::Tool => (1, false, "tool-v1:parked-reactivation-disabled"),
        RecoveryDomain::Permission => (1, false, "permission-v1:parked-reactivation-disabled"),
        RecoveryDomain::Task => (1, false, "task-v1:parked-reactivation-disabled"),
        RecoveryDomain::Provider => (1, false, "provider-v1:parked-reactivation-disabled"),
        RecoveryDomain::Auth => (1, false, "auth-v1:parked-reactivation-disabled"),
        RecoveryDomain::Browser => (1, false, "browser-v1:parked-reactivation-disabled"),
        RecoveryDomain::Terminal => (1, false, "terminal-v1:parked-reactivation-disabled"),
        RecoveryDomain::Delivery => (1, false, "delivery-v1:parked-reactivation-disabled"),
        RecoveryDomain::Release => (1, false, "release-v1:parked-reactivation-disabled"),
        RecoveryDomain::Update => (1, false, "update-v1:parked-reactivation-disabled"),
    }
}

/// A typed durable wait for another local execution owner to release the App.
/// Observing the same unsafe restart point is expected state, not a failed
/// recovery attempt, so it must never consume the technical recovery budget.
pub const UPDATE_SAFE_POINT_PENDING: &str = "update_safe_point_pending";

/// How many durable remediations one failure signature may buy before the
/// system admits it is not making progress. Counted as a cumulative tally per
/// `(objective, recovery_generation, failure_signature)` rather than a
/// consecutive streak: an intervening different failure code must not hand the
/// same broken route a fresh budget. Only an explicit user reprompt after
/// exhaustion starts another independently bounded generation.
pub const MAX_SIGNATURE_RECOVERY_ATTEMPTS: i64 = 5;

/// The verdict "you finished, but the evidence does not support it".
pub const COMPLETION_EVIDENCE_INCOMPLETE: &str = "completion_evidence_incomplete";

/// A completion verdict that saw no new evidence cannot improve by being asked
/// again: the model already answered, the arbiter already refused it, and
/// nothing between two identical rounds changed. Every further round costs the
/// user another copy of the same answer — 2026-09-08 produced EIGHT between
/// 10:01 and 10:23, and exhausted the budget anyway — so this class converges
/// far sooner than the generic transient ladder.
///
/// This is not a tighter leash on progress. Folding the evidence digest into
/// the failure signature is what makes stagnation observable: a round that DID
/// gather something mints a different signature and starts again with the full
/// budget. Only a round that changed nothing is charged here.
const MAX_STAGNANT_COMPLETION_ATTEMPTS: i64 = 2;

/// How many repeats of ONE signature are allowed before the Objective parks.
pub(crate) fn max_signature_attempts_for(failure_code: Option<&str>) -> i64 {
    if failure_code == Some(COMPLETION_EVIDENCE_INCOMPLETE) {
        return MAX_STAGNANT_COMPLETION_ATTEMPTS;
    }
    if failure_code == Some(PROVIDER_TRANSPORT_UNREACHABLE) {
        // U25: a network outage must not be charged like N independent failures
        // of the same broken thing. The window (`TRANSPORT_OUTAGE_WINDOW_MS`)
        // is what ends an outage; this ceiling only bounds real retries that ran
        // once the path was up again.
        return MAX_TRANSPORT_RECOVERY_ATTEMPTS;
    }
    MAX_SIGNATURE_RECOVERY_ATTEMPTS
}

/// A repeating failure signature must not spend the whole recovery budget in
/// seconds. Scheduling every repeat at a flat five seconds means five identical
/// errors — a condition lasting well under a minute — permanently park the
/// Objective. Each repeat now pushes the next observation further out, so a
/// transient condition has room to clear before the ceiling is reached, while a
/// genuinely stuck one still exhausts.
const RECOVERY_BACKOFF_BASE_MS: i64 = 5_000;
const RECOVERY_BACKOFF_FACTOR: i64 = 4;
const RECOVERY_BACKOFF_CAP_MS: i64 = 5 * 60 * 1_000;

/// Attempts older than this no longer count toward the signature ceiling. The
/// budget is meant to detect "not making progress right now", not to remember
/// that the same error happened once a day for a week.
const SIGNATURE_RECOVERY_WINDOW_MS: i64 = 30 * 60 * 1_000;

/// M30: how long a settlement waits for the live run it just told to stop to
/// reach its next boundary. Deliberately short — it is a courtesy window that
/// lets the loop stop its in-flight command and flush its last message before
/// the terminal state is written, not a deadline the settlement depends on.
/// Commands themselves are killed at the boundary, so a run that is mid-tool
/// still stops here, and one that somehow does not is caught by the durable
/// settlement watermark on `messages` instead of by waiting longer.
const LIVE_RUN_STOP_GRACE_MS: u64 = 2_000;

/// A provider endpoint that keeps answering "unavailable" is not the kind of
/// transient condition the growing ladder was built for. Waiting it out only
/// shows "等待中" for twelve minutes while nothing can change until the user
/// picks another model, so this class converges on a short flat interval and
/// reaches its ceiling — and therefore a visible, settled state — in about a
/// minute instead.
pub const PROVIDER_ENDPOINT_UNAVAILABLE: &str = "provider_endpoint_unavailable";
const DEAD_ENDPOINT_RETRY_MS: i64 = 10_000;

/// U25 (2026-10-08). The transport never reached a verdict: DNS, TCP connect,
/// TLS, a read timeout, a reset, or a gateway that answered nothing useful.
/// Unlike `PROVIDER_ENDPOINT_UNAVAILABLE` this is a property of the *network
/// path*, not an answer from the service, so it gets its own code and its own
/// policy: wait it out, then say so honestly.
pub const PROVIDER_TRANSPORT_UNREACHABLE: &str = "provider_transport_unreachable";

/// How long one transport outage may last before the objective stops waiting
/// and settles. Fifteen minutes is deliberately far past the events that
/// motivate this: the 2026-10-08 DeepSeek outage recovered on its own after
/// ~3 minutes, a Wi‑Fi switch or VPN reconnect is seconds, and a proxy restart
/// is under a minute — while still being short enough that nobody is left
/// watching "等待中" for an afternoon.
const TRANSPORT_OUTAGE_WINDOW_MS: i64 = 15 * 60 * 1_000;

/// How often the cheap reachability probe is repeated while the path is down.
/// The probe is what keeps an outage from spending the recovery budget: a
/// connect that still fails means there is nothing to retry yet, so no model
/// turn runs and no attempt is charged.
const TRANSPORT_PROBE_INTERVAL_MS: i64 = 20_000;
const TRANSPORT_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The growing ladder for the retries that *do* run (the path is reachable
/// again but the turn still failed): 15s, 45s, 135s, then 4 minutes each. Eight
/// of those span well over the window above, which is what "growing backoff for
/// 10–15 minutes" needs; the window, not this ladder, is what ends an outage.
const TRANSPORT_RETRY_BASE_MS: i64 = 15_000;
const TRANSPORT_RETRY_FACTOR: i64 = 3;
const TRANSPORT_RETRY_CAP_MS: i64 = 4 * 60 * 1_000;

/// One outage is one failure, not N. The retry ceiling is raised for transport
/// failures because the probe (not the ladder) is what turns an outage into a
/// wait: these attempts are real turns that ran against a reachable service.
const MAX_TRANSPORT_RECOVERY_ATTEMPTS: i64 = 8;

/// Advance a session's activity time so the sidebar — which orders by
/// `sessions.updated_at` and renders a relative time from it — does not bury a
/// session the recovery ceiling just wrote a visible notice into. Measured on a
/// real user's database: sessions written to at 14:40 still sorted as three days
/// old, because settling an incident wrote the message and nothing else.
///
/// Best-effort and schema-tolerant by design: it runs inside the settlement
/// transaction (the pool may allow a single connection, so it cannot open its
/// own), several test pools carry no `sessions` table at all, and a sidebar row
/// one update stale is a far smaller problem than failing the settlement.
async fn touch_session_in_settlement(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    session_id: &str,
    now: i64,
) {
    let has_sessions: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='sessions'",
    )
    .fetch_optional(&mut **tx)
    .await
    .ok()
    .flatten();
    if has_sessions.is_none() {
        return;
    }
    let _ = sqlx::query("UPDATE sessions SET updated_at=? WHERE id=? AND updated_at<?")
        .bind(now)
        .bind(session_id)
        .bind(now)
        .execute(&mut **tx)
        .await;
}

/// U18/R2: an Objective owns exactly ONE live chat turn at a time. Every path
/// that closes the objective's own turn must also close the turns it left
/// behind — otherwise a user reprompt (or a steer during system recovery) leaves
/// the superseded root `active` forever: the sidebar keeps showing 运行中 and the
/// composer can only enqueue "当前执行结束后发送".
///
/// `keep_root_turn_id` names the single turn that stays live (the objective's
/// `COALESCE(resume_cursor, root_turn_id)`); `None` closes every non-terminal
/// turn, which is what a terminal objective requires.
///
/// Idempotent by construction — only non-terminal rows are touched — so a second
/// startup pass writes nothing. It never edits `sessions`, preserving U3's S1
/// rule that background convergence must not reorder the sidebar.
pub(crate) async fn settle_superseded_chat_turns_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    objective_id: &str,
    keep_root_turn_id: Option<&str>,
    terminal_reason: &str,
    now: i64,
) -> anyhow::Result<u64> {
    let has_turn_state: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='chat_turn_state'",
    )
    .fetch_one(&mut **tx)
    .await?;
    if has_turn_state == 0 {
        return Ok(0);
    }
    // Build the SET list from the columns this database actually has: the
    // conversation projection grew over several releases (`turn_settled_at`,
    // `stream_closed_at`, …), and older/minimal fixtures only carry the
    // original shape. Writing a column that is absent would turn convergence
    // itself into a startup failure.
    let columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('chat_turn_state')")
            .fetch_all(&mut **tx)
            .await?;
    let has = |column: &str| columns.iter().any(|name| name == column);
    let has_objectives: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='objectives'",
    )
    .fetch_one(&mut **tx)
    .await?;

    enum Binding {
        Int(i64),
        Text(String),
    }
    let mut sets: Vec<String> = vec!["status='completed'".into()];
    let mut binds: Vec<Binding> = Vec::new();
    if has("phase") {
        sets.push("phase='finalizing'".into());
    }
    if has("revision") {
        sets.push("revision=revision+1".into());
    }
    if has("recent_activity_kind") {
        sets.push("recent_activity_kind='superseded'".into());
    }
    if has("recent_activity_label") {
        sets.push("recent_activity_label='该回合已被同一目标的新回合接管'".into());
    }
    if has("waiting_reason") {
        sets.push("waiting_reason=NULL".into());
    }
    if has("updated_at") {
        sets.push("updated_at=?".into());
        binds.push(Binding::Int(now));
    }
    if has("completed_at") {
        sets.push("completed_at=COALESCE(completed_at, ?)".into());
        binds.push(Binding::Int(now));
    }
    if has("terminal_reason") {
        sets.push("terminal_reason=?".into());
        binds.push(Binding::Text(terminal_reason.to_string()));
    }
    if has("objective_revision") && has_objectives == 1 {
        sets.push(
            "objective_revision=COALESCE((SELECT revision FROM objectives WHERE id=?), objective_revision)"
                .into(),
        );
        binds.push(Binding::Text(objective_id.to_string()));
    }
    if has("turn_settled_at") {
        sets.push("turn_settled_at=COALESCE(turn_settled_at, ?)".into());
        binds.push(Binding::Int(now));
    }
    if has("stream_closed_at") {
        sets.push("stream_closed_at=COALESCE(stream_closed_at, ?)".into());
        binds.push(Binding::Int(now));
    }
    if has("next_action") {
        sets.push("next_action=NULL".into());
    }
    let statement = format!(
        "UPDATE chat_turn_state SET {}
         WHERE objective_id=?
           AND root_turn_id<>COALESCE(?, '')
           AND status NOT IN ('completed','cancelled')",
        sets.join(", ")
    );
    let mut query = sqlx::query(&statement);
    for binding in &binds {
        query = match binding {
            Binding::Int(value) => query.bind(*value),
            Binding::Text(value) => query.bind(value.clone()),
        };
    }
    let result = query
        .bind(objective_id)
        .bind(keep_root_turn_id.unwrap_or(""))
        .execute(&mut **tx)
        .await?;
    Ok(result.rows_affected())
}

/// The text a parked system incident shows. It stays system-owned either way —
/// no `requires_user_action` is manufactured — but it must not tell someone
/// "你不需要补充输入" when the only thing that can move the objective forward is
/// them choosing a reachable model.
pub(crate) fn parked_incident_message(failure_code: Option<&str>) -> &'static str {
    if failure_code == Some(PROVIDER_ENDPOINT_UNAVAILABLE) {
        // The endpoint *answered* and refused (the model is gone or switched
        // off), so picking another model is a real, honest next step — but it is
        // an offer, not a demand: the system keeps observing routes on its own.
        return "这条模型线路明确拒绝了请求（例如该模型已不可用）。本回合已停在安全边界，\
进度和上下文都已保留：可以在输入框旁换成另一个可用模型继续同一目标，不必重述需求。";
    }
    if failure_code == Some(PROVIDER_TRANSPORT_UNREACHABLE) {
        return "网络或模型服务连不上了，本回合已停在安全边界。进度和上下文都已保留：\
网络或服务恢复后直接回一句「继续」，就会在原来的进度上接着做。";
    }
    "本回合的自动恢复已达到安全上限，已登记为系统故障。你不需要补充输入；CodeFactory 会在恢复策略或能力更新后续接同一目标。"
}

/// What to do about a transport outage on the next observation. Pure so the
/// policy — wait, retry, or give up — is testable without a network.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TransportOutageAction {
    /// Still unreachable, still inside the window: look again later. No model
    /// turn runs and no recovery budget is spent.
    Wait { delay_ms: i64 },
    /// The path answers again (or there is nothing to probe): let the ordinary
    /// retry ladder run.
    Retry,
    /// Unreachable for the whole window: settle honestly.
    GiveUp {
        unreachable_ms: i64,
        attempts: i64,
    },
}

pub(crate) fn transport_outage_action(
    now: i64,
    first_unreachable_at: i64,
    attempts: i64,
    probe_reachable: bool,
) -> TransportOutageAction {
    // A probe that answers means the outage is over, whatever the clock says:
    // the retry that follows is a real attempt against a live path.
    if probe_reachable {
        return TransportOutageAction::Retry;
    }
    let unreachable_ms = now.saturating_sub(first_unreachable_at).max(0);
    if unreachable_ms >= TRANSPORT_OUTAGE_WINDOW_MS {
        return TransportOutageAction::GiveUp {
            unreachable_ms,
            attempts: attempts.max(1),
        };
    }
    // Never step past the end of the window: the window is the answer, not
    // something the probe interval is allowed to walk out of.
    let remaining = TRANSPORT_OUTAGE_WINDOW_MS - unreachable_ms;
    TransportOutageAction::Wait {
        delay_ms: TRANSPORT_PROBE_INTERVAL_MS.min(remaining).max(1_000),
    }
}

/// The host and port a failed request was aimed at, read back out of the
/// transport error itself (`error sending request for url
/// (https://api.deepseek.com/chat/completions)`). The recorded failure detail
/// already carries that text, so the probe needs no new plumbing — and it
/// probes the exact route that just failed rather than some generic endpoint.
pub(crate) fn probe_target_from_error_text(text: &str) -> Option<(String, u16)> {
    let scheme_end = text.find("://")?;
    let rest = &text[scheme_end + 3..];
    let host_end = rest
        .find(|c: char| {
            c == '/' || c == '"' || c == ')' || c == '?' || c == '\'' || c.is_whitespace()
        })
        .unwrap_or(rest.len());
    let authority = &rest[..host_end];
    if authority.is_empty() {
        return None;
    }
    let default_port = if text[..scheme_end].eq_ignore_ascii_case("http") {
        80
    } else {
        443
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && !host.contains(']') => match port.parse::<u16>() {
            Ok(port) => (host.to_string(), port),
            Err(_) => (authority.to_string(), default_port),
        },
        _ => (authority.to_string(), default_port),
    };
    (!host.is_empty()).then_some((host, port))
}

/// The cheap check requirement 2 asks for: one TCP connect to the route that
/// just failed. It never issues a model request, so a path that is still down
/// costs a connection attempt instead of an attempt at the task.
pub(crate) async fn probe_transport_reachability(host: &str, port: u16) -> bool {
    matches!(
        tokio::time::timeout(
            TRANSPORT_PROBE_TIMEOUT,
            tokio::net::TcpStream::connect((host, port))
        )
        .await,
        Ok(Ok(_))
    )
}

/// The honest end of a transport outage, in plain language: how long the
/// network or service was unreachable, how many times it was tried, that the
/// work is kept, and how to pick it back up. Same banned-vocabulary guard as the
/// failure summary — this is shown to users verbatim.
pub(crate) fn transport_outage_terminal_message(unreachable_ms: i64, attempts: i64) -> String {
    let minutes = (unreachable_ms.max(0) as f64 / 60_000.0).round().max(1.0) as i64;
    format!(
        "网络或模型服务连不上，已经连续连不上约 {minutes} 分钟，期间一共试了 {attempts} 次，都没能连上，\
所以这件事先停在这里。已经做好的改动和对话内容都保留着，没有丢。\
等网络或模型服务恢复正常后，直接回一句「继续」，就会在原来的进度上接着做。"
    )
}

/// U33 / CF-PFB-R12. The longest a still-recovering session may sit without any
/// attempt. Every wait this app schedules is clamped to it, so "no attempt for
/// more than five minutes" is a defect by construction rather than a promise.
/// Five minutes is deliberately five times the transport probe interval and
/// above the transport ladder's cap, so the clamp only ever bites a schedule
/// that was already walking away from the user (production 2026-10-09 saw gaps
/// of ~40 minutes between retries on one parked objective).
pub(crate) const MAX_WAIT_BEFORE_NEXT_ATTEMPT_MS: i64 = 5 * 60 * 1_000;

/// U33 / CF-PFB-R12. Clamp a proposed observation time to the bound above.
pub(crate) fn bounded_next_observation_at(now: i64, proposed_at: i64) -> i64 {
    proposed_at.min(now.saturating_add(MAX_WAIT_BEFORE_NEXT_ATTEMPT_MS))
}

fn recovery_backoff_ms_for(failure_code: Option<&str>, prior_attempts: i64) -> i64 {
    if failure_code == Some(PROVIDER_ENDPOINT_UNAVAILABLE) {
        return DEAD_ENDPOINT_RETRY_MS;
    }
    if failure_code == Some(PROVIDER_TRANSPORT_UNREACHABLE) {
        // U25: a transient transport condition needs room to clear, so it keeps
        // a growing ladder — but a bounded one, because the outage window above
        // is what decides when waiting has gone on long enough.
        let mut delay = TRANSPORT_RETRY_BASE_MS;
        for _ in 0..prior_attempts.clamp(0, 8) {
            delay = delay.saturating_mul(TRANSPORT_RETRY_FACTOR);
            if delay >= TRANSPORT_RETRY_CAP_MS {
                return TRANSPORT_RETRY_CAP_MS;
            }
        }
        return delay.min(TRANSPORT_RETRY_CAP_MS);
    }
    recovery_backoff_ms(prior_attempts)
}

fn recovery_backoff_ms(prior_attempts: i64) -> i64 {
    let mut delay = RECOVERY_BACKOFF_BASE_MS;
    for _ in 0..prior_attempts.clamp(0, 8) {
        delay = delay.saturating_mul(RECOVERY_BACKOFF_FACTOR);
        if delay >= RECOVERY_BACKOFF_CAP_MS {
            return RECOVERY_BACKOFF_CAP_MS;
        }
    }
    delay.min(RECOVERY_BACKOFF_CAP_MS)
}

/// Backstop for signatures that never repeat. Some routes embed varying error
/// text in the signature (task recovery hashes the message), which would
/// otherwise leave every per-signature tally at 1 forever. This bounds one
/// objective's total system-owned recovery regardless of how the signature
/// churns; it is deliberately far above the per-signature ceiling so ordinary
/// multi-stage recovery never trips it.
pub const MAX_OBJECTIVE_RECOVERY_ATTEMPTS: i64 = 20;

/// Enough of a stack or provider response to recognise the fault, far short of
/// a whole stream. A stored diagnostic is for reading later, not for archiving
/// payloads, and it is redacted before it is truncated.
const FAILURE_DETAIL_MAX_CHARS: usize = 1_500;

impl ObjectiveStatus {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }

    /// Every terminal status as a SQL literal list, kept in step with
    /// [`Self::is_terminal`]. Query helpers asking "is this Objective still
    /// live?" build their `[NOT] IN (...)` clause from this rather than
    /// repeating the vocabulary and silently forgetting a member.
    pub const TERMINAL_SQL: &'static str = "'completed', 'cancelled', 'failed'";

    pub const fn is_system_owned(self) -> bool {
        matches!(self, Self::Active | Self::WaitingSystem)
    }
}

string_enum!(RecoveryDomain {
    Chat => "chat",
    Context => "context",
    Tool => "tool",
    Permission => "permission",
    Task => "task",
    Provider => "provider",
    Auth => "auth",
    Browser => "browser",
    Terminal => "terminal",
    Delivery => "delivery",
    Release => "release",
    Update => "update",
});

impl RecoveryDomain {
    /// Closed world used by the recovery adapter registry and its conformance
    /// tests. Adding a domain without registering an adapter must fail review
    /// and tests instead of silently falling back to another domain's runner.
    pub const ALL: [Self; 12] = [
        Self::Chat,
        Self::Context,
        Self::Tool,
        Self::Permission,
        Self::Task,
        Self::Provider,
        Self::Auth,
        Self::Browser,
        Self::Terminal,
        Self::Delivery,
        Self::Release,
        Self::Update,
    ];
}

string_enum!(DecisionType {
    Continue => "continue",
    Waiting => "waiting",
    ApplyRecommended => "apply_recommended",
    PlatformIncident => "platform_incident",
    FailedInternal => "failed_internal",
    CoreInputRequired => "core_input_required",
    AuthorizationRequired => "authorization_required",
    NeedsBusinessDecision => "needs_business_decision",
    Complete => "complete",
    Cancelled => "cancelled",
});

string_enum!(EvidenceKind {
    InformationalAnswer => "informational_answer",
    CurrentStateAcceptance => "current_state_acceptance",
    ChangeSet => "change_set",
    PostChangeValidation => "post_change_validation",
    DeliveryReceipt => "delivery_receipt",
    LiveVerification => "live_verification",
});

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectiveSnapshot {
    pub id: String,
    pub revision: i64,
    pub kind: ObjectiveKind,
    pub session_id: Option<String>,
    pub root_turn_id: Option<String>,
    pub task_id: Option<String>,
    pub delivery_run_id: Option<String>,
    pub status: ObjectiveStatus,
    pub decision_type: DecisionType,
    pub domain: RecoveryDomain,
    pub requested_acceptance: String,
    pub reached_acceptance: Option<String>,
    pub requires_user_action: bool,
    pub request_key: Option<String>,
    pub decision_key: Option<String>,
    pub action_signature: Option<String>,
    pub failure_code: Option<String>,
    pub failure_signature: Option<String>,
    pub recovery_owner: Option<String>,
    pub remediation_id: Option<String>,
    pub resume_cursor: Option<String>,
    pub output_started: bool,
    pub side_effect_started: bool,
    pub next_observation_at: Option<i64>,
    pub lease_owner: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub evidence_ref: Option<String>,
    pub cancellation_provenance: Option<String>,
    pub attention_request: Option<UserAttentionRequest>,
    pub last_progress_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
    #[serde(default)]
    pub recovery_generation: i64,
}

impl ObjectiveSnapshot {
    pub fn new(
        id: impl Into<String>,
        kind: ObjectiveKind,
        domain: RecoveryDomain,
        requested_acceptance: impl Into<String>,
    ) -> Self {
        let now = Utc::now().timestamp_millis();
        Self {
            id: id.into(),
            revision: 1,
            kind,
            session_id: None,
            root_turn_id: None,
            task_id: None,
            delivery_run_id: None,
            status: ObjectiveStatus::Active,
            decision_type: DecisionType::Continue,
            domain,
            requested_acceptance: requested_acceptance.into(),
            reached_acceptance: None,
            requires_user_action: false,
            request_key: None,
            decision_key: None,
            action_signature: None,
            failure_code: None,
            failure_signature: None,
            recovery_owner: Some("objective-supervisor".into()),
            remediation_id: None,
            resume_cursor: None,
            output_started: false,
            side_effect_started: false,
            next_observation_at: None,
            lease_owner: None,
            lease_expires_at: None,
            evidence_ref: None,
            cancellation_provenance: None,
            attention_request: None,
            last_progress_at: Some(now),
            created_at: now,
            updated_at: now,
            completed_at: None,
            recovery_generation: 0,
        }
    }

    pub fn as_decision(&self) -> DecisionEnvelope {
        DecisionEnvelope {
            objective_id: self.id.clone(),
            revision: self.revision + 1,
            domain: self.domain,
            decision_type: self.decision_type,
            status: self.status,
            failure_code: self.failure_code.clone(),
            failure_signature: self.failure_signature.clone(),
            recovery_owner: self.recovery_owner.clone(),
            remediation_id: self.remediation_id.clone(),
            next_observation_at: self.next_observation_at,
            next_action_authorized: self.status.is_system_owned(),
            requires_user_action: self.requires_user_action,
            request_key: self.request_key.clone(),
            decision_key: self.decision_key.clone(),
            action_signature: self.action_signature.clone(),
            output_started: self.output_started,
            side_effect_started: self.side_effect_started,
            resume_cursor: self.resume_cursor.clone(),
            reached_acceptance: self.reached_acceptance.clone(),
            evidence: None,
            visible_final_message_id: None,
            cancellation_provenance: None,
            attention_request: self.attention_request.clone(),
            // Routing a decision is not itself user activity; callers that
            // settle state left by an earlier process mark it `as_convergence()`.
            settlement_origin: SettlementOrigin::Live,
            transport_probe_wait: false,
        }
    }

    fn from_row(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<Self> {
        Ok(Self {
            id: row.try_get("id")?,
            revision: row.try_get("revision")?,
            kind: ObjectiveKind::parse(row.try_get::<String, _>("kind")?.as_str())?,
            session_id: row.try_get("session_id")?,
            root_turn_id: row.try_get("root_turn_id")?,
            task_id: row.try_get("task_id")?,
            delivery_run_id: row.try_get("delivery_run_id")?,
            status: ObjectiveStatus::parse(row.try_get::<String, _>("status")?.as_str())?,
            decision_type: DecisionType::parse(
                row.try_get::<String, _>("decision_type")?.as_str(),
            )?,
            domain: RecoveryDomain::parse(row.try_get::<String, _>("domain")?.as_str())?,
            requested_acceptance: row.try_get("requested_acceptance")?,
            reached_acceptance: row.try_get("reached_acceptance")?,
            requires_user_action: row.try_get::<i64, _>("requires_user_action")? != 0,
            request_key: row.try_get("request_key")?,
            decision_key: row.try_get("decision_key")?,
            action_signature: row.try_get("action_signature")?,
            failure_code: row.try_get("failure_code")?,
            failure_signature: row.try_get("failure_signature")?,
            recovery_owner: row.try_get("recovery_owner")?,
            remediation_id: row.try_get("remediation_id")?,
            resume_cursor: row.try_get("resume_cursor")?,
            output_started: row.try_get::<i64, _>("output_started")? != 0,
            side_effect_started: row.try_get::<i64, _>("side_effect_started")? != 0,
            next_observation_at: row.try_get("next_observation_at")?,
            lease_owner: row.try_get("lease_owner")?,
            lease_expires_at: row.try_get("lease_expires_at")?,
            evidence_ref: row.try_get("evidence_ref")?,
            cancellation_provenance: row.try_get("cancellation_provenance")?,
            attention_request: row
                .try_get::<Option<String>, _>("attention_request_json")
                .unwrap_or(None)
                .map(|value| serde_json::from_str(&value))
                .transpose()?,
            last_progress_at: row.try_get("last_progress_at")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            completed_at: row.try_get("completed_at")?,
            // Minimal legacy/adapter schemas may be observed before the
            // startup compatibility pass has added the column. Generation
            // zero is the exact historical meaning; ensure_schema persists it
            // before any new recovery can be admitted.
            recovery_generation: row.try_get("recovery_generation").unwrap_or(0),
        })
    }
}

#[derive(Debug, Clone)]
pub struct CreateObjective {
    pub id: String,
    pub kind: ObjectiveKind,
    pub session_id: Option<String>,
    pub root_turn_id: Option<String>,
    pub domain: RecoveryDomain,
    pub requested_acceptance: String,
    pub created_surface: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObjectiveEvidence {
    pub id: String,
    pub kind: EvidenceKind,
    pub scope: String,
    pub digest: String,
    pub evidence_ref: String,
    pub observed_at: i64,
    pub reached_acceptance: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserAttentionRequest {
    pub kind: String,
    pub prompt: String,
    pub missing_inputs: Vec<String>,
    pub attempted_routes: Vec<String>,
    pub decision_options: Vec<String>,
    pub recommended_option: Option<String>,
    pub irreversible: bool,
    pub no_safe_default_reason: Option<String>,
    pub resume_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionEnvelope {
    pub objective_id: String,
    pub revision: i64,
    pub domain: RecoveryDomain,
    pub decision_type: DecisionType,
    pub status: ObjectiveStatus,
    pub failure_code: Option<String>,
    pub failure_signature: Option<String>,
    pub recovery_owner: Option<String>,
    pub remediation_id: Option<String>,
    pub next_observation_at: Option<i64>,
    pub next_action_authorized: bool,
    pub requires_user_action: bool,
    pub request_key: Option<String>,
    pub decision_key: Option<String>,
    pub action_signature: Option<String>,
    pub output_started: bool,
    pub side_effect_started: bool,
    pub resume_cursor: Option<String>,
    pub reached_acceptance: Option<String>,
    pub evidence: Option<ObjectiveEvidence>,
    pub visible_final_message_id: Option<String>,
    pub cancellation_provenance: Option<String>,
    pub attention_request: Option<UserAttentionRequest>,
    /// Who is settling this decision: a turn failing while the user is actually
    /// in the session, or startup/background convergence of rows an earlier
    /// process left behind. Only the former is user activity and may advance
    /// `sessions.updated_at` — see `SettlementOrigin`.
    #[serde(default)]
    pub settlement_origin: SettlementOrigin,
    /// U25: this wait is a transport outage that is *still* unreachable, so it
    /// must not queue a remediation. A queued remediation is a charged recovery
    /// attempt plus a full model turn aimed at a path we just proved is down;
    /// the cheap probe replaces both until the path answers again.
    #[serde(default)]
    pub transport_probe_wait: bool,
}

/// Whether settling a decision is user-visible activity.
///
/// A stalled pre-#553 Objective is settled at startup by backgrounds
/// reconcilers. That is bookkeeping about a session the user is not sitting in,
/// so advancing its `updated_at` drags a long-dead conversation to the top of
/// the sidebar (S1). Live failures keep the old behaviour: the session the user
/// is watching should reflect that something just happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SettlementOrigin {
    /// A turn is failing while the user is in the session.
    #[default]
    Live,
    /// Startup/background convergence of state left by an earlier process.
    Convergence,
}

impl DecisionEnvelope {
    /// Mark this decision as startup/background convergence rather than live
    /// user activity, so settling it cannot reorder the sidebar.
    pub fn as_convergence(mut self) -> Self {
        self.settlement_origin = SettlementOrigin::Convergence;
        self
    }

    /// A system-owned recovery ceiling reached a terminal `failed` objective.
    /// This is a real failure terminal, not a paused incident: the turn is
    /// settled, no lease/observation remains, and nothing will ever wake it.
    fn is_parked_system_incident(&self) -> bool {
        self.status == ObjectiveStatus::Failed
            && self.decision_type == DecisionType::FailedInternal
            && self.failure_code.as_deref() == Some(TECHNICAL_RECOVERY_EXHAUSTED)
            && !self.requires_user_action
    }

    fn validate(&self, objective: &ObjectiveSnapshot) -> anyhow::Result<()> {
        if self.objective_id != objective.id || self.revision != objective.revision + 1 {
            bail!("objective decision revision/identity mismatch");
        }
        if objective.status.is_terminal() {
            bail!("terminal objective cannot accept another decision");
        }
        let user_state = matches!(
            self.status,
            ObjectiveStatus::WaitingCoreInput
                | ObjectiveStatus::WaitingAuthorization
                | ObjectiveStatus::WaitingBusinessDecision
        );
        let user_decision = matches!(
            self.decision_type,
            DecisionType::CoreInputRequired
                | DecisionType::AuthorizationRequired
                | DecisionType::NeedsBusinessDecision
        );
        if self.requires_user_action != (user_state && user_decision) {
            bail!("requires_user_action does not match typed attention state");
        }
        if self.status == ObjectiveStatus::WaitingSystem
            && !self.is_parked_system_incident()
            && (self
                .recovery_owner
                .as_deref()
                .unwrap_or_default()
                .is_empty()
                || self
                    .remediation_id
                    .as_deref()
                    .unwrap_or_default()
                    .is_empty()
                || self.next_observation_at.is_none())
        {
            bail!("system wait requires owner, remediation and next observation");
        }
        if self.is_parked_system_incident()
            && (self.recovery_owner.as_deref() != Some(OBJECTIVE_INCIDENT_CONTROLLER)
                || self.remediation_id.is_some()
                || self.next_observation_at.is_some()
                || self.request_key.is_some())
        {
            bail!("parked system incident requires the incident controller and no user/retry wait");
        }
        if matches!(
            self.decision_type,
            DecisionType::CoreInputRequired | DecisionType::AuthorizationRequired
        ) && self.request_key.as_deref().unwrap_or_default().is_empty()
        {
            bail!("core input/authorization requires a stable request_key");
        }
        if self.decision_type == DecisionType::CoreInputRequired {
            let attention = self
                .attention_request
                .as_ref()
                .ok_or_else(|| anyhow!("core input requires a persisted attention payload"))?;
            if attention.kind != "core_input"
                || attention.prompt.trim().is_empty()
                || attention.missing_inputs.is_empty()
                || attention.attempted_routes.is_empty()
                || !attention.decision_options.is_empty()
            {
                bail!("core input attention payload is incomplete");
            }
        }
        if self.decision_type == DecisionType::AuthorizationRequired
            && self
                .action_signature
                .as_deref()
                .unwrap_or_default()
                .is_empty()
        {
            bail!("authorization requires an action signature");
        }
        if self.decision_type == DecisionType::NeedsBusinessDecision
            && self.decision_key.as_deref().unwrap_or_default().is_empty()
        {
            bail!("business decision requires a stable decision_key");
        }
        if self.decision_type == DecisionType::NeedsBusinessDecision {
            let attention = self.attention_request.as_ref().ok_or_else(|| {
                anyhow!("business decision requires a persisted attention payload")
            })?;
            let recommended = attention.recommended_option.as_deref().unwrap_or_default();
            if attention.kind != "business_decision"
                || attention.prompt.trim().is_empty()
                || attention.decision_options.len() < 2
                || !attention
                    .decision_options
                    .iter()
                    .any(|item| item == recommended)
                || !attention.irreversible
                || attention
                    .no_safe_default_reason
                    .as_deref()
                    .unwrap_or_default()
                    .trim()
                    .is_empty()
            {
                bail!("business decision attention payload is incomplete");
            }
        }
        if matches!(
            self.failure_code.as_deref(),
            Some(
                TECHNICAL_RECOVERY_EXHAUSTED
                    | "external_state_uncertain"
                    | "completion_evidence_incomplete"
            )
        ) && self.requires_user_action
        {
            bail!("technical failure cannot require user action");
        }
        if self.decision_type == DecisionType::Complete
            && (self.status != ObjectiveStatus::Completed || self.evidence.is_none())
        {
            bail!("completion requires typed evidence from CompletionArbiter");
        }
        if self.decision_type == DecisionType::Cancelled
            && !matches!(
                self.cancellation_provenance.as_deref(),
                Some("explicit_cancel" | "explicit_deny")
            )
        {
            bail!("cancellation requires explicit user provenance");
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum RouteSignal {
    TechnicalFailure {
        domain: RecoveryDomain,
        failure_code: String,
        failure_signature: String,
        next_observation_at: i64,
        resume_cursor: Option<String>,
    },
    CapabilityRestored {
        domain: RecoveryDomain,
        reason: String,
        next_observation_at: i64,
        resume_cursor: Option<String>,
    },
    CoreInputRequired {
        domain: RecoveryDomain,
        request_key: String,
        missing_inputs: Vec<String>,
        attempted_routes: Vec<String>,
        resume_cursor: Option<String>,
    },
    AuthorizationRequired {
        domain: RecoveryDomain,
        request_key: String,
        action_signature: String,
        resume_cursor: Option<String>,
    },
    BusinessDecisionRequired {
        domain: RecoveryDomain,
        decision_key: String,
        prompt: String,
        options: Vec<String>,
        recommended_option: String,
        no_safe_default_reason: String,
        resume_cursor: Option<String>,
    },
    Cancelled {
        domain: RecoveryDomain,
        provenance: String,
    },
}

pub struct DecisionRouter;

impl DecisionRouter {
    pub fn route(
        objective: &ObjectiveSnapshot,
        signal: RouteSignal,
    ) -> anyhow::Result<DecisionEnvelope> {
        let mut decision = objective.as_decision();
        decision.failure_code = None;
        decision.failure_signature = None;
        decision.request_key = None;
        decision.decision_key = None;
        decision.action_signature = None;
        decision.attention_request = None;
        decision.evidence = None;
        decision.visible_final_message_id = None;
        decision.cancellation_provenance = None;
        match signal {
            RouteSignal::TechnicalFailure {
                domain,
                failure_code,
                failure_signature,
                next_observation_at,
                resume_cursor,
            } => {
                decision.domain = domain;
                decision.decision_type = match failure_code.as_str() {
                    "platform_incident" | "permission_timed_out" | "permission_channel_closed" => {
                        DecisionType::PlatformIncident
                    }
                    "failed_internal" | "panic" => DecisionType::FailedInternal,
                    _ => DecisionType::Waiting,
                };
                decision.status = ObjectiveStatus::WaitingSystem;
                decision.failure_code = Some(failure_code);
                decision.failure_signature = Some(failure_signature);
                decision.recovery_owner = Some(format!("objective-supervisor:{}", domain.as_str()));
                decision.remediation_id = Some(Uuid::new_v4().to_string());
                decision.next_observation_at = Some(next_observation_at);
                decision.next_action_authorized = true;
                decision.requires_user_action = false;
                decision.resume_cursor = resume_cursor;
            }
            RouteSignal::CapabilityRestored {
                domain,
                reason,
                next_observation_at,
                resume_cursor,
            } => {
                decision.domain = domain;
                // Keep the stable authorization identity while the restored
                // capability is queued and claimed. Domain adapters bind their
                // secret-free receipt to this key; clearing it here would split
                // the OAuth callback from the very Objective it must resume.
                decision.request_key = objective.request_key.clone();
                decision.decision_type = DecisionType::ApplyRecommended;
                decision.status = ObjectiveStatus::WaitingSystem;
                decision.failure_code = Some(reason.clone());
                decision.failure_signature = Some(reason);
                decision.recovery_owner = Some(format!("objective-supervisor:{}", domain.as_str()));
                decision.remediation_id = Some(Uuid::new_v4().to_string());
                decision.next_observation_at = Some(next_observation_at);
                decision.next_action_authorized = true;
                decision.requires_user_action = false;
                decision.resume_cursor = resume_cursor;
            }
            RouteSignal::CoreInputRequired {
                domain,
                request_key,
                missing_inputs,
                attempted_routes,
                resume_cursor,
            } => {
                if missing_inputs.is_empty() || attempted_routes.is_empty() {
                    bail!("core input requires missing_inputs and attempted_routes");
                }
                decision.domain = domain;
                decision.decision_type = DecisionType::CoreInputRequired;
                decision.status = ObjectiveStatus::WaitingCoreInput;
                decision.request_key = Some(request_key);
                decision.attention_request = Some(UserAttentionRequest {
                    kind: "core_input".into(),
                    prompt: format!(
                        "请提供以下无法由系统安全推导的核心输入：{}",
                        missing_inputs.join("、")
                    ),
                    missing_inputs,
                    attempted_routes,
                    decision_options: Vec::new(),
                    recommended_option: None,
                    irreversible: false,
                    no_safe_default_reason: None,
                    resume_cursor: resume_cursor.clone(),
                });
                decision.requires_user_action = true;
                decision.next_action_authorized = false;
                decision.recovery_owner = None;
                decision.remediation_id = None;
                decision.next_observation_at = None;
                decision.resume_cursor = resume_cursor;
            }
            RouteSignal::AuthorizationRequired {
                domain,
                request_key,
                action_signature,
                resume_cursor,
            } => {
                decision.domain = domain;
                decision.decision_type = DecisionType::AuthorizationRequired;
                decision.status = ObjectiveStatus::WaitingAuthorization;
                decision.request_key = Some(request_key);
                decision.action_signature = Some(action_signature);
                decision.requires_user_action = true;
                decision.next_action_authorized = false;
                decision.recovery_owner = None;
                decision.remediation_id = None;
                decision.next_observation_at = None;
                decision.resume_cursor = resume_cursor;
            }
            RouteSignal::BusinessDecisionRequired {
                domain,
                decision_key,
                prompt,
                options,
                recommended_option,
                no_safe_default_reason,
                resume_cursor,
            } => {
                if options.len() < 2
                    || !options.contains(&recommended_option)
                    || prompt.trim().is_empty()
                    || no_safe_default_reason.trim().is_empty()
                {
                    bail!("business decision requires finite options, a recommendation and no-safe-default reason");
                }
                decision.domain = domain;
                decision.decision_type = DecisionType::NeedsBusinessDecision;
                decision.status = ObjectiveStatus::WaitingBusinessDecision;
                decision.decision_key = Some(decision_key);
                decision.attention_request = Some(UserAttentionRequest {
                    kind: "business_decision".into(),
                    prompt,
                    missing_inputs: Vec::new(),
                    attempted_routes: Vec::new(),
                    decision_options: options,
                    recommended_option: Some(recommended_option),
                    irreversible: true,
                    no_safe_default_reason: Some(no_safe_default_reason),
                    resume_cursor: resume_cursor.clone(),
                });
                decision.requires_user_action = true;
                decision.next_action_authorized = false;
                decision.recovery_owner = None;
                decision.remediation_id = None;
                decision.next_observation_at = None;
                decision.resume_cursor = resume_cursor;
            }
            RouteSignal::Cancelled { domain, provenance } => {
                decision.domain = domain;
                decision.decision_type = DecisionType::Cancelled;
                decision.status = ObjectiveStatus::Cancelled;
                decision.requires_user_action = false;
                decision.next_action_authorized = false;
                decision.recovery_owner = None;
                decision.remediation_id = None;
                decision.next_observation_at = None;
                decision.cancellation_provenance = Some(provenance);
            }
        }
        decision.validate(objective)?;
        Ok(decision)
    }
}

pub struct CompletionArbiter;

impl CompletionArbiter {
    pub fn decide(
        objective: &ObjectiveSnapshot,
        evidence: &[ObjectiveEvidence],
    ) -> anyhow::Result<DecisionEnvelope> {
        let has = |kind| evidence.iter().any(|item| item.kind == kind);
        let satisfied = match objective.kind {
            ObjectiveKind::Informational => {
                has(EvidenceKind::InformationalAnswer) || has(EvidenceKind::CurrentStateAcceptance)
            }
            ObjectiveKind::LocalMutation => {
                has(EvidenceKind::CurrentStateAcceptance)
                    || (has(EvidenceKind::ChangeSet) && has(EvidenceKind::PostChangeValidation))
            }
            ObjectiveKind::Delivery => has(EvidenceKind::DeliveryReceipt),
            ObjectiveKind::Live => {
                has(EvidenceKind::DeliveryReceipt) && has(EvidenceKind::LiveVerification)
            }
            ObjectiveKind::LegacyOrphan => false,
        };
        if !satisfied {
            bail!("completion evidence does not satisfy objective kind");
        }
        let terminal_evidence = evidence
            .last()
            .cloned()
            .ok_or_else(|| anyhow!("completion evidence is empty"))?;
        let mut decision = objective.as_decision();
        decision.decision_type = DecisionType::Complete;
        decision.status = ObjectiveStatus::Completed;
        decision.reached_acceptance = Some(terminal_evidence.reached_acceptance.clone());
        decision.evidence = Some(terminal_evidence);
        decision.recovery_owner = None;
        decision.remediation_id = None;
        decision.next_observation_at = None;
        decision.next_action_authorized = false;
        decision.requires_user_action = false;
        decision.validate(objective)?;
        Ok(decision)
    }
}

/// Convert a transport-level terminal into one Objective decision. A model
/// reply, a closed stream, or an exhausted run budget never decides business
/// completion by itself; the objective kind and durable evidence do.
pub fn decision_for_run_outcome(
    objective: &ObjectiveSnapshot,
    outcome: &codefactory_agent_loop::run::RunOutcome,
) -> anyhow::Result<DecisionEnvelope> {
    decision_for_run_outcome_with_reason(objective, outcome, None)
}

pub fn decision_for_run_outcome_with_reason(
    objective: &ObjectiveSnapshot,
    outcome: &codefactory_agent_loop::run::RunOutcome,
    terminal_reason: Option<&str>,
) -> anyhow::Result<DecisionEnvelope> {
    use codefactory_agent_loop::run::StopReason;

    if outcome.stop_reason == StopReason::Cancelled
        || (outcome.stop_reason == StopReason::Blocked
            && terminal_reason == Some("permission_denied_by_user"))
    {
        return DecisionRouter::route(
            objective,
            RouteSignal::Cancelled {
                domain: RecoveryDomain::Chat,
                provenance: if terminal_reason == Some("permission_denied_by_user") {
                    "explicit_deny".into()
                } else {
                    "explicit_cancel".into()
                },
            },
        );
    }

    if outcome.stop_reason == StopReason::Finished {
        let now = Utc::now().timestamp_millis();
        let scope = objective
            .root_turn_id
            .as_deref()
            .or(objective.task_id.as_deref())
            .unwrap_or(&objective.id)
            .to_string();
        let evidence_ref = format!("objective-run:{}:{}", objective.id, objective.revision + 1);
        let digest = |material: &str| format!("sha256:{:x}", Sha256::digest(material.as_bytes()));
        let mut evidence = Vec::new();
        let completion = &outcome.completion_evidence;

        match objective.kind {
            ObjectiveKind::Informational if !outcome.final_text.trim().is_empty() => {
                evidence.push(ObjectiveEvidence {
                    id: Uuid::new_v4().to_string(),
                    kind: EvidenceKind::InformationalAnswer,
                    scope: scope.clone(),
                    digest: digest(&outcome.final_text),
                    evidence_ref: evidence_ref.clone(),
                    observed_at: now,
                    reached_acceptance: objective.requested_acceptance.clone(),
                });
            }
            ObjectiveKind::LocalMutation if completion.completed => {
                let mutation = completion
                    .last_source_mutation_sequence
                    .or(completion.last_mutation_sequence);
                let validation = completion
                    .last_successful_project_test_sequence
                    .or(completion.last_successful_verification_sequence);
                if let Some(sequence) = mutation {
                    evidence.push(ObjectiveEvidence {
                        id: Uuid::new_v4().to_string(),
                        kind: EvidenceKind::ChangeSet,
                        scope: scope.clone(),
                        digest: digest(&format!("mutation:{sequence}")),
                        evidence_ref: evidence_ref.clone(),
                        observed_at: now,
                        reached_acceptance: objective.requested_acceptance.clone(),
                    });
                    if let Some(sequence) = validation {
                        evidence.push(ObjectiveEvidence {
                            id: Uuid::new_v4().to_string(),
                            kind: EvidenceKind::PostChangeValidation,
                            scope: scope.clone(),
                            digest: digest(&format!("validation:{sequence}")),
                            evidence_ref: evidence_ref.clone(),
                            observed_at: now,
                            reached_acceptance: objective.requested_acceptance.clone(),
                        });
                    }
                } else if let Some(sequence) = validation {
                    // An implementation-capable turn may discover that no
                    // workspace mutation is needed (for example a corrected
                    // installed-Skill command).  A real successful probe is
                    // typed current-state acceptance; prose alone still emits
                    // no evidence and remains non-terminal.
                    evidence.push(ObjectiveEvidence {
                        id: Uuid::new_v4().to_string(),
                        kind: EvidenceKind::CurrentStateAcceptance,
                        scope: scope.clone(),
                        digest: digest(&format!("current-state-validation:{sequence}")),
                        evidence_ref: evidence_ref.clone(),
                        observed_at: now,
                        reached_acceptance: objective.requested_acceptance.clone(),
                    });
                }
            }
            ObjectiveKind::Delivery | ObjectiveKind::Live
                if completion.delivery_completion_satisfied =>
            {
                evidence.push(ObjectiveEvidence {
                    id: Uuid::new_v4().to_string(),
                    kind: EvidenceKind::DeliveryReceipt,
                    scope: scope.clone(),
                    digest: digest(&format!(
                        "delivery:{:?}:{:?}",
                        completion.delivery_requested_ceiling, completion.delivery_reached_ceiling
                    )),
                    evidence_ref: evidence_ref.clone(),
                    observed_at: now,
                    reached_acceptance: objective.requested_acceptance.clone(),
                });
                if objective.kind == ObjectiveKind::Live
                    && !completion.required_observable_states.is_empty()
                    && completion
                        .required_observable_states
                        .iter()
                        .all(|required| completion.observed_observable_states.contains(required))
                {
                    evidence.push(ObjectiveEvidence {
                        id: Uuid::new_v4().to_string(),
                        kind: EvidenceKind::LiveVerification,
                        scope: scope.clone(),
                        digest: digest(&completion.observed_observable_states.join("\n")),
                        evidence_ref: evidence_ref.clone(),
                        observed_at: now,
                        reached_acceptance: objective.requested_acceptance.clone(),
                    });
                }
            }
            _ => {}
        }

        if let Ok(mut decision) = CompletionArbiter::decide(objective, &evidence) {
            decision.visible_final_message_id = outcome.final_message_id.clone();
            return Ok(decision);
        }
    }

    if outcome.stop_reason == StopReason::Blocked
        && terminal_reason == Some("browser_pairing_required")
    {
        return DecisionRouter::route(
            objective,
            RouteSignal::CoreInputRequired {
                domain: RecoveryDomain::Browser,
                request_key: format!("browser-pairing:{}", objective.id),
                missing_inputs: vec!["load_and_pair_browser_extension".into()],
                attempted_routes: vec!["browser_extension_bridge".into()],
                resume_cursor: objective
                    .resume_cursor
                    .clone()
                    .or_else(|| objective.root_turn_id.clone())
                    .or_else(|| objective.task_id.clone()),
            },
        );
    }

    let (failure_code, domain) = match outcome.stop_reason {
        StopReason::PlatformIncident
            if matches!(
                terminal_reason,
                Some("permission_timed_out" | "permission_channel_closed")
            ) =>
        {
            (
                terminal_reason.expect("matched permission interruption"),
                RecoveryDomain::Permission,
            )
        }
        StopReason::PlatformIncident
            if matches!(
                terminal_reason,
                Some(
                    "context_compaction_exhausted"
                        | "context_overflow_after_compaction"
                        | "context_compression_unavailable"
                )
            ) =>
        {
            (
                terminal_reason.expect("matched typed context recovery outcome"),
                RecoveryDomain::Context,
            )
        }
        StopReason::PlatformIncident
            if terminal_reason.is_some_and(|reason| reason.starts_with("browser_")) =>
        {
            (
                terminal_reason.expect("matched typed browser recovery outcome"),
                RecoveryDomain::Browser,
            )
        }
        StopReason::PlatformIncident
            if terminal_reason.is_some_and(|reason| reason.starts_with("delivery_")) =>
        {
            (
                terminal_reason.expect("matched typed delivery recovery outcome"),
                RecoveryDomain::Delivery,
            )
        }
        StopReason::PlatformIncident => (
            terminal_reason.unwrap_or("platform_incident"),
            RecoveryDomain::Tool,
        ),
        StopReason::FailedInternal => ("failed_internal", RecoveryDomain::Chat),
        StopReason::BudgetExhausted => ("run_budget_exhausted", RecoveryDomain::Chat),
        StopReason::IterationCeiling => ("iteration_ceiling", RecoveryDomain::Chat),
        StopReason::Incomplete => ("objective_incomplete", RecoveryDomain::Chat),
        StopReason::Blocked => ("run_blocked", RecoveryDomain::Chat),
        StopReason::Finished => (COMPLETION_EVIDENCE_INCOMPLETE, RecoveryDomain::Chat),
        StopReason::Cancelled => unreachable!("explicit cancellation handled above"),
    };
    DecisionRouter::route(
        objective,
        RouteSignal::TechnicalFailure {
            domain,
            failure_code: failure_code.into(),
            failure_signature: format!(
                "{}:{:?}:{}{}",
                objective.id,
                outcome.stop_reason,
                terminal_reason.unwrap_or("none"),
                // A completion verdict is only the SAME failure when the
                // evidence behind it is unchanged. The signature used to be
                // constant per Objective, so a round that gathered real
                // evidence was charged exactly like a round that gathered none:
                // progress was punished and stagnation excused, identically.
                // Fold the evidence state in and the two become distinguishable
                // — which is what lets `max_signature_attempts_for` cut a
                // stagnant loop short without ever shortening a productive one.
                match outcome.stop_reason {
                    StopReason::Finished | StopReason::Incomplete => format!(
                        ":{:x}",
                        Sha256::digest(
                            format!("{:?}", outcome.completion_evidence).as_bytes()
                        )
                    ),
                    _ => String::new(),
                }
            ),
            next_observation_at: Utc::now().timestamp_millis() + 5_000,
            resume_cursor: if matches!(
                domain,
                RecoveryDomain::Context
                    | RecoveryDomain::Browser
                    | RecoveryDomain::Delivery
                    | RecoveryDomain::Tool
            ) {
                objective
                    .resume_cursor
                    .clone()
                    .or_else(|| objective.root_turn_id.clone())
            } else {
                objective.root_turn_id.clone().or(objective.task_id.clone())
            },
        },
    )
}

#[derive(Clone)]
pub struct ObjectiveStore {
    pool: SqlitePool,
}

#[derive(Debug, Clone)]
struct DeliveryCompletionCandidate {
    run_id: String,
    binding_id: String,
    resource_generation: i64,
    stage: String,
    claim_epoch: i64,
    repo_identity: String,
    worktree_identity: String,
    expected_head_sha: String,
    change_set_digest: String,
    requested_ceiling: String,
    reached_ceiling: String,
    canonical_pr_number: i64,
    canonical_pr_url: String,
    action_signature: String,
    reconciliation_receipt_id: String,
    already_terminal: bool,
    takeover_lease_owner: Option<String>,
}

#[derive(Debug, Clone)]
struct DeliveryTakeoverSettlement {
    run_id: String,
    claim_epoch: i64,
    lease_owner: String,
    binding_id: String,
    resource_generation: i64,
    action_signature: String,
}

#[derive(Debug, Clone)]
struct DeliveryIdentityParkPermit {
    run_id: String,
    claim_epoch: i64,
    lease_owner: String,
    failure_signature: String,
    attempt_index: i64,
}

fn delivery_ceiling_rank(value: &str) -> Option<i64> {
    match value {
        "local" => Some(0),
        "committed" => Some(1),
        "pushed" => Some(2),
        "pr_open" => Some(3),
        "ci_green" => Some(4),
        "merge_queued" => Some(5),
        "merged" => Some(6),
        "release_triggered" => Some(7),
        "deployment_succeeded" => Some(8),
        "live_verified" => Some(9),
        _ => None,
    }
}

fn requested_delivery_ceiling_rank(value: &str) -> Option<i64> {
    match value {
        "pr_only" => Some(3),
        "through_ci_green" => Some(4),
        "through_merge" => Some(6),
        "through_release" => Some(9),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct ClaimedRemediation {
    pub objective: ObjectiveSnapshot,
    pub remediation_id: String,
    pub domain: RecoveryDomain,
    pub failure_code: String,
    /// Monotonic claim generation. Owner strings are process-scoped and can
    /// reclaim the same row after expiry; the epoch is what fences an older
    /// future owned by that same process.
    pub claim_epoch: i64,
    pub binding_id: Option<String>,
    pub resource_generation: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct ClaimDueBatch {
    pub claims: Vec<ClaimedRemediation>,
    /// Objective transitions committed while evaluating claim eligibility.
    /// These are post-commit publication receipts, not new work claims.
    pub terminal_transitions: Vec<ObjectiveSnapshot>,
}

fn objective_binding_digest(objective_id: &str, resource_kind: &str, resource_id: &str) -> String {
    let material = format!("{objective_id}\0{resource_kind}\0{resource_id}");
    format!("sha256:{:x}", Sha256::digest(material.as_bytes()))
}

/// Idempotently bind one opaque persisted resource to its Objective. The
/// digest contains only opaque ids; tool arguments, user content and secrets
/// are never copied into the identity ledger.
async fn ensure_objective_binding(
    tx: &mut Transaction<'_, Sqlite>,
    objective_id: &str,
    domain: RecoveryDomain,
    resource_kind: &str,
    resource_id: &str,
    now: i64,
) -> anyhow::Result<(String, i64)> {
    let digest = objective_binding_digest(objective_id, resource_kind, resource_id);
    sqlx::query(
        "INSERT OR IGNORE INTO objective_bindings
         (id, objective_id, domain, resource_kind, resource_id,
          resource_generation, identity_digest, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, 1, ?, ?, ?)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(objective_id)
    .bind(domain.as_str())
    .bind(resource_kind)
    .bind(resource_id)
    .bind(&digest)
    .bind(now)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    let (binding_id, bound_objective_id, generation, stored_digest): (String, String, i64, String) =
        sqlx::query_as(
            "SELECT id, objective_id, resource_generation, identity_digest
         FROM objective_bindings
         WHERE domain=? AND resource_kind=? AND resource_id=?
           AND resource_generation=1",
        )
        .bind(domain.as_str())
        .bind(resource_kind)
        .bind(resource_id)
        .fetch_one(&mut **tx)
        .await?;
    if bound_objective_id != objective_id || stored_digest != digest {
        bail!("objective binding identity conflict for {resource_kind}:{resource_id}");
    }
    Ok((binding_id, generation))
}

/// β (2026-09-29): the wording-constraint audit insert, shared by the pool and
/// transaction writers so both keep identical projection columns.
const TURN_WORDING_CONSTRAINT_INSERT: &str = "INSERT INTO objective_events
     (id, objective_id, revision, event_type, status, decision_type,
      domain, failure_code, detail_json, created_at)
     SELECT ?, id, revision, 'turn_wording_constraint', status,
            decision_type, domain, NULL, ?, ? FROM objectives WHERE id=?";

fn turn_wording_constraint_detail(payload: &serde_json::Value) -> String {
    const TURN_CONSTRAINT_DETAIL_MAX_CHARS: usize = 2048;
    let mut detail = payload.to_string();
    if detail.len() > TURN_CONSTRAINT_DETAIL_MAX_CHARS {
        detail.truncate(TURN_CONSTRAINT_DETAIL_MAX_CHARS);
    }
    detail
}

impl ObjectiveStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Persist the executable recovery contract shipped by this binary. The
    /// digest is deliberately derived from the stable domain/revision contract,
    /// not a version string, so unrelated releases cannot wake parked work.
    pub async fn sync_recovery_capabilities(&self) -> anyhow::Result<()> {
        let now = Utc::now().timestamp_millis();
        for domain in RecoveryDomain::ALL {
            // Revision 1 proves only the historical Chat incident path covered
            // by HLT-001. Other domains retain their ordinary remediation
            // adapters, but parked-incident reactivation stays disabled until
            // that domain ships its own safety/identity scenario and revision.
            let (revision, executable, contract) = recovery_capability_contract(domain);
            let digest = format!(
                "sha256:{:x}",
                Sha256::digest(
                    format!(
                        "recovery-contract\0{}\0{}\0{}\0{}",
                        domain.as_str(),
                        revision,
                        i64::from(executable),
                        contract
                    )
                    .as_bytes()
                )
            );
            let existing: Option<(i64, String)> = sqlx::query_as(
                "SELECT revision, contract_digest FROM recovery_capabilities WHERE domain=?",
            )
            .bind(domain.as_str())
            .fetch_optional(&self.pool)
            .await?;
            if let Some((stored_revision, stored_digest)) = existing.as_ref() {
                if *stored_revision > revision {
                    bail!(
                        "recovery contract for {} would downgrade capability revision {} to {}",
                        domain.as_str(), stored_revision, revision
                    );
                }
                if *stored_revision == revision && stored_digest != &digest {
                    bail!(
                        "recovery contract for {} changed without a capability revision bump",
                        domain.as_str()
                    );
                }
            }
            let synced = sqlx::query(
                "INSERT INTO recovery_capabilities
                 (domain, revision, contract_digest, executable, updated_at)
                 VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT(domain) DO UPDATE SET
                   revision=excluded.revision,
                   contract_digest=excluded.contract_digest,
                   executable=excluded.executable,
                   updated_at=excluded.updated_at
                 WHERE recovery_capabilities.revision<=excluded.revision",
            )
            .bind(domain.as_str())
            .bind(revision)
            .bind(digest)
            .bind(i64::from(executable))
            .bind(now)
            .execute(&self.pool)
            .await?;
            if synced.rows_affected() != 1 {
                bail!(
                    "recovery contract for {} changed concurrently to a newer revision",
                    domain.as_str()
                );
            }
        }
        Ok(())
    }

    /// Atomically convert parked incidents into ordinary due remediations only
    /// when a newer executable contract exists and replay safety is proven.
    /// The incident row is the compare-and-swap admission ledger, so concurrent
    /// processes cannot create two active remediations for one capability bump.
    pub async fn reactivate_eligible_incidents(
        &self,
        limit: i64,
    ) -> anyhow::Result<Vec<ObjectiveSnapshot>> {
        let candidates = sqlx::query_as::<
            _,
            (
                String,
                String,
                i64,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<i64>,
            ),
        >(
            "SELECT incident.objective_id, incident.domain, capability.revision,
                    objective.failure_signature, objective.session_id,
                    COALESCE(incident.resume_cursor, objective.resume_cursor,
                             objective.root_turn_id),
                    binding.id, binding.resource_generation
             FROM objective_incidents incident
             JOIN objectives objective ON objective.id=incident.objective_id
             JOIN recovery_capabilities capability ON capability.domain=incident.domain
             LEFT JOIN objective_bindings binding
               ON binding.objective_id=objective.id
              AND binding.domain=incident.domain
              AND incident.domain='chat'
              AND binding.resource_kind='chat_root_turn'
              AND binding.resource_id=COALESCE(incident.resume_cursor,
                                                objective.resume_cursor,
                                                objective.root_turn_id)
             WHERE incident.status='open'
               AND objective.status='waiting_system'
               AND objective.failure_code=?
               AND objective.recovery_owner=?
               AND objective.remediation_id IS NULL
               AND capability.executable=1
               AND capability.revision>incident.blocked_capability_revision
             ORDER BY incident.updated_at, incident.objective_id
             LIMIT ?",
        )
        .bind(TECHNICAL_RECOVERY_EXHAUSTED)
        .bind(OBJECTIVE_INCIDENT_CONTROLLER)
        .bind(limit.max(1))
        .fetch_all(&self.pool)
        .await?;

        let mut reactivated = Vec::new();
        for (
            objective_id,
            domain_value,
            capability_revision,
            prior_signature,
            session_id,
            resume_cursor,
            binding_id,
            resource_generation,
        ) in candidates
        {
            let domain = RecoveryDomain::parse(&domain_value)?;
            let now = Utc::now().timestamp_millis();
            let mut tx = self.pool.begin().await?;
            let unsafe_side_effects: i64 = sqlx::query_scalar(
                "SELECT
                   (SELECT COUNT(*) FROM side_effect_receipts receipt
                      WHERE receipt.objective_id=objective.id
                        AND receipt.status IN ('started','unknown'))
                   + CASE
                       WHEN objective.side_effect_started=1
                        AND NOT EXISTS (
                          SELECT 1 FROM side_effect_receipts settled
                          WHERE settled.objective_id=objective.id
                            AND settled.status IN ('committed','reconciled','cancelled')
                        )
                       THEN 1 ELSE 0 END
                 FROM objectives objective WHERE objective.id=?",
            )
            .bind(&objective_id)
            .fetch_one(&mut *tx)
            .await?;
            if unsafe_side_effects > 0 {
                sqlx::query(
                    "UPDATE objective_incidents
                     SET reactivation_status='blocked_safety', updated_at=?
                     WHERE objective_id=? AND status='open'
                       AND blocked_capability_revision<?",
                )
                .bind(now)
                .bind(&objective_id)
                .bind(capability_revision)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                continue;
            }
            if domain == RecoveryDomain::Chat
                && (session_id.is_none()
                    || resume_cursor.is_none()
                    || binding_id.is_none()
                    || resource_generation.is_none())
            {
                sqlx::query(
                    "UPDATE objective_incidents
                     SET reactivation_status='blocked_identity', updated_at=?
                     WHERE objective_id=? AND status='open'",
                )
                .bind(now)
                .bind(&objective_id)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                continue;
            }

            let admitted = sqlx::query(
                "UPDATE objective_incidents
                 SET status='resolved', reactivation_status='admitted',
                     reactivation_count=reactivation_count+1,
                     last_reactivated_revision=?, resolved_at=?, updated_at=?
                 WHERE objective_id=? AND status='open'
                   AND blocked_capability_revision<?",
            )
            .bind(capability_revision)
            .bind(now)
            .bind(now)
            .bind(&objective_id)
            .bind(capability_revision)
            .execute(&mut *tx)
            .await?;
            if admitted.rows_affected() != 1 {
                tx.rollback().await?;
                continue;
            }

            let remediation_id = Uuid::new_v4().to_string();
            let failure_code = "recovery_capability_reactivated";
            let failure_signature = format!(
                "{}:capability:{}:{}",
                prior_signature.unwrap_or_else(|| "legacy-incident".into()),
                domain_value,
                capability_revision
            );
            let objective = sqlx::query(
                "UPDATE objectives SET revision=revision+1,
                   status='waiting_system', decision_type='waiting', domain=?,
                   requires_user_action=0, request_key=NULL, decision_key=NULL,
                   attention_request_json=NULL, failure_code=?, failure_signature=?,
                   recovery_owner=?, remediation_id=?, resume_cursor=?,
                   next_observation_at=?, lease_owner=NULL, lease_expires_at=NULL,
                   completed_at=NULL, last_progress_at=?, updated_at=?
                 WHERE id=? AND status='waiting_system' AND failure_code=?
                   AND recovery_owner=? AND remediation_id IS NULL",
            )
            .bind(domain.as_str())
            .bind(failure_code)
            .bind(&failure_signature)
            .bind(format!("objective-supervisor:{}", domain.as_str()))
            .bind(&remediation_id)
            .bind(&resume_cursor)
            .bind(now)
            .bind(now)
            .bind(now)
            .bind(&objective_id)
            .bind(TECHNICAL_RECOVERY_EXHAUSTED)
            .bind(OBJECTIVE_INCIDENT_CONTROLLER)
            .execute(&mut *tx)
            .await?;
            if objective.rows_affected() != 1 {
                tx.rollback().await?;
                continue;
            }
            let recovery_generation: i64 =
                sqlx::query_scalar("SELECT recovery_generation FROM objectives WHERE id=?")
                    .bind(&objective_id)
                    .fetch_one(&mut *tx)
                    .await?;
            sqlx::query(
                "INSERT INTO objective_remediations
                 (id, objective_id, domain, status, failure_code,
                  failure_signature, strategy, approach_index, attempt_index,
                  execution_attempt_index, recovery_generation, resume_cursor,
                  binding_id, next_observation_at, created_at, updated_at)
                 VALUES (?, ?, ?, 'queued', ?, ?, 'reconcile_then_resume',
                         0, 0, 0, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&remediation_id)
            .bind(&objective_id)
            .bind(domain.as_str())
            .bind(failure_code)
            .bind(&failure_signature)
            .bind(recovery_generation)
            .bind(&resume_cursor)
            .bind(&binding_id)
            .bind(now)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await?;

            let has_chat_turn_state: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name='chat_turn_state'",
            )
            .fetch_one(&mut *tx)
            .await?;
            if has_chat_turn_state == 1 {
                if let Some(root_turn_id) = resume_cursor.as_deref() {
                    sqlx::query(
                        "UPDATE chat_turn_state SET revision=revision+1,
                       phase='recovering', status='waiting_system',
                       recent_activity_kind='system_recovery',
                       recent_activity_label='系统恢复能力已更新，正在自动续接',
                       waiting_reason=?, updated_at=?, completed_at=NULL,
                       terminal_reason=NULL, objective_revision=(
                         SELECT revision FROM objectives WHERE id=?
                       ), turn_settled_at=NULL, stream_closed_at=NULL,
                       terminal_revision=NULL, visible_final_message_id=NULL,
                       visible_final_kind=NULL, next_action='resume_automatically'
                     WHERE objective_id=? AND root_turn_id=?",
                    )
                    .bind(failure_code)
                    .bind(now)
                    .bind(&objective_id)
                    .bind(&objective_id)
                    .bind(root_turn_id)
                    .execute(&mut *tx)
                    .await?;
                }
            }
            sqlx::query(
                "INSERT INTO objective_events
                 (id, objective_id, revision, event_type, status, decision_type,
                  domain, failure_code, recovery_owner, detail_json, created_at)
                 SELECT ?, id, revision, 'incident_capability_reactivated',
                        status, decision_type, domain, failure_code, recovery_owner,
                        ?, ? FROM objectives WHERE id=?",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(
                serde_json::json!({
                    "capability_revision": capability_revision,
                    "reactivation": "system_owned",
                })
                .to_string(),
            )
            .bind(now)
            .bind(&objective_id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            if let Some(snapshot) = self.get(&objective_id).await? {
                reactivated.push(snapshot);
            }
        }
        Ok(reactivated)
    }

    /// Keep a bounded, redacted copy of a technical failure's text beside the
    /// decision that acted on it.
    ///
    /// `failure_signature` is a SHA of the error text, which makes repeats easy
    /// to COUNT and impossible to READ. On 2026-09-08 four Objectives exhausted
    /// their recovery budget on `agent_loop_error` — one signature spanning two
    /// unrelated sessions, so plainly systematic — and every `decision_applied`
    /// row carried a NULL `detail_json`, so nothing on the machine could say
    /// what the error had been.
    ///
    /// This records a diagnosis, never a decision: it leaves revision, status
    /// and the recovery budget untouched, so failing to write it can never
    /// change what the control plane does.
    pub async fn record_failure_detail(
        &self,
        objective_id: &str,
        domain: RecoveryDomain,
        failure_code: &str,
        failure_signature: &str,
        error_text: &str,
    ) -> anyhow::Result<()> {
        let detail = serde_json::json!({
            "failure_signature": failure_signature,
            "error_text": crate::trajectory::redact_text(error_text, FAILURE_DETAIL_MAX_CHARS),
        })
        .to_string();
        sqlx::query(
            "INSERT INTO objective_events
             (id, objective_id, revision, event_type, status, decision_type,
              domain, failure_code, detail_json, created_at)
             SELECT ?, id, revision, 'technical_failure_detail', status,
                    decision_type, ?, ?, ?, ? FROM objectives WHERE id=?",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(domain.as_str())
        .bind(failure_code)
        .bind(detail)
        .bind(Utc::now().timestamp_millis())
        .bind(objective_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// β (2026-09-28): record that a wording-level constraint was turned into an
    /// instruction for this turn — evidence that the constraint was seen and had
    /// NO capability effect. Best-effort on purpose: audit must never block turn
    /// admission, exactly like [`Self::record_failure_detail`].
    pub async fn record_turn_wording_constraint(
        &self,
        objective_id: &str,
        payload: &serde_json::Value,
    ) {
        if let Err(error) = sqlx::query(TURN_WORDING_CONSTRAINT_INSERT)
            .bind(Uuid::new_v4().to_string())
            .bind(turn_wording_constraint_detail(payload))
            .bind(Utc::now().timestamp_millis())
            .bind(objective_id)
            .execute(&self.pool)
            .await
        {
            tracing::warn!(
                "objective {objective_id}: turn wording constraint audit skipped: {error}"
            );
        }
    }

    /// β fixture 7 (2026-09-29): the same audit, written on the caller's
    /// transaction. Chat admission creates the Objective inside that
    /// transaction, so this is the only connection that can see the row — and
    /// SQLite admits a single writer, so the pool variant would first wait out
    /// `busy_timeout` and then drop the event, exactly what production showed
    /// (`turn_wording_constraint` never reached `objective_events`). Still
    /// best-effort: a failed audit only logs and never fails the transaction
    /// that carries the turn.
    pub async fn record_turn_wording_constraint_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        objective_id: &str,
        payload: &serde_json::Value,
    ) {
        if let Err(error) = sqlx::query(TURN_WORDING_CONSTRAINT_INSERT)
            .bind(Uuid::new_v4().to_string())
            .bind(turn_wording_constraint_detail(payload))
            .bind(Utc::now().timestamp_millis())
            .bind(objective_id)
            .execute(&mut **tx)
            .await
        {
            tracing::warn!(
                "objective {objective_id}: turn wording constraint audit skipped: {error}"
            );
        }
    }

    /// Settle `active` Objectives that nothing will ever wake again.
    ///
    /// The supervisor claims DUE REMEDIATIONS. An Objective that is `active`
    /// with no lease, no live remediation and no `next_observation_at` owns no
    /// such row, so every sweep passes over it: it stops, and the UI keeps
    /// saying 进行中. On 2026-09-08 one session sat exactly like this for 83
    /// minutes — not failed, not finished, just lost, and with no record a user
    /// or a later session could act on.
    ///
    /// Liveness is read from `chat_turn_state.updated_at`, which a running turn
    /// republishes on every step. `objectives.last_progress_at` cannot serve:
    /// it advances only when a DECISION is applied, so a healthy ten-minute
    /// build looks identical to a dead turn.
    pub async fn reap_stalled_active_objectives(
        &self,
        stall_after_ms: i64,
    ) -> anyhow::Result<Vec<ObjectiveSnapshot>> {
        let now = Utc::now().timestamp_millis();
        let heartbeat = if table_exists(&self.pool, "chat_turn_state").await? {
            "COALESCE((SELECT MAX(turn.updated_at) FROM chat_turn_state turn
                       WHERE turn.objective_id = objectives.id),
                      objectives.last_progress_at, objectives.updated_at)"
        } else {
            "COALESCE(objectives.last_progress_at, objectives.updated_at)"
        };
        let ids: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT id FROM objectives
             WHERE status='active'
               AND lease_owner IS NULL
               AND next_observation_at IS NULL
               AND {heartbeat} < ?
               AND NOT EXISTS (
                     SELECT 1 FROM objective_remediations remediation
                     WHERE remediation.objective_id = objectives.id
                       AND remediation.status NOT IN
                           ('completed', 'cancelled', 'superseded'))
             ORDER BY created_at"
        ))
        .bind(now - stall_after_ms)
        .fetch_all(&self.pool)
        .await?;

        let mut reaped = Vec::new();
        for id in ids {
            let Some(current) = self.get(&id).await? else {
                continue;
            };
            // Re-check under the current revision: a turn that woke up between
            // the scan and here must keep running.
            if current.status != ObjectiveStatus::Active {
                continue;
            }
            let decision = DecisionRouter::route(
                &current,
                RouteSignal::TechnicalFailure {
                    domain: current.domain,
                    failure_code: OBJECTIVE_PROGRESS_STALLED.into(),
                    failure_signature: format!("{OBJECTIVE_PROGRESS_STALLED}:{}", current.id),
                    next_observation_at: now + 5_000,
                    resume_cursor: current
                        .resume_cursor
                        .clone()
                        .or_else(|| current.root_turn_id.clone()),
                },
            )?;
            reaped.push(
                self.apply_decision(current.revision, decision.as_convergence())
                    .await?,
            );
        }
        Ok(reaped)
    }

    /// Cancel side-effect receipts left unsettled on an Objective that has
    /// already reached a terminal state.
    ///
    /// `uncertain > 0` in the mutation fence counts every `started`/`unknown`
    /// receipt for an Objective, and only the recovery ladder ever settles one.
    /// When the Objective itself is `completed` or `cancelled` that ladder is
    /// gone, so the receipt can never be settled and never stops counting.
    ///
    /// A 2026-09-08 audit of the production database found five such receipts
    /// aged 17 to 20 days, on Objectives cancelled weeks earlier; the only
    /// previous remedy had been manual surgery on the database. Declaring these
    /// dead is not a guess about whether the effect landed — it is a statement
    /// that no code path can ever act on the answer.
    ///
    /// Receipts on a LIVE Objective are deliberately untouched: there, "we do
    /// not know whether this landed" is still a real question, and assuming it
    /// did not is how a side effect gets performed twice.
    pub async fn cancel_receipts_on_terminal_objectives(&self) -> anyhow::Result<u64> {
        let swept = sqlx::query(
            "UPDATE side_effect_receipts
             SET status='cancelled',
                 summary_json=?,
                 observed_at=?
             WHERE status IN ('started', 'unknown')
               AND objective_id IN (
                     SELECT id FROM objectives
                     WHERE status IN ('completed', 'cancelled', 'failed'))",
        )
        .bind(
            serde_json::json!({
                "status": "cancelled",
                "recovery": "objective_terminal_before_settlement",
            })
            .to_string(),
        )
        .bind(Utc::now().timestamp_millis())
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(swept)
    }

    pub async fn create(&self, create: CreateObjective) -> anyhow::Result<ObjectiveSnapshot> {
        let now = Utc::now().timestamp_millis();
        let process_instance = current_process_instance();
        sqlx::query(
            "INSERT INTO objectives
             (id, revision, kind, session_id, root_turn_id, status, decision_type,
              domain, requested_acceptance, requires_user_action, recovery_owner,
              created_surface, created_process_instance,
              last_observed_process_instance, last_progress_at, created_at, updated_at)
             VALUES (?, 1, ?, ?, ?, 'active', 'continue', ?, ?, 0,
                     'objective-supervisor', ?, ?, ?, ?, ?, ?)",
        )
        .bind(&create.id)
        .bind(create.kind.as_str())
        .bind(&create.session_id)
        .bind(&create.root_turn_id)
        .bind(create.domain.as_str())
        .bind(&create.requested_acceptance)
        .bind(&create.created_surface)
        .bind(&process_instance)
        .bind(&process_instance)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .context("create objective")?;
        self.get(&create.id)
            .await?
            .ok_or_else(|| anyhow!("created objective disappeared"))
    }

    /// Idempotently materialize the business objective for a persisted chat
    /// root turn and bind the legacy transport projection to it. The partial
    /// unique index is the cross-process guard; `INSERT OR IGNORE` plus a
    /// read-back makes concurrent command/setup recovery safe.
    pub async fn ensure_chat_objective(
        &self,
        session_id: &str,
        root_turn_id: &str,
        kind: ObjectiveKind,
        requested_acceptance: &str,
    ) -> anyhow::Result<ObjectiveSnapshot> {
        self.ensure_or_continue_chat_objective(
            session_id,
            root_turn_id,
            None,
            kind,
            requested_acceptance,
        )
        .await
    }

    /// Atomically bind a new chat root either to its one authoritative open
    /// Objective or to a newly-created Objective. A contextual continuation
    /// must never manufacture a second identity: missing legacy bindings and
    /// multiple candidates both fail closed for explicit reconciliation.
    pub async fn ensure_or_continue_chat_objective(
        &self,
        session_id: &str,
        root_turn_id: &str,
        continuation_root_turn_id: Option<&str>,
        kind: ObjectiveKind,
        requested_acceptance: &str,
    ) -> anyhow::Result<ObjectiveSnapshot> {
        let mut tx = self.pool.begin().await?;
        let objective = self
            .ensure_or_continue_chat_objective_in_tx(
                &mut tx,
                session_id,
                root_turn_id,
                continuation_root_turn_id,
                kind,
                requested_acceptance,
            )
            .await?;
        tx.commit().await?;
        Ok(objective)
    }

    /// Transaction-scoped form of chat Objective admission. Callers may bind
    /// the transport root, durable run control, and Objective identity in one
    /// commit; this helper never commits or reads through the pool.
    pub(crate) async fn ensure_or_continue_chat_objective_in_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        session_id: &str,
        root_turn_id: &str,
        continuation_root_turn_id: Option<&str>,
        kind: ObjectiveKind,
        requested_acceptance: &str,
    ) -> anyhow::Result<ObjectiveSnapshot> {
        let now = Utc::now().timestamp_millis();
        let process_instance = current_process_instance();
        let mut legacy_root_to_reconcile = None;

        let stop_requested: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM chat_session_cancel_intents
             WHERE session_id=? AND status='requested'",
        )
        .bind(session_id)
        .fetch_one(&mut **tx)
        .await?;
        if stop_requested > 0 {
            bail!("chat admission fenced by durable session cancellation");
        }

        let current_binding = sqlx::query_scalar::<_, Option<String>>(
            "SELECT objective_id FROM chat_turn_state WHERE root_turn_id=? AND session_id=?",
        )
        .bind(root_turn_id)
        .bind(session_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| anyhow!("chat root turn missing while ensuring objective"))?
        .filter(|value| !value.is_empty());

        let mut candidates = Vec::new();
        if let Some(objective_id) = current_binding.as_ref() {
            let exists: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM objectives WHERE id=?")
                .bind(objective_id)
                .fetch_one(&mut **tx)
                .await?;
            if exists != 1 {
                bail!("chat objective binding points to missing objective");
            }
            candidates.push(objective_id.clone());
        }

        if let Some(continuation_root_turn_id) = continuation_root_turn_id {
            let continuation_binding = sqlx::query_scalar::<_, Option<String>>(
                "SELECT objective_id FROM chat_turn_state
                 WHERE root_turn_id=? AND session_id=?",
            )
            .bind(continuation_root_turn_id)
            .bind(session_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| anyhow!("contextual continuation root turn is missing"))?
            .filter(|value| !value.is_empty());
            if let Some(continuation_binding) = continuation_binding {
                let status =
                    sqlx::query_scalar::<_, String>("SELECT status FROM objectives WHERE id=?")
                        .bind(&continuation_binding)
                        .fetch_optional(&mut **tx)
                        .await?
                        .ok_or_else(|| anyhow!("contextual continuation objective is missing"))?;
                if !matches!(status.as_str(), "completed" | "cancelled" | "legacy_orphan") {
                    candidates.push(continuation_binding);
                }
            } else {
                // A pre-0006 root is still an authoritative continuation
                // anchor, but it has no safe synthesized identity. Reconcile
                // that exact root inside this transaction so both transport
                // projections receive one opaque Objective id.
                legacy_root_to_reconcile = Some(continuation_root_turn_id.to_string());
            }
        }

        candidates.sort();
        candidates.dedup();
        if candidates.len() > 1 {
            bail!("multiple open objectives match contextual chat continuation");
        }

        if let Some(objective_id) = candidates.pop() {
            if current_binding.as_deref() == Some(objective_id.as_str())
                && legacy_root_to_reconcile.is_none()
            {
                ensure_objective_binding(
                    &mut *tx,
                    &objective_id,
                    RecoveryDomain::Chat,
                    "chat_root_turn",
                    root_turn_id,
                    now,
                )
                .await?;
                let row = sqlx::query("SELECT * FROM objectives WHERE id=?")
                    .bind(&objective_id)
                    .fetch_one(&mut **tx)
                    .await
                    .context("read bound chat objective")?;
                return ObjectiveSnapshot::from_row(&row);
            }
            let row = sqlx::query("SELECT * FROM objectives WHERE id=?")
                .bind(&objective_id)
                .fetch_one(&mut **tx)
                .await?;
            let objective = ObjectiveSnapshot::from_row(&row)?;
            let target_kind = if objective_kind_rank(kind) > objective_kind_rank(objective.kind) {
                kind
            } else {
                objective.kind
            };
            let target_acceptance = if target_kind == kind {
                requested_acceptance
            } else {
                objective.requested_acceptance.as_str()
            };
            // U18/R1: remember whether the objective was mid-recovery, so the
            // takeover audit below can name what the user's message superseded.
            let prior_status = objective.status;
            let next_revision = objective.revision + 1;
            let next_recovery_generation =
                if objective.failure_code.as_deref() == Some(TECHNICAL_RECOVERY_EXHAUSTED) {
                    objective.recovery_generation + 1
                } else {
                    objective.recovery_generation
                };
            let updated = sqlx::query(
                "UPDATE objectives SET revision=?, kind=?, requested_acceptance=?,
                   status='active', decision_type='continue', domain='chat',
                   requires_user_action=0, request_key=NULL, decision_key=NULL,
                   attention_request_json=NULL,
                   failure_code=NULL, failure_signature=NULL,
                   recovery_owner='chat-foreground', remediation_id=NULL,
                   resume_cursor=?, next_observation_at=NULL,
                   lease_owner=NULL, lease_expires_at=NULL,
                   recovery_generation=?,
                   last_observed_process_instance=?,
                   last_progress_at=?, updated_at=?, completed_at=NULL
                 WHERE id=? AND revision=?
                   AND status NOT IN ('completed','cancelled','legacy_orphan')",
            )
            .bind(next_revision)
            .bind(target_kind.as_str())
            .bind(target_acceptance)
            .bind(root_turn_id)
            .bind(next_recovery_generation)
            .bind(&process_instance)
            .bind(now)
            .bind(now)
            .bind(&objective_id)
            .bind(objective.revision)
            .execute(&mut **tx)
            .await?;
            if updated.rows_affected() != 1 {
                bail!("objective changed while binding contextual continuation");
            }
            sqlx::query(
                "UPDATE objective_remediations SET status='superseded',
                   lease_owner=NULL, lease_expires_at=NULL,
                   last_progress_at=?, updated_at=?
                 WHERE objective_id=?
                   AND status NOT IN ('completed','cancelled','superseded')",
            )
            .bind(now)
            .bind(now)
            .bind(&objective_id)
            .execute(&mut **tx)
            .await?;
            // U18/R1+R2: the user's message takes the Objective over. Settle
            // every turn this Objective leaves behind in the SAME transaction
            // that moves `resume_cursor` onto the new turn — otherwise the
            // superseded root stays `active` forever, the session shows 运行中
            // with no turn to finish, and the composer can only queue
            // "当前执行结束后发送". The new turn is user-driven, so no
            // remediation is created here and the system recovery budget is
            // untouched.
            let superseded_turns = settle_superseded_chat_turns_in_tx(
                tx,
                &objective_id,
                Some(root_turn_id),
                "superseded_by_user_reprompt",
                now,
            )
            .await?;
            if superseded_turns > 0 || prior_status == ObjectiveStatus::WaitingSystem {
                sqlx::query(
                    "INSERT INTO objective_events
                     (id, objective_id, revision, event_type, status, decision_type,
                      domain, recovery_owner, detail_json, created_at)
                     VALUES (?, ?, ?, 'user_steer_superseded_remediation', 'active',
                             'continue', 'chat', 'chat-foreground', ?, ?)",
                )
                .bind(Uuid::new_v4().to_string())
                .bind(&objective_id)
                .bind(next_revision)
                .bind(
                    serde_json::json!({
                        "root_turn_id": root_turn_id,
                        "prior_status": prior_status.as_str(),
                        "settled_turns": superseded_turns,
                        "budget": "user_driven_excluded",
                    })
                    .to_string(),
                )
                .bind(now)
                .execute(&mut **tx)
                .await?;
            }
            if current_binding.is_none() {
                let linked = sqlx::query(
                    "UPDATE chat_turn_state SET objective_id=?
                     WHERE root_turn_id=? AND session_id=? AND objective_id IS NULL",
                )
                .bind(&objective_id)
                .bind(root_turn_id)
                .bind(session_id)
                .execute(&mut **tx)
                .await?;
                if linked.rows_affected() != 1 {
                    bail!("chat root objective binding changed concurrently");
                }
            }
            if let Some(legacy_root_turn_id) = legacy_root_to_reconcile.as_deref() {
                let linked = sqlx::query(
                    "UPDATE chat_turn_state SET objective_id=?
                     WHERE root_turn_id=? AND session_id=? AND objective_id IS NULL",
                )
                .bind(&objective_id)
                .bind(legacy_root_turn_id)
                .bind(session_id)
                .execute(&mut **tx)
                .await?;
                if linked.rows_affected() != 1 {
                    bail!("legacy chat root objective binding changed concurrently");
                }
            }
            ensure_objective_binding(
                &mut *tx,
                &objective_id,
                RecoveryDomain::Chat,
                "chat_root_turn",
                root_turn_id,
                now,
            )
            .await?;
            if let Some(legacy_root_turn_id) = legacy_root_to_reconcile.as_deref() {
                ensure_objective_binding(
                    &mut *tx,
                    &objective_id,
                    RecoveryDomain::Chat,
                    "chat_root_turn",
                    legacy_root_turn_id,
                    now,
                )
                .await?;
            }
            sqlx::query(
                "INSERT INTO objective_events
                 (id, objective_id, revision, event_type, status, decision_type,
                  domain, recovery_owner, detail_json, created_at)
                 VALUES (?, ?, ?, ?, 'active', 'continue',
                         'chat', 'chat-foreground', ?, ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(&objective_id)
            .bind(next_revision)
            .bind(if legacy_root_to_reconcile.is_some() {
                "legacy_root_reconciled"
            } else {
                "contextual_root_bound"
            })
            .bind(
                serde_json::json!({
                    "root_turn_id": root_turn_id,
                    "legacy_root_turn_id": legacy_root_to_reconcile,
                    "recovery_generation": next_recovery_generation,
                })
                .to_string(),
            )
            .bind(now)
            .execute(&mut **tx)
            .await?;
            let row = sqlx::query("SELECT * FROM objectives WHERE id=?")
                .bind(&objective_id)
                .fetch_one(&mut **tx)
                .await
                .context("read continued chat objective")?;
            return ObjectiveSnapshot::from_row(&row);
        }

        let candidate_id = Uuid::new_v4().to_string();
        let objective_root_turn_id = legacy_root_to_reconcile.as_deref().unwrap_or(root_turn_id);
        sqlx::query(
            "INSERT OR IGNORE INTO objectives
             (id, revision, kind, session_id, root_turn_id, status, decision_type,
              domain, requested_acceptance, requires_user_action, recovery_owner,
              created_surface, created_process_instance,
              last_observed_process_instance, last_progress_at, created_at, updated_at)
             VALUES (?, 1, ?, ?, ?, 'active', 'continue', 'chat', ?, 0,
                     'objective-supervisor:chat', 'project_chat', ?, ?, ?, ?, ?)",
        )
        .bind(&candidate_id)
        .bind(kind.as_str())
        .bind(session_id)
        .bind(objective_root_turn_id)
        .bind(requested_acceptance)
        .bind(&process_instance)
        .bind(&process_instance)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&mut **tx)
        .await
        .context("ensure chat objective")?;
        let objective_id: String = sqlx::query_scalar(
            "SELECT id FROM objectives WHERE root_turn_id=?
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(objective_root_turn_id)
        .fetch_one(&mut **tx)
        .await
        .context("read ensured chat objective")?;
        let linked = sqlx::query(
            "UPDATE chat_turn_state SET objective_id=? WHERE root_turn_id=? AND session_id=?",
        )
        .bind(&objective_id)
        .bind(root_turn_id)
        .bind(session_id)
        .execute(&mut **tx)
        .await
        .context("bind chat objective")?;
        if linked.rows_affected() != 1 {
            bail!("chat root turn missing while binding objective");
        }
        if let Some(legacy_root_turn_id) = legacy_root_to_reconcile.as_deref() {
            let legacy_linked = sqlx::query(
                "UPDATE chat_turn_state SET objective_id=?
                 WHERE root_turn_id=? AND session_id=? AND objective_id IS NULL",
            )
            .bind(&objective_id)
            .bind(legacy_root_turn_id)
            .bind(session_id)
            .execute(&mut **tx)
            .await?;
            if legacy_linked.rows_affected() != 1 {
                bail!("legacy chat root turn changed while reconciling objective");
            }
            sqlx::query(
                "INSERT INTO objective_events
                 (id, objective_id, revision, event_type, status, decision_type,
                  domain, recovery_owner, detail_json, created_at)
                 VALUES (?, ?, 1, 'legacy_root_reconciled', 'active', 'continue',
                         'chat', 'objective-supervisor:chat', ?, ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(&objective_id)
            .bind(
                serde_json::json!({
                    "root_turn_id": root_turn_id,
                    "legacy_root_turn_id": legacy_root_turn_id,
                })
                .to_string(),
            )
            .bind(now)
            .execute(&mut **tx)
            .await?;
        }
        ensure_objective_binding(
            &mut *tx,
            &objective_id,
            RecoveryDomain::Chat,
            "chat_root_turn",
            root_turn_id,
            now,
        )
        .await?;
        if let Some(legacy_root_turn_id) = legacy_root_to_reconcile.as_deref() {
            ensure_objective_binding(
                &mut *tx,
                &objective_id,
                RecoveryDomain::Chat,
                "chat_root_turn",
                legacy_root_turn_id,
                now,
            )
            .await?;
        }
        let row = sqlx::query("SELECT * FROM objectives WHERE id=?")
            .bind(&objective_id)
            .fetch_one(&mut **tx)
            .await
            .context("read ensured chat objective")?;
        ObjectiveSnapshot::from_row(&row)
    }

    pub async fn ensure_task_objective(
        &self,
        session_id: &str,
        task_id: &str,
        requested_acceptance: &str,
    ) -> anyhow::Result<ObjectiveSnapshot> {
        let now = Utc::now().timestamp_millis();
        let process_instance = current_process_instance();
        let candidate_id = Uuid::new_v4().to_string();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT OR IGNORE INTO objectives
             (id, revision, kind, session_id, task_id, status, decision_type,
              domain, requested_acceptance, requires_user_action, recovery_owner,
              created_surface, created_process_instance,
              last_observed_process_instance, last_progress_at, created_at, updated_at)
             VALUES (?, 1, 'local_mutation', ?, ?, 'active', 'continue', 'task', ?, 0,
                     'objective-supervisor:task', 'task_scheduler', ?, ?, ?, ?, ?)",
        )
        .bind(&candidate_id)
        .bind(session_id)
        .bind(task_id)
        .bind(requested_acceptance)
        .bind(&process_instance)
        .bind(&process_instance)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&mut *tx)
        .await
        .context("ensure task objective")?;
        let objective_id: String = sqlx::query_scalar(
            "SELECT id FROM objectives WHERE task_id=? ORDER BY created_at DESC LIMIT 1",
        )
        .bind(task_id)
        .fetch_one(&mut *tx)
        .await?;
        let linked = sqlx::query("UPDATE task_runs SET objective_id=? WHERE id=? AND session_id=?")
            .bind(&objective_id)
            .bind(task_id)
            .bind(session_id)
            .execute(&mut *tx)
            .await?;
        if linked.rows_affected() != 1 {
            bail!("task row missing while binding objective");
        }
        ensure_objective_binding(
            &mut tx,
            &objective_id,
            RecoveryDomain::Task,
            "task_run",
            task_id,
            now,
        )
        .await?;
        tx.commit().await?;
        self.get(&objective_id)
            .await?
            .ok_or_else(|| anyhow!("ensured task objective disappeared"))
    }

    pub async fn get(&self, id: &str) -> anyhow::Result<Option<ObjectiveSnapshot>> {
        let row = sqlx::query("SELECT * FROM objectives WHERE id=?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| ObjectiveSnapshot::from_row(&row)).transpose()
    }

    pub async fn get_by_root_turn(
        &self,
        root_turn_id: &str,
    ) -> anyhow::Result<Option<ObjectiveSnapshot>> {
        let row = sqlx::query(
            "SELECT * FROM objectives WHERE root_turn_id=?
             ORDER BY created_at DESC, revision DESC LIMIT 1",
        )
        .bind(root_turn_id)
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| ObjectiveSnapshot::from_row(&row)).transpose()
    }

    /// Persist an explicit user cancellation for one exact chat binding. The
    /// Objective CAS, remediation settlement, and pointed DeliveryRun fence
    /// share the same transaction through `apply_decision`; a stale runner can
    /// still finish an already-in-flight read, but cannot pass another mutation
    /// permit after this returns.
    pub async fn cancel_chat_exact(
        &self,
        session_id: &str,
        root_turn_id: &str,
        objective_id: &str,
    ) -> anyhow::Result<ObjectiveSnapshot> {
        let binding_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM chat_turn_state
             WHERE session_id=? AND root_turn_id=? AND objective_id=?",
        )
        .bind(session_id)
        .bind(root_turn_id)
        .bind(objective_id)
        .fetch_one(&self.pool)
        .await?;
        if binding_count != 1 {
            bail!("chat cancellation identity mismatch");
        }
        self.cancel_session_owned_exact(session_id, objective_id)
            .await
    }

    async fn cancel_session_owned_exact(
        &self,
        session_id: &str,
        objective_id: &str,
    ) -> anyhow::Result<ObjectiveSnapshot> {
        for _ in 0..4 {
            let current = self
                .get(objective_id)
                .await?
                .ok_or_else(|| anyhow!("chat cancellation objective missing"))?;
            if current.session_id.as_deref() != Some(session_id) {
                bail!("chat cancellation identity mismatch");
            }
            if matches!(
                current.status,
                ObjectiveStatus::Cancelled | ObjectiveStatus::Completed
            ) {
                return Ok(current);
            }
            let decision = DecisionRouter::route(
                &current,
                RouteSignal::Cancelled {
                    domain: RecoveryDomain::Chat,
                    provenance: "explicit_cancel".into(),
                },
            )?;
            match self.apply_decision(current.revision, decision).await {
                Ok(cancelled) => return Ok(cancelled),
                Err(error) if error.to_string().contains("revision") => continue,
                Err(error) => return Err(error),
            }
        }
        bail!("chat cancellation could not win a stable Objective revision")
    }

    /// Persist a session-wide stop fence before touching any individual
    /// Objective. Re-requesting a settled stop starts a new intent; no later
    /// chat admission may pass while this row remains `requested`.
    pub async fn request_chat_session_cancel(&self, session_id: &str) -> anyhow::Result<()> {
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO chat_session_cancel_intents
             (session_id, status, requested_at, settled_at, updated_at)
             VALUES (?, 'requested', ?, NULL, ?)
             ON CONFLICT(session_id) DO UPDATE SET
               status='requested', requested_at=excluded.requested_at,
               settled_at=NULL, updated_at=excluded.updated_at",
        )
        .bind(session_id)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Consume one durable session stop. The intent remains requested on any
    /// error, keeping recovery fenced. Success rechecks the complete live set
    /// in the transaction that settles the fence.
    pub async fn consume_chat_session_cancel(
        &self,
        session_id: &str,
    ) -> anyhow::Result<Vec<(String, Option<String>, ObjectiveSnapshot)>> {
        let mut cancelled = Vec::new();
        loop {
            let live = sqlx::query_as::<_, (String, Option<String>)>(
                "SELECT objective.id, objective.root_turn_id
                 FROM objectives objective
                 WHERE objective.session_id=?
                   AND objective.root_turn_id IS NOT NULL
                   AND objective.status NOT IN ('completed','cancelled','failed','legacy_orphan')
                 ORDER BY objective.updated_at DESC",
            )
            .bind(session_id)
            .fetch_all(&self.pool)
            .await?;
            if live.is_empty() {
                break;
            }
            for (objective_id, root_turn_id) in live {
                let snapshot = self
                    .cancel_session_owned_exact(session_id, &objective_id)
                    .await?;
                if !matches!(
                    snapshot.status,
                    ObjectiveStatus::Cancelled | ObjectiveStatus::Completed
                ) {
                    bail!("session cancellation left a nonterminal Objective");
                }
                cancelled.push((objective_id, root_turn_id, snapshot));
            }
        }

        let now = Utc::now().timestamp_millis();
        let mut tx = self.pool.begin().await?;
        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objectives objective
             WHERE objective.session_id=?
               AND objective.root_turn_id IS NOT NULL
               AND objective.status NOT IN ('completed','cancelled','failed','legacy_orphan')",
        )
        .bind(session_id)
        .fetch_one(&mut *tx)
        .await?;
        if remaining != 0 {
            bail!("session cancellation raced a new live Objective");
        }
        let settled = sqlx::query(
            "UPDATE chat_session_cancel_intents
             SET status='settled', settled_at=?, updated_at=?
             WHERE session_id=? AND status='requested'",
        )
        .bind(now)
        .bind(now)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
        if settled.rows_affected() != 1 {
            let status = sqlx::query_scalar::<_, String>(
                "SELECT status FROM chat_session_cancel_intents WHERE session_id=?",
            )
            .bind(session_id)
            .fetch_optional(&mut *tx)
            .await?;
            if status.as_deref() != Some("settled") {
                bail!("session cancellation intent disappeared before settlement");
            }
        }
        sqlx::query(
            "UPDATE chat_run_controls
             SET status='cancelled',
                 cancel_requested_at=COALESCE(cancel_requested_at, ?),
                 settled_at=COALESCE(settled_at, ?), updated_at=?
             WHERE session_id=? AND status IN ('active','cancel_requested')",
        )
        .bind(now)
        .bind(now)
        .bind(now)
        .bind(session_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(cancelled)
    }

    /// Finish crash-left session stops before any recovery admission.
    pub async fn consume_pending_chat_session_cancellations(&self) -> anyhow::Result<usize> {
        let sessions = sqlx::query_scalar::<_, String>(
            "SELECT session_id FROM chat_session_cancel_intents
             WHERE status='requested' ORDER BY requested_at, session_id",
        )
        .fetch_all(&self.pool)
        .await?;
        for session_id in &sessions {
            self.consume_chat_session_cancel(session_id).await?;
        }
        Ok(sessions.len())
    }

    /// Consume exact crash-left chat stop intents before any startup recovery
    /// supervisor may claim the same Objective. Rows without a complete
    /// root/opaque binding remain pending for explicit identity reconciliation;
    /// they are never guessed from session recency.
    pub async fn consume_pending_chat_cancellations(&self) -> anyhow::Result<usize> {
        let rows = sqlx::query_as::<_, (String, String, String, String, i64, String)>(
            "SELECT control.run_instance_id, control.session_id,
                    control.root_turn_id,
                    COALESCE(control.objective_id, turn.objective_id),
                    COALESCE(control.objective_revision, objective.revision),
                    objective.status
             FROM chat_run_controls control
             JOIN chat_turn_state turn
               ON turn.root_turn_id=control.root_turn_id
              AND turn.session_id=control.session_id
              AND turn.objective_id IS NOT NULL
              AND (control.objective_id IS NULL
                   OR turn.objective_id=control.objective_id)
             JOIN objectives objective
               ON objective.id=COALESCE(control.objective_id, turn.objective_id)
             WHERE control.status='cancel_requested'
               AND control.root_turn_id IS NOT NULL
             ORDER BY control.cancel_requested_at, control.run_instance_id",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut consumed = 0;
        for (
            run_instance_id,
            session_id,
            root_turn_id,
            objective_id,
            objective_revision,
            objective_status,
        ) in rows
        {
            let now = Utc::now().timestamp_millis();
            let bound = sqlx::query(
                "UPDATE chat_run_controls
                 SET objective_id=?, objective_revision=?, updated_at=?
                 WHERE run_instance_id=? AND session_id=? AND root_turn_id=?
                   AND status='cancel_requested'
                   AND (objective_id IS NULL OR objective_id=?)
                   AND EXISTS (
                     SELECT 1 FROM chat_turn_state turn
                     WHERE turn.root_turn_id=? AND turn.session_id=?
                       AND turn.objective_id=?
                   )",
            )
            .bind(&objective_id)
            .bind(objective_revision)
            .bind(now)
            .bind(&run_instance_id)
            .bind(&session_id)
            .bind(&root_turn_id)
            .bind(&objective_id)
            .bind(&root_turn_id)
            .bind(&session_id)
            .bind(&objective_id)
            .execute(&self.pool)
            .await?;
            if bound.rows_affected() != 1 {
                bail!("startup chat cancellation identity changed before settlement");
            }
            let control_status = if objective_status == ObjectiveStatus::Completed.as_str() {
                "completed"
            } else {
                let cancelled = self
                    .cancel_chat_exact(&session_id, &root_turn_id, &objective_id)
                    .await?;
                if cancelled.status != ObjectiveStatus::Cancelled {
                    bail!("startup chat cancellation did not reach terminal state");
                }
                "cancelled"
            };
            let updated = sqlx::query(
                "UPDATE chat_run_controls
                 SET status=?, settled_at=?, updated_at=?
                 WHERE run_instance_id=? AND session_id=? AND root_turn_id=?
                   AND objective_id=? AND status='cancel_requested'",
            )
            .bind(control_status)
            .bind(now)
            .bind(now)
            .bind(&run_instance_id)
            .bind(&session_id)
            .bind(&root_turn_id)
            .bind(&objective_id)
            .execute(&self.pool)
            .await?;
            if updated.rows_affected() != 1 {
                bail!("startup chat cancellation changed during settlement");
            }
            consumed += 1;
        }
        Ok(consumed)
    }

    /// A process-local chat future disappeared with the prior process. Retire
    /// its transport ownership before stale Objective reconciliation so the
    /// same session can be admitted exactly once after restart. The Objective
    /// remains nonterminal and is transferred to system-owned recovery by the
    /// following startup step; no user input or side effect is synthesized.
    pub async fn reconcile_stale_chat_run_controls(
        &self,
        current_process_instance: &str,
    ) -> anyhow::Result<usize> {
        let now = Utc::now().timestamp_millis();
        let updated = sqlx::query(
            "UPDATE chat_run_controls
             SET status='completed', settled_at=?, updated_at=?
             WHERE status='active'
               AND created_process_instance<>?",
        )
        .bind(now)
        .bind(now)
        .bind(current_process_instance)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() as usize)
    }

    /// Repair the v1.81.9 admission regression without asking the user to send
    /// the same message again. That build persisted and bound a real
    /// `core_input_response`, then immediately re-applied the exhausted budget
    /// from generation zero. The exact latest root remains in `resume_cursor`.
    ///
    /// Generation zero is part of the compatibility fence: after this repair,
    /// normal admission increments every later exhausted reprompt before it can
    /// run, so startup can never buy another budget merely by restarting.
    pub async fn reconcile_unconsumed_exhausted_chat_reprompts(&self) -> anyhow::Result<usize> {
        let candidates = sqlx::query_as::<_, (String, i64, String, String)>(
            "SELECT objective.id, objective.revision, objective.session_id,
                    objective.resume_cursor
             FROM objectives objective
             JOIN chat_turn_state turn
               ON turn.objective_id=objective.id
              AND turn.session_id=objective.session_id
              AND turn.root_turn_id=objective.resume_cursor
             JOIN messages message
               ON message.id=turn.root_turn_id
              AND message.session_id=turn.session_id
              AND message.role='user'
             WHERE objective.status='waiting_core_input'
               AND objective.failure_code=?
               AND objective.recovery_generation=0
               AND turn.status='waiting_core_input'
               AND turn.user_reprompt_driver='core_input_response'
             ORDER BY objective.updated_at, objective.id",
        )
        .bind(TECHNICAL_RECOVERY_EXHAUSTED)
        .fetch_all(&self.pool)
        .await?;
        let mut reconciled = 0;
        for (objective_id, revision, session_id, root_turn_id) in candidates {
            let now = Utc::now().timestamp_millis();
            let remediation_id = Uuid::new_v4().to_string();
            let failure_code = "exhausted_user_reprompt_unconsumed";
            let failure_signature = format!("{objective_id}:{root_turn_id}:{failure_code}");
            let mut tx = self.pool.begin().await?;
            let updated = sqlx::query(
                "UPDATE objectives SET revision=revision+1,
                   recovery_generation=1, status='waiting_system',
                   decision_type='waiting', domain='chat',
                   requires_user_action=0, request_key=NULL, decision_key=NULL,
                   attention_request_json=NULL,
                   failure_code=?, failure_signature=?,
                   recovery_owner='objective-supervisor:chat', remediation_id=?,
                   next_observation_at=?, lease_owner=NULL, lease_expires_at=NULL,
                   completed_at=NULL, last_progress_at=?, updated_at=?
                 WHERE id=? AND revision=? AND session_id=? AND resume_cursor=?
                   AND status='waiting_core_input' AND failure_code=?
                   AND recovery_generation=0",
            )
            .bind(failure_code)
            .bind(&failure_signature)
            .bind(&remediation_id)
            .bind(now)
            .bind(now)
            .bind(now)
            .bind(&objective_id)
            .bind(revision)
            .bind(&session_id)
            .bind(&root_turn_id)
            .bind(TECHNICAL_RECOVERY_EXHAUSTED)
            .execute(&mut *tx)
            .await?;
            if updated.rows_affected() != 1 {
                tx.rollback().await?;
                continue;
            }
            sqlx::query(
                "INSERT INTO objective_remediations
                 (id, objective_id, domain, status, failure_code,
                  failure_signature, strategy, approach_index, attempt_index,
                  recovery_generation, resume_cursor, next_observation_at,
                  created_at, updated_at)
                 VALUES (?, ?, 'chat', 'queued', ?, ?,
                         'reconcile_then_resume', 0, 0, 1, ?, ?, ?, ?)",
            )
            .bind(&remediation_id)
            .bind(&objective_id)
            .bind(failure_code)
            .bind(&failure_signature)
            .bind(&root_turn_id)
            .bind(now)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            let projected = sqlx::query(
                "UPDATE chat_turn_state SET revision=revision+1,
                   phase='recovering', status='waiting_system',
                   recent_activity_kind='system_recovery',
                   recent_activity_label='正在续接你已发送的消息',
                   waiting_reason='exhausted_user_reprompt_unconsumed',
                   updated_at=?, completed_at=NULL, terminal_reason=NULL
                 WHERE root_turn_id=? AND session_id=? AND objective_id=?
                   AND status='waiting_core_input'
                   AND user_reprompt_driver='core_input_response'",
            )
            .bind(now)
            .bind(&root_turn_id)
            .bind(&session_id)
            .bind(&objective_id)
            .execute(&mut *tx)
            .await?;
            if projected.rows_affected() != 1 {
                tx.rollback().await?;
                continue;
            }
            sqlx::query(
                "INSERT INTO objective_events
                 (id, objective_id, revision, event_type, status, decision_type,
                  domain, failure_code, recovery_owner, detail_json, created_at)
                 VALUES (?, ?, ?, 'exhausted_user_reprompt_recovered',
                         'waiting_system', 'waiting', 'chat', ?,
                         'objective-supervisor:chat', ?, ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(&objective_id)
            .bind(revision + 1)
            .bind(failure_code)
            .bind(
                serde_json::json!({
                    "root_turn_id": root_turn_id,
                    "recovery_generation": 1,
                    "compatibility_repair": "v1.81.9",
                })
                .to_string(),
            )
            .bind(now)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            reconciled += 1;
        }
        Ok(reconciled)
    }

    /// R4 start-up convergence of the retired "recovery exhausted" limbo.
    ///
    /// Releases before this one parked a technical ceiling as a *non-terminal*
    /// system-owned wait and opened an incident that only a capability-version
    /// change could clear — which, by construction, never happened. Real users
    /// were left with a sentence they could not act on and a status that never
    /// moved.
    ///
    /// Existing rows are converted to the honest failure terminal: the
    /// objective finishes, its incident closes, its turn stops presenting
    /// itself as running, and the notice is written once. The guarded `WHERE`
    /// clauses make a second start a no-op, so no objective is told twice.
    pub async fn reclassify_synthetic_technical_handbacks(&self) -> anyhow::Result<usize> {
        // `updated_at` is the row's suspension time: the moment the objective
        // gave up. The backfilled notice carries that timestamp instead of the
        // startup time, so a task that stalled three days ago does not resurface
        // as today's news.
        let candidates =
            sqlx::query_as::<_, (String, i64, Option<String>, Option<String>, i64)>(
                "SELECT id, revision, session_id,
                        COALESCE(resume_cursor, root_turn_id), updated_at
             FROM objectives
             WHERE failure_code=?
               AND (
                 (status='waiting_core_input' AND decision_type='core_input_required')
                 OR (status='waiting_system' AND decision_type='failed_internal')
               )",
            )
            .bind(TECHNICAL_RECOVERY_EXHAUSTED)
            .fetch_all(&self.pool)
            .await?;
        let mut reclassified = 0;
        for (objective_id, revision, session_id, terminal_root_turn_id, suspended_at) in candidates
        {
            let now = Utc::now().timestamp_millis();
            let next_revision = revision + 1;
            let visible_final_message_id =
                format!("system-incident-{}-{}", objective_id, next_revision);
            let mut tx = self.pool.begin().await?;
            let updated = sqlx::query(
                "UPDATE objectives SET revision=?, status='failed',
                   decision_type='failed_internal', requires_user_action=0,
                   request_key=NULL, decision_key=NULL, attention_request_json=NULL,
                   failure_code=?, recovery_owner=?, remediation_id=NULL,
                   next_observation_at=NULL, lease_owner=NULL, lease_expires_at=NULL,
                   completed_at=?, updated_at=?
                 WHERE id=? AND revision=?
                   AND status IN ('waiting_core_input','waiting_system')
                   AND failure_code=?",
            )
            .bind(next_revision)
            .bind(TECHNICAL_RECOVERY_EXHAUSTED)
            .bind(OBJECTIVE_INCIDENT_CONTROLLER)
            .bind(now)
            .bind(now)
            .bind(&objective_id)
            .bind(revision)
            .bind(TECHNICAL_RECOVERY_EXHAUSTED)
            .execute(&mut *tx)
            .await?;
            if updated.rows_affected() != 1 {
                tx.rollback().await?;
                continue;
            }
            if let Some(session_id) = session_id.as_deref() {
                let has_messages: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type='table' AND name='messages'",
                )
                .fetch_one(&mut *tx)
                .await?;
                if has_messages == 1 {
                    sqlx::query(
                        "INSERT OR IGNORE INTO messages
                         (id, session_id, role, content, completion_state, created_at)
                         VALUES (?, ?, 'assistant', ?, NULL, ?)",
                    )
                    .bind(&visible_final_message_id)
                    .bind(session_id)
                    .bind("这件事没做成：你交代的目标没有达成。试过的办法和保留下来的改动都还在会话里；直接回一句「继续」或「把这些改动交付」就可以接着做。")
                    .bind(suspended_at)
                    .execute(&mut *tx)
                    .await?;
                }
                // Deliberately no `touch_session_in_settlement`: a startup
                // backfill is not user activity. Advancing `sessions.updated_at`
                // here would drag a long-dead conversation to the top of the
                // sidebar, which is exactly the "为什么这个又跑到最上面" bug.
            }
            let has_turn_state: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name='chat_turn_state'",
            )
            .fetch_one(&mut *tx)
            .await?;
            if has_turn_state == 1 {
                sqlx::query(
                    "UPDATE chat_turn_state SET revision=revision+1,
                   phase='finalizing', status='completed',
                   recent_activity_kind='objective_failed',
                   recent_activity_label='这件事没做成，已把试过的办法和保留下来的改动写给你',
                   waiting_reason=NULL, updated_at=?,
                   completed_at=COALESCE(completed_at, ?),
                   terminal_reason='objective_failed',
                   objective_revision=?,
                   turn_settled_at=COALESCE(turn_settled_at, ?),
                   stream_closed_at=COALESCE(stream_closed_at, ?),
                   terminal_revision=?, visible_final_message_id=?,
                   visible_final_kind='assistant_final',
                   next_action=NULL
                 WHERE objective_id=? AND root_turn_id=?",
                )
                .bind(now)
                .bind(now)
                .bind(next_revision)
                .bind(now)
                .bind(now)
                .bind(next_revision)
                .bind(&visible_final_message_id)
                .bind(&objective_id)
                .bind(&terminal_root_turn_id)
                .execute(&mut *tx)
                .await?;
            }
            let has_incidents: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name='objective_incidents'",
            )
            .fetch_one(&mut *tx)
            .await?;
            if has_incidents == 1 {
                sqlx::query(
                    "UPDATE objective_incidents
                     SET status='resolved', resolved_at=?, updated_at=?,
                         reactivation_status='resolved'
                     WHERE objective_id=? AND status='open'",
                )
                .bind(now)
                .bind(now)
                .bind(&objective_id)
                .execute(&mut *tx)
                .await?;
            }
            let has_delivery_runs: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name='delivery_runs'",
            )
            .fetch_one(&mut *tx)
            .await?;
            if has_delivery_runs == 1 {
                sqlx::query(
                    "UPDATE delivery_runs
                     SET status='failed', wait_class=NULL, next_action=NULL,
                         next_action_authorized=0,
                         lease_owner=NULL, lease_expires_at=NULL,
                         failure_code=COALESCE(failure_code, 'objective_failed'),
                         failure_class=COALESCE(failure_class, 'objective_failed'),
                         last_observed_at=?, updated_at=?
                     WHERE objective_id=?
                       AND status NOT IN ('completed','failed','cancelled','rejected')",
                )
                .bind(now)
                .bind(now)
                .bind(&objective_id)
                .execute(&mut *tx)
                .await?;
            }
            tx.commit().await?;
            reclassified += 1;
        }
        // Incidents left open by an objective the user already moved past —
        // completed, cancelled, or now failed — close here too, so nothing in
        // the ledger keeps claiming it is waiting for something.
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "UPDATE objective_incidents
             SET status='resolved', resolved_at=?, updated_at=?,
                 reactivation_status='resolved'
             WHERE status='open'
               AND EXISTS (
                 SELECT 1 FROM objectives objective
                 WHERE objective.id=objective_incidents.objective_id
                   AND objective.status IN ('completed','cancelled','failed')
               )",
        )
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(reclassified)
    }

    /// Lease due recovery work with compare-and-swap semantics. A stale
    /// process can observe candidates but cannot claim a row after another
    /// supervisor has advanced its lease or status.
    pub async fn claim_due_remediation_batch(
        &self,
        owner: &str,
        limit: i64,
        lease_ms: i64,
    ) -> anyhow::Result<ClaimDueBatch> {
        let now = Utc::now().timestamp_millis();
        let rows = sqlx::query(
            "SELECT remediation.id AS remediation_id,
                    remediation.objective_id, remediation.domain,
                    remediation.failure_code, remediation.failure_signature,
                    remediation.strategy
             FROM objective_remediations remediation
             JOIN objectives objective ON objective.id=remediation.objective_id
             WHERE objective.status='waiting_system'
               AND objective.remediation_id=remediation.id
               AND remediation.status IN ('queued','waiting','claimed')
               AND remediation.next_observation_at<=?
               AND (remediation.lease_expires_at IS NULL OR remediation.lease_expires_at<=?)
               AND NOT EXISTS (
                 SELECT 1 FROM chat_session_cancel_intents stop
                 WHERE stop.session_id=objective.session_id AND stop.status='requested'
               )
             ORDER BY remediation.next_observation_at, remediation.created_at
             LIMIT ?",
        )
        .bind(now)
        .bind(now)
        .bind(limit.max(1))
        .fetch_all(&self.pool)
        .await?;
        let mut claimed = Vec::new();
        let mut terminal_transitions = Vec::new();
        for row in rows {
            let remediation_id: String = row.try_get("remediation_id")?;
            let objective_id: String = row.try_get("objective_id")?;
            let domain = RecoveryDomain::parse(row.try_get::<String, _>("domain")?.as_str())?;
            let failure_code: String = row.try_get("failure_code")?;
            let failure_signature: String = row.try_get("failure_signature")?;
            let strategy: String = row.try_get("strategy")?;
            let lease_expires_at = now + lease_ms.max(1);
            let mut tx = self.pool.begin().await?;
            if strategy == "reconcile_then_resume" {
                let (signature_attempts, objective_attempts): (i64, i64) = sqlx::query_as(
                    "SELECT
                       COALESCE(SUM(CASE WHEN remediation.failure_signature=?
                                         THEN remediation.execution_attempt_index ELSE 0 END), 0),
                       COALESCE(SUM(remediation.execution_attempt_index), 0)
                     FROM objective_remediations remediation
                     LEFT JOIN objective_decisions decision
                       ON decision.remediation_id=remediation.id
                      AND decision.objective_id=remediation.objective_id
                     WHERE remediation.objective_id=?
                       AND remediation.strategy='reconcile_then_resume'
                       AND remediation.recovery_generation=(
                         SELECT recovery_generation FROM objectives WHERE id=?
                       )
                       AND remediation.created_at>=?
                       AND COALESCE(decision.decision_type, 'waiting')<>'apply_recommended'",
                )
                .bind(&failure_signature)
                .bind(&objective_id)
                .bind(&objective_id)
                .bind(now - SIGNATURE_RECOVERY_WINDOW_MS)
                .fetch_one(&mut *tx)
                .await?;
                if signature_attempts >= max_signature_attempts_for(Some(&failure_code))
                    || objective_attempts >= MAX_OBJECTIVE_RECOVERY_ATTEMPTS
                {
                    tx.rollback().await?;
                    let current = match self.get(&objective_id).await? {
                        Some(current) if current.status == ObjectiveStatus::WaitingSystem => {
                            current
                        }
                        _ => continue,
                    };
                    let decision = DecisionRouter::route(
                        &current,
                        RouteSignal::TechnicalFailure {
                            domain,
                            failure_code: failure_code.clone(),
                            failure_signature: failure_signature.clone(),
                            next_observation_at: now,
                            resume_cursor: current.resume_cursor.clone(),
                        },
                    )?;
                    match self
                        .apply_decision(current.revision, decision.as_convergence())
                        .await
                    {
                        Ok(transition) => terminal_transitions.push(transition),
                        Err(error) if error.to_string().contains("revision") => {
                            // Another process may have committed the same
                            // terminal transition after this poll selected the
                            // row. Re-read the durable winner so this process
                            // can still publish the idempotent UI projection.
                            if let Some(transition) = self.get(&objective_id).await? {
                                if transition.failure_code.as_deref()
                                    == Some(TECHNICAL_RECOVERY_EXHAUSTED)
                                    && transition.recovery_owner.as_deref()
                                        == Some(OBJECTIVE_INCIDENT_CONTROLLER)
                                {
                                    terminal_transitions.push(transition);
                                }
                            }
                        }
                        Err(error) => return Err(error),
                    }
                    continue;
                }
            }
            let updated = sqlx::query(
                "UPDATE objective_remediations
                 SET status='claimed', attempt_index=attempt_index+1,
                     lease_owner=?, lease_expires_at=?, updated_at=?
                 WHERE id=? AND objective_id=?
                   AND status IN ('queued','waiting','claimed')
                   AND next_observation_at<=?
                   AND (lease_expires_at IS NULL OR lease_expires_at<=?)",
            )
            .bind(owner)
            .bind(lease_expires_at)
            .bind(now)
            .bind(&remediation_id)
            .bind(&objective_id)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            if updated.rows_affected() != 1 {
                tx.rollback().await?;
                continue;
            }
            let objective_updated = sqlx::query(
                "UPDATE objectives SET lease_owner=?, lease_expires_at=?, updated_at=?
                 WHERE id=? AND status='waiting_system' AND remediation_id=?
                   AND (lease_expires_at IS NULL OR lease_expires_at<=?)",
            )
            .bind(owner)
            .bind(lease_expires_at)
            .bind(now)
            .bind(&objective_id)
            .bind(&remediation_id)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            if objective_updated.rows_affected() != 1 {
                tx.rollback().await?;
                continue;
            }
            let (claim_epoch, binding_id, failure_code): (i64, Option<String>, String) =
                sqlx::query_as(
                    "SELECT attempt_index, binding_id, failure_code FROM objective_remediations
                 WHERE id=? AND objective_id=? AND status='claimed'
                   AND lease_owner=?",
                )
                .bind(&remediation_id)
                .bind(&objective_id)
                .bind(owner)
                .fetch_one(&mut *tx)
                .await?;
            let resource_generation = if let Some(binding_id) = binding_id.as_deref() {
                Some(
                    sqlx::query_scalar::<_, i64>(
                        "SELECT resource_generation FROM objective_bindings
                         WHERE id=? AND objective_id=?",
                    )
                    .bind(binding_id)
                    .bind(&objective_id)
                    .fetch_one(&mut *tx)
                    .await?,
                )
            } else {
                None
            };
            tx.commit().await?;
            if let Some(objective) = self.get(&objective_id).await? {
                claimed.push(ClaimedRemediation {
                    objective,
                    remediation_id,
                    domain,
                    failure_code,
                    claim_epoch,
                    binding_id,
                    resource_generation,
                });
            }
        }
        Ok(ClaimDueBatch {
            claims: claimed,
            terminal_transitions,
        })
    }

    pub async fn claim_due_remediations(
        &self,
        owner: &str,
        limit: i64,
        lease_ms: i64,
    ) -> anyhow::Result<Vec<ClaimedRemediation>> {
        Ok(self
            .claim_due_remediation_batch(owner, limit, lease_ms)
            .await?
            .claims)
    }

    pub async fn defer_claimed_remediation(
        &self,
        objective_id: &str,
        remediation_id: &str,
        owner: &str,
        claim_epoch: i64,
        delay_ms: i64,
    ) -> anyhow::Result<()> {
        let now = Utc::now().timestamp_millis();
        let next = now + delay_ms.max(1_000);
        let mut tx = self.pool.begin().await?;
        let remediation = sqlx::query(
            "UPDATE objective_remediations
             SET status='waiting',
                 next_observation_at=?, lease_owner=NULL, lease_expires_at=NULL,
                 updated_at=?
             WHERE id=? AND objective_id=? AND status='claimed' AND lease_owner=?
               AND attempt_index=? AND lease_expires_at>?",
        )
        .bind(next)
        .bind(now)
        .bind(remediation_id)
        .bind(objective_id)
        .bind(owner)
        .bind(claim_epoch)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if remediation.rows_affected() != 1 {
            bail!("remediation claim ownership changed before defer");
        }
        let objective = sqlx::query(
            "UPDATE objectives SET next_observation_at=?, lease_owner=NULL,
               lease_expires_at=NULL, updated_at=?
             WHERE id=? AND status='waiting_system' AND remediation_id=? AND lease_owner=?
               AND lease_expires_at>?",
        )
        .bind(next)
        .bind(now)
        .bind(objective_id)
        .bind(remediation_id)
        .bind(owner)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if objective.rows_affected() != 1 {
            bail!("objective claim ownership changed before defer");
        }
        tx.commit().await?;
        Ok(())
    }

    /// Charge the bounded recovery budget only after an executable adapter has
    /// passed observation/reconciliation and still owns the exact claim. Lease
    /// acquisition, takeover and observe-only polling are coordination events,
    /// not failed recovery attempts.
    pub async fn charge_claimed_remediation_attempt(
        &self,
        objective_id: &str,
        remediation_id: &str,
        owner: &str,
        claim_epoch: i64,
    ) -> anyhow::Result<bool> {
        let now = Utc::now().timestamp_millis();
        let updated = sqlx::query(
            "UPDATE objective_remediations
             SET execution_attempt_index=execution_attempt_index+1,
                 last_progress_at=?, updated_at=?
             WHERE id=? AND objective_id=? AND status='claimed'
               AND lease_owner=? AND attempt_index=? AND lease_expires_at>?",
        )
        .bind(now)
        .bind(now)
        .bind(remediation_id)
        .bind(objective_id)
        .bind(owner)
        .bind(claim_epoch)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    /// Extend both halves of a claimed remediation lease atomically. Returning
    /// `false` means ownership changed (or the remediation was superseded), so
    /// the caller must stop rather than execute another side effect.
    pub async fn renew_claimed_remediation(
        &self,
        objective_id: &str,
        remediation_id: &str,
        owner: &str,
        claim_epoch: i64,
        lease_ms: i64,
    ) -> anyhow::Result<bool> {
        let now = Utc::now().timestamp_millis();
        let lease_expires_at = now + lease_ms.max(1);
        let mut tx = self.pool.begin().await?;
        let remediation = sqlx::query(
            "UPDATE objective_remediations
             SET lease_expires_at=?, updated_at=?
             WHERE id=? AND objective_id=? AND status='claimed' AND lease_owner=?
               AND attempt_index=? AND lease_expires_at>?",
        )
        .bind(lease_expires_at)
        .bind(now)
        .bind(remediation_id)
        .bind(objective_id)
        .bind(owner)
        .bind(claim_epoch)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if remediation.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(false);
        }
        let objective = sqlx::query(
            "UPDATE objectives
             SET lease_expires_at=?, updated_at=?
             WHERE id=? AND status='waiting_system' AND remediation_id=? AND lease_owner=?
               AND lease_expires_at>?",
        )
        .bind(lease_expires_at)
        .bind(now)
        .bind(objective_id)
        .bind(remediation_id)
        .bind(owner)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if objective.rows_affected() != 1 {
            tx.rollback().await?;
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Observe whether an adapter still owns the exact durable claim it was
    /// launched for. This is intentionally stronger than comparing only the
    /// owner string: a same-process reclaim increments `attempt_index`, and a
    /// resource rebind invalidates the generation carried by the old permit.
    pub async fn claim_is_current(
        &self,
        permit: &codefactory_agent_loop::tool::MutationPermit,
    ) -> anyhow::Result<bool> {
        if permit.binding_id.is_some() != permit.resource_generation.is_some() {
            bail!("mutation permit binding and resource generation must be paired");
        }
        let now = Utc::now().timestamp_millis();
        let claimed: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)
             FROM objective_remediations remediation
             JOIN objectives objective
               ON objective.id=remediation.objective_id
              AND objective.remediation_id=remediation.id
             WHERE remediation.id=? AND remediation.objective_id=?
               AND remediation.status='claimed' AND remediation.lease_owner=?
               AND remediation.attempt_index=? AND remediation.lease_expires_at>?
               AND COALESCE(remediation.binding_id, '')=COALESCE(?, '')
               AND objective.status='waiting_system' AND objective.lease_owner=?
               AND objective.lease_expires_at>?",
        )
        .bind(&permit.remediation_id)
        .bind(&permit.objective_id)
        .bind(&permit.owner)
        .bind(permit.claim_epoch)
        .bind(now)
        .bind(&permit.binding_id)
        .bind(&permit.owner)
        .bind(now)
        .fetch_one(&self.pool)
        .await?;
        if claimed != 1 {
            return Ok(false);
        }
        if let Some(binding_id) = permit.binding_id.as_deref() {
            let generation = sqlx::query_scalar::<_, i64>(
                "SELECT resource_generation FROM objective_bindings
                 WHERE id=? AND objective_id=?",
            )
            .bind(binding_id)
            .bind(&permit.objective_id)
            .fetch_optional(&self.pool)
            .await?;
            if generation != permit.resource_generation {
                bail!("mutation permit binding generation changed while claim was live");
            }
        }
        Ok(true)
    }

    /// On process start, convert objectives that were active in an older
    /// process into durable system-owned recovery. Transport interruption is
    /// not completion and never becomes a user handoff.
    pub async fn reconcile_stale_active_objectives(
        &self,
        current_process_instance: &str,
    ) -> anyhow::Result<usize> {
        let rows = sqlx::query(
            "SELECT * FROM objectives
             WHERE status='active'
               AND COALESCE(last_observed_process_instance,
                            created_process_instance, '') <> ?
             ORDER BY created_at",
        )
        .bind(current_process_instance)
        .fetch_all(&self.pool)
        .await?;
        let mut reconciled = 0;
        for row in rows {
            let prior_process = row
                .try_get::<Option<String>, _>("last_observed_process_instance")?
                .or(row.try_get::<Option<String>, _>("created_process_instance")?)
                .unwrap_or_else(|| "legacy-process".into());
            let objective = ObjectiveSnapshot::from_row(&row)?;
            let decision = DecisionRouter::route(
                &objective,
                RouteSignal::TechnicalFailure {
                    domain: objective.domain,
                    failure_code: "process_restarted".into(),
                    failure_signature: format!(
                        "{}:{}:{}",
                        objective.id, prior_process, current_process_instance
                    ),
                    next_observation_at: Utc::now().timestamp_millis(),
                    resume_cursor: objective
                        .root_turn_id
                        .clone()
                        .or(objective.task_id.clone())
                        .or(objective.delivery_run_id.clone()),
                },
            )?;
            match self
                .apply_decision(objective.revision, decision.as_convergence())
                .await
            {
                Ok(_) => reconciled += 1,
                Err(error) if error.to_string().contains("revision") => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(reconciled)
    }

    /// U18/R4: startup turn convergence, idempotent and independent of the
    /// Objective-status pass above.
    ///
    /// Two shapes deadlock a conversation and both are repaired here:
    /// 1. a live Objective with more than one non-terminal chat turn — a user
    ///    reprompt or steer added a turn but the superseded root was never
    ///    settled; the Objective's `COALESCE(resume_cursor, root_turn_id)` is
    ///    the one turn that keeps running, everything else closes;
    /// 2. an Objective that is already terminal (including `legacy_orphan`) but
    ///    still owns `active`/`waiting_system` turns — pure fake running.
    ///
    /// It writes only `chat_turn_state`, never `sessions.updated_at` (U3's S1
    /// rule), and only touches non-terminal rows, so a second start closes
    /// nothing.
    pub async fn reconcile_stale_chat_turns(&self) -> anyhow::Result<usize> {
        let has_turn_state: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='chat_turn_state'",
        )
        .fetch_one(&self.pool)
        .await?;
        if has_turn_state == 0 {
            return Ok(0);
        }
        let mut settled = 0usize;

        // Shape 2: terminal Objectives own no live turn.
        let terminal_objectives = sqlx::query_scalar::<_, String>(
            "SELECT objective.id FROM objectives objective
             WHERE objective.status IN ('completed','cancelled','failed','legacy_orphan')
               AND EXISTS (
                 SELECT 1 FROM chat_turn_state turn
                 WHERE turn.objective_id=objective.id
                   AND turn.status NOT IN ('completed','cancelled'))",
        )
        .fetch_all(&self.pool)
        .await?;
        for objective_id in terminal_objectives {
            let now = Utc::now().timestamp_millis();
            let mut tx = self.pool.begin().await?;
            let closed = settle_superseded_chat_turns_in_tx(
                &mut tx,
                &objective_id,
                None,
                "objective_already_terminal",
                now,
            )
            .await?;
            tx.commit().await?;
            settled += closed as usize;
        }

        // Shape 1: a live Objective with ghost turns keeps only its current one.
        let live_objectives = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT objective.id,
                    NULLIF(COALESCE(NULLIF(objective.resume_cursor, ''),
                                    objective.root_turn_id), '')
             FROM objectives objective
             WHERE objective.status NOT IN ('completed','cancelled','failed','legacy_orphan')
               AND EXISTS (
                 SELECT 1 FROM chat_turn_state turn
                 WHERE turn.objective_id=objective.id
                   AND turn.status NOT IN ('completed','cancelled')
                   AND turn.root_turn_id<>NULLIF(COALESCE(NULLIF(objective.resume_cursor, ''),
                                                          objective.root_turn_id), ''))",
        )
        .fetch_all(&self.pool)
        .await?;
        for (objective_id, active_root) in live_objectives {
            let now = Utc::now().timestamp_millis();
            let mut tx = self.pool.begin().await?;
            let closed = settle_superseded_chat_turns_in_tx(
                &mut tx,
                &objective_id,
                active_root.as_deref(),
                "superseded_at_startup",
                now,
            )
            .await?;
            tx.commit().await?;
            settled += closed as usize;
        }

        Ok(settled)
    }

    /// Convert satisfied authorization requests into immediate system-owned
    /// recovery. The original objective/root turn is preserved; no user
    /// message is replayed or synthesized.
    pub async fn resume_waiting_authorizations(
        &self,
        domain: RecoveryDomain,
        request_key_prefix: &str,
    ) -> anyhow::Result<usize> {
        let rows = sqlx::query(
            "SELECT * FROM objectives
             WHERE status='waiting_authorization' AND domain=?
               AND request_key LIKE ?
             ORDER BY created_at",
        )
        .bind(domain.as_str())
        .bind(format!("{request_key_prefix}%"))
        .fetch_all(&self.pool)
        .await?;
        let mut resumed = 0;
        for row in rows {
            let objective = ObjectiveSnapshot::from_row(&row)?;
            let decision = DecisionRouter::route(
                &objective,
                RouteSignal::CapabilityRestored {
                    domain,
                    reason: "authorization_restored".into(),
                    next_observation_at: Utc::now().timestamp_millis(),
                    resume_cursor: objective.resume_cursor.clone(),
                },
            )?;
            match self
                .apply_decision(objective.revision, decision.as_convergence())
                .await
            {
                Ok(_) => resumed += 1,
                Err(error) if error.to_string().contains("revision") => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(resumed)
    }

    pub async fn resume_waiting_core_inputs(
        &self,
        domain: RecoveryDomain,
        request_key_prefix: &str,
        reason: &str,
    ) -> anyhow::Result<usize> {
        let rows = sqlx::query(
            "SELECT * FROM objectives
             WHERE status='waiting_core_input' AND domain=?
               AND request_key LIKE ? ORDER BY created_at",
        )
        .bind(domain.as_str())
        .bind(format!("{request_key_prefix}%"))
        .fetch_all(&self.pool)
        .await?;
        let mut resumed = 0;
        for row in rows {
            let objective = ObjectiveSnapshot::from_row(&row)?;
            let decision = DecisionRouter::route(
                &objective,
                RouteSignal::CapabilityRestored {
                    domain,
                    reason: reason.into(),
                    next_observation_at: Utc::now().timestamp_millis(),
                    resume_cursor: objective.resume_cursor.clone(),
                },
            )?;
            match self
                .apply_decision(objective.revision, decision.as_convergence())
                .await
            {
                Ok(_) => resumed += 1,
                Err(error) if error.to_string().contains("revision") => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(resumed)
    }

    pub async fn apply_decision(
        &self,
        expected_revision: i64,
        decision: DecisionEnvelope,
    ) -> anyhow::Result<ObjectiveSnapshot> {
        self.apply_decision_inner(expected_revision, decision, None, None, None)
            .await
    }

    /// Settle a recovery attempt only while its exact owner+epoch lease is
    /// still live. A same-process reclaim deliberately keeps the owner string,
    /// so omitting the epoch here would let the stale future supersede the new
    /// remediation after takeover.
    pub async fn apply_claimed_decision(
        &self,
        expected_revision: i64,
        decision: DecisionEnvelope,
        permit: &codefactory_agent_loop::tool::MutationPermit,
    ) -> anyhow::Result<ObjectiveSnapshot> {
        self.apply_decision_inner(expected_revision, decision, Some(permit), None, None)
            .await
    }

    /// Park a repeated local Delivery identity conflict only while the exact
    /// DeliveryRun owner+claim epoch is still live. The Objective, linked run,
    /// chat turn and run controls converge in the same SQLite transaction.
    pub async fn park_delivery_identity_incident_after_takeover(
        &self,
        objective_id: &str,
        run_id: &str,
        process: &crate::agent::delivery_run::ProcessIdentity,
        claim_epoch: i64,
        failure_signature: &str,
        attempt_index: i64,
    ) -> anyhow::Result<ObjectiveSnapshot> {
        let current = self
            .get(objective_id)
            .await?
            .ok_or_else(|| anyhow!("objective not found"))?;
        if current.status == ObjectiveStatus::Failed
            && current.failure_code.as_deref() == Some(TECHNICAL_RECOVERY_EXHAUSTED)
            && current.recovery_owner.as_deref() == Some(OBJECTIVE_INCIDENT_CONTROLLER)
            && current.remediation_id.is_none()
            && current.next_observation_at.is_none()
            && !current.requires_user_action
        {
            return Ok(current);
        }
        let mut decision = DecisionRouter::route(
            &current,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Delivery,
                failure_code: "delivery_identity_conflict".into(),
                failure_signature: failure_signature.into(),
                next_observation_at: Utc::now().timestamp_millis(),
                resume_cursor: current
                    .resume_cursor
                    .clone()
                    .or_else(|| current.root_turn_id.clone())
                    .or_else(|| current.task_id.clone()),
            },
        )?;
        decision.decision_type = DecisionType::FailedInternal;
        decision.status = ObjectiveStatus::Failed;
        decision.failure_code = Some(TECHNICAL_RECOVERY_EXHAUSTED.into());
        decision.failure_signature = Some(failure_signature.into());
        decision.recovery_owner = Some(OBJECTIVE_INCIDENT_CONTROLLER.into());
        decision.remediation_id = None;
        decision.next_observation_at = None;
        decision.next_action_authorized = false;
        decision.requires_user_action = false;
        decision.request_key = None;
        decision.attention_request = None;
        self.apply_decision_inner(
            current.revision,
            decision,
            None,
            None,
            Some(DeliveryIdentityParkPermit {
                run_id: run_id.into(),
                claim_epoch,
                lease_owner: process.instance_id.clone(),
                failure_signature: failure_signature.into(),
                attempt_index,
            }),
        )
        .await
    }

    /// Combine newly-observed evidence with the typed durable evidence already
    /// accepted for this Objective. This is what lets a Live Objective resume
    /// after DeliveryRun settlement and later complete from a real live
    /// observation without replaying delivery or fabricating that observation
    /// from `reached_ceiling`.
    pub async fn completion_decision_with_persisted_evidence(
        &self,
        objective: &ObjectiveSnapshot,
        mut newly_observed: Vec<ObjectiveEvidence>,
    ) -> anyhow::Result<DecisionEnvelope> {
        let current = self
            .get(&objective.id)
            .await?
            .ok_or_else(|| anyhow!("objective not found"))?;
        if current.revision != objective.revision || current.status != objective.status {
            bail!("objective changed before persisted completion evidence was combined");
        }
        let rows = sqlx::query(
            "SELECT id, kind, scope, digest, evidence_ref, observed_at
             FROM objective_evidence WHERE objective_id=? ORDER BY observed_at, created_at, id",
        )
        .bind(&objective.id)
        .fetch_all(&self.pool)
        .await?;
        let mut evidence = Vec::with_capacity(rows.len() + newly_observed.len());
        for row in rows {
            evidence.push(ObjectiveEvidence {
                id: row.try_get("id")?,
                kind: EvidenceKind::parse(row.try_get::<String, _>("kind")?.as_str())?,
                scope: row.try_get("scope")?,
                digest: row.try_get("digest")?,
                evidence_ref: row.try_get("evidence_ref")?,
                observed_at: row.try_get("observed_at")?,
                reached_acceptance: objective.requested_acceptance.clone(),
            });
        }
        evidence.append(&mut newly_observed);
        CompletionArbiter::decide(objective, &evidence)
    }

    /// Settle a DeliveryRun whose external effects were already positively
    /// reconciled by takeover. This function performs no provider or git I/O;
    /// it only consumes the exact pointed run/epoch ledger and advances the
    /// provider-replay rows, generic receipt, DeliveryRun and Objective in one
    /// SQLite transaction.
    pub async fn settle_reconciled_delivery_after_takeover(
        &self,
        run_id: &str,
        claim_epoch: i64,
        lease_owner: &str,
    ) -> anyhow::Result<ObjectiveSnapshot> {
        if claim_epoch <= 0 {
            bail!("delivery takeover claim epoch must be positive");
        }
        if lease_owner.trim().is_empty() {
            bail!("delivery takeover lease owner must be explicit");
        }
        let objective_id = sqlx::query_scalar::<_, String>(
            "SELECT objective.id
             FROM objectives objective
             JOIN delivery_runs run
               ON run.id=objective.delivery_run_id AND run.objective_id=objective.id
             WHERE run.id=?",
        )
        .bind(run_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| anyhow!("delivery takeover has no exact pointed Objective"))?;
        let current = self
            .get(&objective_id)
            .await?
            .ok_or_else(|| anyhow!("pointed delivery Objective disappeared"))?;

        if current.status.is_terminal()
            || (current.kind == ObjectiveKind::Live
                && current.status == ObjectiveStatus::WaitingSystem)
        {
            let settled: i64 = sqlx::query_scalar(
                "SELECT COUNT(*)
                 FROM delivery_runs run
                 JOIN objectives objective
                   ON objective.id=run.objective_id AND objective.delivery_run_id=run.id
                 WHERE run.id=? AND run.claim_epoch=?
                   AND run.reconciled_claim_epoch=run.claim_epoch
                   AND run.status='completed'
                   AND EXISTS (
                     SELECT 1 FROM side_effect_receipts receipt
                     WHERE receipt.objective_id=objective.id
                       AND receipt.status='reconciled'
                       AND json_extract(receipt.summary_json, '$.delivery_run_id')=run.id
                   )
                   AND EXISTS (
                     SELECT 1 FROM objective_evidence evidence
                     WHERE evidence.objective_id=objective.id
                       AND evidence.kind='delivery_receipt'
                   )",
            )
            .bind(run_id)
            .bind(claim_epoch)
            .fetch_one(&self.pool)
            .await?;
            if settled == 1 {
                return Ok(current);
            }
            if current.status.is_terminal() {
                bail!("terminal Objective has no exact reconciled DeliveryRun receipt");
            }
        }

        if !matches!(current.kind, ObjectiveKind::Delivery | ObjectiveKind::Live) {
            bail!("pointed DeliveryRun cannot settle a non-delivery Objective kind");
        }
        if !current.status.is_system_owned() {
            bail!("DeliveryRun settlement requires a system-owned Objective");
        }
        let run = sqlx::query(
            "SELECT run.status, run.stage, run.claim_epoch, run.reconciled_claim_epoch,
                    run.next_action_authorized, run.autonomous_completion,
                    run.requested_ceiling, run.reached_ceiling,
                    run.repo_identity, run.worktree_identity,
                    run.expected_head_sha, run.change_set_digest,
                    run.canonical_pr_number, run.canonical_pr_url,
                    run.canonical_head_sha, run.root_turn_id, run.task_id,
                    run.lease_owner, run.lease_expires_at
             FROM delivery_runs run
             JOIN objectives objective
               ON objective.id=run.objective_id AND objective.delivery_run_id=run.id
             WHERE run.id=? AND run.objective_id=?",
        )
        .bind(run_id)
        .bind(&current.id)
        .fetch_one(&self.pool)
        .await?;
        let run_status: String = run.try_get("status")?;
        if run_status != "awaiting_completion_arbitration" {
            bail!("delivery takeover is not awaiting completion arbitration");
        }
        let stage: String = run.try_get("stage")?;
        if stage != "complete" {
            bail!("delivery takeover has not reached its complete stage");
        }
        let persisted_claim_epoch: i64 = run.try_get("claim_epoch")?;
        let reconciled_claim_epoch: i64 = run.try_get("reconciled_claim_epoch")?;
        if persisted_claim_epoch != claim_epoch || reconciled_claim_epoch != claim_epoch {
            bail!("delivery takeover claim epoch is stale or unreconciled");
        }
        let next_action_authorized: i64 = run.try_get("next_action_authorized")?;
        let autonomous_completion: i64 = run.try_get("autonomous_completion")?;
        if next_action_authorized != 1 || autonomous_completion != 1 {
            bail!("delivery takeover lacks autonomous completion authority");
        }
        let persisted_lease_owner: Option<String> = run.try_get("lease_owner")?;
        let lease_expires_at: Option<i64> = run.try_get("lease_expires_at")?;
        if persisted_lease_owner.as_deref() != Some(lease_owner)
            || lease_expires_at.is_none_or(|expires_at| expires_at <= Utc::now().timestamp_millis())
        {
            bail!("delivery takeover lease expired or changed before settlement");
        }
        let requested_ceiling: String = run.try_get("requested_ceiling")?;
        let reached_ceiling: String = run.try_get("reached_ceiling")?;
        let requested_rank = requested_delivery_ceiling_rank(&requested_ceiling)
            .ok_or_else(|| anyhow!("delivery takeover has an invalid requested ceiling"))?;
        let reached_rank = delivery_ceiling_rank(&reached_ceiling)
            .ok_or_else(|| anyhow!("delivery takeover has an invalid reached ceiling"))?;
        if reached_rank < requested_rank {
            bail!("delivery takeover has not reached its requested ceiling");
        }
        let repo_identity: String = run.try_get("repo_identity")?;
        let worktree_identity: String = run.try_get("worktree_identity")?;
        let expected_head_sha: String = run.try_get("expected_head_sha")?;
        let change_set_digest: String = run.try_get("change_set_digest")?;
        if [
            repo_identity.as_str(),
            worktree_identity.as_str(),
            expected_head_sha.as_str(),
            change_set_digest.as_str(),
        ]
        .iter()
        .any(|value| value.trim().is_empty())
        {
            bail!("delivery takeover has incomplete durable identity");
        }
        let canonical_pr_number: Option<i64> = run.try_get("canonical_pr_number")?;
        let canonical_pr_url: Option<String> = run.try_get("canonical_pr_url")?;
        let canonical_head_sha: Option<String> = run.try_get("canonical_head_sha")?;
        if canonical_pr_number.is_none()
            || canonical_pr_url
                .as_deref()
                .is_none_or(|value| value.trim().is_empty())
            || canonical_head_sha.as_deref() != Some(expected_head_sha.as_str())
        {
            bail!("delivery takeover canonical PR/head identity is incomplete or inconsistent");
        }
        let unresolved_intents: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_mutation_intents
             WHERE run_id=? AND status IN ('started','unknown')",
        )
        .bind(run_id)
        .fetch_one(&self.pool)
        .await?;
        if unresolved_intents != 0 {
            bail!("delivery takeover has an unresolved mutation intent");
        }
        let (resource_kind, resource_id) = match (
            run.try_get::<Option<String>, _>("root_turn_id")?,
            run.try_get::<Option<String>, _>("task_id")?,
        ) {
            (Some(root_turn_id), None) => ("chat_root_turn", root_turn_id),
            (None, Some(task_id)) => ("task_run", task_id),
            _ => bail!("delivery takeover cannot prove one authoritative resource"),
        };
        let binding = sqlx::query(
            "SELECT id, resource_generation FROM objective_bindings
             WHERE objective_id=? AND resource_kind=? AND resource_id=?
             ORDER BY resource_generation DESC, updated_at DESC, id DESC
             LIMIT 1",
        )
        .bind(&current.id)
        .bind(resource_kind)
        .bind(&resource_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| anyhow!("delivery takeover has no current binding"))?;
        let binding_id: String = binding.try_get("id")?;
        let resource_generation: i64 = binding.try_get("resource_generation")?;
        let action_signatures: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT action_signature FROM tool_calls
             WHERE objective_id=? AND binding_id=? AND resource_generation=?
               AND tool_name='deliver_changes'
               AND NULLIF(TRIM(action_signature), '') IS NOT NULL
             ORDER BY action_signature",
        )
        .bind(&current.id)
        .bind(&binding_id)
        .bind(resource_generation)
        .fetch_all(&self.pool)
        .await?;
        if action_signatures.len() != 1 {
            bail!("delivery takeover requires one stable action signature on its current binding");
        }
        let action_signature = action_signatures[0].clone();
        let tool_call_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM tool_calls
             WHERE objective_id=? AND binding_id=? AND resource_generation=?
               AND tool_name='deliver_changes' AND action_signature=?",
        )
        .bind(&current.id)
        .bind(&binding_id)
        .bind(resource_generation)
        .bind(&action_signature)
        .fetch_one(&self.pool)
        .await?;
        if tool_call_count == 0 {
            bail!("delivery takeover has no exact normalized action to settle");
        }
        let now = Utc::now().timestamp_millis();
        let scope = current
            .root_turn_id
            .clone()
            .or(current.task_id.clone())
            .unwrap_or_else(|| current.id.clone());
        let delivery_evidence = ObjectiveEvidence {
            id: format!(
                "sha256:{:x}",
                Sha256::digest(
                    format!(
                        "delivery_takeover_evidence\0{}\0{}\0{}",
                        current.id, run_id, claim_epoch
                    )
                    .as_bytes()
                )
            ),
            kind: EvidenceKind::DeliveryReceipt,
            scope,
            digest: format!(
                "sha256:{:x}",
                Sha256::digest(format!("{run_id}\0{claim_epoch}").as_bytes())
            ),
            evidence_ref: format!("delivery-run:{run_id}:epoch:{claim_epoch}"),
            observed_at: now,
            reached_acceptance: current.requested_acceptance.clone(),
        };
        let mut decision = if current.kind == ObjectiveKind::Delivery {
            CompletionArbiter::decide(&current, &[delivery_evidence.clone()])?
        } else {
            let continuation_domain = if current.task_id.is_some() {
                RecoveryDomain::Task
            } else if current.root_turn_id.is_some() {
                RecoveryDomain::Chat
            } else {
                RecoveryDomain::Delivery
            };
            DecisionRouter::route(
                &current,
                RouteSignal::TechnicalFailure {
                    domain: continuation_domain,
                    failure_code: "live_verification_pending_after_delivery".into(),
                    failure_signature: format!(
                        "sha256:{:x}",
                        Sha256::digest(
                            format!("{}\0{}\0{}", current.id, run_id, claim_epoch).as_bytes()
                        )
                    ),
                    next_observation_at: now,
                    resume_cursor: current.root_turn_id.clone().or(current.task_id.clone()),
                },
            )?
        };
        // Waiting decisions may carry accepted non-terminal evidence. It is
        // persisted now, while CompletionArbiter still requires a separate
        // LiveVerification before it can ever emit `complete`.
        decision.evidence = Some(delivery_evidence);
        self.apply_decision_inner(
            current.revision,
            decision,
            None,
            Some(DeliveryTakeoverSettlement {
                run_id: run_id.to_string(),
                claim_epoch,
                lease_owner: lease_owner.to_string(),
                binding_id,
                resource_generation,
                action_signature,
            }),
            None,
        )
        .await
    }

    /// Bound system-owned recovery so it can never spin forever.
    ///
    /// Recovery may insert a new remediation or defer and reclaim the same row
    /// after an adapter failure. `execution_attempt_index` records real executions, so its
    /// sum across the current durable recovery generation bounds both shapes
    /// of retry while older generations stay queryable as audit history.
    ///
    /// Two bounds, both keyed on the current generation's durable history
    /// rather than a live streak:
    ///
    /// * per `(objective, recovery_generation, failure_signature)` — the same
    ///   signature recurring is the definition of no progress. It is a
    ///   generation-lifetime tally, not a consecutive run, so an intervening
    ///   *different* failure code cannot hand the same broken route a fresh
    ///   budget.
    /// * per `(objective, recovery_generation)` — a backstop for routes whose
    ///   signature embeds varying text, where every per-signature tally would
    ///   otherwise stay at 1.
    ///
    /// User-driven remediations are excluded from both counts — a restored
    /// capability or an authorized permission is the user changing the world,
    /// not the system retrying itself, and must never spend the recovery
    /// budget. They are recognised two ways because they are written two ways:
    /// a `CapabilityRestored` route lands an `apply_recommended` decision, and
    /// permission authorization inserts its remediation directly with the
    /// `resume_authorized_action` strategy and no decision row at all.
    /// U25: what a transport outage does on the next observation. The decision
    /// is either kept waiting (no model turn, no attempt charged) or settled as
    /// the honest failure once the window is over.
    async fn bound_transport_outage(
        &self,
        mut decision: DecisionEnvelope,
    ) -> anyhow::Result<DecisionEnvelope> {
        let now = Utc::now().timestamp_millis();
        let (first_unreachable_at, attempts, error_text) = self
            .transport_outage_series(&decision.objective_id, PROVIDER_TRANSPORT_UNREACHABLE)
            .await?;
        let probe_reachable = match error_text
            .as_deref()
            .and_then(probe_target_from_error_text)
        {
            Some((host, port)) => probe_transport_reachability(&host, port).await,
            // Nothing to probe means nothing to learn from waiting here; keep
            // the ordinary ladder rather than inventing a wait with no signal.
            None => true,
        };
        match transport_outage_action(
            now,
            first_unreachable_at.unwrap_or(now),
            attempts,
            probe_reachable,
        ) {
            TransportOutageAction::Retry => Ok(decision),
            TransportOutageAction::Wait { delay_ms } => {
                // Keep the owner and the remediation identity the router minted:
                // a system wait is only valid with all three, and the identity is
                // what lets the next observation be recognised as the *same*
                // outage rather than a fresh failure. No remediation row is
                // inserted for this decision (see `transport_probe_wait`), so
                // nothing is claimable and no model turn runs while the path is
                // still down.
                decision.decision_type = DecisionType::Waiting;
                decision.status = ObjectiveStatus::WaitingSystem;
                decision.request_key = None;
                decision.next_observation_at = Some(now + delay_ms);
                decision.next_action_authorized = false;
                decision.requires_user_action = false;
                decision.transport_probe_wait = true;
                Ok(decision)
            }
            TransportOutageAction::GiveUp {
                unreachable_ms,
                attempts,
            } => {
                let text = transport_outage_terminal_message(unreachable_ms, attempts);
                self.record_transport_outage_terminal(
                    &decision.objective_id,
                    unreachable_ms,
                    attempts,
                    &text,
                )
                .await;
                // U3's failed terminal, reached honestly: the objective stays
                // `failed` with a code that explains itself. No new intermediate
                // state is invented for the user to decode.
                decision.decision_type = DecisionType::FailedInternal;
                decision.status = ObjectiveStatus::Failed;
                decision.failure_code = Some(PROVIDER_TRANSPORT_UNREACHABLE.into());
                decision.request_key = None;
                decision.remediation_id = None;
                decision.next_observation_at = None;
                decision.next_action_authorized = false;
                decision.requires_user_action = false;
                decision.recovery_owner = Some(OBJECTIVE_INCIDENT_CONTROLLER.into());
                Ok(decision)
            }
        }
    }

    /// The one durable series a transport outage forms: (first seen, attempts,
    /// the error text that names the route). Grouped by failure *code*, not by
    /// error text — the text carries a fresh URL and timing per run, and
    /// counting per signature would turn one outage into N independent failures.
    async fn transport_outage_series(
        &self,
        objective_id: &str,
        failure_code: &str,
    ) -> anyhow::Result<(Option<i64>, i64, Option<String>)> {
        let rows: Vec<(i64, Option<String>)> = sqlx::query_as(
            "SELECT created_at, detail_json FROM objective_events
             WHERE objective_id=? AND event_type='technical_failure_detail'
               AND failure_code=?
             ORDER BY created_at ASC, rowid ASC",
        )
        .bind(objective_id)
        .bind(failure_code)
        .fetch_all(&self.pool)
        .await?;
        let first = rows.first().map(|(created_at, _)| *created_at);
        let attempts = rows.len() as i64;
        let error_text = rows.first().and_then(|(_, detail)| {
            detail
                .as_deref()
                .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
                .and_then(|value| {
                    value
                        .get("error_text")
                        .and_then(|text| text.as_str())
                        .map(str::to_string)
                })
        });
        Ok((first, attempts.max(1), error_text))
    }

    /// Durable, plain-language record of how an outage ended. Best-effort like
    /// every other diagnostic here: it must never be the thing that loses the
    /// decision it describes.
    async fn record_transport_outage_terminal(
        &self,
        objective_id: &str,
        unreachable_ms: i64,
        attempts: i64,
        message: &str,
    ) {
        if crate::agent::failure_summary::assert_no_internal_vocabulary(message).is_err() {
            tracing::error!(
                objective_id = %objective_id,
                "transport outage terminal message failed the user-safe vocabulary guard"
            );
        }
        let detail = serde_json::json!({
            "unreachable_ms": unreachable_ms,
            "attempts": attempts,
            "message": message,
        })
        .to_string();
        let _ = sqlx::query(
            "INSERT INTO objective_events
             (id, objective_id, revision, event_type, status, decision_type,
              domain, failure_code, detail_json, created_at)
             SELECT ?, id, revision, 'transport_outage_terminal', status,
                    decision_type, 'chat', ?, ?, ? FROM objectives WHERE id=?
               AND NOT EXISTS (
                 SELECT 1 FROM objective_events
                 WHERE objective_id=? AND event_type='transport_outage_terminal'
               )",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(PROVIDER_TRANSPORT_UNREACHABLE)
        .bind(detail)
        .bind(Utc::now().timestamp_millis())
        .bind(objective_id)
        .bind(objective_id)
        .execute(&self.pool)
        .await;
    }

    /// How long a repeating failure signature may keep being re-observed before
    /// the objective admits it is not making progress and settles. Deterministic
    /// verdicts (an unreconcilable turn identity) settle on the first
    /// occurrence; a transport outage is decided earlier still, by the window in
    /// [`Self::bound_transport_outage`].
    async fn bound_system_recovery(
        &self,
        mut decision: DecisionEnvelope,
    ) -> anyhow::Result<DecisionEnvelope> {
        if decision.domain == RecoveryDomain::Update
            && decision.failure_code.as_deref() == Some(UPDATE_SAFE_POINT_PENDING)
        {
            return Ok(decision);
        }
        if decision.failure_code.as_deref() == Some(PROVIDER_TRANSPORT_UNREACHABLE) {
            // U25: a network or service outage is not a verdict about the task.
            // Wait it out with a cheap reachability probe — and give up honestly
            // once the window is over — before the ordinary ladder gets a chance
            // to spend the recovery budget on a path that is simply down.
            return self.bound_transport_outage(decision).await;
        }
        if decision.failure_code.as_deref() == Some(CHAT_IDENTITY_UNRECONCILABLE) {
            // U18/R3: the remediation's turn identity cannot be reconciled with
            // the Objective's live turn, so replaying the same call returns the
            // same answer. Waiting for a budget to run out would only make the
            // user watch a dead session; settle the honest failure terminal on
            // the first occurrence and keep the exact reason for forensics.
            decision.decision_type = DecisionType::FailedInternal;
            decision.status = ObjectiveStatus::Failed;
            decision.request_key = None;
            decision.recovery_owner = Some(OBJECTIVE_INCIDENT_CONTROLLER.into());
            decision.remediation_id = None;
            decision.next_observation_at = None;
            decision.next_action_authorized = false;
            decision.requires_user_action = false;
            return Ok(decision);
        }
        if decision.status != ObjectiveStatus::WaitingSystem
            || !matches!(
                decision.decision_type,
                DecisionType::Waiting
                    | DecisionType::PlatformIncident
                    | DecisionType::FailedInternal
            )
        {
            return Ok(decision);
        }
        let signature = decision.failure_signature.clone().unwrap_or_default();
        let (signature_attempts, objective_attempts): (i64, i64) = sqlx::query_as(
            "SELECT
               COALESCE(SUM(CASE WHEN remediation.failure_signature=?
                                 THEN remediation.execution_attempt_index ELSE 0 END), 0),
               COALESCE(SUM(remediation.execution_attempt_index), 0)
             FROM objective_remediations remediation
             LEFT JOIN objective_decisions decision
               ON decision.remediation_id=remediation.id
              AND decision.objective_id=remediation.objective_id
             WHERE remediation.objective_id=?
               AND remediation.strategy='reconcile_then_resume'
               AND remediation.recovery_generation=(
                 SELECT recovery_generation FROM objectives WHERE id=?
               )
               AND remediation.created_at>=?
               AND COALESCE(decision.decision_type, 'waiting')<>'apply_recommended'",
        )
        .bind(&signature)
        .bind(&decision.objective_id)
        .bind(&decision.objective_id)
        .bind(Utc::now().timestamp_millis() - SIGNATURE_RECOVERY_WINDOW_MS)
        .fetch_one(&self.pool)
        .await?;
        if signature_attempts < max_signature_attempts_for(decision.failure_code.as_deref())
            && objective_attempts < MAX_OBJECTIVE_RECOVERY_ATTEMPTS
        {
            return Ok(decision);
        }

        tracing::warn!(
            objective_id = %decision.objective_id,
            failure_code = decision.failure_code.as_deref().unwrap_or("unknown"),
            domain = decision.domain.as_str(),
            signature_attempts,
            objective_attempts,
            "system recovery made no progress; parking a system-owned incident"
        );
        decision.decision_type = DecisionType::FailedInternal;
        decision.status = ObjectiveStatus::Failed;
        decision.request_key = None;
        // Keep the exhausted signature for forensics; the failure code becomes
        // the typed reason so the transport turn and the objective agree.
        decision.failure_code = Some(TECHNICAL_RECOVERY_EXHAUSTED.into());
        decision.recovery_owner = Some(OBJECTIVE_INCIDENT_CONTROLLER.into());
        decision.remediation_id = None;
        decision.next_observation_at = None;
        decision.next_action_authorized = false;
        decision.requires_user_action = false;
        Ok(decision)
    }

    /// M30 — "once a task has been declared over, nothing may keep running
    /// behind it, and the closing statement must be the last word".
    ///
    /// Background settlement (the incident controller, convergence, the
    /// delivery supervisor) used to open its transaction, write the terminal
    /// Objective status, the `objective_failed` turn projection and the closing
    /// statement, and only *then* — by omission — leave the run alone. The
    /// settled turn went out while `chat_run_controls` was still `active`, so
    /// nobody told the running `AgentLoop` to stop: an in-flight command ran to
    /// completion and its result landed *below* the closing statement, and a
    /// command with an external side effect kept writing after the user had
    /// been told the task was over.
    ///
    /// Order matters, and this is the only place that can enforce it, because
    /// it runs before the settlement transaction opens:
    ///
    /// 1. request the durable stop (`cancel_requested_at`) for every `active`
    ///    run of this Objective,
    /// 2. raise the process-local cooperative flag the loop and its in-flight
    ///    tool actually observe (a durable row alone stops nothing),
    /// 3. give the run a bounded window to reach its next boundary, so its
    ///    in-flight tool is stopped and its last message is written while the
    ///    turn is still open,
    /// 4. and only then let the caller write the terminal state and the closing
    ///    statement.
    ///
    /// Best-effort by construction: it is a stop *request*, so it never fails a
    /// settlement, and the bounded wait can never deadlock recovery. The
    /// no-late-output guarantee does not rest on this timing — `messages` writes
    /// are gated by the durable settlement watermark — so a run that is somehow
    /// still alive past the window is still stopped, just later.
    async fn request_live_run_stop_before_settlement(&self, objective_id: &str, terminal: bool) {
        if !terminal {
            return;
        }
        let has_controls: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type='table' AND name='chat_run_controls'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        if has_controls != 1 {
            return;
        }
        let has_cancel_column: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('chat_run_controls')
             WHERE name='cancel_requested_at'",
        )
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);
        if has_cancel_column != 1 {
            return;
        }
        let run_instance_ids: Vec<String> = sqlx::query_scalar(
            "SELECT run_instance_id FROM chat_run_controls
             WHERE objective_id=? AND status='active'",
        )
        .bind(objective_id)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        if run_instance_ids.is_empty() {
            return;
        }
        let now = Utc::now().timestamp_millis();
        // Step 1: the durable half. `cancel_requested_at` is what the run's next
        // boundary reads and what a restarted process reconciles against.
        let _ = sqlx::query(
            "UPDATE chat_run_controls
             SET status='cancel_requested',
                 cancel_requested_at=COALESCE(cancel_requested_at, ?),
                 updated_at=?
             WHERE objective_id=? AND status='active'",
        )
        .bind(now)
        .bind(now)
        .bind(objective_id)
        .execute(&self.pool)
        .await;
        // Step 2: the process-local half, without which the durable row is a
        // note nobody is reading.
        let mut told_to_stop = false;
        for run_instance_id in &run_instance_ids {
            if crate::request_chat_run_stop(run_instance_id) {
                told_to_stop = true;
            }
        }
        // Step 3: a bounded wait for that boundary, when this process actually
        // owns the flag. Two independent signals end it — the run's control
        // being dropped, or the run having written its own terminal
        // run-control row — whichever the loop reaches first.
        if told_to_stop {
            let deadline = std::time::Instant::now()
                + std::time::Duration::from_millis(LIVE_RUN_STOP_GRACE_MS);
            while std::time::Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
                if run_instance_ids
                    .iter()
                    .all(|run_instance_id| !crate::chat_run_is_live(run_instance_id))
                {
                    break;
                }
                let still_cancelling: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM chat_run_controls
                     WHERE objective_id=? AND status='cancel_requested'",
                )
                .bind(objective_id)
                .fetch_one(&self.pool)
                .await
                .unwrap_or(0);
                if still_cancelling == 0 {
                    break;
                }
            }
        }
        // Step 4: hand the run control back to the settlement. The stop is
        // recorded — `cancel_requested_at` stays as the audit fact — but a
        // *pending* `cancel_requested` status would fence this very settlement:
        // the durable-cancellation guard refuses every non-cancelled decision
        // while a chat run control still shows a stop outstanding. The run has
        // been told to stop and has had its bounded window, so the request is no
        // longer outstanding and the terminal write below owns the real final
        // status of both the Objective and its run control.
        let _ = sqlx::query(
            "UPDATE chat_run_controls
             SET status='active', updated_at=?
             WHERE objective_id=? AND status='cancel_requested'",
        )
        .bind(now)
        .bind(objective_id)
        .execute(&self.pool)
        .await;
    }

    async fn apply_decision_inner(
        &self,
        expected_revision: i64,
        decision: DecisionEnvelope,
        permit: Option<&codefactory_agent_loop::tool::MutationPermit>,
        delivery_takeover: Option<DeliveryTakeoverSettlement>,
        delivery_identity_park: Option<DeliveryIdentityParkPermit>,
    ) -> anyhow::Result<ObjectiveSnapshot> {
        let current = self
            .get(&decision.objective_id)
            .await?
            .ok_or_else(|| anyhow!("objective not found"))?;
        if current.revision != expected_revision {
            bail!(
                "objective revision conflict: expected {expected_revision}, actual {}",
                current.revision
            );
        }
        decision.validate(&current)?;
        // Runs before the transaction opens: a deterministic pool may allow a
        // single connection, and asking for a second one mid-transaction
        // deadlocks recovery into a timeout.
        let decision = self.bound_system_recovery(decision).await?;
        decision.validate(&current)?;
        // M30: an Objective that something other than its own agent loop is
        // about to end must stop the live run FIRST. Runs before the
        // transaction opens — a deterministic pool may allow a single
        // connection, and this needs the pool itself.
        //
        // Gated on `permit.is_none()`: the agent loop's own settlement holds the
        // mutation permit for the turn it is committing, and stopping a run that
        // is already finishing its own turn is both pointless and wrong — it
        // would flip the live run control to `cancel_requested` on a completion
        // commit. Only an outside actor (incident controller, convergence,
        // delivery supervisor) has no permit and needs the stop.
        self.request_live_run_stop_before_settlement(
            &decision.objective_id,
            decision.status.is_terminal() && permit.is_none(),
        )
        .await;
        let now = Utc::now().timestamp_millis();
        let process_instance = current_process_instance();
        let completed_at = decision.status.is_terminal().then_some(now);
        let recovery_generation = if current.status == ObjectiveStatus::WaitingCoreInput
            && current.failure_code.as_deref() == Some(TECHNICAL_RECOVERY_EXHAUSTED)
            && decision.status == ObjectiveStatus::WaitingSystem
            && decision.decision_type == DecisionType::ApplyRecommended
        {
            current.recovery_generation + 1
        } else {
            current.recovery_generation
        };
        let evidence_ref = decision
            .evidence
            .as_ref()
            .map(|evidence| evidence.evidence_ref.clone());
        let mut delivery_run_to_complete: Option<DeliveryCompletionCandidate> = None;
        // R2: the user-visible failure report is generated from structured data
        // before the transaction opens — the pool may allow a single
        // connection, and the report reads the real workspace on disk.
        let failure_summary_message = if decision.is_parked_system_incident() {
            self.build_failure_summary(&current, &decision).await
        } else {
            None
        }
        .unwrap_or_else(|| {
            "这件事没做成。你交代的目标没有达成，当前进度已保留在会话里；\
直接回一句「继续」或「把这些改动交付」就可以接着做。"
                .to_string()
        });
        let mut tx = self.pool.begin().await?;
        // The decision transaction may be the only SQLite connection in a
        // deterministic test or embedded deployment. Inspect schema through
        // that same transaction; asking the pool for a second connection here
        // deadlocks max_connections=1 and turns recovery into a 30s timeout.
        let has_chat_cancel_projection: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type='table' AND name IN ('chat_run_controls','chat_turn_state')",
        )
        .fetch_one(&mut *tx)
        .await?;
        let has_chat_cancel_projection = has_chat_cancel_projection == 2;
        if decision.decision_type != DecisionType::Cancelled && has_chat_cancel_projection {
            let pending_session_cancellations: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM chat_session_cancel_intents stop
                 JOIN objectives objective ON objective.session_id=stop.session_id
                 WHERE stop.status='requested' AND objective.id=?",
            )
            .bind(&decision.objective_id)
            .fetch_one(&mut *tx)
            .await?;
            if pending_session_cancellations > 0 {
                bail!("objective decision fenced by durable session cancellation");
            }
            let pending_chat_cancellations: i64 = sqlx::query_scalar(
                "SELECT COUNT(*)
                 FROM chat_run_controls control
                 JOIN chat_turn_state turn
                   ON turn.root_turn_id=control.root_turn_id
                  AND turn.session_id=control.session_id
                  AND turn.objective_id IS NOT NULL
                  AND (control.objective_id IS NULL
                       OR control.objective_id=turn.objective_id)
                 WHERE control.status='cancel_requested'
                   AND COALESCE(control.objective_id, turn.objective_id)=?",
            )
            .bind(&decision.objective_id)
            .fetch_one(&mut *tx)
            .await?;
            if pending_chat_cancellations > 0 {
                bail!("objective decision fenced by durable chat cancellation");
            }
        }
        if let Some(permit) = permit {
            if permit.objective_id != decision.objective_id {
                bail!("mutation permit objective does not match decision");
            }
            let remediation_claim = sqlx::query(
                "UPDATE objective_remediations SET updated_at=updated_at
                 WHERE id=? AND objective_id=? AND status='claimed'
                   AND lease_owner=? AND attempt_index=? AND lease_expires_at>?
                   AND COALESCE(binding_id, '')=COALESCE(?, '')",
            )
            .bind(&permit.remediation_id)
            .bind(&permit.objective_id)
            .bind(&permit.owner)
            .bind(permit.claim_epoch)
            .bind(now)
            .bind(&permit.binding_id)
            .execute(&mut *tx)
            .await?;
            if remediation_claim.rows_affected() != 1 {
                bail!("remediation claim expired or changed before settlement");
            }
            let objective_claim = sqlx::query(
                "UPDATE objectives SET updated_at=updated_at
                 WHERE id=? AND status='waiting_system' AND remediation_id=?
                   AND lease_owner=? AND lease_expires_at>?",
            )
            .bind(&permit.objective_id)
            .bind(&permit.remediation_id)
            .bind(&permit.owner)
            .bind(now)
            .execute(&mut *tx)
            .await?;
            if objective_claim.rows_affected() != 1 {
                bail!("objective claim expired or changed before settlement");
            }
        }
        if decision.status == ObjectiveStatus::Completed || delivery_takeover.is_some() {
            let pointed_delivery_run_id: Option<String> = sqlx::query_scalar(
                "SELECT delivery_run_id FROM objectives WHERE id=? AND revision=?",
            )
            .bind(&decision.objective_id)
            .bind(expected_revision)
            .fetch_one(&mut *tx)
            .await?;
            if let Some(takeover) = delivery_takeover.as_ref() {
                if pointed_delivery_run_id.as_deref() != Some(takeover.run_id.as_str()) {
                    bail!("delivery takeover does not match the authoritative Objective pointer");
                }
                let pending_calls: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM tool_calls tool
                     JOIN objective_bindings binding
                       ON binding.id=tool.binding_id AND binding.objective_id=tool.objective_id
                     JOIN delivery_runs run
                       ON run.objective_id=tool.objective_id
                     WHERE run.id=? AND run.objective_id=?
                       AND run.claim_epoch=? AND run.reconciled_claim_epoch=run.claim_epoch
                       AND run.status='awaiting_completion_arbitration'
                       AND tool.tool_name='deliver_changes'
                       AND tool.status='pending'
                       AND tool.binding_id=?
                       AND tool.resource_generation=?
                       AND tool.action_signature=?
                       AND tool.resource_generation=binding.resource_generation
                       AND (
                         (binding.resource_kind='chat_root_turn'
                          AND binding.resource_id=run.root_turn_id)
                         OR
                         (binding.resource_kind='task_run'
                          AND binding.resource_id=run.task_id)
                       )",
                )
                .bind(&takeover.run_id)
                .bind(&decision.objective_id)
                .bind(takeover.claim_epoch)
                .bind(&takeover.binding_id)
                .bind(takeover.resource_generation)
                .bind(&takeover.action_signature)
                .fetch_one(&mut *tx)
                .await?;
                if pending_calls == 0 {
                    bail!("delivery takeover has no crash-left pending tool call to settle");
                }
                let result_json = serde_json::json!({
                    "status": "done",
                    "delivery_run_id": takeover.run_id,
                    "claim_epoch": takeover.claim_epoch,
                    "reconciled_after_takeover": true,
                })
                .to_string();
                let tool_rows = sqlx::query(
                    "SELECT tool.id, message.session_id
                     FROM tool_calls tool
                     JOIN messages message ON message.id=tool.message_id
                     JOIN objective_bindings binding
                       ON binding.id=tool.binding_id AND binding.objective_id=tool.objective_id
                     JOIN delivery_runs run ON run.objective_id=tool.objective_id
                     WHERE run.id=? AND run.objective_id=?
                       AND run.claim_epoch=? AND run.reconciled_claim_epoch=run.claim_epoch
                       AND tool.tool_name='deliver_changes' AND tool.status='pending'
                       AND tool.binding_id=?
                       AND tool.resource_generation=?
                       AND tool.action_signature=?
                       AND tool.resource_generation=binding.resource_generation
                       AND (
                         (binding.resource_kind='chat_root_turn'
                          AND binding.resource_id=run.root_turn_id)
                         OR
                         (binding.resource_kind='task_run'
                          AND binding.resource_id=run.task_id)
                       )",
                )
                .bind(&takeover.run_id)
                .bind(&decision.objective_id)
                .bind(takeover.claim_epoch)
                .bind(&takeover.binding_id)
                .bind(takeover.resource_generation)
                .bind(&takeover.action_signature)
                .fetch_all(&mut *tx)
                .await?;
                for row in tool_rows {
                    let trace_id: String = row.try_get("id")?;
                    let session_id: String = row.try_get("session_id")?;
                    let provider_tool_call_id = trace_id
                        .strip_prefix(&format!("{session_id}:"))
                        .ok_or_else(|| {
                            anyhow!("normalized delivery tool call identity is malformed")
                        })?;
                    let updated = sqlx::query(
                        "UPDATE tool_calls
                         SET status='done', result=?, error=NULL, duration_ms=COALESCE(duration_ms, 0)
                         WHERE id=? AND status='pending'",
                    )
                    .bind(&result_json)
                    .bind(&trace_id)
                    .execute(&mut *tx)
                    .await?;
                    if updated.rows_affected() != 1 {
                        bail!("delivery tool call changed during takeover settlement");
                    }
                    let replay_message_id = format!("{trace_id}:result");
                    let replay_content = serde_json::json!({
                        "tool_call_id": provider_tool_call_id,
                        "content": result_json.clone(),
                        "status": "done",
                    })
                    .to_string();
                    sqlx::query(
                        "INSERT INTO messages (id, session_id, role, content, created_at)
                         VALUES (?, ?, 'tool', ?, ?)
                         ON CONFLICT(id) DO UPDATE SET
                           session_id=excluded.session_id, role='tool', content=excluded.content",
                    )
                    .bind(replay_message_id)
                    .bind(session_id)
                    .bind(replay_content)
                    .bind(now)
                    .execute(&mut *tx)
                    .await?;
                }
            }
            let unresolved_receipts: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM side_effect_receipts
                 WHERE objective_id=? AND status IN ('not_started','started','unknown')",
            )
            .bind(&decision.objective_id)
            .fetch_one(&mut *tx)
            .await?;
            if unresolved_receipts > 0 {
                bail!(
                    "objective completion refused with {unresolved_receipts} unresolved side-effect receipt(s)"
                );
            }
            let tool_calls_exist: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name='tool_calls'",
            )
            .fetch_one(&mut *tx)
            .await?;
            if tool_calls_exist == 1 {
                let unresolved_tool_calls: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM tool_calls
                     WHERE objective_id=? AND status='pending'",
                )
                .bind(&decision.objective_id)
                .fetch_one(&mut *tx)
                .await?;
                if unresolved_tool_calls > 0 {
                    bail!(
                        "objective completion refused with {unresolved_tool_calls} unresolved tool call(s)"
                    );
                }
            }

            let objective_side_effect_started: i64 = sqlx::query_scalar(
                "SELECT side_effect_started FROM objectives
                 WHERE id=? AND revision=?",
            )
            .bind(&decision.objective_id)
            .bind(expected_revision)
            .fetch_one(&mut *tx)
            .await?;
            let current_binding: Option<(String, i64, i64)> =
                if let Some(binding_id) = permit.and_then(|permit| permit.binding_id.as_deref()) {
                    sqlx::query_as(
                        "SELECT id, resource_generation, side_effect_started
                         FROM objective_bindings
                         WHERE id=? AND objective_id=?",
                    )
                    .bind(binding_id)
                    .bind(&decision.objective_id)
                    .fetch_optional(&mut *tx)
                    .await?
                } else {
                    sqlx::query_as(
                        "SELECT id, resource_generation, side_effect_started
                         FROM objective_bindings
                         WHERE objective_id=?
                         ORDER BY CASE WHEN resource_id=? THEN 0 ELSE 1 END,
                                  resource_generation DESC, updated_at DESC, id DESC
                         LIMIT 1",
                    )
                    .bind(&decision.objective_id)
                    .bind(decision.resume_cursor.as_deref().unwrap_or(""))
                    .fetch_optional(&mut *tx)
                    .await?
                };
            let binding_side_effect_started = current_binding
                .as_ref()
                .is_some_and(|(_, _, started)| *started != 0);
            if pointed_delivery_run_id.is_some()
                || objective_side_effect_started != 0
                || binding_side_effect_started
            {
                let Some((binding_id, resource_generation, _)) = current_binding else {
                    bail!(
                        "objective completion refused because a side effect started without a current Objective binding"
                    );
                };
                if tool_calls_exist != 1 {
                    bail!(
                        "objective completion refused because a side effect started without normalized tool attribution"
                    );
                }
                let attributed_actions: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM tool_calls
                     WHERE objective_id=? AND binding_id=? AND resource_generation=?
                       AND NULLIF(TRIM(action_signature), '') IS NOT NULL",
                )
                .bind(&decision.objective_id)
                .bind(&binding_id)
                .bind(resource_generation)
                .fetch_one(&mut *tx)
                .await?;
                if attributed_actions == 0 {
                    bail!(
                        "objective completion refused because a side effect started without a trustworthy current attributed receipt"
                    );
                }
                let delivery_signatures: Vec<String> = sqlx::query_scalar(
                    "SELECT DISTINCT action_signature FROM tool_calls
                     WHERE objective_id=? AND binding_id=? AND resource_generation=?
                       AND tool_name='deliver_changes'
                       AND NULLIF(TRIM(action_signature), '') IS NOT NULL
                     ORDER BY action_signature",
                )
                .bind(&decision.objective_id)
                .bind(&binding_id)
                .bind(resource_generation)
                .fetch_all(&mut *tx)
                .await?;
                if pointed_delivery_run_id.is_some() && delivery_signatures.is_empty() {
                    bail!(
                        "objective completion refused because its pointed DeliveryRun lacks a completed current deliver_changes action"
                    );
                }
                if delivery_signatures.len() > 1 {
                    bail!(
                        "objective completion refused because current deliver_changes actions do not have one stable action signature"
                    );
                }
                if let Some(action_signature) = delivery_signatures.first() {
                    let completed_action_rows: i64 = sqlx::query_scalar(
                        "SELECT COUNT(*) FROM tool_calls
                         WHERE objective_id=? AND binding_id=? AND resource_generation=?
                           AND tool_name='deliver_changes' AND status='done'
                           AND action_signature=?",
                    )
                    .bind(&decision.objective_id)
                    .bind(&binding_id)
                    .bind(resource_generation)
                    .bind(action_signature)
                    .fetch_one(&mut *tx)
                    .await?;
                    if completed_action_rows == 0 {
                        bail!(
                            "objective completion refused because deliver_changes has no completed current action"
                        );
                    }

                    let candidate_rows = sqlx::query(
                        "SELECT run.id, run.stage, run.claim_epoch,
                                run.repo_identity, run.worktree_identity,
                                run.expected_head_sha, run.change_set_digest,
                                run.requested_ceiling, run.reached_ceiling,
                                run.canonical_pr_number, run.canonical_pr_url
                         FROM delivery_runs AS run
                         JOIN objective_bindings AS binding
                           ON binding.id=? AND binding.objective_id=run.objective_id
                         JOIN objectives AS objective
                           ON objective.id=run.objective_id
                          AND objective.delivery_run_id=run.id
                         WHERE run.objective_id=?
                           AND run.run_kind='deliver_changes'
                           AND run.status IN ('awaiting_completion_arbitration','completed')
                           AND run.stage='complete'
                           AND run.claim_epoch>0
                           AND run.reconciled_claim_epoch=run.claim_epoch
                           AND (
                             (run.status='awaiting_completion_arbitration'
                              AND run.next_action_authorized=1)
                             OR
                             (run.status='completed'
                              AND run.next_action_authorized=0)
                           )
                           AND COALESCE(run.next_action, '')=''
                           AND COALESCE(run.failure_signature, '')=''
                           AND COALESCE(run.wait_class, 'none')='none'
                           AND NULLIF(TRIM(run.repo_identity), '') IS NOT NULL
                           AND NULLIF(TRIM(run.worktree_identity), '') IS NOT NULL
                           AND NULLIF(TRIM(run.expected_head_sha), '') IS NOT NULL
                           AND NULLIF(TRIM(run.change_set_digest), '') IS NOT NULL
                           AND (
                             (binding.resource_kind='chat_root_turn'
                              AND run.root_turn_id=binding.resource_id)
                             OR
                             (binding.resource_kind='task_run'
                              AND run.task_id=binding.resource_id)
                           )
                           AND CASE run.reached_ceiling
                             WHEN 'local' THEN 0
                             WHEN 'committed' THEN 1
                             WHEN 'pushed' THEN 2
                             WHEN 'pr_open' THEN 3
                             WHEN 'ci_green' THEN 4
                             WHEN 'merge_queued' THEN 5
                             WHEN 'merged' THEN 6
                             WHEN 'release_triggered' THEN 7
                             WHEN 'deployment_succeeded' THEN 8
                             WHEN 'live_verified' THEN 9
                             ELSE -1
                           END >= CASE run.requested_ceiling
                             WHEN 'pr_only' THEN 3
                             WHEN 'through_ci_green' THEN 4
                             WHEN 'through_merge' THEN 6
                             WHEN 'through_release' THEN 9
                             ELSE 100
                           END
                           AND run.canonical_pr_number IS NOT NULL
                           AND NULLIF(TRIM(run.canonical_pr_url), '') IS NOT NULL
                           AND run.canonical_head_sha=run.expected_head_sha
                           AND NOT EXISTS (
                             SELECT 1 FROM delivery_mutation_intents AS intent
                             WHERE intent.run_id=run.id
                               AND intent.status IN ('started','unknown')
                           )",
                    )
                    .bind(&binding_id)
                    .bind(&decision.objective_id)
                    .fetch_all(&mut *tx)
                    .await?;
                    if candidate_rows.len() != 1 {
                        bail!(
                            "objective completion refused because deliver_changes requires exactly one pointed receipt-backed DeliveryRun at its requested ceiling"
                        );
                    }
                    let candidate = &candidate_rows[0];
                    let run_id: String = candidate.try_get("id")?;
                    let stage: String = candidate.try_get("stage")?;
                    let claim_epoch: i64 = candidate.try_get("claim_epoch")?;
                    let repo_identity: String = candidate.try_get("repo_identity")?;
                    let worktree_identity: String = candidate.try_get("worktree_identity")?;
                    let expected_head_sha: String = candidate.try_get("expected_head_sha")?;
                    let change_set_digest: String = candidate.try_get("change_set_digest")?;
                    let requested_ceiling: String = candidate.try_get("requested_ceiling")?;
                    let reached_ceiling: String = candidate.try_get("reached_ceiling")?;
                    let canonical_pr_number: i64 = candidate.try_get("canonical_pr_number")?;
                    let canonical_pr_url: String = candidate.try_get("canonical_pr_url")?;
                    let already_terminal: bool = sqlx::query_scalar::<_, String>(
                        "SELECT status FROM delivery_runs WHERE id=?",
                    )
                    .bind(&run_id)
                    .fetch_one(&mut *tx)
                    .await?
                        == "completed";
                    let identity_material = serde_json::json!({
                        "objective_id": decision.objective_id,
                        "delivery_run_id": run_id,
                        "claim_epoch": claim_epoch,
                        "repo_identity": repo_identity,
                        "worktree_identity": worktree_identity,
                        "expected_head_sha": expected_head_sha,
                        "change_set_digest": change_set_digest,
                        "requested_ceiling": requested_ceiling,
                        "reached_ceiling": reached_ceiling,
                        "canonical_pr_number": canonical_pr_number,
                        "canonical_pr_url": canonical_pr_url,
                    })
                    .to_string();
                    let external_identity_digest =
                        format!("sha256:{:x}", Sha256::digest(identity_material.as_bytes()));
                    let idempotency_material = format!(
                        "delivery_completion\0{}\0{}\0{}\0{}",
                        decision.objective_id, binding_id, action_signature, run_id,
                    );
                    let idempotency_key = format!(
                        "sha256:{:x}",
                        Sha256::digest(idempotency_material.as_bytes())
                    );
                    let reconciliation_receipt_id = format!(
                        "sha256:{:x}",
                        Sha256::digest(
                            format!("side_effect_receipt\0{idempotency_key}").as_bytes()
                        )
                    );
                    let summary_json = serde_json::json!({
                        "delivery_run_id": run_id,
                        "claim_epoch": claim_epoch,
                        "requested_ceiling": requested_ceiling,
                        "reached_ceiling": reached_ceiling,
                    })
                    .to_string();
                    let mut exact_receipt: i64 = sqlx::query_scalar(
                        "SELECT COUNT(*) FROM side_effect_receipts
                         WHERE id=? AND objective_id=? AND binding_id=?
                           AND action_fingerprint=? AND idempotency_key=?
                           AND status='reconciled' AND external_identity_digest=?",
                    )
                    .bind(&reconciliation_receipt_id)
                    .bind(&decision.objective_id)
                    .bind(&binding_id)
                    .bind(action_signature)
                    .bind(&idempotency_key)
                    .bind(&external_identity_digest)
                    .fetch_one(&mut *tx)
                    .await?;
                    if exact_receipt == 0 {
                        sqlx::query(
                            "INSERT INTO side_effect_receipts
                             (id, objective_id, binding_id, revision, action_fingerprint,
                              idempotency_key, status, external_identity_digest,
                              summary_json, created_at, observed_at)
                             VALUES (?, ?, ?, ?, ?, ?, 'reconciled', ?, ?, ?, ?)",
                        )
                        .bind(&reconciliation_receipt_id)
                        .bind(&decision.objective_id)
                        .bind(&binding_id)
                        .bind(expected_revision)
                        .bind(action_signature)
                        .bind(&idempotency_key)
                        .bind(&external_identity_digest)
                        .bind(summary_json)
                        .bind(now)
                        .bind(now)
                        .execute(&mut *tx)
                        .await?;
                        exact_receipt = 1;
                    }
                    if exact_receipt != 1 {
                        bail!(
                            "objective completion refused because the DeliveryRun reconciliation receipt conflicted"
                        );
                    }
                    delivery_run_to_complete = Some(DeliveryCompletionCandidate {
                        run_id,
                        binding_id: binding_id.clone(),
                        resource_generation,
                        stage,
                        claim_epoch,
                        repo_identity,
                        worktree_identity,
                        expected_head_sha,
                        change_set_digest,
                        requested_ceiling,
                        reached_ceiling,
                        canonical_pr_number,
                        canonical_pr_url,
                        action_signature: action_signature.clone(),
                        reconciliation_receipt_id,
                        already_terminal,
                        takeover_lease_owner: delivery_takeover
                            .as_ref()
                            .map(|takeover| takeover.lease_owner.clone()),
                    });
                }

                let unmatched_actions: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM tool_calls AS tool
                     WHERE tool.objective_id=? AND tool.binding_id=?
                       AND tool.resource_generation=?
                       AND NULLIF(TRIM(tool.action_signature), '') IS NOT NULL
                       AND NOT EXISTS (
                           SELECT 1 FROM side_effect_receipts AS receipt
                           LEFT JOIN tool_recovery_call_links exact_link
                             ON exact_link.receipt_id=receipt.id
                           WHERE receipt.objective_id=tool.objective_id
                             AND receipt.binding_id=?
                             AND (
                               receipt.status IN ('committed','reconciled')
                               OR (
                                 receipt.status='cancelled'
                                 AND tool.status IN ('error','cancelled','denied')
                               )
                             )
                             AND (
                               exact_link.tool_call_id=tool.id
                               OR (
                                 NOT EXISTS (
                                   SELECT 1 FROM tool_recovery_call_links own_link
                                   WHERE own_link.tool_call_id=tool.id
                                 )
                                 AND receipt.action_fingerprint=tool.action_signature
                               )
                             )
                       )",
                )
                .bind(&decision.objective_id)
                .bind(&binding_id)
                .bind(resource_generation)
                .bind(&binding_id)
                .fetch_one(&mut *tx)
                .await?;
                if unmatched_actions > 0 {
                    bail!(
                        "objective completion refused with {unmatched_actions} current attributed side effect(s) lacking a matching committed receipt"
                    );
                }
            }
        }
        if permit.is_some_and(|permit| {
            permit.binding_id.is_some() != permit.resource_generation.is_some()
        }) {
            bail!("mutation permit binding and resource generation must be paired");
        }
        let authoritative_binding_id =
            if let Some(binding_id) = permit.and_then(|permit| permit.binding_id.clone()) {
                let generation = sqlx::query_scalar::<_, i64>(
                    "SELECT resource_generation FROM objective_bindings
                 WHERE id=? AND objective_id=?",
                )
                .bind(&binding_id)
                .bind(&decision.objective_id)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(generation) = generation else {
                    bail!("mutation permit binding does not belong to objective");
                };
                if permit.and_then(|permit| permit.resource_generation) != Some(generation) {
                    bail!("mutation permit resource generation changed before settlement");
                }
                Some(binding_id)
            } else {
                sqlx::query_scalar::<_, String>(
                    "SELECT id FROM objective_bindings
                 WHERE objective_id=?
                 ORDER BY CASE WHEN resource_id=? THEN 0 ELSE 1 END,
                          updated_at DESC, resource_generation DESC
                 LIMIT 1",
                )
                .bind(&decision.objective_id)
                .bind(decision.resume_cursor.as_deref().unwrap_or(""))
                .fetch_optional(&mut *tx)
                .await?
            };
        let attention_request_json = decision
            .attention_request
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let result = sqlx::query(
            "UPDATE objectives SET
               revision=?, status=?, decision_type=?, domain=?,
               reached_acceptance=?, requires_user_action=?, request_key=?,
               decision_key=?, action_signature=?, attention_request_json=?,
               failure_code=?, failure_signature=?,
               recovery_owner=?, remediation_id=?, resume_cursor=?,
               output_started=MAX(output_started, ?),
               side_effect_started=MAX(side_effect_started, ?), next_observation_at=?,
               lease_owner=NULL, lease_expires_at=NULL, evidence_ref=?,
               cancellation_provenance=?, last_observed_process_instance=?,
               recovery_generation=?, last_progress_at=?, updated_at=?, completed_at=?
             WHERE id=? AND revision=?",
        )
        .bind(decision.revision)
        .bind(decision.status.as_str())
        .bind(decision.decision_type.as_str())
        .bind(decision.domain.as_str())
        .bind(&decision.reached_acceptance)
        .bind(i64::from(decision.requires_user_action))
        .bind(&decision.request_key)
        .bind(&decision.decision_key)
        .bind(&decision.action_signature)
        .bind(&attention_request_json)
        .bind(&decision.failure_code)
        .bind(&decision.failure_signature)
        .bind(&decision.recovery_owner)
        .bind(&decision.remediation_id)
        .bind(&decision.resume_cursor)
        .bind(i64::from(decision.output_started))
        .bind(i64::from(decision.side_effect_started))
        .bind(decision.next_observation_at)
        .bind(&evidence_ref)
        .bind(&decision.cancellation_provenance)
        .bind(&process_instance)
        .bind(recovery_generation)
        .bind(now)
        .bind(now)
        .bind(completed_at)
        .bind(&decision.objective_id)
        .bind(expected_revision)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() != 1 {
            bail!("objective revision changed while applying decision");
        }
        if matches!(
            decision.status,
            ObjectiveStatus::Completed | ObjectiveStatus::Cancelled
        ) {
            crate::agent::execution_workspace::mark_objective_terminal_in_tx(
                &mut tx,
                &decision.objective_id,
                now,
            )
            .await?;
        }

        if decision.status == ObjectiveStatus::Completed {
            let terminal_projection_columns: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM pragma_table_info('chat_turn_state')
                 WHERE name IN ('revision','phase','status','recent_activity_kind',
                                'recent_activity_label','waiting_reason','updated_at',
                                'completed_at','terminal_reason','objective_id',
                                'turn_settled_at','stream_closed_at','terminal_revision',
                                'visible_final_message_id','visible_final_kind','next_action',
                                'objective_revision')",
            )
            .fetch_one(&mut *tx)
            .await?;
            let terminal_root_turn_id = current
                .resume_cursor
                .as_deref()
                .or(current.root_turn_id.as_deref());
            let has_bound_turn: i64 = if let Some(root_turn_id) = terminal_root_turn_id {
                sqlx::query_scalar(
                    "SELECT COUNT(*) FROM chat_turn_state
                     WHERE objective_id=? AND root_turn_id=?",
                )
                .bind(&decision.objective_id)
                .bind(root_turn_id)
                .fetch_one(&mut *tx)
                .await
                .unwrap_or(0)
            } else {
                0
            };
            if has_bound_turn > 0 {
                if terminal_projection_columns != 17 || has_bound_turn != 1 {
                    bail!("chat completion requires exactly one complete terminal turn projection");
                }
                let message_columns: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM pragma_table_info('messages')
                     WHERE name IN ('id','session_id','role','content','completion_state')",
                )
                .fetch_one(&mut *tx)
                .await?;
                if message_columns != 5 {
                    bail!("chat completion requires a queryable visible-final message schema");
                }
                let session_id = current
                    .session_id
                    .as_deref()
                    .ok_or_else(|| anyhow!("chat completion has no session identity"))?;
                let root_turn_id = terminal_root_turn_id
                    .ok_or_else(|| anyhow!("chat completion has no root turn identity"))?;
                let visible_final_message_id = decision
                    .visible_final_message_id
                    .as_deref()
                    .ok_or_else(|| {
                        anyhow!("chat completion has no exact visible-final message identity")
                    })?;
                let accepted_visible_final = sqlx::query_scalar::<_, String>(
                    "SELECT assistant.id FROM messages assistant
                     WHERE assistant.id=? AND assistant.session_id=? AND assistant.role='assistant'
                       AND (assistant.completion_state IS NULL OR assistant.completion_state='')
                       AND TRIM(assistant.content)<>''
                       AND assistant.rowid>(
                         SELECT rowid FROM messages
                         WHERE id=? AND session_id=? AND role='user'
                       )
                     LIMIT 1",
                )
                .bind(visible_final_message_id)
                .bind(session_id)
                .bind(root_turn_id)
                .bind(session_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or_else(|| anyhow!("Completion Arbiter found no accepted visible final"))?;
                sqlx::query(
                    "UPDATE chat_turn_state
                     SET revision=revision+1, phase='finalizing', status='completed',
                         recent_activity_kind='objective_completed',
                         recent_activity_label='目标证据已满足', waiting_reason=NULL,
                         updated_at=?, completed_at=COALESCE(completed_at, ?),
                         terminal_reason='complete', objective_revision=?,
                         turn_settled_at=COALESCE(turn_settled_at, ?),
                         stream_closed_at=COALESCE(stream_closed_at, ?),
                         terminal_revision=?, visible_final_message_id=?,
                         visible_final_kind='assistant_final', next_action=NULL
                     WHERE objective_id=? AND root_turn_id=?
                       AND (terminal_revision IS NULL OR terminal_revision<=?)",
                )
                .bind(now)
                .bind(now)
                .bind(decision.revision)
                .bind(now)
                .bind(now)
                .bind(decision.revision)
                .bind(&accepted_visible_final)
                .bind(&decision.objective_id)
                .bind(root_turn_id)
                .bind(decision.revision)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "UPDATE chat_run_controls
                     SET status='completed', objective_revision=?,
                         settled_at=COALESCE(settled_at, ?), updated_at=?
                     WHERE objective_id=? AND root_turn_id=?
                       AND status IN ('active','cancel_requested')",
                )
                .bind(decision.revision)
                .bind(now)
                .bind(now)
                .bind(&decision.objective_id)
                .bind(root_turn_id)
                .execute(&mut *tx)
                .await?;
            }
            // U18/R2: a terminal Objective owns no live turn. Close any ghost
            // turn the objective accumulated (reprompts / steers) so the session
            // stops rendering 运行中 and the composer is immediately usable.
            settle_superseded_chat_turns_in_tx(
                &mut tx,
                &decision.objective_id,
                None,
                "objective_terminated",
                now,
            )
            .await?;
        }

        if decision.status == ObjectiveStatus::Cancelled {
            let turn_projection_columns: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM pragma_table_info('chat_turn_state')
                 WHERE name IN ('revision','phase','status','recent_activity_kind',
                                'recent_activity_label','waiting_reason','updated_at',
                                'completed_at','terminal_reason','objective_id')",
            )
            .fetch_one(&mut *tx)
            .await?;
            if turn_projection_columns == 10 {
                sqlx::query(
                    "UPDATE chat_turn_state
                     SET revision=revision+1, phase='cancelled', status='cancelled',
                         recent_activity_kind='cancelled', recent_activity_label='已停止',
                         waiting_reason=NULL, updated_at=?, completed_at=?,
                         terminal_reason='cancelled'
                     WHERE objective_id=?",
                )
                .bind(now)
                .bind(now)
                .bind(&decision.objective_id)
                .execute(&mut *tx)
                .await?;
            }
        }

        if decision.is_parked_system_incident() {
            // Exhaustion is one state transition, not an Objective write
            // followed by best-effort projections. Otherwise a crash can leave
            // a chat turn recovering forever or a pending task redispatchable.
            let has_delivery_runs: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name='delivery_runs'",
            )
            .fetch_one(&mut *tx)
            .await?;
            if has_delivery_runs == 1 {
                let parked_delivery = if let Some(park) = delivery_identity_park.as_ref() {
                    let parked = sqlx::query(
                        "UPDATE delivery_runs
                         SET status='failed',
                             wait_class=NULL,
                             next_action=NULL,
                             next_action_authorized=0,
                             lease_owner=NULL, lease_expires_at=NULL,
                             failure_code='objective_failed',
                             failure_class='objective_failed',
                             failure_signature=?, stage_attempt=?,
                             last_observed_at=?, updated_at=?
                         WHERE id=? AND objective_id=?
                           AND id=(SELECT delivery_run_id FROM objectives WHERE id=?)
                           AND lease_owner=? AND claim_epoch=? AND lease_expires_at>?
                           AND status NOT IN ('completed','failed','cancelled','rejected')
                         RETURNING id, stage, wait_class",
                    )
                    .bind(&park.failure_signature)
                    .bind(park.attempt_index)
                    .bind(now)
                    .bind(now)
                    .bind(&park.run_id)
                    .bind(&decision.objective_id)
                    .bind(&decision.objective_id)
                    .bind(&park.lease_owner)
                    .bind(park.claim_epoch)
                    .bind(now)
                    .fetch_optional(&mut *tx)
                    .await?;
                    if parked.is_none() {
                        bail!("delivery identity park permit expired or no longer matches the authoritative run");
                    }
                    parked
                } else {
                    sqlx::query(
                        "UPDATE delivery_runs
                     SET status='failed',
                         wait_class=NULL,
                         next_action=NULL,
                         next_action_authorized=0,
                         lease_owner=NULL, lease_expires_at=NULL,
                         failure_code=COALESCE(failure_code, 'objective_failed'),
                         failure_class=COALESCE(failure_class, 'objective_failed'),
                         last_observed_at=?, updated_at=?
                     WHERE objective_id=?
                       AND id=(SELECT delivery_run_id FROM objectives WHERE id=?)
                       AND status NOT IN ('completed','failed','cancelled','rejected')
                     RETURNING id, stage, wait_class",
                    )
                    .bind(now)
                    .bind(now)
                    .bind(&decision.objective_id)
                    .bind(&decision.objective_id)
                    .fetch_optional(&mut *tx)
                    .await?
                };
                if let Some(run) = parked_delivery {
                    let has_delivery_events: i64 = sqlx::query_scalar(
                        "SELECT COUNT(*) FROM sqlite_master
                         WHERE type='table' AND name='delivery_run_events'",
                    )
                    .fetch_one(&mut *tx)
                    .await?;
                    if has_delivery_events == 1 {
                        let run_id: String = run.try_get("id")?;
                        let stage: String = run.try_get("stage")?;
                        sqlx::query(
                            "INSERT INTO delivery_run_events
                             (id, run_id, event_kind, stage, status, wait_class,
                              detail_json, process_instance, created_at)
                             VALUES (?, ?, 'objective_failed', ?, 'failed', NULL, ?, ?, ?)",
                        )
                        .bind(Uuid::new_v4().to_string())
                        .bind(run_id)
                        .bind(stage)
                        .bind(
                            serde_json::json!({
                                "reason": "linked_objective_failed",
                                "next_action": null,
                                "failure_signature": delivery_identity_park.as_ref().map(|park| &park.failure_signature),
                                "stage_attempt": delivery_identity_park.as_ref().map(|park| park.attempt_index),
                                "claim_epoch": delivery_identity_park.as_ref().map(|park| park.claim_epoch),
                            })
                            .to_string(),
                        )
                        .bind(OBJECTIVE_INCIDENT_CONTROLLER)
                        .bind(now)
                        .execute(&mut *tx)
                        .await?;
                    }
                }
            }
            let turn_projection_columns: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM pragma_table_info('chat_turn_state')
                 WHERE name IN ('revision','phase','status','recent_activity_kind',
                                'recent_activity_label','waiting_reason','updated_at',
                                'completed_at','terminal_reason','objective_id')",
            )
            .fetch_one(&mut *tx)
            .await?;
            let has_objective_link: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM pragma_table_info('chat_turn_state')
                 WHERE name='objective_id'",
            )
            .fetch_one(&mut *tx)
            .await?;
            let terminal_root_turn_id = current
                .resume_cursor
                .as_deref()
                .or(current.root_turn_id.as_deref());
            let has_bound_turn: i64 = if has_objective_link == 1 && terminal_root_turn_id.is_some()
            {
                sqlx::query_scalar(
                    "SELECT COUNT(*) FROM chat_turn_state
                     WHERE objective_id=? AND root_turn_id=?",
                )
                .bind(&decision.objective_id)
                .bind(terminal_root_turn_id.unwrap())
                .fetch_one(&mut *tx)
                .await?
            } else {
                0
            };
            if has_bound_turn > 0 {
                if turn_projection_columns != 10 || has_bound_turn != 1 {
                    bail!("system incident requires exactly one complete terminal turn projection");
                }
                let durable_settlement_columns: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM pragma_table_info('chat_turn_state')
                     WHERE name IN ('turn_settled_at','stream_closed_at','terminal_revision',
                                    'visible_final_message_id','visible_final_kind','next_action')",
                )
                .fetch_one(&mut *tx)
                .await?;
                if durable_settlement_columns != 6 {
                    bail!("system incident requires durable terminal settlement columns");
                }
                let visible_final_message_id = format!(
                    "system-incident-{}-{}",
                    decision.objective_id, decision.revision
                );
                let message_columns: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM pragma_table_info('messages')
                     WHERE name IN ('id','session_id','role','content','completion_state','created_at')",
                )
                .fetch_one(&mut *tx)
                .await?;
                if message_columns != 6 {
                    bail!("system incident requires a queryable visible-final message schema");
                }
                let session_id = current
                    .session_id
                    .as_deref()
                    .ok_or_else(|| anyhow!("chat system incident has no session identity"))?;
                sqlx::query(
                    "INSERT OR IGNORE INTO messages
                     (id, session_id, role, content, completion_state, created_at)
                     VALUES (?, ?, 'assistant', ?, NULL, ?)",
                )
                .bind(&visible_final_message_id)
                .bind(session_id)
                .bind(failure_summary_message.as_str())
                .bind(now)
                .execute(&mut *tx)
                .await?;
                // Startup/background convergence settles rows a previous
                // process left behind; that is not user activity, so it must
                // not advance the session and reorder the sidebar.
                if decision.settlement_origin == SettlementOrigin::Live {
                    touch_session_in_settlement(&mut tx, session_id, now).await;
                }
                let projected = sqlx::query(
                    "UPDATE chat_turn_state
                     SET revision=revision+1, phase='finalizing', status='completed',
                         recent_activity_kind='objective_failed',
                         recent_activity_label='这件事没做成，已把试过的办法和保留下来的改动写给你',
                         waiting_reason=NULL,
                         updated_at=?, completed_at=?, terminal_reason='objective_failed',
                         objective_revision=?,
                         turn_settled_at=COALESCE(turn_settled_at, ?),
                         stream_closed_at=COALESCE(stream_closed_at, ?),
                         terminal_revision=?, visible_final_message_id=?,
                         visible_final_kind='assistant_final',
                         next_action=NULL
                     WHERE objective_id=? AND root_turn_id=?",
                )
                .bind(now)
                .bind(now)
                .bind(decision.revision)
                .bind(now)
                .bind(now)
                .bind(decision.revision)
                .bind(&visible_final_message_id)
                .bind(&decision.objective_id)
                .bind(terminal_root_turn_id.unwrap())
                .execute(&mut *tx)
                .await?;
                if projected.rows_affected() != 1 {
                    bail!("system incident terminal turn projection lost its exact binding");
                }
            }

            let terminalizable_tool_columns: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM pragma_table_info('tool_calls')
                 WHERE name IN ('objective_id','status','result')",
            )
            .fetch_one(&mut *tx)
            .await?;
            if terminalizable_tool_columns == 3 {
                sqlx::query(
                    "UPDATE tool_calls
                     SET status='blocked', result=?
                     WHERE objective_id=?
                       AND status IN ('pending','running','waiting','waiting_permission')",
                )
                .bind("这件事没做成，这一步没有继续执行；已完成的改动都保留在工作区里。")
                .bind(&decision.objective_id)
                .execute(&mut *tx)
                .await?;
            }

            let has_chat_run_controls: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name='chat_run_controls'",
            )
            .fetch_one(&mut *tx)
            .await?;
            if has_chat_run_controls == 1 && terminal_root_turn_id.is_some() {
                sqlx::query(
                    "UPDATE chat_run_controls
                     SET status='completed', objective_revision=?,
                         settled_at=COALESCE(settled_at, ?), updated_at=?
                     WHERE objective_id=? AND root_turn_id=?
                       AND status IN ('active','cancel_requested')",
                )
                .bind(decision.revision)
                .bind(now)
                .bind(now)
                .bind(&decision.objective_id)
                .bind(terminal_root_turn_id.unwrap())
                .execute(&mut *tx)
                .await?;
            }

            // U18/R2/R4: the honest failure terminal is a terminal Objective, so
            // no turn of it may stay live. Reprompts and steers leave extra
            // non-terminal rows behind; close them here (not one-by-one at each
            // exit) so the conversation is immediately usable again.
            settle_superseded_chat_turns_in_tx(
                &mut tx,
                &decision.objective_id,
                None,
                "objective_failed",
                now,
            )
            .await?;

            let has_task_runs: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM sqlite_master
                 WHERE type='table' AND name='task_runs'",
            )
            .fetch_one(&mut *tx)
            .await?;
            if has_task_runs == 1 {
                let completed_at = Utc::now().to_rfc3339();
                sqlx::query(
                    "UPDATE task_runs
                     SET status='failed', completed_at=?, error=?,
                         recovery_state=NULL, next_observation_at=NULL,
                         owner_pid=NULL, owner_start_token=NULL
                     WHERE objective_id=? AND status IN ('pending','running')",
                )
                .bind(completed_at)
                .bind("这件事没做成，已停止自动重试；保留下来的改动仍在工作区里。")
                .bind(&decision.objective_id)
                .execute(&mut *tx)
                .await?;
            }

            // R1/R4: the incident ledger closes here instead of opening a
            // capability-gated wait. Nothing in a future build changes this
            // outcome, so keeping the row open would only leave a permanent
            // "waiting for something" record the user cannot act on.
            sqlx::query(
                "UPDATE objective_incidents
                 SET status='resolved', resolved_at=?, updated_at=?,
                     reactivation_status='resolved'
                 WHERE objective_id=? AND status='open'",
            )
            .bind(now)
            .bind(now)
            .bind(&decision.objective_id)
            .execute(&mut *tx)
            .await?;
        }

        if decision.status == ObjectiveStatus::Cancelled {
            let cancelled_run = sqlx::query(
                "UPDATE delivery_runs
                 SET status='cancelled', next_action=NULL, next_action_authorized=0,
                     wait_class=NULL, failure_signature=NULL, failure_code=NULL,
                     failure_class=NULL, remediation_id=NULL,
                     business_decision_key=NULL, decision_options_json=NULL,
                     recommended_option=NULL, safe_default_action=NULL,
                     decision_reason=NULL, core_input_request_key=NULL,
                     core_inputs_json=NULL, core_input_attempts_json=NULL,
                     core_input_resume_stage=NULL, core_input_request_count=0,
                     lease_owner=NULL, lease_expires_at=NULL,
                     last_progress_at=?, progress_revision=progress_revision+1,
                     updated_at=?
                 WHERE objective_id=?
                   AND id=(SELECT delivery_run_id FROM objectives WHERE id=?)
                   AND status NOT IN ('completed','failed','cancelled','rejected')
                 RETURNING id, stage",
            )
            .bind(now)
            .bind(now)
            .bind(&decision.objective_id)
            .bind(&decision.objective_id)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(run) = cancelled_run {
                let run_id: String = run.try_get("id")?;
                let stage: String = run.try_get("stage")?;
                sqlx::query(
                    "INSERT INTO delivery_run_events
                     (id, run_id, event_kind, stage, status, wait_class,
                      detail_json, process_instance, created_at)
                     VALUES (?, ?, 'objective_cancelled', ?, 'cancelled', NULL, ?, ?, ?)",
                )
                .bind(Uuid::new_v4().to_string())
                .bind(run_id)
                .bind(stage)
                .bind(
                    serde_json::json!({
                        "objective_id": decision.objective_id,
                        "objective_revision": decision.revision,
                        "cancellation_provenance": decision.cancellation_provenance,
                    })
                    .to_string(),
                )
                .bind(&process_instance)
                .bind(now)
                .execute(&mut *tx)
                .await?;
            }
        }

        if let Some(candidate) = &delivery_run_to_complete {
            let completed_rows = if candidate.already_terminal {
                let exact_terminal: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM delivery_runs
                     WHERE id=? AND objective_id=? AND status='completed'
                       AND stage=? AND claim_epoch=?
                       AND reconciled_claim_epoch=claim_epoch
                       AND repo_identity=? AND worktree_identity=?
                       AND expected_head_sha=? AND change_set_digest=?
                       AND requested_ceiling=? AND reached_ceiling=?
                       AND canonical_pr_number=? AND canonical_pr_url=?
                       AND canonical_head_sha=expected_head_sha
                       AND next_action_authorized=0
                       AND lease_owner IS NULL AND lease_expires_at IS NULL",
                )
                .bind(&candidate.run_id)
                .bind(&decision.objective_id)
                .bind(&candidate.stage)
                .bind(candidate.claim_epoch)
                .bind(&candidate.repo_identity)
                .bind(&candidate.worktree_identity)
                .bind(&candidate.expected_head_sha)
                .bind(&candidate.change_set_digest)
                .bind(&candidate.requested_ceiling)
                .bind(&candidate.reached_ceiling)
                .bind(candidate.canonical_pr_number)
                .bind(&candidate.canonical_pr_url)
                .fetch_one(&mut *tx)
                .await?;
                exact_terminal
            } else {
                sqlx::query(
                    "UPDATE delivery_runs
                 SET status='completed', next_action=NULL, next_action_authorized=0,
                     wait_class=NULL, failure_signature=NULL, failure_code=NULL,
                     failure_class=NULL, remediation_id=NULL,
                     business_decision_key=NULL, decision_options_json=NULL,
                     recommended_option=NULL, safe_default_action=NULL,
                     decision_reason=NULL, core_input_request_key=NULL,
                     core_inputs_json=NULL, core_input_attempts_json=NULL,
                     core_input_resume_stage=NULL, core_input_request_count=0,
                     lease_owner=NULL, lease_expires_at=NULL,
                     last_progress_at=?, progress_revision=progress_revision+1,
                     updated_at=?
                 WHERE id=? AND objective_id=?
                   AND run_kind='deliver_changes'
                   AND status='awaiting_completion_arbitration'
                   AND stage=? AND claim_epoch=?
                   AND reconciled_claim_epoch=claim_epoch
                   AND repo_identity=? AND worktree_identity=?
                   AND expected_head_sha=? AND change_set_digest=?
                   AND requested_ceiling=? AND reached_ceiling=?
                   AND canonical_pr_number=? AND canonical_pr_url=?
                   AND canonical_head_sha=expected_head_sha
                   AND next_action_authorized=1
                   AND (? IS NULL OR (lease_owner=? AND lease_expires_at>?))
                   AND COALESCE(next_action, '')=''
                   AND COALESCE(failure_signature, '')=''
                   AND COALESCE(wait_class, 'none')='none'
                   AND EXISTS (
                     SELECT 1 FROM objectives AS objective
                     WHERE objective.id=delivery_runs.objective_id
                       AND objective.delivery_run_id=delivery_runs.id
                   )
                   AND EXISTS (
                     SELECT 1 FROM objective_bindings AS binding
                     WHERE binding.id=?
                       AND binding.objective_id=delivery_runs.objective_id
                       AND binding.resource_generation=?
                       AND (
                         (binding.resource_kind='chat_root_turn'
                          AND delivery_runs.root_turn_id=binding.resource_id)
                         OR
                         (binding.resource_kind='task_run'
                          AND delivery_runs.task_id=binding.resource_id)
                       )
                   )
                   AND NOT EXISTS (
                     SELECT 1 FROM delivery_mutation_intents AS intent
                     WHERE intent.run_id=delivery_runs.id
                       AND intent.status IN ('started','unknown')
                   )",
                )
                .bind(now)
                .bind(now)
                .bind(&candidate.run_id)
                .bind(&decision.objective_id)
                .bind(&candidate.stage)
                .bind(candidate.claim_epoch)
                .bind(&candidate.repo_identity)
                .bind(&candidate.worktree_identity)
                .bind(&candidate.expected_head_sha)
                .bind(&candidate.change_set_digest)
                .bind(&candidate.requested_ceiling)
                .bind(&candidate.reached_ceiling)
                .bind(candidate.canonical_pr_number)
                .bind(&candidate.canonical_pr_url)
                .bind(&candidate.takeover_lease_owner)
                .bind(&candidate.takeover_lease_owner)
                .bind(now)
                .bind(&candidate.binding_id)
                .bind(candidate.resource_generation)
                .execute(&mut *tx)
                .await?
                .rows_affected() as i64
            };
            if completed_rows != 1 {
                bail!("DeliveryRun completion proof changed during Objective arbitration");
            }
            if !candidate.already_terminal {
                sqlx::query(
                    "INSERT INTO delivery_run_events
                 (id, run_id, event_kind, stage, status, wait_class,
                  detail_json, process_instance, created_at)
                 VALUES (?, ?, 'objective_completed', ?, 'completed', ?, ?, ?, ?)",
                )
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(&candidate.run_id)
                .bind(&candidate.stage)
                .bind(Option::<String>::None)
                .bind(
                    serde_json::json!({
                        "objective_id": decision.objective_id,
                        "objective_revision": decision.revision,
                        "binding_id": authoritative_binding_id,
                        "action_signature": candidate.action_signature,
                        "reconciliation_receipt_id": candidate.reconciliation_receipt_id,
                    })
                    .to_string(),
                )
                .bind(&process_instance)
                .bind(now)
                .execute(&mut *tx)
                .await?;
            }
        }

        if let Some(evidence) = &decision.evidence {
            sqlx::query(
                "INSERT INTO objective_evidence
                 (id, objective_id, revision, kind, scope, digest, evidence_ref,
                  observed_at, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&evidence.id)
            .bind(&decision.objective_id)
            .bind(decision.revision)
            .bind(evidence.kind.as_str())
            .bind(&evidence.scope)
            .bind(&evidence.digest)
            .bind(&evidence.evidence_ref)
            .bind(evidence.observed_at)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }

        let prior_remediation_status = if decision.status == ObjectiveStatus::Completed {
            "completed"
        } else if decision.status == ObjectiveStatus::Cancelled {
            "cancelled"
        } else {
            "superseded"
        };
        sqlx::query(
            "UPDATE objective_remediations SET status=?, lease_owner=NULL,
               lease_expires_at=NULL, last_progress_at=?, updated_at=?
             WHERE objective_id=?
               AND status NOT IN ('completed','cancelled','superseded')",
        )
        .bind(prior_remediation_status)
        .bind(now)
        .bind(now)
        .bind(&decision.objective_id)
        .execute(&mut *tx)
        .await?;

        if decision.status == ObjectiveStatus::WaitingSystem
            && !decision.is_parked_system_incident()
            && !decision.transport_probe_wait
        {
            let strategy = if decision.domain == RecoveryDomain::Update
                && decision.failure_code.as_deref() == Some(UPDATE_SAFE_POINT_PENDING)
            {
                "wait_for_restart_safe_point"
            } else {
                "reconcile_then_resume"
            };
            // Push a repeating signature further out. Without this every repeat
            // is scheduled at the caller's flat +5s, so five identical errors —
            // under a minute of a transient condition — exhaust the budget and
            // park the Objective for good.
            let repeat_signature = decision
                .failure_signature
                .as_deref()
                .ok_or_else(|| anyhow!("waiting_system failure signature missing"))?;
            let prior_attempts: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM objective_remediations
                 WHERE objective_id=? AND failure_signature=?
                   AND recovery_generation=(
                     SELECT recovery_generation FROM objectives WHERE id=?
                   )
                   AND created_at>=?",
            )
            .bind(&decision.objective_id)
            .bind(repeat_signature)
            .bind(&decision.objective_id)
            .bind(now - SIGNATURE_RECOVERY_WINDOW_MS)
            .fetch_one(&mut *tx)
            .await?;
            let requested_observation = decision
                .next_observation_at
                .ok_or_else(|| anyhow!("waiting_system next observation missing"))?;
            // Only a *repeat* backs off. The first occurrence keeps whatever the
            // caller asked for, so ordinary recovery stays as responsive as it
            // was and nothing that observes immediately is delayed. U33/R12: the
            // backoff ladder is clamped, so a parked session always gets an
            // attempt within MAX_WAIT_BEFORE_NEXT_ATTEMPT_MS.
            let scheduled_observation = if prior_attempts > 0 {
                bounded_next_observation_at(
                    now,
                    requested_observation.max(
                        now + recovery_backoff_ms_for(decision.failure_code.as_deref(), prior_attempts),
                    ),
                )
            } else {
                requested_observation
            };
            sqlx::query(
                "INSERT INTO objective_remediations
                 (id, objective_id, binding_id, domain, status, failure_code, failure_signature,
                  strategy, approach_index, attempt_index, recovery_generation, resume_cursor,
                  next_observation_at, created_at, updated_at)
                 VALUES (?, ?, ?, ?, 'queued', ?, ?, ?, 0, 0,
                         (SELECT recovery_generation FROM objectives WHERE id=?), ?, ?, ?, ?)",
            )
            .bind(
                decision
                    .remediation_id
                    .as_deref()
                    .ok_or_else(|| anyhow!("waiting_system remediation id missing"))?,
            )
            .bind(&decision.objective_id)
            .bind(&authoritative_binding_id)
            .bind(decision.domain.as_str())
            .bind(
                decision
                    .failure_code
                    .as_deref()
                    .ok_or_else(|| anyhow!("waiting_system failure code missing"))?,
            )
            .bind(
                decision
                    .failure_signature
                    .as_deref()
                    .ok_or_else(|| anyhow!("waiting_system failure signature missing"))?,
            )
            .bind(strategy)
            .bind(&decision.objective_id)
            .bind(&decision.resume_cursor)
            .bind(scheduled_observation)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }

        let envelope_json = serde_json::to_string(&decision)?;
        sqlx::query(
            "INSERT INTO objective_decisions
             (id, objective_id, revision, domain, decision_type, failure_code,
              failure_signature, recovery_owner, remediation_id,
              requires_user_action, output_started, side_effect_started,
              envelope_json, evidence_ref, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&decision.objective_id)
        .bind(decision.revision)
        .bind(decision.domain.as_str())
        .bind(decision.decision_type.as_str())
        .bind(&decision.failure_code)
        .bind(&decision.failure_signature)
        .bind(&decision.recovery_owner)
        .bind(&decision.remediation_id)
        .bind(i64::from(decision.requires_user_action))
        .bind(i64::from(decision.output_started))
        .bind(i64::from(decision.side_effect_started))
        .bind(envelope_json)
        .bind(&evidence_ref)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO objective_events
             (id, objective_id, revision, event_type, status, decision_type,
              domain, failure_code, recovery_owner, created_at)
             VALUES (?, ?, ?, 'decision_applied', ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&decision.objective_id)
        .bind(decision.revision)
        .bind(decision.status.as_str())
        .bind(decision.decision_type.as_str())
        .bind(decision.domain.as_str())
        .bind(&decision.failure_code)
        .bind(&decision.recovery_owner)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        self.get(&decision.objective_id)
            .await?
            .ok_or_else(|| anyhow!("updated objective disappeared"))
    }
}

impl ObjectiveStore {
    /// Structured inputs for the user-visible failure report: which approaches
    /// were tried and why each stopped, plus what survived in the execution
    /// workspace and any PR already opened for it. Step two — "automatically
    /// try a different approach" — consumes this same structure instead of
    /// parsing prose.
    async fn build_failure_summary(
        &self,
        current: &ObjectiveSnapshot,
        decision: &DecisionEnvelope,
    ) -> Option<String> {
        let attempts = self
            .collect_attempt_summaries(&decision.objective_id, decision.revision)
            .await;
        let work = self.collect_preserved_work(current).await;
        let goal = current.requested_acceptance.clone();
        // U1b: when the change is already delivered and the only thing the
        // system could not establish is that the checks were rerun, telling the
        // user "这件事没做成" is false. State the delivery and name what is
        // unconfirmed instead.
        if let Some(delivered) = self.delivered_unverified(current, decision, &work).await {
            return Some(crate::agent::failure_summary::build_delivered_unverified_report(
                &goal, &work, &delivered,
            ));
        }
        match crate::agent::failure_summary::build_failure_report(
            &goal,
            attempts.clone(),
            work.clone(),
        ) {
            Ok(text) => Some(text),
            // The goal text comes from the user and may itself trip the
            // vocabulary guard. The report must still be delivered, so retry
            // with an unnamed goal instead of dropping the explanation.
            Err(_) => {
                crate::agent::failure_summary::build_failure_report("", attempts, work).ok()
            }
        }
    }

    async fn collect_attempt_summaries(
        &self,
        objective_id: &str,
        upto_revision: i64,
    ) -> Vec<crate::agent::failure_summary::AttemptSummary> {
        let rows: Vec<(String, String, i64)> = sqlx::query_as(
            "SELECT COALESCE(failure_code, ''), domain, COUNT(*)
             FROM objective_decisions
             WHERE objective_id=? AND revision<?
             GROUP BY failure_code, domain
             ORDER BY MIN(created_at), failure_code, domain",
        )
        .bind(objective_id)
        .bind(upto_revision)
        .fetch_all(&self.pool)
        .await
        .unwrap_or_default();
        rows.into_iter()
            .map(|(code, domain, attempts)| crate::agent::failure_summary::AttemptSummary {
                approach: crate::agent::failure_summary::plain_approach(Some(domain.as_str())),
                attempts,
                reason: crate::agent::failure_summary::plain_failure_reason(
                    (!code.is_empty()).then_some(code.as_str()),
                ),
            })
            .collect()
    }

    async fn collect_preserved_work(
        &self,
        current: &ObjectiveSnapshot,
    ) -> crate::agent::failure_summary::PreservedWork {
        let mut work = crate::agent::failure_summary::PreservedWork::default();
        if let Some(session_id) = current.session_id.as_deref() {
            if let Ok(Some(view)) =
                crate::agent::execution_workspace::latest_for_session(&self.pool, session_id).await
            {
                let root = std::path::PathBuf::from(&view.worktree_path);
                let (changes, total) = workspace_changes(&root, &view.base_sha);
                work.location = Some(view.worktree_path.clone());
                work.branch = Some(view.branch_name.clone());
                work.changes = changes;
                work.total_changed_files = total;
            }
        }
        if let Some((url, status)) = self.latest_delivery_pr(&current.id).await {
            work.pr_url = Some(url);
            work.pr_state = Some(delivery_state_label(&status).to_string());
        }
        work
    }

    /// U1b: `Some(delivery)` only when the failure that ended this Objective was
    /// the completion gate refusing to confirm the checks *and* the work already
    /// landed somewhere the user can open. Everything else keeps the honest
    /// failure terminal unchanged.
    async fn delivered_unverified(
        &self,
        current: &ObjectiveSnapshot,
        decision: &DecisionEnvelope,
        work: &crate::agent::failure_summary::PreservedWork,
    ) -> Option<crate::agent::failure_summary::DeliveredUnverified> {
        let gate = self.latest_completion_gate_verdict(&current.id).await;
        let verification_was_the_gap = gate
            .as_ref()
            .is_some_and(|verdict| verdict.0 == COMPLETION_EVIDENCE_INCOMPLETE)
            || decision.failure_code.as_deref() == Some(COMPLETION_EVIDENCE_INCOMPLETE)
            || current.failure_code.as_deref() == Some(COMPLETION_EVIDENCE_INCOMPLETE);
        if !verification_was_the_gap {
            return None;
        }
        let reference = self.delivered_reference(current, work).await?;
        Some(crate::agent::failure_summary::DeliveredUnverified {
            reference,
            on_pull_request: work.pr_url.is_some(),
            checks: gate.map(|verdict| verdict.1).unwrap_or_default(),
        })
    }

    /// The delivery that already landed for this Objective: a canonical PR when
    /// one is recorded, otherwise commits the execution workspace already
    /// carries beyond its baseline. `None` means nothing has been delivered, so
    /// the failure terminal must stay a real failure.
    async fn delivered_reference(
        &self,
        current: &ObjectiveSnapshot,
        work: &crate::agent::failure_summary::PreservedWork,
    ) -> Option<String> {
        if let Some(url) = work.pr_url.as_deref() {
            return Some(
                pull_request_reference(url).unwrap_or_else(|| "已经开好的 PR".to_string()),
            );
        }
        let session_id = current.session_id.as_deref()?;
        let view = crate::agent::execution_workspace::latest_for_session(&self.pool, session_id)
            .await
            .ok()
            .flatten()?;
        let root = std::path::PathBuf::from(&view.worktree_path);
        if workspace_commits_ahead(&root, &view.base_sha) <= 0 {
            return None;
        }
        Some(match work.branch.as_deref() {
            Some(branch) if !branch.is_empty() => format!("分支 {branch}"),
            _ => "当前工作区".to_string(),
        })
    }

    /// The latest completion-gate verdict for this Objective: the verdict plus
    /// the structured unmet checks, as written by R1 into
    /// `objective_events.detail_json`. Best-effort and schema-tolerant, exactly
    /// like the ledger reads beside it: a database without the table simply has
    /// no verdict to report.
    async fn latest_completion_gate_verdict(
        &self,
        objective_id: &str,
    ) -> Option<(String, Vec<crate::agent::failure_summary::UnmetCheck>)> {
        let detail: String = sqlx::query_scalar(
            "SELECT detail_json FROM objective_events
             WHERE objective_id=? AND event_type='completion_gate_verdict'
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(objective_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()?;
        let parsed: serde_json::Value = serde_json::from_str(&detail).ok()?;
        let verdict = parsed
            .get("verdict")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        if verdict.is_empty() {
            return None;
        }
        let checks = parsed
            .get("blockers")
            .and_then(serde_json::Value::as_array)
            .map(|blockers| {
                blockers
                    .iter()
                    .map(|blocker| crate::agent::failure_summary::UnmetCheck {
                        message: json_string(blocker, "message"),
                        check: json_optional_string(blocker, "check"),
                        command: json_optional_string(blocker, "command"),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some((verdict, checks))
    }

    /// The PR a previous delivery attempt already opened, when both the schema
    /// and the row carry it. Best-effort and schema-tolerant by design: an
    /// older database without the column must still produce a report.
    async fn latest_delivery_pr(&self, objective_id: &str) -> Option<(String, String)> {
        let has_table: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='delivery_runs'",
        )
        .fetch_one(&self.pool)
        .await
        .ok()?;
        if has_table != 1 {
            return None;
        }
        let has_column: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('delivery_runs')
             WHERE name='canonical_pr_url'",
        )
        .fetch_one(&self.pool)
        .await
        .ok()?;
        if has_column != 1 {
            return None;
        }
        sqlx::query_as::<_, (String, String)>(
            "SELECT canonical_pr_url, status FROM delivery_runs
             WHERE objective_id=? AND canonical_pr_url IS NOT NULL
             ORDER BY updated_at DESC LIMIT 1",
        )
        .bind(objective_id)
        .fetch_optional(&self.pool)
        .await
        .ok()
        .flatten()
    }
}

/// Uncommitted changes in an execution workspace relative to its baseline,
/// plus files that are not tracked yet. Read-only git calls; a missing or
/// non-repository path yields an empty list rather than failing the settlement
/// that is reporting on it.
fn workspace_changes(
    root: &std::path::Path,
    base_sha: &str,
) -> (Vec<crate::agent::failure_summary::PreservedChange>, i64) {
    use crate::util::no_window::NoWindow;
    use std::process::Command;

    let mut changes = Vec::new();
    let mut numstat = Command::new("git").no_window();
    numstat
        .arg("-C")
        .arg(root)
        .args(["diff", "--numstat"])
        .arg(base_sha);
    if let Ok(output) = numstat.output()
    {
        if output.status.success() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let mut parts = line.splitn(3, '\t');
                let added = parts.next().unwrap_or("0");
                let removed = parts.next().unwrap_or("0");
                let path = parts.next().unwrap_or("").trim();
                if path.is_empty() {
                    continue;
                }
                changes.push(crate::agent::failure_summary::PreservedChange {
                    path: path.to_string(),
                    added: added.parse().unwrap_or(0),
                    removed: removed.parse().unwrap_or(0),
                    untracked: false,
                });
            }
        }
    }
    let mut untracked = Command::new("git").no_window();
    untracked
        .arg("-C")
        .arg(root)
        .args(["ls-files", "--others", "--exclude-standard"]);
    if let Ok(output) = untracked.output()
    {
        if output.status.success() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let path = line.trim();
                if path.is_empty() {
                    continue;
                }
                changes.push(crate::agent::failure_summary::PreservedChange {
                    path: path.to_string(),
                    added: 0,
                    removed: 0,
                    untracked: true,
                });
            }
        }
    }
    let total = changes.len() as i64;
    (changes, total)
}

/// Commits the execution workspace already carries beyond its baseline (U1b).
/// Read-only, and a missing or non-repository path counts as zero: a delivery
/// claim is only ever made on positive evidence.
fn workspace_commits_ahead(root: &std::path::Path, base_sha: &str) -> i64 {
    use crate::util::no_window::NoWindow;
    use std::process::Command;
    if base_sha.trim().is_empty() || !root.is_dir() {
        return 0;
    }
    let output = Command::new("git")
        .no_window()
        .arg("-C")
        .arg(root)
        .args(["rev-list", "--count", &format!("{base_sha}..HEAD")])
        .output();
    match output {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap_or(0),
        _ => 0,
    }
}

/// `PR #572` for a GitHub pull-request URL, matching the wording the run's own
/// delivery reference uses (U1b). `None` for anything that is not one.
fn pull_request_reference(text: &str) -> Option<String> {
    for marker in ["/pull/", "/pulls/"] {
        let mut offset = 0;
        while let Some(index) = text[offset..].find(marker) {
            let start = offset + index + marker.len();
            let digits: String = text[start..]
                .chars()
                .take_while(|character| character.is_ascii_digit())
                .collect();
            if !digits.is_empty() {
                return Some(format!("PR #{digits}"));
            }
            offset = start;
        }
    }
    None
}

/// `detail_json` string field, absent or mistyped fields collapsing to empty.
fn json_string(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `detail_json` optional string field, with blank values treated as absent.
fn json_optional_string(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// User-facing wording for a delivery run status.
fn delivery_state_label(status: &str) -> &'static str {
    match status {
        "completed" | "merged" | "released" => "已经完成",
        "failed" => "这一步没成",
        "cancelled" => "已经取消",
        _ => "还在进行中",
    }
}

/// Extend the persisted `objectives.status` vocabulary with the terminal
/// `failed` value.
///
/// SQLite cannot widen an existing CHECK constraint in place, so the table is
/// rebuilt from its own live DDL with exactly one clause rewritten. The whole
/// routine is guarded by reading `sqlite_master` first, which makes it a no-op
/// on every startup whose schema is already widened — and keeps it correct for
/// databases that were created by an older release, a test fixture, or a
/// hand-built schema.
pub async fn ensure_objective_failed_status(pool: &SqlitePool) -> crate::errors::Result<()> {
    let ddl: Option<String> = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='objectives'",
    )
    .fetch_optional(pool)
    .await?;
    let Some(ddl) = ddl else {
        return Ok(());
    };
    if ddl.contains("'failed'") {
        return Ok(());
    }
    let marker = "'cancelled', 'legacy_orphan'";
    let Some(position) = ddl.find("CREATE TABLE") else {
        return Ok(());
    };
    if !ddl.contains(marker) {
        return Ok(());
    }
    let (head, tail) = ddl.split_at(position);
    let rebuilt = format!(
        "{head}{}",
        tail.replacen("objectives", "objectives_failed_rebuild", 1)
            .replace(marker, "'cancelled', 'failed', 'legacy_orphan'")
    );
    let indexes: Vec<String> = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master
         WHERE type='index' AND tbl_name='objectives' AND sql IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;

    let mut conn = pool.acquire().await?;
    // Both PRAGMAs are connection-scoped and must be set outside a transaction.
    // `foreign_keys=OFF` keeps the dependent tables from cascading away during
    // the drop/rename; `legacy_alter_table=ON` stops SQLite (>= 3.26) from
    // re-validating every trigger in the schema on RENAME — the schema-wide
    // check aborts on `trg_permission_intents_scope_insert`, which legitimately
    // references `objectives` at the moment the rename runs.
    sqlx::query("PRAGMA foreign_keys=OFF")
        .execute(&mut *conn)
        .await?;
    sqlx::query("PRAGMA legacy_alter_table=ON")
        .execute(&mut *conn)
        .await?;

    // One transaction around the whole rebuild, so a failure at any step (most
    // realistically the index re-creation) leaves the original table exactly as
    // it was. Without this the durable state was a lone `objectives_failed_rebuild`
    // table and the next start silently created an empty `objectives`, losing
    // every task the user had.
    let rebuilt_result = async {
        sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
        let step = async {
            sqlx::query(&rebuilt).execute(&mut *conn).await?;
            sqlx::query("INSERT INTO objectives_failed_rebuild SELECT * FROM objectives")
                .execute(&mut *conn)
                .await?;
            sqlx::query("DROP TABLE objectives").execute(&mut *conn).await?;
            sqlx::query("ALTER TABLE objectives_failed_rebuild RENAME TO objectives")
                .execute(&mut *conn)
                .await?;
            for index in &indexes {
                sqlx::query(index).execute(&mut *conn).await?;
            }
            let violations = sqlx::query("PRAGMA foreign_key_check")
                .fetch_all(&mut *conn)
                .await?;
            if !violations.is_empty() {
                return Err(sqlx::Error::Protocol(
                    "objective status migration broke a foreign key reference".into(),
                ));
            }
            Ok::<(), sqlx::Error>(())
        }
        .await;
        match step {
            Ok(()) => {
                sqlx::query("COMMIT").execute(&mut *conn).await?;
                Ok(())
            }
            Err(error) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
                Err(error)
            }
        }
    }
    .await;

    // Restore the connection's normal contract before it goes back to the pool.
    let _ = sqlx::query("PRAGMA legacy_alter_table=OFF")
        .execute(&mut *conn)
        .await;
    let _ = sqlx::query("PRAGMA foreign_keys=ON")
        .execute(&mut *conn)
        .await;
    rebuilt_result?;
    Ok(())
}

/// Install the unified Objective schema even when a historical database has a
/// conflicting sqlx migration version/checksum. The DDL is intentionally the
/// same file used by fresh installs and is safe to execute on every startup.
pub async fn ensure_schema(pool: &SqlitePool) -> crate::errors::Result<()> {
    sqlx::raw_sql(include_str!(
        "../../migrations/0007_unified_objective_control_plane.sql"
    ))
    .execute(pool)
    .await?;
    sqlx::raw_sql(include_str!("../../migrations/0009_chat_run_controls.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../../migrations/0010_permission_intents.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!(
        "../../migrations/0011_provider_auth_recovery.sql"
    ))
    .execute(pool)
    .await?;
    sqlx::raw_sql(include_str!(
        "../../migrations/0013_context_recovery_intents.sql"
    ))
    .execute(pool)
    .await?;
    sqlx::raw_sql(include_str!(
        "../../migrations/0014_browser_recovery_contracts.sql"
    ))
    .execute(pool)
    .await?;
    sqlx::raw_sql(include_str!(
        "../../migrations/0015_tool_recovery_contracts.sql"
    ))
    .execute(pool)
    .await?;
    sqlx::raw_sql(include_str!(
        "../../migrations/0016_chat_session_cancel_intents.sql"
    ))
    .execute(pool)
    .await?;
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS objective_incidents (
           id TEXT PRIMARY KEY,
           objective_id TEXT NOT NULL UNIQUE REFERENCES objectives(id) ON DELETE CASCADE,
           status TEXT NOT NULL CHECK(status IN ('open','resolved')),
           failure_code TEXT NOT NULL,
           failure_signature TEXT,
           owner TEXT NOT NULL,
           resume_cursor TEXT,
           opened_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL,
           resolved_at INTEGER
         );
         CREATE INDEX IF NOT EXISTS idx_objective_incidents_status
           ON objective_incidents(status, updated_at);",
    )
    .execute(pool)
    .await?;
    sqlx::raw_sql(
        "CREATE TABLE IF NOT EXISTS recovery_capabilities (
           domain TEXT PRIMARY KEY,
           revision INTEGER NOT NULL CHECK(revision >= 1),
           contract_digest TEXT NOT NULL,
           executable INTEGER NOT NULL CHECK(executable IN (0, 1)),
           updated_at INTEGER NOT NULL
         );",
    )
    .execute(pool)
    .await?;
    ensure_column(
        pool,
        "objective_incidents",
        "domain",
        "TEXT NOT NULL DEFAULT 'chat'",
    )
    .await?;
    ensure_column(
        pool,
        "objective_incidents",
        "blocked_capability_revision",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    ensure_column(
        pool,
        "objective_incidents",
        "reactivation_status",
        "TEXT NOT NULL DEFAULT 'waiting_capability'",
    )
    .await?;
    ensure_column(
        pool,
        "objective_incidents",
        "reactivation_count",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    ensure_column(
        pool,
        "objective_incidents",
        "last_reactivated_revision",
        "INTEGER",
    )
    .await?;
    ensure_column(
        pool,
        "objective_remediations",
        "execution_attempt_index",
        "INTEGER NOT NULL DEFAULT 0",
    )
    .await?;
    // Runs after the DDL above so a database created by an older release (or a
    // fixture carrying the narrow status vocabulary) is widened even though
    // `CREATE TABLE IF NOT EXISTS` left it untouched.
    ensure_objective_failed_status(pool).await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_objective_incidents_reactivation
         ON objective_incidents(status, reactivation_status, domain,
                                blocked_capability_revision, updated_at)",
    )
    .execute(pool)
    .await?;
    ensure_column(
        pool,
        "objectives",
        "recovery_generation",
        "INTEGER NOT NULL DEFAULT 0 CHECK(recovery_generation >= 0)",
    )
    .await?;
    ensure_column(
        pool,
        "objective_remediations",
        "recovery_generation",
        "INTEGER NOT NULL DEFAULT 0 CHECK(recovery_generation >= 0)",
    )
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_objective_remediations_generation
         ON objective_remediations(objective_id, recovery_generation, failure_signature)",
    )
    .execute(pool)
    .await?;

    // Preserve historical duplicate rows as immutable audit evidence, while
    // ratcheting every upgraded database so no new cross-revision duplicate
    // can be admitted. A unique index is still installed for clean databases
    // below, but it cannot be created when legacy duplicates already exist.
    sqlx::query(
        "CREATE TRIGGER IF NOT EXISTS trg_side_effect_receipts_idempotency_ratchet
         BEFORE INSERT ON side_effect_receipts
         WHEN EXISTS (
             SELECT 1 FROM side_effect_receipts
             WHERE objective_id = NEW.objective_id
               AND idempotency_key = NEW.idempotency_key
         )
         BEGIN
             SELECT RAISE(ABORT, 'duplicate side-effect receipt idempotency key');
         END",
    )
    .execute(pool)
    .await?;

    let duplicate_idempotency_groups: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM (
             SELECT objective_id, idempotency_key
             FROM side_effect_receipts
             GROUP BY objective_id, idempotency_key
             HAVING COUNT(*) > 1
         )",
    )
    .fetch_one(pool)
    .await?;
    if duplicate_idempotency_groups == 0 {
        sqlx::query(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_side_effect_receipts_objective_idempotency
             ON side_effect_receipts(objective_id, idempotency_key)",
        )
        .execute(pool)
        .await?;
    } else {
        tracing::error!(
            duplicate_groups = duplicate_idempotency_groups,
            "historical side-effect receipts violate cross-revision idempotency; preserved rows, installed the insert ratchet, and skipped the unique index"
        );
    }

    // Compatibility projections are nullable and therefore safe for old
    // readers. Identity is linked only by ObjectiveStore after it has proved a
    // unique immutable chain; schema setup never guesses old ownership.
    for (table, column, column_type) in [
        ("chat_turn_state", "objective_id", "TEXT"),
        ("chat_turn_state", "turn_settled_at", "INTEGER"),
        ("chat_turn_state", "stream_closed_at", "INTEGER"),
        ("chat_turn_state", "terminal_revision", "INTEGER"),
        ("chat_turn_state", "visible_final_message_id", "TEXT"),
        ("chat_turn_state", "visible_final_kind", "TEXT"),
        ("chat_turn_state", "next_action", "TEXT"),
        ("chat_turn_state", "objective_revision", "INTEGER"),
        ("objectives", "attention_request_json", "TEXT"),
        ("task_runs", "objective_id", "TEXT"),
        ("task_runs", "recovery_state", "TEXT"),
        ("task_runs", "next_observation_at", "INTEGER"),
        ("task_attempts", "objective_id", "TEXT"),
        ("tool_calls", "objective_id", "TEXT"),
        ("tool_calls", "binding_id", "TEXT"),
        ("tool_calls", "action_signature", "TEXT"),
        (
            "tool_calls",
            "resource_generation",
            "INTEGER NOT NULL DEFAULT 1",
        ),
        ("delivery_runs", "objective_id", "TEXT"),
    ] {
        ensure_column(pool, table, column, column_type).await?;
    }

    for (table, statement) in [
        (
            "chat_turn_state",
            "CREATE INDEX IF NOT EXISTS idx_chat_turn_state_objective ON chat_turn_state(objective_id)",
        ),
        (
            "task_runs",
            "CREATE INDEX IF NOT EXISTS idx_task_runs_objective ON task_runs(objective_id)",
        ),
        (
            "task_attempts",
            "CREATE INDEX IF NOT EXISTS idx_task_attempts_objective ON task_attempts(objective_id)",
        ),
        (
            "tool_calls",
            "CREATE INDEX IF NOT EXISTS idx_tool_calls_objective_binding
             ON tool_calls(objective_id, binding_id, resource_generation)",
        ),
    ] {
        if table_exists(pool, table).await? {
            sqlx::query(statement).execute(pool).await?;
        }
    }
    Ok(())
}

async fn table_exists(pool: &SqlitePool, table: &str) -> crate::errors::Result<bool> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?")
            .bind(table)
            .fetch_one(pool)
            .await?;
    Ok(count > 0)
}

async fn ensure_column(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    column_type: &str,
) -> crate::errors::Result<()> {
    let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
        .fetch_all(pool)
        .await?;
    if rows.is_empty() {
        return Ok(());
    }
    let exists = rows.iter().any(|row| {
        row.try_get::<String, _>("name")
            .map(|name| name == column)
            .unwrap_or(false)
    });
    if !exists {
        sqlx::query(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {column_type}"
        ))
        .execute(pool)
        .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use codefactory_agent_core::CompletionEvidence;
    use codefactory_agent_loop::run::{RunOutcome, StopReason};
    use sqlx::sqlite::SqlitePoolOptions;

    async fn pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        ensure_schema(&pool).await.unwrap();
        pool
    }

    /// The settlement projection a real app carries. `ensure_schema` installs
    /// the Objective control plane only; sessions, messages and chat_turn_state
    /// come from the storage migrations, so tests that exercise settlement build
    /// exactly the columns those paths read and write.
    async fn settlement_tables(pool: &SqlitePool) {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS sessions (
               id TEXT PRIMARY KEY, title TEXT NOT NULL, cwd TEXT NOT NULL,
               model_id TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL)",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS messages (
               id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT NOT NULL,
               content TEXT NOT NULL, completion_state TEXT, created_at INTEGER NOT NULL)",
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS chat_turn_state (
               root_turn_id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
               revision INTEGER NOT NULL DEFAULT 1, phase TEXT NOT NULL DEFAULT 'working',
               status TEXT NOT NULL, recent_activity_kind TEXT, recent_activity_label TEXT,
               waiting_reason TEXT, updated_at INTEGER NOT NULL DEFAULT 0, completed_at INTEGER,
               terminal_reason TEXT, turn_settled_at INTEGER, stream_closed_at INTEGER,
               terminal_revision INTEGER, objective_revision INTEGER,
               visible_final_message_id TEXT, visible_final_kind TEXT, next_action TEXT,
               objective_id TEXT)",
        )
        .execute(pool)
        .await
        .unwrap();
    }

    /// B1 fixture: the `objectives` DDL an already-installed release wrote
    /// (status CHECK without `'failed'`), the two indexes that ship with it, and
    /// the `permission_intents` trigger that references `objectives` by name.
    /// Synthetic rows only.
    const LEGACY_OBJECTIVES_DDL: &str = "\
CREATE TABLE objectives (
    id TEXT PRIMARY KEY,
    revision INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL CHECK(status IN (
        'active', 'waiting_system', 'completed', 'cancelled', 'legacy_orphan')),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
)";

    const LEGACY_OBJECTIVES_ROWS: i64 = 3;

    async fn legacy_objectives_fixture_with_triggers() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(LEGACY_OBJECTIVES_DDL)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE INDEX idx_objectives_due ON objectives(status, updated_at)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE INDEX idx_objectives_session ON objectives(created_at)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE objective_bindings (
               id TEXT PRIMARY KEY,
               objective_id TEXT NOT NULL,
               resource_generation INTEGER NOT NULL DEFAULT 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE permission_intents (
               intent_id TEXT PRIMARY KEY,
               objective_id TEXT NOT NULL,
               objective_revision INTEGER NOT NULL,
               binding_id TEXT NOT NULL,
               resource_generation INTEGER NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        // Verbatim from the installed permission migration: this trigger's
        // reference to `objectives` is what made a bare ALTER TABLE ... RENAME
        // abort with `no such table: main.objectives`.
        sqlx::query(
            "CREATE TRIGGER trg_permission_intents_scope_insert
             BEFORE INSERT ON permission_intents
             WHEN NOT EXISTS (
                    SELECT 1 FROM objectives
                    WHERE id=NEW.objective_id AND revision=NEW.objective_revision
                      AND status NOT IN ('completed','cancelled','legacy_orphan')
                  )
               OR NOT EXISTS (
                    SELECT 1 FROM objective_bindings
                    WHERE id=NEW.binding_id AND objective_id=NEW.objective_id
                      AND resource_generation=NEW.resource_generation
                  )
             BEGIN
                 SELECT RAISE(ABORT, 'stale permission Objective scope');
             END",
        )
        .execute(&pool)
        .await
        .unwrap();
        for index in 0..LEGACY_OBJECTIVES_ROWS {
            let stamp = 1_700_000_000_000i64 + index;
            sqlx::query(
                "INSERT INTO objectives (id, revision, status, created_at, updated_at)
                 VALUES (?, 1, 'active', ?, ?)",
            )
            .bind(format!("objective-legacy-{index}"))
            .bind(stamp)
            .bind(stamp)
            .execute(&pool)
            .await
            .unwrap();
        }
        pool
    }

    /// B1: on a database that already has the permission triggers, rebuilding
    /// `objectives` used to stop at the rename and leave only a temporary table
    /// behind — the next start then created an empty `objectives` and every task
    /// the user had was gone. The migration must widen the vocabulary, keep the
    /// rows, keep the triggers, and rebuild the indexes.
    #[tokio::test]
    async fn legacy_objectives_ddl_gains_the_failed_status_without_losing_rows() {
        let pool = legacy_objectives_fixture_with_triggers().await;

        ensure_objective_failed_status(&pool).await.unwrap();

        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM objectives")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, LEGACY_OBJECTIVES_ROWS, "every task row must survive");
        let leftovers: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='objectives_failed_rebuild'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(leftovers, 0, "no temporary table may survive the migration");
        let indexes: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type='index' AND tbl_name='objectives' AND name LIKE 'idx_objectives_%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(indexes, 2, "both indexes must be recreated");
        let triggers: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type='trigger' AND name='trg_permission_intents_scope_insert'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(triggers, 1, "the dependent trigger must survive the rebuild");
        let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(integrity, "ok");

        // The widened vocabulary is the contract the honest-failure terminal
        // depends on: the status the whole design writes must be accepted.
        sqlx::query("UPDATE objectives SET status='failed' WHERE id='objective-legacy-0'")
            .execute(&pool)
            .await
            .unwrap();
        let status: String = sqlx::query_scalar("SELECT status FROM objectives WHERE id=?")
            .bind("objective-legacy-0")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "failed");

        // Every later start is a no-op rather than a second rebuild.
        ensure_objective_failed_status(&pool).await.unwrap();
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM objectives")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, LEGACY_OBJECTIVES_ROWS);
    }

    /// B1: an in-transaction failure rolls the whole rebuild back. The failure
    /// is injected with a real foreign-key violation, which lands on the same
    /// `foreign_key_check` gate production uses — after create, insert, drop and
    /// rename have already run.
    #[tokio::test]
    async fn a_failed_objectives_rebuild_rolls_back_and_keeps_the_original_table() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(LEGACY_OBJECTIVES_DDL)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE INDEX idx_objectives_due ON objectives(status, updated_at)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE permission_intents (
               intent_id TEXT PRIMARY KEY,
               objective_id TEXT NOT NULL REFERENCES objectives(id),
               objective_revision INTEGER NOT NULL,
               binding_id TEXT NOT NULL,
               resource_generation INTEGER NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        for index in 0..LEGACY_OBJECTIVES_ROWS {
            let stamp = 1_700_000_000_000i64 + index;
            sqlx::query(
                "INSERT INTO objectives (id, revision, status, created_at, updated_at)
                 VALUES (?, 1, 'active', ?, ?)",
            )
            .bind(format!("objective-legacy-{index}"))
            .bind(stamp)
            .bind(stamp)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query("PRAGMA foreign_keys=OFF")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO permission_intents
             (intent_id, objective_id, objective_revision, binding_id, resource_generation)
             VALUES ('intent-orphan', 'objective-that-never-existed', 1, 'binding-orphan', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let error = ensure_objective_failed_status(&pool).await.unwrap_err();
        assert!(
            error.to_string().contains("foreign key"),
            "the foreign-key gate must be the injected failure: {error}"
        );

        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM objectives")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, LEGACY_OBJECTIVES_ROWS, "the original table must be intact");
        let leftovers: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='objectives_failed_rebuild'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(leftovers, 0, "a rollback must not leave a temporary table");
        let ddl: String =
            sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE type='table' AND name='objectives'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(
            !ddl.contains("'failed'"),
            "a rolled back migration must keep the pre-migration contract: {ddl}"
        );
        assert!(
            sqlx::query("UPDATE objectives SET status='failed' WHERE id='objective-legacy-0'")
                .execute(&pool)
                .await
                .is_err(),
            "the restored table must still reject the value the reboot would have accepted"
        );
    }

    /// S1 follow-up (#555): the *whole* startup convergence of a stalled task
    /// must leave the session's `updated_at` alone — not only the notice
    /// backfill. Settling a row a previous process left behind is bookkeeping,
    /// not user activity, so it must not drag an old conversation to the top of
    /// the sidebar.
    ///
    /// The seeded time is an hour in the past on purpose: a stray write then
    /// reproduces on every platform instead of landing in the same millisecond
    /// by luck.
    #[tokio::test]
    async fn startup_convergence_of_a_stalled_task_never_touches_the_session_order() {
        async fn session_updated_at(pool: &sqlx::SqlitePool) -> i64 {
            sqlx::query_scalar("SELECT updated_at FROM sessions WHERE id='session-s1-followup'")
                .fetch_one(pool)
                .await
                .unwrap()
        }

        let pool = pool().await;
        settlement_tables(&pool).await;
        let store = ObjectiveStore::new(pool.clone());
        let session_id = "session-s1-followup";
        let suspended_at = Utc::now().timestamp_millis() - 60 * 60 * 1000;
        sqlx::query(
            "INSERT INTO sessions (id, title, cwd, model_id, created_at, updated_at)
             VALUES (?, '旧会话', '/tmp/s1-followup', 'model-synthetic', ?, ?)",
        )
        .bind(session_id)
        .bind(suspended_at)
        .bind(suspended_at)
        .execute(&pool)
        .await
        .unwrap();
        // The stalled task's own message dates from the suspension too, so the
        // boot-time activity backfill has nothing newer to pull forward and the
        // assertion below can only be moved by the convergence itself.
        sqlx::query(
            "INSERT OR IGNORE INTO messages (id, session_id, role, content, created_at)
             VALUES ('message-s1-followup', ?, 'user', '整理这个卡住的长任务', ?)",
        )
        .bind(session_id)
        .bind(suspended_at)
        .execute(&pool)
        .await
        .unwrap();
        let objective = store
            .create(CreateObjective {
                id: "objective-s1-followup".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some(session_id.into()),
                root_turn_id: Some("turn-s1-followup".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        // The pre-#553 shape: parked in `waiting_system` behind an open
        // incident that only a capability bump could clear, nothing queued, and
        // its turn still presenting itself as running.
        sqlx::query(
            "UPDATE objectives SET status='waiting_system', decision_type='failed_internal',
               failure_code=?, recovery_owner=?, resume_cursor=?, next_observation_at=NULL,
               lease_owner=NULL, lease_expires_at=NULL, updated_at=?
             WHERE id=?",
        )
        .bind(TECHNICAL_RECOVERY_EXHAUSTED)
        .bind(OBJECTIVE_INCIDENT_CONTROLLER)
        .bind("turn-s1-followup")
        .bind(suspended_at)
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objective_incidents
             (id, objective_id, status, failure_code, owner, opened_at, updated_at)
             VALUES ('incident-s1-followup', ?, 'open', ?, ?, ?, ?)",
        )
        .bind(&objective.id)
        .bind(TECHNICAL_RECOVERY_EXHAUSTED)
        .bind(OBJECTIVE_INCIDENT_CONTROLLER)
        .bind(suspended_at)
        .bind(suspended_at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE chat_turn_state
             SET status='waiting_system', next_action='await_system_recovery',
                 waiting_reason='await_system_recovery', terminal_reason=NULL
             WHERE objective_id=?",
        )
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();

        // Production startup order, one step at a time, so a stray write is
        // attributed to the step that made it rather than merely detected.
        let process_instance = current_process_instance();
        let mut steps: Vec<(&str, i64)> = Vec::new();
        let _ = store
            .reconcile_stale_chat_run_controls(&process_instance)
            .await
            .unwrap();
        steps.push((
            "reconcile_stale_chat_run_controls",
            session_updated_at(&pool).await,
        ));
        let reclassified = store
            .reclassify_synthetic_technical_handbacks()
            .await
            .unwrap();
        steps.push((
            "reclassify_synthetic_technical_handbacks",
            session_updated_at(&pool).await,
        ));
        store.sync_recovery_capabilities().await.unwrap();
        steps.push(("sync_recovery_capabilities", session_updated_at(&pool).await));
        let _ = store.reactivate_eligible_incidents(32).await.unwrap();
        steps.push((
            "reactivate_eligible_incidents",
            session_updated_at(&pool).await,
        ));
        let _ =
            crate::agent::objective_supervisor::reconcile_provider_recovery_on_startup(&pool)
                .await
                .unwrap();
        steps.push((
            "reconcile_provider_recovery_on_startup",
            session_updated_at(&pool).await,
        ));
        let _ = store
            .reconcile_stale_active_objectives(&process_instance)
            .await
            .unwrap();
        steps.push((
            "reconcile_stale_active_objectives",
            session_updated_at(&pool).await,
        ));
        let _ = store
            .claim_due_remediations("s1-followup", 8, 60_000)
            .await
            .unwrap();

        assert_eq!(reclassified, 1, "the stalled task must converge exactly once");
        let status: String = sqlx::query_scalar("SELECT status FROM objectives WHERE id=?")
            .bind(&objective.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            status, "failed",
            "the stalled task must reach the failed terminal"
        );
        let offender = steps
            .iter()
            .copied()
            .find(|(_, at)| *at != suspended_at)
            .unwrap_or(("none", suspended_at));
        assert_eq!(
            offender,
            ("none", suspended_at),
            "startup convergence is not user activity: sessions.updated_at must stay at the suspension time"
        );
    }

    /// S1: the startup backfill writes a notice, not user activity. Advancing
    /// `sessions.updated_at` dragged a long-dead conversation to the top of the
    /// sidebar; the notice must also carry the time the task actually stalled
    /// rather than the moment the app was opened.
    #[tokio::test]
    async fn startup_backfill_leaves_session_order_alone_and_dates_the_notice_when_the_task_stalled()
    {
        let pool = pool().await;
        settlement_tables(&pool).await;
        let store = ObjectiveStore::new(pool.clone());
        let session_id = "session-s1-backfill";
        let suspended_at = Utc::now().timestamp_millis() - 3 * 24 * 60 * 60 * 1000;
        sqlx::query(
            "INSERT INTO sessions (id, title, cwd, model_id, created_at, updated_at)
             VALUES (?, '旧会话', '/tmp/s1-backfill', 'model-synthetic', ?, ?)",
        )
        .bind(session_id)
        .bind(suspended_at)
        .bind(suspended_at)
        .execute(&pool)
        .await
        .unwrap();
        let objective = store
            .create(CreateObjective {
                id: "objective-s1-backfill".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some(session_id.into()),
                root_turn_id: Some("turn-s1-backfill".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        sqlx::query(
            "UPDATE objectives SET status='waiting_system', decision_type='failed_internal',
               failure_code=?, recovery_owner=?, resume_cursor=?, next_observation_at=NULL,
               lease_owner=NULL, lease_expires_at=NULL, updated_at=?
             WHERE id=?",
        )
        .bind(TECHNICAL_RECOVERY_EXHAUSTED)
        .bind(OBJECTIVE_INCIDENT_CONTROLLER)
        .bind("turn-s1-backfill")
        .bind(suspended_at)
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objective_incidents
             (id, objective_id, status, failure_code, owner, opened_at, updated_at)
             VALUES ('incident-s1-backfill', ?, 'open', ?, ?, ?, ?)",
        )
        .bind(&objective.id)
        .bind(TECHNICAL_RECOVERY_EXHAUSTED)
        .bind(OBJECTIVE_INCIDENT_CONTROLLER)
        .bind(suspended_at)
        .bind(suspended_at)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, session_id, status, next_action, objective_id)
             VALUES ('turn-s1-backfill', ?, 'waiting_system', 'await_system_recovery', ?)",
        )
        .bind(session_id)
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            store.reclassify_synthetic_technical_handbacks().await.unwrap(),
            1
        );

        let session_updated: i64 = sqlx::query_scalar("SELECT updated_at FROM sessions WHERE id=?")
            .bind(session_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            session_updated, suspended_at,
            "a startup backfill is not user activity and must not reorder the sidebar"
        );
        let (role, created_at): (String, i64) = sqlx::query_as(
            "SELECT role, created_at FROM messages
             WHERE session_id=? ORDER BY created_at DESC, rowid DESC LIMIT 1",
        )
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_ne!(role, "user", "the notice is never attributed to the user");
        assert_eq!(
            created_at, suspended_at,
            "the notice is dated when the task stalled, not when the app started"
        );
        let status: String = sqlx::query_scalar("SELECT status FROM objectives WHERE id=?")
            .bind(&objective.id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(status, "failed");
    }

    /// S2: `failed` is terminal, so receipts that can never be answered are
    /// swept exactly like the ones on completed or cancelled work — while a live
    /// Objective keeps the "we do not know whether this landed" question.
    #[tokio::test]
    async fn receipts_on_a_failed_objective_are_swept_as_terminal() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let failed = store
            .create(CreateObjective {
                id: "objective-s2-failed-receipts".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-s2-failed-receipts".into()),
                root_turn_id: Some("turn-s2-failed-receipts".into()),
                domain: RecoveryDomain::Tool,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let live = store
            .create(CreateObjective {
                id: "objective-s2-live-receipts".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-s2-live-receipts".into()),
                root_turn_id: Some("turn-s2-live-receipts".into()),
                domain: RecoveryDomain::Tool,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = Utc::now().timestamp_millis();
        for (receipt_id, objective_id) in [
            ("receipt-s2-failed", &failed.id),
            ("receipt-s2-live", &live.id),
        ] {
            sqlx::query(
                "INSERT INTO side_effect_receipts
                 (id, objective_id, revision, action_fingerprint, idempotency_key,
                  status, created_at, observed_at)
                 VALUES (?, ?, 1, 'sha256:s2-action', ?, 'started', ?, ?)",
            )
            .bind(receipt_id)
            .bind(objective_id)
            .bind(format!("sha256:{receipt_id}"))
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "UPDATE objectives SET status='failed', decision_type='failed_internal',
               failure_code=?, completed_at=?, updated_at=? WHERE id=?",
        )
        .bind(TECHNICAL_RECOVERY_EXHAUSTED)
        .bind(now)
        .bind(now)
        .bind(&failed.id)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            store.cancel_receipts_on_terminal_objectives().await.unwrap(),
            1,
            "only the receipt on the terminal objective is decided"
        );
        let decided: String =
            sqlx::query_scalar("SELECT status FROM side_effect_receipts WHERE id='receipt-s2-failed'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(decided, "cancelled");
        let untouched: String =
            sqlx::query_scalar("SELECT status FROM side_effect_receipts WHERE id='receipt-s2-live'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            untouched, "started",
            "a live objective must keep its unresolved receipt"
        );
    }

    /// S3: the failure notice promises "直接回复就能在这些成果上继续". The next
    /// message in that session reopens the same Objective in place, on the
    /// execution workspace it already owned, and leaves nothing for it to queue
    /// behind.
    #[tokio::test]
    async fn a_failed_objective_reopens_in_place_on_the_same_workspace_without_queueing() {
        let pool = pool().await;
        settlement_tables(&pool).await;
        let store = ObjectiveStore::new(pool.clone());
        let session_id = "session-s3-reopen";
        let objective = store
            .create(CreateObjective {
                id: "objective-s3-reopen".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some(session_id.into()),
                root_turn_id: Some("turn-s3-old".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id, resource_generation,
              identity_digest, created_at, updated_at)
             VALUES ('binding-s3-workspace', ?, 'chat', 'execution_workspace',
                     'workspace-s3', 1, 'sha256:s3-workspace', ?, ?)",
        )
        .bind(&objective.id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        // Objective failed, its turn projected as a finished turn with nothing
        // left for system recovery.
        sqlx::query(
            "UPDATE objectives SET status='failed', decision_type='failed_internal',
               failure_code=?, recovery_generation=1, requires_user_action=0,
               completed_at=?, updated_at=? WHERE id=?",
        )
        .bind(TECHNICAL_RECOVERY_EXHAUSTED)
        .bind(now)
        .bind(now)
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, session_id, status, next_action, objective_id)
             VALUES ('turn-s3-old', ?, 'completed', NULL, ?)",
        )
        .bind(session_id)
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, session_id, status, next_action, objective_id)
             VALUES ('turn-s3-new', ?, 'active', NULL, NULL)",
        )
        .bind(session_id)
        .execute(&pool)
        .await
        .unwrap();

        let reopened = store
            .ensure_or_continue_chat_objective(
                session_id,
                "turn-s3-new",
                Some("turn-s3-old"),
                ObjectiveKind::LocalMutation,
                "validated_change",
            )
            .await
            .unwrap();

        assert_eq!(
            reopened.id, objective.id,
            "the next sentence must continue the same task, not open a second one"
        );
        assert_eq!(reopened.status, ObjectiveStatus::Active);
        assert_eq!(
            reopened.recovery_generation, 2,
            "reopening a failed Objective is a new recovery generation"
        );
        assert_eq!(reopened.failure_code, None);
        let workspace: String = sqlx::query_scalar(
            "SELECT resource_id FROM objective_bindings
             WHERE objective_id=? AND resource_kind='execution_workspace'",
        )
        .bind(&objective.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            workspace, "workspace-s3",
            "the work stays in the workspace it was already using"
        );
        let pending: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM chat_turn_state
             WHERE session_id=? AND status IN ('active','waiting_system')
               AND next_action='await_system_recovery'",
        )
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            pending, 0,
            "nothing may be left that the user's next message has to queue behind"
        );
        let bound: String =
            sqlx::query_scalar("SELECT objective_id FROM chat_turn_state WHERE root_turn_id='turn-s3-new'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(bound, objective.id);
    }

    /// β fixture 6 — the wording constraint is audited durably, and the record
    /// says explicitly that it had no capability effect.
    #[tokio::test]
    async fn a_turn_wording_constraint_is_recorded_without_a_capability_effect() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-turn-wording-constraint".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-turn-wording-constraint".into()),
                root_turn_id: Some("turn-turn-wording-constraint".into()),
                domain: RecoveryDomain::Tool,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();

        let payload = crate::agent::constraint_audit_payload("先分析一下，不要改代码")
            .expect("a read-only sentence yields an audit payload");
        store
            .record_turn_wording_constraint(&objective.id, &payload)
            .await;

        let (event_type, detail): (String, String) = sqlx::query_as(
            "SELECT event_type, detail_json FROM objective_events \
             WHERE objective_id = ? AND event_type = 'turn_wording_constraint'",
        )
        .bind(&objective.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(event_type, "turn_wording_constraint");
        let detail: serde_json::Value = serde_json::from_str(&detail).unwrap();
        assert_eq!(detail["capability_effect"], "none");
    }

    #[tokio::test]
    async fn fresh_schema_enforces_cross_revision_side_effect_receipt_idempotency() {
        let pool = pool().await;
        let index_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type='index'
               AND name='idx_side_effect_receipts_objective_idempotency'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(index_exists, 1);

        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-cross-revision-idempotency".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-cross-revision-idempotency".into()),
                root_turn_id: Some("turn-cross-revision-idempotency".into()),
                domain: RecoveryDomain::Tool,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, revision, action_fingerprint, idempotency_key,
              status, created_at, observed_at)
             VALUES (?, ?, 1, 'sha256:first-action', 'sha256:stable-key',
                     'committed', ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&objective.id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let duplicate = sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, revision, action_fingerprint, idempotency_key,
              status, created_at, observed_at)
             VALUES (?, ?, 2, 'sha256:second-action', 'sha256:stable-key',
                     'committed', ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&objective.id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await;
        assert!(duplicate.is_err());
    }

    #[tokio::test]
    async fn historical_duplicate_receipts_are_preserved_but_new_duplicates_are_rejected() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(include_str!(
            "../../migrations/0007_unified_objective_control_plane.sql"
        ))
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-historical-duplicate-ratchet".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-historical-duplicate-ratchet".into()),
                root_turn_id: Some("turn-historical-duplicate-ratchet".into()),
                domain: RecoveryDomain::Tool,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = Utc::now().timestamp_millis();
        for (id, revision, action) in [
            ("historical-receipt-a", 1_i64, "sha256:action-a"),
            ("historical-receipt-b", 2_i64, "sha256:action-b"),
        ] {
            sqlx::query(
                "INSERT INTO side_effect_receipts
                 (id, objective_id, revision, action_fingerprint, idempotency_key,
                  status, created_at, observed_at)
                 VALUES (?, ?, ?, ?, 'sha256:historical-stable-key',
                         'committed', ?, ?)",
            )
            .bind(id)
            .bind(&objective.id)
            .bind(revision)
            .bind(action)
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }

        ensure_schema(&pool).await.unwrap();
        let preserved: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM side_effect_receipts
             WHERE objective_id=? AND idempotency_key='sha256:historical-stable-key'",
        )
        .bind(&objective.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(preserved, 2, "schema repair must not delete audit evidence");

        let third = sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, revision, action_fingerprint, idempotency_key,
              status, created_at, observed_at)
             VALUES ('historical-receipt-c', ?, 3, 'sha256:action-c',
                     'sha256:historical-stable-key', 'committed', ?, ?)",
        )
        .bind(&objective.id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await;
        assert!(
            third.is_err(),
            "legacy duplicates may remain for audit, but the schema ratchet must reject new ones"
        );
    }

    const CURRENT_ACTION_SIGNATURE: &str = "sha256:current-action";

    struct GenericReceiptCompletionFixture {
        pool: SqlitePool,
        store: ObjectiveStore,
        objective: ObjectiveSnapshot,
        evidence: ObjectiveEvidence,
        old_binding_id: String,
        current_binding_id: String,
        now: i64,
    }

    async fn generic_receipt_completion_fixture(test_id: &str) -> GenericReceiptCompletionFixture {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE tool_calls (
               id TEXT PRIMARY KEY,
               objective_id TEXT,
               tool_name TEXT NOT NULL,
               status TEXT NOT NULL,
               binding_id TEXT,
               action_signature TEXT,
               resource_generation INTEGER NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: format!("objective-{test_id}"),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some(format!("session-{test_id}")),
                root_turn_id: Some(format!("turn-{test_id}")),
                domain: RecoveryDomain::Tool,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = Utc::now().timestamp_millis();
        let old_binding_id = format!("binding-{test_id}-generation-1");
        let current_binding_id = format!("binding-{test_id}-generation-2");
        for (binding_id, generation, side_effect_started) in [
            (old_binding_id.as_str(), 1_i64, 0_i64),
            (current_binding_id.as_str(), 2_i64, 1_i64),
        ] {
            sqlx::query(
                "INSERT INTO objective_bindings
                 (id, objective_id, domain, resource_kind, resource_id,
                  resource_generation, identity_digest, side_effect_started,
                  created_at, updated_at)
                 VALUES (?, ?, 'tool', 'chat_root_turn', ?, ?, ?, ?, ?, ?)",
            )
            .bind(binding_id)
            .bind(&objective.id)
            .bind(format!("turn-{test_id}"))
            .bind(generation)
            .bind(format!("sha256:binding-{test_id}-{generation}"))
            .bind(side_effect_started)
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query("UPDATE objectives SET side_effect_started=1 WHERE id=?")
            .bind(&objective.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO tool_calls
             (id, objective_id, tool_name, status, binding_id,
              action_signature, resource_generation)
             VALUES (?, ?, 'bash', 'done', ?, ?, 2)",
        )
        .bind(format!("tool-{test_id}"))
        .bind(&objective.id)
        .bind(&current_binding_id)
        .bind(CURRENT_ACTION_SIGNATURE)
        .execute(&pool)
        .await
        .unwrap();
        let objective = store.get(&objective.id).await.unwrap().unwrap();
        assert!(objective.side_effect_started);
        let evidence = ObjectiveEvidence {
            id: format!("evidence-{test_id}"),
            kind: EvidenceKind::CurrentStateAcceptance,
            scope: format!("turn-{test_id}"),
            digest: "sha256:validated-current-state".into(),
            evidence_ref: format!("db:test-validation/{test_id}"),
            observed_at: now,
            reached_acceptance: "validated_change".into(),
        };

        GenericReceiptCompletionFixture {
            pool,
            store,
            objective,
            evidence,
            old_binding_id,
            current_binding_id,
            now,
        }
    }

    async fn insert_generic_receipt(
        fixture: &GenericReceiptCompletionFixture,
        receipt_id: &str,
        objective_id: &str,
        binding_id: &str,
        action_signature: &str,
        status: &str,
    ) {
        sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, binding_id, revision, action_fingerprint,
              idempotency_key, status, created_at, observed_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(receipt_id)
        .bind(objective_id)
        .bind(binding_id)
        .bind(fixture.objective.revision)
        .bind(action_signature)
        .bind(format!("sha256:key-{receipt_id}"))
        .bind(status)
        .bind(fixture.now)
        .bind(fixture.now)
        .execute(&fixture.pool)
        .await
        .unwrap();
    }

    fn completion_decision(fixture: &GenericReceiptCompletionFixture) -> DecisionEnvelope {
        CompletionArbiter::decide(&fixture.objective, &[fixture.evidence.clone()]).unwrap()
    }

    #[test]
    fn decision_router_never_hands_a_technical_failure_to_the_user() {
        let objective = ObjectiveSnapshot::new(
            "objective-1",
            ObjectiveKind::LocalMutation,
            RecoveryDomain::Tool,
            "local_validation",
        );
        let decision = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Tool,
                failure_code: "tool_timeout".into(),
                failure_signature: "bash:test:timeout".into(),
                next_observation_at: 1_723_000_030_000,
                resume_cursor: Some("tool-call-7".into()),
            },
        )
        .unwrap();

        assert_eq!(decision.decision_type, DecisionType::Waiting);
        assert_eq!(decision.status, ObjectiveStatus::WaitingSystem);
        assert!(!decision.requires_user_action);
        assert!(decision.recovery_owner.is_some());
        assert!(decision.remediation_id.is_some());
    }

    #[test]
    fn decision_router_requires_typed_fields_for_real_user_attention() {
        let objective = ObjectiveSnapshot::new(
            "objective-2",
            ObjectiveKind::Delivery,
            RecoveryDomain::Auth,
            "release",
        );
        let missing_request_key = DecisionRouter::route(
            &objective,
            RouteSignal::CoreInputRequired {
                domain: RecoveryDomain::Auth,
                request_key: "".into(),
                missing_inputs: vec!["oauth_login".into()],
                attempted_routes: vec!["refresh_token".into()],
                resume_cursor: Some("model-boundary-3".into()),
            },
        );
        assert!(missing_request_key.is_err());

        let decision = DecisionRouter::route(
            &objective,
            RouteSignal::AuthorizationRequired {
                domain: RecoveryDomain::Permission,
                request_key: "permission:publish:42".into(),
                action_signature: "publish:repo:head".into(),
                resume_cursor: Some("tool-call-42".into()),
            },
        )
        .unwrap();
        assert_eq!(decision.status, ObjectiveStatus::WaitingAuthorization);
        assert_eq!(decision.decision_type, DecisionType::AuthorizationRequired);
        assert!(decision.requires_user_action);
        assert_eq!(
            decision.action_signature.as_deref(),
            Some("publish:repo:head")
        );
    }

    #[tokio::test]
    async fn objective_store_uses_revision_cas_and_completion_arbiter_evidence() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-3".into(),
                kind: ObjectiveKind::Informational,
                session_id: Some("session-1".into()),
                root_turn_id: Some("turn-1".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "answer".into(),
                created_surface: "project_chat".into(),
            })
            .await
            .unwrap();

        let waiting = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Provider,
                failure_code: "provider_503".into(),
                failure_signature: "endpoint-a:503".into(),
                next_observation_at: 1_723_000_030_000,
                resume_cursor: Some("model-boundary-1".into()),
            },
        )
        .unwrap();
        let revised = store.apply_decision(1, waiting).await.unwrap();
        assert_eq!(revised.revision, 2);
        assert_eq!(revised.status, ObjectiveStatus::WaitingSystem);
        assert!(store
            .apply_decision(1, revised.as_decision())
            .await
            .is_err());

        let waiting_again = DecisionRouter::route(
            &revised,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Provider,
                failure_code: "provider_503".into(),
                failure_signature: "endpoint-b:503".into(),
                next_observation_at: 1_723_000_060_000,
                resume_cursor: Some("model-boundary-2".into()),
            },
        )
        .unwrap();
        let revised = store.apply_decision(2, waiting_again).await.unwrap();
        assert_eq!(revised.revision, 3);
        let active_remediations: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_remediations
             WHERE objective_id=? AND status NOT IN ('completed','cancelled','superseded')",
        )
        .bind(&revised.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(active_remediations, 1);

        let no_evidence = CompletionArbiter::decide(&revised, &[]);
        assert!(no_evidence.is_err());
        let evidence = ObjectiveEvidence {
            id: "evidence-1".into(),
            kind: EvidenceKind::InformationalAnswer,
            scope: "turn-1".into(),
            digest: "sha256:answer".into(),
            evidence_ref: "db:messages/assistant-1".into(),
            observed_at: 1_723_000_040_000,
            reached_acceptance: "answer".into(),
        };
        let complete = CompletionArbiter::decide(&revised, &[evidence]).unwrap();
        let completed = store.apply_decision(3, complete).await.unwrap();
        assert_eq!(completed.status, ObjectiveStatus::Completed);
        assert!(completed.evidence_ref.is_some());
        let active_remediations: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_remediations
             WHERE objective_id=? AND status NOT IN ('completed','cancelled','superseded')",
        )
        .bind(&completed.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(active_remediations, 0);
    }

    #[tokio::test]
    async fn completion_fails_closed_while_receipt_or_tool_call_is_unresolved() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE tool_calls (
               id TEXT PRIMARY KEY, objective_id TEXT, status TEXT NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-unresolved-effect".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-unresolved-effect".into()),
                root_turn_id: Some("turn-unresolved-effect".into()),
                domain: RecoveryDomain::Tool,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, revision, action_fingerprint, idempotency_key,
              status, created_at, observed_at)
             VALUES ('receipt-unresolved', ?, 1, 'sha256:action', 'sha256:key',
                     'started', ?, ?)",
        )
        .bind(&objective.id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tool_calls(id, objective_id, status)
             VALUES ('tool-unresolved', ?, 'pending')",
        )
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        let evidence = ObjectiveEvidence {
            id: "evidence-unresolved-effect".into(),
            kind: EvidenceKind::CurrentStateAcceptance,
            scope: "turn-unresolved-effect".into(),
            digest: "sha256:validation".into(),
            evidence_ref: "db:test-validation".into(),
            observed_at: now,
            reached_acceptance: "validated_change".into(),
        };
        let complete = CompletionArbiter::decide(&objective, &[evidence]).unwrap();

        assert!(store
            .apply_decision(objective.revision, complete.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("unresolved side-effect receipt"));
        sqlx::query(
            "UPDATE side_effect_receipts SET status='committed' WHERE id='receipt-unresolved'",
        )
        .execute(&pool)
        .await
        .unwrap();
        assert!(store
            .apply_decision(objective.revision, complete.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("unresolved tool call"));
        sqlx::query("UPDATE tool_calls SET status='done' WHERE id='tool-unresolved'")
            .execute(&pool)
            .await
            .unwrap();
        let completed = store
            .apply_decision(objective.revision, complete)
            .await
            .unwrap();
        assert_eq!(completed.status, ObjectiveStatus::Completed);
    }

    #[tokio::test]
    async fn completion_rejects_side_effect_started_without_current_binding_receipt() {
        let fixture = generic_receipt_completion_fixture("zero-receipt").await;

        assert!(fixture
            .store
            .apply_decision(fixture.objective.revision, completion_decision(&fixture))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn completion_rejects_committed_receipt_owned_by_another_objective() {
        let fixture = generic_receipt_completion_fixture("wrong-objective").await;
        let other_objective = fixture
            .store
            .create(CreateObjective {
                id: "objective-receipt-owner".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-receipt-owner".into()),
                root_turn_id: Some("turn-receipt-owner".into()),
                domain: RecoveryDomain::Tool,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let other_binding_id = "binding-receipt-owner";
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, side_effect_started,
              created_at, updated_at)
             VALUES (?, ?, 'tool', 'chat_root_turn', 'turn-receipt-owner', 1,
                     'sha256:binding-receipt-owner', 1, ?, ?)",
        )
        .bind(other_binding_id)
        .bind(&other_objective.id)
        .bind(fixture.now)
        .bind(fixture.now)
        .execute(&fixture.pool)
        .await
        .unwrap();
        insert_generic_receipt(
            &fixture,
            "receipt-wrong-objective",
            &other_objective.id,
            other_binding_id,
            CURRENT_ACTION_SIGNATURE,
            "committed",
        )
        .await;

        assert!(fixture
            .store
            .apply_decision(fixture.objective.revision, completion_decision(&fixture))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn completion_rejects_committed_receipt_from_prior_binding_generation() {
        let fixture = generic_receipt_completion_fixture("old-binding-generation").await;
        insert_generic_receipt(
            &fixture,
            "receipt-old-binding-generation",
            &fixture.objective.id,
            &fixture.old_binding_id,
            CURRENT_ACTION_SIGNATURE,
            "committed",
        )
        .await;

        assert!(fixture
            .store
            .apply_decision(fixture.objective.revision, completion_decision(&fixture))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn completion_rejects_committed_receipt_for_wrong_action_signature() {
        let fixture = generic_receipt_completion_fixture("wrong-action").await;
        insert_generic_receipt(
            &fixture,
            "receipt-wrong-action",
            &fixture.objective.id,
            &fixture.current_binding_id,
            "sha256:different-action",
            "committed",
        )
        .await;

        assert!(fixture
            .store
            .apply_decision(fixture.objective.revision, completion_decision(&fixture))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn completion_accepts_matching_current_binding_committed_or_reconciled_receipt() {
        for status in ["committed", "reconciled"] {
            let fixture =
                generic_receipt_completion_fixture(&format!("current-receipt-{status}")).await;
            insert_generic_receipt(
                &fixture,
                &format!("receipt-current-{status}"),
                &fixture.objective.id,
                &fixture.current_binding_id,
                CURRENT_ACTION_SIGNATURE,
                status,
            )
            .await;

            let completed = fixture
                .store
                .apply_decision(fixture.objective.revision, completion_decision(&fixture))
                .await
                .unwrap();
            assert_eq!(completed.status, ObjectiveStatus::Completed, "{status}");
        }
    }

    #[tokio::test]
    async fn completion_accepts_cancelled_receipt_only_for_a_failed_tool_call() {
        let fixture = generic_receipt_completion_fixture("cancelled-rejected-command").await;
        sqlx::query("UPDATE tool_calls SET status='error' WHERE objective_id=?")
            .bind(&fixture.objective.id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        insert_generic_receipt(
            &fixture,
            "receipt-cancelled-rejected-command",
            &fixture.objective.id,
            &fixture.current_binding_id,
            CURRENT_ACTION_SIGNATURE,
            "cancelled",
        )
        .await;

        let completed = fixture
            .store
            .apply_decision(fixture.objective.revision, completion_decision(&fixture))
            .await
            .unwrap();
        assert_eq!(completed.status, ObjectiveStatus::Completed);

        let done_fixture = generic_receipt_completion_fixture("cancelled-done-command").await;
        insert_generic_receipt(
            &done_fixture,
            "receipt-cancelled-done-command",
            &done_fixture.objective.id,
            &done_fixture.current_binding_id,
            CURRENT_ACTION_SIGNATURE,
            "cancelled",
        )
        .await;
        assert!(done_fixture
            .store
            .apply_decision(
                done_fixture.objective.revision,
                completion_decision(&done_fixture),
            )
            .await
            .is_err());
    }

    async fn persist_delivery_completion_candidate(
        fixture: &GenericReceiptCompletionFixture,
        run_id: &str,
        reached_ceiling: &str,
    ) {
        crate::agent::delivery_run::ensure_schema(&fixture.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE tool_calls SET tool_name='deliver_changes' WHERE objective_id=?")
            .bind(&fixture.objective.id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        let process = crate::agent::delivery_run::ProcessIdentity::new(
            format!("process-{run_id}"),
            "test",
            "test",
        );
        let run = crate::agent::delivery_run::NewDeliveryRun {
            id: run_id.into(),
            objective_id: fixture.objective.id.clone(),
            run_kind: "deliver_changes".into(),
            session_id: fixture.objective.session_id.clone(),
            root_turn_id: fixture.objective.root_turn_id.clone(),
            task_segment_id: None,
            task_id: None,
            workspace_path: format!("/tmp/{run_id}"),
            worktree_identity: format!("worktree-{run_id}"),
            repo_identity: format!("repo-{run_id}"),
            base_branch: "main".into(),
            head_branch: format!("codex/{run_id}"),
            change_set_digest: format!("sha256:change-set-{run_id}"),
            expected_head_sha: format!("head-{run_id}"),
            canonical_pr_number: None,
            canonical_pr_url: None,
            canonical_head_sha: None,
            requested_ceiling: "through_release".into(),
            reached_ceiling: "local".into(),
            stage: "preflight".into(),
            status: "running".into(),
            wait_class: None,
            next_action: Some("deliver".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        let claim_epoch = crate::agent::delivery_run::create_delivery_run(
            &fixture.pool,
            &run,
            &process,
            fixture.now,
            90_000,
        )
        .await
        .unwrap();
        let pointer: String =
            sqlx::query_scalar("SELECT delivery_run_id FROM objectives WHERE id=?")
                .bind(&fixture.objective.id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
        assert_eq!(pointer, run_id);
        crate::agent::delivery_run::record_delivery_observation(
            &fixture.pool,
            run_id,
            &process,
            claim_epoch,
            &crate::agent::delivery_run::DeliveryObservation {
                head_branch: run.head_branch,
                stage: "complete".into(),
                status: "awaiting_completion_arbitration".into(),
                wait_class: Some("none".into()),
                next_action: None,
                reached_ceiling: reached_ceiling.into(),
                expected_head_sha: run.expected_head_sha.clone(),
                canonical_pr_number: Some(42),
                canonical_pr_url: Some("https://example.invalid/pull/42".into()),
                canonical_head_sha: Some(run.expected_head_sha),
                failure_signature: None,
                core_input: None,
                identity_revision: None,
            },
            fixture.now + 1,
            90_000,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn receipt_backed_delivery_completion_atomically_completes_run_and_objective() {
        let fixture = generic_receipt_completion_fixture("delivery-ledger-bridge").await;
        persist_delivery_completion_candidate(
            &fixture,
            "delivery-ledger-bridge-run",
            "live_verified",
        )
        .await;

        let completed = fixture
            .store
            .apply_decision(fixture.objective.revision, completion_decision(&fixture))
            .await
            .unwrap();
        assert_eq!(completed.status, ObjectiveStatus::Completed);
        let run_status: String = sqlx::query_scalar(
            "SELECT status FROM delivery_runs WHERE id='delivery-ledger-bridge-run'",
        )
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(run_status, "completed");
        let terminal_projection: (
            i64,
            Option<String>,
            Option<i64>,
            Option<String>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT next_action_authorized, lease_owner, lease_expires_at,
                        wait_class, failure_signature
                 FROM delivery_runs WHERE id='delivery-ledger-bridge-run'",
        )
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(terminal_projection, (0, None, None, None, None));
        let receipt: (String, String) = sqlx::query_as(
            "SELECT status, summary_json FROM side_effect_receipts
             WHERE objective_id=? AND binding_id=?
               AND action_fingerprint=?",
        )
        .bind(&fixture.objective.id)
        .bind(&fixture.current_binding_id)
        .bind(CURRENT_ACTION_SIGNATURE)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(receipt.0, "reconciled");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&receipt.1).unwrap()["delivery_run_id"],
            "delivery-ledger-bridge-run"
        );
        let completion_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_run_events
             WHERE run_id='delivery-ledger-bridge-run'
               AND event_kind='objective_completed' AND status='completed'",
        )
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(completion_events, 1);
    }

    async fn crash_left_delivery_fixture(
        test_id: &str,
        kind: ObjectiveKind,
    ) -> GenericReceiptCompletionFixture {
        let fixture = generic_receipt_completion_fixture(test_id).await;
        sqlx::query("ALTER TABLE tool_calls ADD COLUMN message_id TEXT")
            .execute(&fixture.pool)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE tool_calls ADD COLUMN result TEXT")
            .execute(&fixture.pool)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE tool_calls ADD COLUMN error TEXT")
            .execute(&fixture.pool)
            .await
            .unwrap();
        sqlx::query("ALTER TABLE tool_calls ADD COLUMN duration_ms INTEGER")
            .execute(&fixture.pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE messages (
               id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               role TEXT NOT NULL,
               content TEXT NOT NULL,
               created_at INTEGER NOT NULL
             )",
        )
        .execute(&fixture.pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE objectives
             SET kind=?, domain='delivery', requested_acceptance=?
             WHERE id=?",
        )
        .bind(kind.as_str())
        .bind(if kind == ObjectiveKind::Live {
            "live_verification"
        } else {
            "delivery_receipt"
        })
        .bind(&fixture.objective.id)
        .execute(&fixture.pool)
        .await
        .unwrap();
        persist_delivery_completion_candidate(&fixture, &format!("{test_id}-run"), "live_verified")
            .await;
        let assistant_message_id = format!("assistant-{test_id}");
        sqlx::query(
            "INSERT INTO messages(id, session_id, role, content, created_at)
             VALUES (?, ?, 'assistant', '', ?)",
        )
        .bind(&assistant_message_id)
        .bind(fixture.objective.session_id.as_deref().unwrap())
        .bind(fixture.now)
        .execute(&fixture.pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE tool_calls
             SET id=?, message_id=?, status='pending'
             WHERE objective_id=?",
        )
        .bind(format!(
            "{}:provider-delivery-before-crash",
            fixture.objective.session_id.as_deref().unwrap()
        ))
        .bind(&assistant_message_id)
        .bind(&fixture.objective.id)
        .execute(&fixture.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tool_calls
             (id, message_id, objective_id, tool_name, status, binding_id,
              action_signature, resource_generation)
             VALUES (?, ?, ?, 'deliver_changes', 'pending', ?, ?, 2)",
        )
        .bind(format!(
            "{}:provider-delivery-after-reprompt",
            fixture.objective.session_id.as_deref().unwrap()
        ))
        .bind(&assistant_message_id)
        .bind(&fixture.objective.id)
        .bind(&fixture.current_binding_id)
        .bind(CURRENT_ACTION_SIGNATURE)
        .execute(&fixture.pool)
        .await
        .unwrap();

        // The authoritative DeliveryRun pointer, not these legacy latches,
        // decides whether the completion bridge is mandatory.
        sqlx::query("UPDATE objectives SET side_effect_started=0 WHERE id=?")
            .bind(&fixture.objective.id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        sqlx::query("UPDATE objective_bindings SET side_effect_started=0 WHERE objective_id=?")
            .bind(&fixture.objective.id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        fixture
    }

    #[tokio::test]
    async fn takeover_terminalizes_crash_left_delivery_and_provider_replay_atomically() {
        let fixture =
            crash_left_delivery_fixture("delivery-crash-convergence", ObjectiveKind::Delivery)
                .await;
        let run_id = "delivery-crash-convergence-run";
        let claim_epoch: i64 =
            sqlx::query_scalar("SELECT claim_epoch FROM delivery_runs WHERE id=?")
                .bind(run_id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap();

        let completed = fixture
            .store
            .settle_reconciled_delivery_after_takeover(
                run_id,
                claim_epoch,
                &format!("process-{run_id}"),
            )
            .await
            .unwrap();
        assert_eq!(completed.status, ObjectiveStatus::Completed);
        let projection: (String, String, i64, i64) = sqlx::query_as(
            "SELECT objective.status, run.status,
                    run.next_action_authorized, run.autonomous_completion
             FROM objectives objective
             JOIN delivery_runs run ON run.id=objective.delivery_run_id
             WHERE objective.id=?",
        )
        .bind(&fixture.objective.id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(projection, ("completed".into(), "completed".into(), 0, 1));
        let calls: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT status, result, error FROM tool_calls
             WHERE objective_id=? ORDER BY id",
        )
        .bind(&fixture.objective.id)
        .fetch_all(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|(status, result, error)| {
            status == "done"
                && result
                    .as_deref()
                    .is_some_and(|value| value.contains(run_id))
                && error.is_none()
        }));
        let replay_messages: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM messages
             WHERE role='tool' AND content LIKE '%provider-delivery-%'
               AND content LIKE '%done%'",
        )
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(replay_messages, 2);
        let receipt_evidence: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_evidence
             WHERE objective_id=? AND kind='delivery_receipt'",
        )
        .bind(&fixture.objective.id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(receipt_evidence, 1);

        let revision = completed.revision;
        let events_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM delivery_run_events WHERE run_id=?")
                .bind(run_id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
        let replay_before: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE role='tool'")
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
        let idempotent = fixture
            .store
            .settle_reconciled_delivery_after_takeover(
                run_id,
                claim_epoch,
                &format!("process-{run_id}"),
            )
            .await
            .unwrap();
        assert_eq!(idempotent.revision, revision);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM delivery_run_events WHERE run_id=?")
                .bind(run_id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap(),
            events_before
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM messages WHERE role='tool'")
                .fetch_one(&fixture.pool)
                .await
                .unwrap(),
            replay_before
        );
    }

    #[tokio::test]
    async fn live_takeover_keeps_system_ownership_until_real_live_verification() {
        let fixture =
            crash_left_delivery_fixture("live-crash-convergence", ObjectiveKind::Live).await;
        let run_id = "live-crash-convergence-run";
        let claim_epoch: i64 =
            sqlx::query_scalar("SELECT claim_epoch FROM delivery_runs WHERE id=?")
                .bind(run_id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap();

        let waiting = fixture
            .store
            .settle_reconciled_delivery_after_takeover(
                run_id,
                claim_epoch,
                &format!("process-{run_id}"),
            )
            .await
            .unwrap();
        assert_eq!(waiting.status, ObjectiveStatus::WaitingSystem);
        assert!(!waiting.requires_user_action);
        assert_eq!(
            waiting.recovery_owner.as_deref(),
            Some("objective-supervisor:chat")
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT status FROM delivery_runs WHERE id=?")
                .bind(run_id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap(),
            "completed"
        );
        let evidence_kinds: Vec<String> = sqlx::query_scalar(
            "SELECT kind FROM objective_evidence WHERE objective_id=? ORDER BY kind",
        )
        .bind(&fixture.objective.id)
        .fetch_all(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(evidence_kinds, vec!["delivery_receipt"]);

        let live_evidence = ObjectiveEvidence {
            id: "live-observation-after-delivery".into(),
            kind: EvidenceKind::LiveVerification,
            scope: fixture.objective.root_turn_id.clone().unwrap(),
            digest: "sha256:real-live-observation".into(),
            evidence_ref: "live-observer:exact-public-artifact".into(),
            observed_at: fixture.now + 10,
            reached_acceptance: "live_verification".into(),
        };
        let decision = fixture
            .store
            .completion_decision_with_persisted_evidence(&waiting, vec![live_evidence])
            .await
            .unwrap();
        let completed = fixture
            .store
            .apply_decision(waiting.revision, decision)
            .await
            .unwrap();
        assert_eq!(completed.status, ObjectiveStatus::Completed);
        let delivery_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_run_events
             WHERE run_id=? AND event_kind='objective_completed'",
        )
        .bind(run_id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(delivery_events, 1, "later Live settlement is idempotent");
    }

    #[tokio::test]
    async fn takeover_delivery_completion_fails_closed_on_identity_or_authority_drift() {
        for (suffix, mutation, expected_error) in [
            ("wrong-binding", "wrong_binding", "current binding"),
            ("multi-action", "multi_action", "stable action signature"),
            ("stale-epoch", "stale_epoch", "claim epoch"),
            (
                "unresolved-intent",
                "unresolved_intent",
                "unresolved mutation intent",
            ),
            ("below-ceiling", "below_ceiling", "requested ceiling"),
            ("flags-zero", "flags_zero", "autonomous completion"),
        ] {
            let fixture = crash_left_delivery_fixture(
                &format!("delivery-crash-negative-{suffix}"),
                ObjectiveKind::Delivery,
            )
            .await;
            let run_id = format!("delivery-crash-negative-{suffix}-run");
            let mut claim_epoch: i64 =
                sqlx::query_scalar("SELECT claim_epoch FROM delivery_runs WHERE id=?")
                    .bind(&run_id)
                    .fetch_one(&fixture.pool)
                    .await
                    .unwrap();
            match mutation {
                "wrong_binding" => {
                    sqlx::query("UPDATE tool_calls SET binding_id=? WHERE objective_id=?")
                        .bind(&fixture.old_binding_id)
                        .bind(&fixture.objective.id)
                        .execute(&fixture.pool)
                        .await
                        .unwrap();
                }
                "multi_action" => {
                    sqlx::query(
                        "UPDATE tool_calls SET action_signature='sha256:second-action'
                         WHERE id LIKE '%after-reprompt'",
                    )
                    .execute(&fixture.pool)
                    .await
                    .unwrap();
                }
                "stale_epoch" => claim_epoch += 1,
                "unresolved_intent" => {
                    sqlx::query(
                        "INSERT INTO delivery_mutation_intents
                         (intent_id, run_id, claim_epoch, rung, operation_key, status,
                          process_instance, started_at, updated_at)
                         VALUES (?, ?, ?, 'provider_release_trigger', 'release:key',
                                 'unknown', 'crashed-process', ?, ?)",
                    )
                    .bind(format!("intent-{suffix}"))
                    .bind(&run_id)
                    .bind(claim_epoch)
                    .bind(fixture.now)
                    .bind(fixture.now)
                    .execute(&fixture.pool)
                    .await
                    .unwrap();
                }
                "below_ceiling" => {
                    sqlx::query("UPDATE delivery_runs SET reached_ceiling='merged' WHERE id=?")
                        .bind(&run_id)
                        .execute(&fixture.pool)
                        .await
                        .unwrap();
                }
                "flags_zero" => {
                    sqlx::query(
                        "UPDATE delivery_runs
                         SET next_action_authorized=0, autonomous_completion=0 WHERE id=?",
                    )
                    .bind(&run_id)
                    .execute(&fixture.pool)
                    .await
                    .unwrap();
                }
                _ => unreachable!(),
            }

            let error = fixture
                .store
                .settle_reconciled_delivery_after_takeover(
                    &run_id,
                    claim_epoch,
                    &format!("process-{run_id}"),
                )
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains(expected_error), "{suffix}: {error}");
            let state: (String, String) = sqlx::query_as(
                "SELECT objective.status, run.status
                 FROM objectives objective
                 JOIN delivery_runs run ON run.id=objective.delivery_run_id
                 WHERE objective.id=?",
            )
            .bind(&fixture.objective.id)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
            assert_eq!(
                state,
                ("active".into(), "awaiting_completion_arbitration".into()),
                "{suffix}"
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM side_effect_receipts WHERE objective_id=?"
                )
                .bind(&fixture.objective.id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap(),
                0,
                "{suffix}"
            );
            assert_eq!(
                sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM tool_calls
                     WHERE objective_id=? AND status='pending'"
                )
                .bind(&fixture.objective.id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap(),
                2,
                "{suffix}"
            );
        }
    }

    #[tokio::test]
    async fn takeover_refuses_to_hide_an_unrelated_pending_tool_call_and_rolls_back() {
        let fixture = crash_left_delivery_fixture(
            "delivery-crash-unrelated-pending",
            ObjectiveKind::Delivery,
        )
        .await;
        let run_id = "delivery-crash-unrelated-pending-run";
        let claim_epoch: i64 =
            sqlx::query_scalar("SELECT claim_epoch FROM delivery_runs WHERE id=?")
                .bind(run_id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
        sqlx::query(
            "INSERT INTO tool_calls
             (id, message_id, objective_id, tool_name, status, binding_id,
              action_signature, resource_generation)
             VALUES (?, ?, ?, 'bash', 'pending', ?, 'sha256:unrelated-action', 2)",
        )
        .bind(format!(
            "{}:provider-unrelated-before-crash",
            fixture.objective.session_id.as_deref().unwrap()
        ))
        .bind("assistant-delivery-crash-unrelated-pending")
        .bind(&fixture.objective.id)
        .bind(&fixture.current_binding_id)
        .execute(&fixture.pool)
        .await
        .unwrap();

        let error = fixture
            .store
            .settle_reconciled_delivery_after_takeover(
                run_id,
                claim_epoch,
                &format!("process-{run_id}"),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("unresolved tool call"), "{error}");
        let state: (String, String, i64, i64) = sqlx::query_as(
            "SELECT objective.status, run.status,
                    (SELECT COUNT(*) FROM tool_calls
                     WHERE objective_id=objective.id AND status='pending'),
                    (SELECT COUNT(*) FROM side_effect_receipts
                     WHERE objective_id=objective.id)
             FROM objectives objective
             JOIN delivery_runs run ON run.id=objective.delivery_run_id
             WHERE objective.id=?",
        )
        .bind(&fixture.objective.id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(
            state,
            (
                "active".into(),
                "awaiting_completion_arbitration".into(),
                3,
                0,
            )
        );
    }

    #[tokio::test]
    async fn takeover_requires_the_exact_live_delivery_lease_owner() {
        let fixture =
            crash_left_delivery_fixture("delivery-crash-lease-fence", ObjectiveKind::Delivery)
                .await;
        let run_id = "delivery-crash-lease-fence-run";
        let claim_epoch: i64 =
            sqlx::query_scalar("SELECT claim_epoch FROM delivery_runs WHERE id=?")
                .bind(run_id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
        let wrong_owner = fixture
            .store
            .settle_reconciled_delivery_after_takeover(run_id, claim_epoch, "replacement-owner")
            .await
            .unwrap_err()
            .to_string();
        assert!(
            wrong_owner.contains("lease expired or changed"),
            "{wrong_owner}"
        );
        sqlx::query("UPDATE delivery_runs SET lease_expires_at=0 WHERE id=?")
            .bind(run_id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        let expired = fixture
            .store
            .settle_reconciled_delivery_after_takeover(
                run_id,
                claim_epoch,
                &format!("process-{run_id}"),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(expired.contains("lease expired or changed"), "{expired}");
        let state: (String, String, i64) = sqlx::query_as(
            "SELECT objective.status, run.status,
                    (SELECT COUNT(*) FROM side_effect_receipts
                     WHERE objective_id=objective.id)
             FROM objectives objective
             JOIN delivery_runs run ON run.id=objective.delivery_run_id
             WHERE objective.id=?",
        )
        .bind(&fixture.objective.id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(
            state,
            ("active".into(), "awaiting_completion_arbitration".into(), 0)
        );
    }

    async fn assert_delivery_completion_refused_and_rolled_back(
        fixture: &GenericReceiptCompletionFixture,
        run_id: &str,
        error_fragment: &str,
    ) {
        let error = fixture
            .store
            .apply_decision(fixture.objective.revision, completion_decision(fixture))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains(error_fragment), "unexpected error: {error}");
        let objective_status: String =
            sqlx::query_scalar("SELECT status FROM objectives WHERE id=?")
                .bind(&fixture.objective.id)
                .fetch_one(&fixture.pool)
                .await
                .unwrap();
        assert_eq!(objective_status, "active");
        let run_status: String = sqlx::query_scalar("SELECT status FROM delivery_runs WHERE id=?")
            .bind(run_id)
            .fetch_one(&fixture.pool)
            .await
            .unwrap();
        assert_eq!(run_status, "awaiting_completion_arbitration");
        let receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM side_effect_receipts
             WHERE objective_id=? AND action_fingerprint=?",
        )
        .bind(&fixture.objective.id)
        .bind(CURRENT_ACTION_SIGNATURE)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(receipts, 0, "failed arbitration must roll back its receipt");
        let events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM delivery_run_events
             WHERE run_id=? AND event_kind='objective_completed'",
        )
        .bind(run_id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(events, 0, "failed arbitration must roll back its event");
    }

    #[tokio::test]
    async fn delivery_completion_rejects_missing_authoritative_run_pointer() {
        let fixture = generic_receipt_completion_fixture("delivery-pointer-missing").await;
        let run_id = "delivery-pointer-missing-run";
        persist_delivery_completion_candidate(&fixture, run_id, "live_verified").await;
        sqlx::query("UPDATE objectives SET delivery_run_id=NULL WHERE id=?")
            .bind(&fixture.objective.id)
            .execute(&fixture.pool)
            .await
            .unwrap();

        assert_delivery_completion_refused_and_rolled_back(
            &fixture,
            run_id,
            "exactly one pointed receipt-backed DeliveryRun",
        )
        .await;
    }

    #[tokio::test]
    async fn delivery_completion_rejects_non_done_or_multiple_current_actions() {
        let fixture = generic_receipt_completion_fixture("delivery-action-not-done").await;
        let run_id = "delivery-action-not-done-run";
        persist_delivery_completion_candidate(&fixture, run_id, "live_verified").await;
        sqlx::query("UPDATE tool_calls SET status='waiting' WHERE objective_id=?")
            .bind(&fixture.objective.id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        assert_delivery_completion_refused_and_rolled_back(
            &fixture,
            run_id,
            "no completed current action",
        )
        .await;

        let fixture = generic_receipt_completion_fixture("delivery-action-ambiguous").await;
        let run_id = "delivery-action-ambiguous-run";
        persist_delivery_completion_candidate(&fixture, run_id, "live_verified").await;
        sqlx::query(
            "INSERT INTO tool_calls
             (id, objective_id, tool_name, status, binding_id,
              action_signature, resource_generation)
             VALUES ('tool-delivery-action-ambiguous-2', ?, 'deliver_changes',
                     'done', ?, 'sha256:second-delivery-action', 2)",
        )
        .bind(&fixture.objective.id)
        .bind(&fixture.current_binding_id)
        .execute(&fixture.pool)
        .await
        .unwrap();
        assert_delivery_completion_refused_and_rolled_back(
            &fixture,
            run_id,
            "one stable action signature",
        )
        .await;
    }

    #[tokio::test]
    async fn delivery_completion_rejects_stale_epoch_or_unresolved_mutation_intent() {
        let fixture = generic_receipt_completion_fixture("delivery-stale-epoch").await;
        let run_id = "delivery-stale-epoch-run";
        persist_delivery_completion_candidate(&fixture, run_id, "live_verified").await;
        sqlx::query("UPDATE delivery_runs SET reconciled_claim_epoch=0 WHERE id=?")
            .bind(run_id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        assert_delivery_completion_refused_and_rolled_back(
            &fixture,
            run_id,
            "exactly one pointed receipt-backed DeliveryRun",
        )
        .await;

        let fixture = generic_receipt_completion_fixture("delivery-intent-unknown").await;
        let run_id = "delivery-intent-unknown-run";
        persist_delivery_completion_candidate(&fixture, run_id, "live_verified").await;
        sqlx::query(
            "INSERT INTO delivery_mutation_intents
             (intent_id, run_id, claim_epoch, rung, operation_key, status,
              process_instance, started_at, updated_at)
             VALUES ('intent-delivery-unknown', ?, 1, 'release', 'release:1',
                     'unknown', 'process-test', ?, ?)",
        )
        .bind(run_id)
        .bind(fixture.now)
        .bind(fixture.now)
        .execute(&fixture.pool)
        .await
        .unwrap();
        assert_delivery_completion_refused_and_rolled_back(
            &fixture,
            run_id,
            "exactly one pointed receipt-backed DeliveryRun",
        )
        .await;
    }

    #[tokio::test]
    async fn delivery_completion_rejects_below_ceiling_or_wrong_canonical_head() {
        let fixture = generic_receipt_completion_fixture("delivery-below-ceiling").await;
        let run_id = "delivery-below-ceiling-run";
        persist_delivery_completion_candidate(&fixture, run_id, "merged").await;
        assert_delivery_completion_refused_and_rolled_back(
            &fixture,
            run_id,
            "exactly one pointed receipt-backed DeliveryRun",
        )
        .await;

        let fixture = generic_receipt_completion_fixture("delivery-wrong-head").await;
        let run_id = "delivery-wrong-head-run";
        persist_delivery_completion_candidate(&fixture, run_id, "live_verified").await;
        sqlx::query("UPDATE delivery_runs SET canonical_head_sha='other-head' WHERE id=?")
            .bind(run_id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        assert_delivery_completion_refused_and_rolled_back(
            &fixture,
            run_id,
            "exactly one pointed receipt-backed DeliveryRun",
        )
        .await;
    }

    #[tokio::test]
    async fn valid_delivery_cannot_hide_an_unreceipted_generic_side_effect() {
        let fixture = generic_receipt_completion_fixture("delivery-plus-generic").await;
        let run_id = "delivery-plus-generic-run";
        persist_delivery_completion_candidate(&fixture, run_id, "live_verified").await;
        sqlx::query(
            "INSERT INTO tool_calls
             (id, objective_id, tool_name, status, binding_id,
              action_signature, resource_generation)
             VALUES ('tool-delivery-plus-generic-2', ?, 'bash', 'done',
                     ?, 'sha256:unreceipted-generic-action', 2)",
        )
        .bind(&fixture.objective.id)
        .bind(&fixture.current_binding_id)
        .execute(&fixture.pool)
        .await
        .unwrap();

        assert_delivery_completion_refused_and_rolled_back(
            &fixture,
            run_id,
            "lacking a matching committed receipt",
        )
        .await;
    }

    #[tokio::test]
    async fn pointed_delivery_run_cannot_be_bypassed_by_only_generic_receipts() {
        let fixture = generic_receipt_completion_fixture("pointed-delivery-bypass").await;
        let run_id = "pointed-delivery-bypass-run";
        persist_delivery_completion_candidate(&fixture, run_id, "live_verified").await;
        sqlx::query("UPDATE tool_calls SET tool_name='bash' WHERE objective_id=?")
            .bind(&fixture.objective.id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        insert_generic_receipt(
            &fixture,
            "receipt-pointed-delivery-bypass",
            &fixture.objective.id,
            &fixture.current_binding_id,
            CURRENT_ACTION_SIGNATURE,
            "committed",
        )
        .await;

        let error = fixture
            .store
            .apply_decision(fixture.objective.revision, completion_decision(&fixture))
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("pointed DeliveryRun"), "{error}");
        let state: (String, String) = sqlx::query_as(
            "SELECT objective.status, run.status
             FROM objectives objective
             JOIN delivery_runs run ON run.id=objective.delivery_run_id
             WHERE objective.id=?",
        )
        .bind(&fixture.objective.id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(
            state,
            ("active".into(), "awaiting_completion_arbitration".into())
        );
    }

    #[tokio::test]
    async fn explicit_cancel_atomically_fences_pointed_delivery_from_recovery() {
        let fixture = generic_receipt_completion_fixture("cancel-pointed-delivery").await;
        let run_id = "cancel-pointed-delivery-run";
        persist_delivery_completion_candidate(&fixture, run_id, "live_verified").await;
        sqlx::query("UPDATE delivery_runs SET lease_expires_at=? WHERE id=?")
            .bind(fixture.now - 1)
            .bind(run_id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        let cancelled = DecisionRouter::route(
            &fixture.objective,
            RouteSignal::Cancelled {
                domain: RecoveryDomain::Delivery,
                provenance: "explicit_cancel".into(),
            },
        )
        .unwrap();
        let objective = fixture
            .store
            .apply_decision(fixture.objective.revision, cancelled)
            .await
            .unwrap();
        assert_eq!(objective.status, ObjectiveStatus::Cancelled);
        let run_projection: (String, i64, Option<String>, Option<i64>) = sqlx::query_as(
            "SELECT status, next_action_authorized, lease_owner, lease_expires_at
             FROM delivery_runs WHERE id=?",
        )
        .bind(run_id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(run_projection, ("cancelled".into(), 0, None, None));
        let recovery = crate::agent::delivery_run::plan_startup_recovery(
            &fixture.pool,
            &crate::agent::delivery_run::ProcessIdentity::new(
                "process-after-explicit-cancel",
                "test",
                "test",
            ),
            fixture.now + 2,
            90_000,
        )
        .await
        .unwrap();
        assert!(recovery.claimed.is_empty());
    }

    #[tokio::test]
    async fn completion_attributes_actions_to_exact_binding_not_same_numbered_generation() {
        let fixture = generic_receipt_completion_fixture("exact-binding-attribution").await;
        let old_binding_id = "binding-exact-old-root";
        let current_binding_id = "binding-exact-current-root";
        sqlx::query("DELETE FROM tool_calls WHERE objective_id=?")
            .bind(&fixture.objective.id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM objective_bindings WHERE objective_id=?")
            .bind(&fixture.objective.id)
            .execute(&fixture.pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, side_effect_started,
              created_at, updated_at)
             VALUES
               (?, ?, 'tool', 'chat_root_turn', 'turn-exact-old-root',
                1, 'sha256:binding-exact-old-root', 1, ?, ?),
               (?, ?, 'tool', 'chat_root_turn', 'turn-exact-current-root',
                1, 'sha256:binding-exact-current-root', 1, ?, ?)",
        )
        .bind(old_binding_id)
        .bind(&fixture.objective.id)
        .bind(fixture.now)
        .bind(fixture.now)
        .bind(current_binding_id)
        .bind(&fixture.objective.id)
        .bind(fixture.now + 100)
        .bind(fixture.now + 100)
        .execute(&fixture.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tool_calls
             (id, objective_id, tool_name, status, binding_id,
              action_signature, resource_generation)
             VALUES
               ('tool-exact-old-root', ?, 'bash', 'done', ?,
                'sha256:old-root-action', 1),
               ('tool-exact-current-root', ?, 'bash', 'done', ?, ?, 1)",
        )
        .bind(&fixture.objective.id)
        .bind(old_binding_id)
        .bind(&fixture.objective.id)
        .bind(current_binding_id)
        .bind(CURRENT_ACTION_SIGNATURE)
        .execute(&fixture.pool)
        .await
        .unwrap();
        insert_generic_receipt(
            &fixture,
            "receipt-exact-current-root",
            &fixture.objective.id,
            current_binding_id,
            CURRENT_ACTION_SIGNATURE,
            "committed",
        )
        .await;

        let completed = fixture
            .store
            .apply_decision(fixture.objective.revision, completion_decision(&fixture))
            .await
            .unwrap();
        assert_eq!(completed.status, ObjectiveStatus::Completed);
        let old_receipts: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM side_effect_receipts
             WHERE binding_id=? AND action_fingerprint='sha256:old-root-action'",
        )
        .bind(old_binding_id)
        .fetch_one(&fixture.pool)
        .await
        .unwrap();
        assert_eq!(old_receipts, 0);
    }

    #[tokio::test]
    async fn prior_delivery_wait_row_does_not_block_later_receipt_backed_completion() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE tool_calls (
               id TEXT PRIMARY KEY, objective_id TEXT, status TEXT NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-delivery-wait-history".into(),
                kind: ObjectiveKind::Delivery,
                session_id: Some("session-delivery-wait-history".into()),
                root_turn_id: Some("turn-delivery-wait-history".into()),
                domain: RecoveryDomain::Delivery,
                requested_acceptance: "delivery_receipt".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO tool_calls(id, objective_id, status)
             VALUES ('delivery-wait-history', ?, 'waiting')",
        )
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        let now = Utc::now().timestamp_millis();
        let evidence = ObjectiveEvidence {
            id: "delivery-receipt-history".into(),
            kind: EvidenceKind::DeliveryReceipt,
            scope: "turn-delivery-wait-history".into(),
            digest: "sha256:delivery-receipt".into(),
            evidence_ref: "delivery-run:completed".into(),
            observed_at: now,
            reached_acceptance: "delivery_receipt".into(),
        };
        let complete = CompletionArbiter::decide(&objective, &[evidence]).unwrap();
        let completed = store
            .apply_decision(objective.revision, complete)
            .await
            .unwrap();
        assert_eq!(completed.status, ObjectiveStatus::Completed);
    }

    #[tokio::test]
    async fn completion_commit_atomically_converges_visible_final_turn_run_and_objective() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE messages (
               id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT NOT NULL,
               content TEXT NOT NULL, completion_state TEXT, created_at INTEGER NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
               revision INTEGER NOT NULL DEFAULT 1, phase TEXT NOT NULL,
               status TEXT NOT NULL, recent_activity_kind TEXT,
               recent_activity_label TEXT, waiting_reason TEXT,
               updated_at INTEGER NOT NULL, completed_at INTEGER,
               terminal_reason TEXT, objective_id TEXT,
               turn_settled_at INTEGER, stream_closed_at INTEGER,
               terminal_revision INTEGER, objective_revision INTEGER,
               visible_final_message_id TEXT,
               visible_final_kind TEXT, next_action TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO messages(id, session_id, role, content, created_at) VALUES
             ('turn-atomic-completion', 'session-atomic-completion', 'user', 'synthetic task', ?),
             ('assistant-atomic-final', 'session-atomic-completion', 'assistant', 'verified result', ?),
             ('assistant-newer-provisional', 'session-atomic-completion', 'assistant', 'newer provisional draft', ?)",
        )
        .bind(now)
        .bind(now + 1)
        .bind(now + 2)
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-atomic-completion".into(),
                kind: ObjectiveKind::Informational,
                session_id: Some("session-atomic-completion".into()),
                root_turn_id: Some("turn-atomic-completion".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "informational_answer".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, session_id, phase, status, updated_at, objective_id)
             VALUES ('turn-atomic-completion', 'session-atomic-completion',
                     'working', 'active', ?, ?)",
        )
        .bind(now)
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_run_controls
             (run_instance_id, session_id, root_turn_id, objective_id,
              objective_revision, status, created_process_instance,
              created_at, updated_at)
             VALUES ('run-atomic-completion', 'session-atomic-completion',
                     'turn-atomic-completion', ?, ?, 'active', 'test-process', ?, ?)",
        )
        .bind(&objective.id)
        .bind(objective.revision)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let evidence = ObjectiveEvidence {
            id: "evidence-atomic-completion".into(),
            kind: EvidenceKind::InformationalAnswer,
            scope: "turn-atomic-completion".into(),
            digest: "sha256:verified-result".into(),
            evidence_ref: "message:assistant-atomic-final".into(),
            observed_at: now + 1,
            reached_acceptance: "informational_answer".into(),
        };
        let mut completion = CompletionArbiter::decide(&objective, &[evidence]).unwrap();
        completion.visible_final_message_id = Some("assistant-atomic-final".into());
        let completed = store
            .apply_decision(objective.revision, completion)
            .await
            .unwrap();
        assert_eq!(completed.status, ObjectiveStatus::Completed);
        let turn: (
            String,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<String>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT status, turn_settled_at, stream_closed_at, terminal_revision,
                    visible_final_message_id, visible_final_kind
             FROM chat_turn_state WHERE objective_id=?",
        )
        .bind(&objective.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(turn.0, "completed");
        assert!(turn.1.is_some() && turn.2.is_some());
        assert_eq!(turn.3, Some(completed.revision));
        assert_eq!(turn.4.as_deref(), Some("assistant-atomic-final"));
        assert_eq!(turn.5.as_deref(), Some("assistant_final"));
        let run: (String, Option<i64>) =
            sqlx::query_as("SELECT status, settled_at FROM chat_run_controls WHERE objective_id=?")
                .bind(&objective.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(run.0, "completed");
        assert!(run.1.is_some());
    }

    #[tokio::test]
    async fn completion_rolls_back_when_terminal_turn_schema_is_incomplete() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE messages (
               id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT NOT NULL,
               content TEXT NOT NULL, completion_state TEXT, created_at INTEGER NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
               status TEXT NOT NULL, objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO messages(id, session_id, role, content, created_at) VALUES
             ('turn-incomplete-terminal', 'session-incomplete-terminal', 'user', 'synthetic task', ?),
             ('assistant-incomplete-terminal', 'session-incomplete-terminal', 'assistant', 'verified result', ?)",
        )
        .bind(now)
        .bind(now + 1)
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-incomplete-terminal".into(),
                kind: ObjectiveKind::Informational,
                session_id: Some("session-incomplete-terminal".into()),
                root_turn_id: Some("turn-incomplete-terminal".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "informational_answer".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, status, objective_id)
             VALUES ('turn-incomplete-terminal', 'session-incomplete-terminal', 'active', ?)",
        )
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        let evidence = ObjectiveEvidence {
            id: "evidence-incomplete-terminal".into(),
            kind: EvidenceKind::InformationalAnswer,
            scope: "turn-incomplete-terminal".into(),
            digest: "sha256:verified-result".into(),
            evidence_ref: "message:assistant-incomplete-terminal".into(),
            observed_at: now + 1,
            reached_acceptance: "informational_answer".into(),
        };
        let mut completion = CompletionArbiter::decide(&objective, &[evidence]).unwrap();
        completion.visible_final_message_id = Some("assistant-incomplete-terminal".into());
        let error = store
            .apply_decision(objective.revision, completion)
            .await
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("complete terminal turn projection"));
        let unchanged = store.get(&objective.id).await.unwrap().unwrap();
        assert_eq!(unchanged.revision, objective.revision);
        assert_eq!(unchanged.status, ObjectiveStatus::Active);
    }

    #[tokio::test]
    async fn chat_objective_is_created_once_and_linked_to_the_transport_projection() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               status TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, status)
             VALUES ('turn-linked', 'session-linked', 'active')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let first = store
            .ensure_chat_objective(
                "session-linked",
                "turn-linked",
                ObjectiveKind::LocalMutation,
                "validated_change",
            )
            .await
            .unwrap();
        let second = store
            .ensure_chat_objective(
                "session-linked",
                "turn-linked",
                ObjectiveKind::LocalMutation,
                "validated_change",
            )
            .await
            .unwrap();
        assert_eq!(first.id, second.id);
        let linked: String = sqlx::query_scalar(
            "SELECT objective_id FROM chat_turn_state WHERE root_turn_id='turn-linked'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(linked, first.id);
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM objectives WHERE root_turn_id='turn-linked'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 1);
        let binding: (String, String, i64, String) = sqlx::query_as(
            "SELECT id, objective_id, resource_generation, identity_digest
             FROM objective_bindings
             WHERE domain='chat' AND resource_kind='chat_root_turn'
               AND resource_id='turn-linked'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(binding.1, first.id);
        assert_eq!(binding.2, 1);
        assert!(binding.3.starts_with("sha256:"));
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM objective_bindings
                 WHERE objective_id=? AND domain='chat' AND resource_kind='chat_root_turn'",
            )
            .bind(&first.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            1,
            "idempotent ensure must preserve one authoritative binding"
        );

        let waiting = DecisionRouter::route(
            &second,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Chat,
                failure_code: "provider_timeout".into(),
                failure_signature: "sha256:linked-timeout".into(),
                next_observation_at: Utc::now().timestamp_millis() - 1,
                resume_cursor: Some("turn-linked".into()),
            },
        )
        .unwrap();
        store
            .apply_decision(second.revision, waiting)
            .await
            .unwrap();
        let claim = store
            .claim_due_remediations("binding-supervisor", 1, 30_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(claim.binding_id.as_deref(), Some(binding.0.as_str()));
        assert_eq!(claim.resource_generation, Some(1));
        let permit = codefactory_agent_loop::tool::MutationPermit {
            objective_id: claim.objective.id.clone(),
            remediation_id: claim.remediation_id.clone(),
            owner: "binding-supervisor".into(),
            claim_epoch: claim.claim_epoch,
            binding_id: claim.binding_id.clone(),
            resource_generation: claim.resource_generation,
        };
        let retry = DecisionRouter::route(
            &claim.objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Chat,
                failure_code: "provider_still_unavailable".into(),
                failure_signature: "sha256:linked-timeout-retry".into(),
                next_observation_at: Utc::now().timestamp_millis() + 1_000,
                resume_cursor: Some("turn-linked".into()),
            },
        )
        .unwrap();
        sqlx::query("UPDATE objective_bindings SET resource_generation=2 WHERE id=?")
            .bind(&binding.0)
            .execute(&pool)
            .await
            .unwrap();
        assert!(store
            .apply_claimed_decision(claim.objective.revision, retry.clone(), &permit)
            .await
            .unwrap_err()
            .to_string()
            .contains("resource generation"));
        sqlx::query("UPDATE objective_bindings SET resource_generation=1 WHERE id=?")
            .bind(&binding.0)
            .execute(&pool)
            .await
            .unwrap();
        let retried = store
            .apply_claimed_decision(claim.objective.revision, retry, &permit)
            .await
            .unwrap();
        assert_eq!(retried.status, ObjectiveStatus::WaitingSystem);
        assert_ne!(retried.remediation_id, Some(claim.remediation_id));
    }

    #[tokio::test]
    async fn contextual_root_continues_the_single_open_objective_and_elevates_its_ceiling() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               status TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, status) VALUES
             ('turn-original', 'session-continuation', 'active'),
             ('turn-approval', 'session-continuation', 'active')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let original = store
            .ensure_chat_objective(
                "session-continuation",
                "turn-original",
                ObjectiveKind::LocalMutation,
                "validated_change",
            )
            .await
            .unwrap();

        let continued = store
            .ensure_or_continue_chat_objective(
                "session-continuation",
                "turn-approval",
                Some("turn-original"),
                ObjectiveKind::Delivery,
                "delivery_receipt",
            )
            .await
            .unwrap();

        assert_eq!(continued.id, original.id);
        assert_eq!(continued.kind, ObjectiveKind::Delivery);
        assert!(continued.revision > original.revision);
        let bound: String = sqlx::query_scalar(
            "SELECT objective_id FROM chat_turn_state WHERE root_turn_id='turn-approval'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(bound, original.id);
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM objectives")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn transactional_chat_objective_admission_rolls_back_every_control_plane_write() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               status TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, status) VALUES
             ('turn-transaction-original', 'session-transaction', 'interrupted'),
             ('turn-transaction-next', 'session-transaction', 'active')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let original = store
            .ensure_chat_objective(
                "session-transaction",
                "turn-transaction-original",
                ObjectiveKind::LocalMutation,
                "validated_change",
            )
            .await
            .unwrap();
        let waiting = DecisionRouter::route(
            &original,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Chat,
                failure_code: "provider_timeout".into(),
                failure_signature: "sha256:transaction-timeout".into(),
                next_observation_at: Utc::now().timestamp_millis() + 60_000,
                resume_cursor: Some("turn-transaction-original".into()),
            },
        )
        .unwrap();
        let waiting = store
            .apply_decision(original.revision, waiting)
            .await
            .unwrap();
        let remediation_id = waiting.remediation_id.clone().unwrap();
        let baseline_event_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM objective_events WHERE objective_id=?")
                .bind(&waiting.id)
                .fetch_one(&pool)
                .await
                .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let admitted = store
            .ensure_or_continue_chat_objective_in_tx(
                &mut tx,
                "session-transaction",
                "turn-transaction-next",
                Some("turn-transaction-original"),
                ObjectiveKind::Delivery,
                "delivery_receipt",
            )
            .await
            .unwrap();
        assert_eq!(admitted.id, waiting.id);
        assert_eq!(admitted.revision, waiting.revision + 1);
        assert_eq!(admitted.status, ObjectiveStatus::Active);
        assert_eq!(admitted.kind, ObjectiveKind::Delivery);
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT objective_id FROM chat_turn_state
                 WHERE root_turn_id='turn-transaction-next'",
            )
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
            waiting.id
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM objective_bindings
                 WHERE objective_id=? AND resource_kind='chat_root_turn'
                   AND resource_id='turn-transaction-next'",
            )
            .bind(&waiting.id)
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM objective_remediations WHERE id=?",
            )
            .bind(&remediation_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
            "superseded"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM objective_events WHERE objective_id=?",
            )
            .bind(&waiting.id)
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
            // `contextual_root_bound` plus the U18/CF-ORC-R46 takeover audit
            // (`user_steer_superseded_remediation`): a user message arriving
            // during system recovery must name what it superseded, and that
            // event belongs to the same transaction as the binding move.
            baseline_event_count + 2
        );
        tx.rollback().await.unwrap();

        let rolled_back = store.get(&waiting.id).await.unwrap().unwrap();
        assert_eq!(rolled_back.revision, waiting.revision);
        assert_eq!(rolled_back.status, ObjectiveStatus::WaitingSystem);
        assert_eq!(rolled_back.kind, ObjectiveKind::LocalMutation);
        assert_eq!(
            sqlx::query_scalar::<_, Option<String>>(
                "SELECT objective_id FROM chat_turn_state
                 WHERE root_turn_id='turn-transaction-next'",
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            None
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM objective_bindings
                 WHERE objective_id=? AND resource_kind='chat_root_turn'
                   AND resource_id='turn-transaction-next'",
            )
            .bind(&waiting.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM objective_remediations WHERE id=?",
            )
            .bind(&remediation_id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            "queued"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM objective_events WHERE objective_id=?",
            )
            .bind(&waiting.id)
            .fetch_one(&pool)
            .await
            .unwrap(),
            baseline_event_count
        );
    }

    #[tokio::test]
    async fn contextual_root_fails_closed_when_two_open_objectives_are_present() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               status TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, status) VALUES
             ('turn-prior', 'session-ambiguous', 'active'),
             ('turn-current', 'session-ambiguous', 'active')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool);
        store
            .ensure_chat_objective(
                "session-ambiguous",
                "turn-prior",
                ObjectiveKind::LocalMutation,
                "validated_change",
            )
            .await
            .unwrap();
        store
            .ensure_chat_objective(
                "session-ambiguous",
                "turn-current",
                ObjectiveKind::Informational,
                "informational_answer",
            )
            .await
            .unwrap();

        let error = store
            .ensure_or_continue_chat_objective(
                "session-ambiguous",
                "turn-current",
                Some("turn-prior"),
                ObjectiveKind::Delivery,
                "delivery_receipt",
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("multiple open objectives"));
    }

    #[tokio::test]
    async fn legacy_contextual_root_is_reconciled_to_one_new_opaque_objective() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               status TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, status) VALUES
             ('turn-legacy', 'session-legacy', 'interrupted'),
             ('turn-legacy-continue', 'session-legacy', 'active')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());

        let reconciled = store
            .ensure_or_continue_chat_objective(
                "session-legacy",
                "turn-legacy-continue",
                Some("turn-legacy"),
                ObjectiveKind::LocalMutation,
                "validated_change",
            )
            .await
            .unwrap();

        assert!(!reconciled.id.starts_with("chat:"));
        assert_eq!(reconciled.kind, ObjectiveKind::LocalMutation);
        let bindings: Vec<String> = sqlx::query_scalar(
            "SELECT objective_id FROM chat_turn_state
             WHERE root_turn_id IN ('turn-legacy','turn-legacy-continue')
             ORDER BY root_turn_id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(bindings, vec![reconciled.id.clone(), reconciled.id.clone()]);
        let event_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_events
             WHERE objective_id=? AND event_type='legacy_root_reconciled'",
        )
        .bind(&reconciled.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(event_count, 1);
    }

    #[tokio::test]
    async fn remediation_claim_is_leased_once_and_can_be_deferred_without_user_handoff() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-lease".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-lease".into()),
                root_turn_id: Some("turn-lease".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let waiting = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Chat,
                failure_code: "panic".into(),
                failure_signature: "panic:test".into(),
                next_observation_at: Utc::now().timestamp_millis() - 1,
                resume_cursor: Some("turn-lease".into()),
            },
        )
        .unwrap();
        store.apply_decision(1, waiting).await.unwrap();

        let first = store
            .claim_due_remediations("supervisor-a", 4, 30_000)
            .await
            .unwrap();
        assert_eq!(first.len(), 1);
        let second = store
            .claim_due_remediations("supervisor-b", 4, 30_000)
            .await
            .unwrap();
        assert!(second.is_empty());

        store
            .defer_claimed_remediation(
                &first[0].objective.id,
                &first[0].remediation_id,
                "supervisor-a",
                first[0].claim_epoch,
                1_000,
            )
            .await
            .unwrap();
        let immediate = store
            .claim_due_remediations("supervisor-b", 4, 30_000)
            .await
            .unwrap();
        assert!(immediate.is_empty());
        sqlx::query("UPDATE objective_remediations SET next_observation_at=? WHERE id=?")
            .bind(Utc::now().timestamp_millis() - 1)
            .bind(&first[0].remediation_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE objectives SET next_observation_at=? WHERE id=?")
            .bind(Utc::now().timestamp_millis() - 1)
            .bind(&first[0].objective.id)
            .execute(&pool)
            .await
            .unwrap();
        let reclaimed = store
            .claim_due_remediations("supervisor-a", 4, 30_000)
            .await
            .unwrap();
        assert_eq!(reclaimed.len(), 1);
        assert!(reclaimed[0].claim_epoch > first[0].claim_epoch);
        let attempt_index: i64 = sqlx::query_scalar(
            "SELECT execution_attempt_index FROM objective_remediations WHERE id=?",
        )
        .bind(&first[0].remediation_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            attempt_index, 0,
            "lease acquisition and takeover must not consume recovery budget"
        );
        assert!(!store
            .renew_claimed_remediation(
                &first[0].objective.id,
                &first[0].remediation_id,
                "supervisor-a",
                first[0].claim_epoch,
                30_000,
            )
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn claimed_remediation_lease_is_renewed_only_by_its_current_owner() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-lease-renewal".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-lease-renewal".into()),
                root_turn_id: Some("turn-lease-renewal".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let waiting = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Chat,
                failure_code: "provider_timeout".into(),
                failure_signature: "provider_timeout:test".into(),
                next_observation_at: Utc::now().timestamp_millis() - 1,
                resume_cursor: Some("turn-lease-renewal".into()),
            },
        )
        .unwrap();
        store.apply_decision(1, waiting).await.unwrap();
        let claim = store
            .claim_due_remediations("supervisor-renewal", 1, 1_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let before: i64 =
            sqlx::query_scalar("SELECT lease_expires_at FROM objective_remediations WHERE id=?")
                .bind(&claim.remediation_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let progress_before: (Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT remediation.last_progress_at, objective.last_progress_at
             FROM objective_remediations remediation
             JOIN objectives objective ON objective.id=remediation.objective_id
             WHERE remediation.id=?",
        )
        .bind(&claim.remediation_id)
        .fetch_one(&pool)
        .await
        .unwrap();

        assert!(!store
            .renew_claimed_remediation(
                &claim.objective.id,
                &claim.remediation_id,
                "another-supervisor",
                claim.claim_epoch,
                60_000,
            )
            .await
            .unwrap());
        assert!(store
            .renew_claimed_remediation(
                &claim.objective.id,
                &claim.remediation_id,
                "supervisor-renewal",
                claim.claim_epoch,
                60_000,
            )
            .await
            .unwrap());

        let (renewed_remediation_lease, renewed_objective_lease): (i64, i64) = sqlx::query_as(
            "SELECT remediation.lease_expires_at, objective.lease_expires_at
             FROM objective_remediations remediation
             JOIN objectives objective ON objective.id=remediation.objective_id
             WHERE remediation.id=?",
        )
        .bind(&claim.remediation_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(renewed_remediation_lease > before);
        assert_eq!(renewed_remediation_lease, renewed_objective_lease);
        let progress_after: (Option<i64>, Option<i64>) = sqlx::query_as(
            "SELECT remediation.last_progress_at, objective.last_progress_at
             FROM objective_remediations remediation
             JOIN objectives objective ON objective.id=remediation.objective_id
             WHERE remediation.id=?",
        )
        .bind(&claim.remediation_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            progress_after, progress_before,
            "lease liveness is not progress"
        );

        let expired = Utc::now().timestamp_millis() - 1;
        sqlx::query("UPDATE objective_remediations SET lease_expires_at=? WHERE id=?")
            .bind(expired)
            .bind(&claim.remediation_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE objectives SET lease_expires_at=? WHERE id=?")
            .bind(expired)
            .bind(&claim.objective.id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(!store
            .renew_claimed_remediation(
                &claim.objective.id,
                &claim.remediation_id,
                "supervisor-renewal",
                claim.claim_epoch,
                60_000,
            )
            .await
            .unwrap());

        let (remediation_lease, objective_lease): (i64, i64) = sqlx::query_as(
            "SELECT remediation.lease_expires_at, objective.lease_expires_at
             FROM objective_remediations remediation
             JOIN objectives objective ON objective.id=remediation.objective_id
             WHERE remediation.id=?",
        )
        .bind(&claim.remediation_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(remediation_lease, expired);
        assert_eq!(remediation_lease, objective_lease);
    }

    #[tokio::test]
    async fn expired_claim_is_reclaimed_after_process_loss() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-expired-claim".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-expired-claim".into()),
                root_turn_id: Some("turn-expired-claim".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let waiting = DecisionRouter::route(
            &objective,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Chat,
                failure_code: "process_lost".into(),
                failure_signature: "process_lost:test".into(),
                next_observation_at: Utc::now().timestamp_millis() - 1,
                resume_cursor: Some("turn-expired-claim".into()),
            },
        )
        .unwrap();
        store.apply_decision(1, waiting).await.unwrap();
        let claim = store
            .claim_due_remediations("dead-supervisor", 1, 1_000)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let expired = Utc::now().timestamp_millis() - 1;
        sqlx::query("UPDATE objective_remediations SET lease_expires_at=? WHERE id=?")
            .bind(expired)
            .bind(&claim.remediation_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE objectives SET lease_expires_at=? WHERE id=?")
            .bind(expired)
            .bind(&claim.objective.id)
            .execute(&pool)
            .await
            .unwrap();

        let reclaimed = store
            .claim_due_remediations("replacement-supervisor", 1, 30_000)
            .await
            .unwrap();
        assert_eq!(reclaimed.len(), 1);
        let owner: String =
            sqlx::query_scalar("SELECT lease_owner FROM objective_remediations WHERE id=?")
                .bind(&claim.remediation_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(owner, "replacement-supervisor");
    }

    #[tokio::test]
    async fn startup_reconciles_only_active_objectives_owned_by_an_old_process() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let stale = store
            .create(CreateObjective {
                id: "objective-stale-process".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-stale-process".into()),
                root_turn_id: Some("turn-stale-process".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let current = store
            .create(CreateObjective {
                id: "objective-current-process".into(),
                kind: ObjectiveKind::Informational,
                session_id: Some("session-current-process".into()),
                root_turn_id: Some("turn-current-process".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "informational_answer".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        sqlx::query(
            "UPDATE objectives SET created_process_instance='old-process',
             last_observed_process_instance='old-process' WHERE id=?",
        )
        .bind(&stale.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE objectives SET created_process_instance='current-process',
             last_observed_process_instance='current-process' WHERE id=?",
        )
        .bind(&current.id)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            store
                .reconcile_stale_active_objectives("current-process")
                .await
                .unwrap(),
            1
        );
        let stale = store.get(&stale.id).await.unwrap().unwrap();
        assert_eq!(stale.status, ObjectiveStatus::WaitingSystem);
        assert_eq!(stale.failure_code.as_deref(), Some("process_restarted"));
        assert!(!stale.requires_user_action);
        let current = store.get(&current.id).await.unwrap().unwrap();
        assert_eq!(current.status, ObjectiveStatus::Active);
    }

    #[tokio::test]
    async fn restored_authorization_resumes_the_same_objective_without_a_user_message() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool);
        let objective = store
            .create(CreateObjective {
                id: "objective-auth".into(),
                kind: ObjectiveKind::Informational,
                session_id: Some("session-auth".into()),
                root_turn_id: Some("turn-auth".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "answer".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let authorization = DecisionRouter::route(
            &objective,
            RouteSignal::AuthorizationRequired {
                domain: RecoveryDomain::Auth,
                request_key: "chatgpt-auth:objective-auth".into(),
                action_signature: "oauth:chatgpt:resume:objective-auth".into(),
                resume_cursor: Some("turn-auth".into()),
            },
        )
        .unwrap();
        let waiting = store.apply_decision(1, authorization).await.unwrap();
        assert_eq!(waiting.status, ObjectiveStatus::WaitingAuthorization);

        assert_eq!(
            store
                .resume_waiting_authorizations(RecoveryDomain::Auth, "chatgpt-auth:")
                .await
                .unwrap(),
            1
        );
        let resumed = store.get("objective-auth").await.unwrap().unwrap();
        assert_eq!(resumed.status, ObjectiveStatus::WaitingSystem);
        assert!(!resumed.requires_user_action);
        assert_eq!(resumed.resume_cursor.as_deref(), Some("turn-auth"));
        assert!(resumed.next_observation_at.is_some());
    }

    #[test]
    fn transport_outcome_is_arbitrated_against_the_business_objective() {
        let informational = ObjectiveSnapshot::new(
            "objective-info",
            ObjectiveKind::Informational,
            RecoveryDomain::Chat,
            "answer",
        );
        let answer = RunOutcome {
            final_text: "verified answer".into(),
            final_message_id: Some("assistant-verified-answer".into()),
            completion_evidence: CompletionEvidence {
                completed: true,
                ..Default::default()
            },
            input_tokens: 10,
            output_tokens: 4,
            stop_reason: StopReason::Finished,
        };
        let complete = decision_for_run_outcome(&informational, &answer).unwrap();
        assert_eq!(complete.status, ObjectiveStatus::Completed);
        assert_eq!(complete.decision_type, DecisionType::Complete);

        let local = ObjectiveSnapshot::new(
            "objective-local",
            ObjectiveKind::LocalMutation,
            RecoveryDomain::Chat,
            "validated_change",
        );
        let prose_only = decision_for_run_outcome(&local, &answer).unwrap();
        assert_eq!(prose_only.status, ObjectiveStatus::WaitingSystem);
        assert!(!prose_only.requires_user_action);

        let verified_current_state = RunOutcome {
            final_text: "SKILL_RECOVERY_OK".into(),
            final_message_id: Some("assistant-skill-recovery".into()),
            completion_evidence: CompletionEvidence {
                outcome_count: 3,
                last_successful_verification_sequence: Some(3),
                last_bounded_probe_sequence: Some(3),
                completed: true,
                ..Default::default()
            },
            input_tokens: 12,
            output_tokens: 3,
            stop_reason: StopReason::Finished,
        };
        let no_change_complete = decision_for_run_outcome(&local, &verified_current_state).unwrap();
        assert_eq!(no_change_complete.status, ObjectiveStatus::Completed);
        assert_eq!(no_change_complete.decision_type, DecisionType::Complete);
        assert_eq!(
            no_change_complete.evidence.as_ref().map(|item| item.kind),
            Some(EvidenceKind::CurrentStateAcceptance),
        );
        assert_eq!(
            no_change_complete.visible_final_message_id.as_deref(),
            Some("assistant-skill-recovery"),
        );
    }

    #[test]
    fn platform_and_ceiling_outcomes_remain_system_owned() {
        let objective = ObjectiveSnapshot::new(
            "objective-recovery",
            ObjectiveKind::LocalMutation,
            RecoveryDomain::Chat,
            "validated_change",
        );
        for (stop_reason, expected_type) in [
            (StopReason::PlatformIncident, DecisionType::PlatformIncident),
            (StopReason::FailedInternal, DecisionType::FailedInternal),
            (StopReason::IterationCeiling, DecisionType::Waiting),
        ] {
            let outcome = RunOutcome {
                final_text: String::new(),
                final_message_id: None,
                completion_evidence: CompletionEvidence::default(),
                input_tokens: 0,
                output_tokens: 0,
                stop_reason,
            };
            let decision = decision_for_run_outcome(&objective, &outcome).unwrap();
            assert_eq!(decision.status, ObjectiveStatus::WaitingSystem);
            assert_eq!(decision.decision_type, expected_type);
            assert!(!decision.requires_user_action);
        }
    }

    #[test]
    fn permission_transport_interruptions_keep_the_permission_recovery_domain() {
        let objective = ObjectiveSnapshot::new(
            "objective-permission-interruption",
            ObjectiveKind::LocalMutation,
            RecoveryDomain::Chat,
            "validated_change",
        );
        let outcome = RunOutcome {
            final_text: String::new(),
            final_message_id: None,
            completion_evidence: CompletionEvidence::default(),
            input_tokens: 0,
            output_tokens: 0,
            stop_reason: StopReason::PlatformIncident,
        };

        for reason in ["permission_timed_out", "permission_channel_closed"] {
            let decision =
                decision_for_run_outcome_with_reason(&objective, &outcome, Some(reason)).unwrap();
            assert_eq!(decision.domain, RecoveryDomain::Permission);
            assert_eq!(decision.decision_type, DecisionType::PlatformIncident);
            assert_eq!(decision.status, ObjectiveStatus::WaitingSystem);
            assert_eq!(decision.failure_code.as_deref(), Some(reason));
            assert!(!decision.requires_user_action);
        }
    }

    #[test]
    fn typed_context_transport_outcomes_keep_the_context_recovery_domain() {
        let mut objective = ObjectiveSnapshot::new(
            "objective-context-interruption",
            ObjectiveKind::Informational,
            RecoveryDomain::Chat,
            "answer",
        );
        objective.root_turn_id = Some("turn-context-anchor".into());
        objective.resume_cursor = Some("turn-context-active".into());
        let outcome = RunOutcome {
            final_text: String::new(),
            final_message_id: None,
            completion_evidence: CompletionEvidence::default(),
            input_tokens: 0,
            output_tokens: 0,
            stop_reason: StopReason::PlatformIncident,
        };

        for reason in [
            "context_compaction_exhausted",
            "context_overflow_after_compaction",
            "context_compression_unavailable",
        ] {
            let decision =
                decision_for_run_outcome_with_reason(&objective, &outcome, Some(reason)).unwrap();
            assert_eq!(decision.domain, RecoveryDomain::Context);
            assert_eq!(decision.decision_type, DecisionType::Waiting);
            assert_eq!(decision.status, ObjectiveStatus::WaitingSystem);
            assert_eq!(decision.failure_code.as_deref(), Some(reason));
            assert_eq!(
                decision.resume_cursor.as_deref(),
                Some("turn-context-active")
            );
            assert!(!decision.requires_user_action);
        }
    }

    #[test]
    fn typed_browser_transport_outcomes_keep_the_browser_recovery_domain() {
        let mut objective = ObjectiveSnapshot::new(
            "objective-browser-interruption",
            ObjectiveKind::LocalMutation,
            RecoveryDomain::Chat,
            "validated_change",
        );
        objective.root_turn_id = Some("turn-browser-anchor".into());
        objective.resume_cursor = Some("turn-browser-active".into());
        let outcome = RunOutcome {
            final_text: String::new(),
            final_message_id: None,
            completion_evidence: CompletionEvidence::default(),
            input_tokens: 0,
            output_tokens: 0,
            stop_reason: StopReason::PlatformIncident,
        };

        for reason in [
            "browser_observation_contract_required",
            "browser_external_state_uncertain",
            "browser_observation_conflict",
        ] {
            let decision =
                decision_for_run_outcome_with_reason(&objective, &outcome, Some(reason)).unwrap();
            assert_eq!(decision.domain, RecoveryDomain::Browser);
            assert_eq!(decision.status, ObjectiveStatus::WaitingSystem);
            assert_eq!(decision.failure_code.as_deref(), Some(reason));
            assert_eq!(
                decision.resume_cursor.as_deref(),
                Some("turn-browser-active")
            );
            assert!(!decision.requires_user_action);
        }

        let pairing = decision_for_run_outcome_with_reason(
            &objective,
            &RunOutcome {
                stop_reason: StopReason::Blocked,
                ..outcome
            },
            Some("browser_pairing_required"),
        )
        .unwrap();
        assert_eq!(pairing.domain, RecoveryDomain::Browser);
        assert_eq!(pairing.status, ObjectiveStatus::WaitingCoreInput);
        assert_eq!(pairing.decision_type, DecisionType::CoreInputRequired);
        assert!(pairing.requires_user_action);
        assert_eq!(
            pairing.resume_cursor.as_deref(),
            Some("turn-browser-active")
        );
        assert!(pairing
            .request_key
            .as_deref()
            .is_some_and(|key| key.starts_with("browser-pairing:")));
    }

    #[test]
    fn typed_delivery_transport_outcomes_keep_the_delivery_recovery_domain() {
        let mut objective = ObjectiveSnapshot::new(
            "objective-delivery-interruption",
            ObjectiveKind::LocalMutation,
            RecoveryDomain::Chat,
            "validated_change",
        );
        objective.root_turn_id = Some("turn-delivery-anchor".into());
        objective.resume_cursor = Some("turn-delivery-active".into());
        let outcome = RunOutcome {
            final_text: String::new(),
            final_message_id: None,
            completion_evidence: CompletionEvidence::default(),
            input_tokens: 0,
            output_tokens: 0,
            stop_reason: StopReason::PlatformIncident,
        };

        for reason in [
            "delivery_identity_conflict",
            "delivery_external_state_uncertain",
            "delivery_mutation_receipt_unknown",
        ] {
            let decision =
                decision_for_run_outcome_with_reason(&objective, &outcome, Some(reason)).unwrap();
            assert_eq!(decision.domain, RecoveryDomain::Delivery);
            assert_eq!(decision.status, ObjectiveStatus::WaitingSystem);
            assert_eq!(decision.failure_code.as_deref(), Some(reason));
            assert_eq!(
                decision.resume_cursor.as_deref(),
                Some("turn-delivery-active")
            );
            assert!(!decision.requires_user_action);
        }
    }

    #[test]
    fn typed_tool_transport_outcomes_preserve_the_exact_recovery_reason() {
        let mut objective = ObjectiveSnapshot::new(
            "objective-tool-interruption",
            ObjectiveKind::LocalMutation,
            RecoveryDomain::Chat,
            "validated_change",
        );
        objective.root_turn_id = Some("turn-tool-anchor".into());
        objective.resume_cursor = Some("turn-tool-active".into());
        let outcome = RunOutcome {
            final_text: String::new(),
            final_message_id: None,
            completion_evidence: CompletionEvidence::default(),
            input_tokens: 0,
            output_tokens: 0,
            stop_reason: StopReason::PlatformIncident,
        };

        for reason in [
            "external_state_uncertain",
            "tool_observation_conflict",
            "tool_observation_contract_missing",
            "mutation_permit_lost",
        ] {
            let decision =
                decision_for_run_outcome_with_reason(&objective, &outcome, Some(reason)).unwrap();
            assert_eq!(decision.domain, RecoveryDomain::Tool);
            assert_eq!(decision.status, ObjectiveStatus::WaitingSystem);
            assert_eq!(decision.failure_code.as_deref(), Some(reason));
            assert_eq!(decision.resume_cursor.as_deref(), Some("turn-tool-active"));
            assert!(!decision.requires_user_action);
        }
    }

    #[tokio::test]
    async fn browser_pairing_capability_restores_the_same_objective_without_reprompt() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool);
        let objective = store
            .create(CreateObjective {
                id: "objective-browser-pairing".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-browser-pairing".into()),
                root_turn_id: Some("turn-browser-pairing".into()),
                domain: RecoveryDomain::Browser,
                requested_acceptance: "browser_attached".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let waiting = DecisionRouter::route(
            &objective,
            RouteSignal::CoreInputRequired {
                domain: RecoveryDomain::Browser,
                request_key: format!("browser-pairing:{}", objective.id),
                missing_inputs: vec!["load_and_pair_browser_extension".into()],
                attempted_routes: vec!["browser_extension_bridge".into()],
                resume_cursor: objective.root_turn_id.clone(),
            },
        )
        .unwrap();
        let waiting = store
            .apply_decision(objective.revision, waiting)
            .await
            .unwrap();
        assert_eq!(waiting.status, ObjectiveStatus::WaitingCoreInput);
        let attention = waiting
            .attention_request
            .as_ref()
            .expect("real core input must survive persistence");
        assert_eq!(attention.kind, "core_input");
        assert_eq!(
            attention.missing_inputs,
            ["load_and_pair_browser_extension"]
        );
        assert_eq!(attention.attempted_routes, ["browser_extension_bridge"]);
        assert!(!attention.prompt.trim().is_empty());
        let envelope: String = sqlx::query_scalar(
            "SELECT envelope_json FROM objective_decisions
             WHERE objective_id=? AND revision=?",
        )
        .bind(&objective.id)
        .bind(waiting.revision)
        .fetch_one(&store.pool)
        .await
        .unwrap();
        assert!(envelope.contains("load_and_pair_browser_extension"));

        assert_eq!(
            store
                .resume_waiting_core_inputs(
                    RecoveryDomain::Browser,
                    "browser-pairing:",
                    "browser_pairing_restored",
                )
                .await
                .unwrap(),
            1
        );
        let resumed = store.get(&objective.id).await.unwrap().unwrap();
        assert_eq!(resumed.id, objective.id);
        assert_eq!(resumed.root_turn_id, objective.root_turn_id);
        assert_eq!(resumed.status, ObjectiveStatus::WaitingSystem);
        assert_eq!(resumed.domain, RecoveryDomain::Browser);
        assert!(!resumed.requires_user_action);
        assert_eq!(
            resumed.failure_code.as_deref(),
            Some("browser_pairing_restored")
        );
    }

    #[tokio::test]
    async fn durable_cancel_intent_fences_a_late_finished_outcome_before_objective_cas() {
        let pool = pool().await;
        crate::agent::delivery_run::ensure_schema(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-cancel-before-finished".into(),
                kind: ObjectiveKind::Informational,
                session_id: Some("session-cancel-before-finished".into()),
                root_turn_id: Some("turn-cancel-before-finished".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "informational_answer".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, objective_id)
             VALUES (?, ?, ?)",
        )
        .bind(objective.root_turn_id.as_deref().unwrap())
        .bind(objective.session_id.as_deref().unwrap())
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO chat_run_controls
             (run_instance_id, session_id, root_turn_id, objective_id, objective_revision, status,
              created_process_instance, cancel_requested_at, created_at, updated_at)
             VALUES ('run-cancel-before-finished', ?, ?, ?, ?, 'cancel_requested',
                     'process-before-finished', ?, ?, ?)",
        )
        .bind(objective.session_id.as_deref().unwrap())
        .bind(objective.root_turn_id.as_deref().unwrap())
        .bind(&objective.id)
        .bind(objective.revision)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let late = RunOutcome {
            final_text: "late model answer".into(),
            final_message_id: Some("assistant-late-answer".into()),
            completion_evidence: CompletionEvidence {
                completed: true,
                ..Default::default()
            },
            input_tokens: 1,
            output_tokens: 1,
            stop_reason: StopReason::Finished,
        };
        let complete = decision_for_run_outcome(&objective, &late).unwrap();
        let error = store
            .apply_decision(objective.revision, complete)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("cancellation"), "{error}");

        assert_eq!(store.consume_pending_chat_cancellations().await.unwrap(), 1);
        let cancelled = store.get(&objective.id).await.unwrap().unwrap();
        assert_eq!(cancelled.status, ObjectiveStatus::Cancelled);
        assert_eq!(
            cancelled.cancellation_provenance.as_deref(),
            Some("explicit_cancel")
        );
    }

    #[tokio::test]
    async fn terminal_objective_rejects_late_finished_and_technical_failure_decisions() {
        let pool = pool().await;
        crate::agent::delivery_run::ensure_schema(&pool)
            .await
            .unwrap();
        crate::agent::execution_workspace::ensure_schema(&pool)
            .await
            .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-terminal-monotonic".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: None,
                root_turn_id: None,
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO execution_workspaces
             (id, objective_id, repo_identity, repo_root, git_common_dir,
              worktree_path, worktree_identity, branch_name, base_ref, base_sha,
              head_sha, state, lease_owner, lease_expires_at, created_at, updated_at)
             VALUES ('workspace-terminal-monotonic', ?, 'repo-terminal-monotonic',
                     '/tmp/source-terminal-monotonic', '/tmp/common-terminal-monotonic',
                     '/tmp/worktree-terminal-monotonic', 'gitdir-terminal-monotonic',
                     'codefactory/objective-terminal-monotonic', 'origin/main',
                     'base-terminal-monotonic', 'head-terminal-monotonic', 'active',
                     'process-terminal-monotonic', ?, ?, ?)",
        )
        .bind(&objective.id)
        .bind(now + 120_000)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let cancelled = DecisionRouter::route(
            &objective,
            RouteSignal::Cancelled {
                domain: RecoveryDomain::Chat,
                provenance: "explicit_cancel".into(),
            },
        )
        .unwrap();
        let cancelled = store
            .apply_decision(objective.revision, cancelled)
            .await
            .unwrap();
        let workspace_terminal: (String, Option<String>, Option<i64>) = sqlx::query_as(
            "SELECT state, lease_owner, lease_expires_at
             FROM execution_workspaces WHERE objective_id=?",
        )
        .bind(&objective.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(workspace_terminal, ("cleanup_pending".into(), None, None));

        let late = RunOutcome {
            final_text: "late answer".into(),
            final_message_id: Some("assistant-late-answer".into()),
            completion_evidence: CompletionEvidence {
                completed: true,
                ..Default::default()
            },
            input_tokens: 1,
            output_tokens: 1,
            stop_reason: StopReason::Finished,
        };
        assert!(decision_for_run_outcome(&cancelled, &late)
            .unwrap_err()
            .to_string()
            .contains("terminal"));
        assert!(DecisionRouter::route(
            &cancelled,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Chat,
                failure_code: "late_agent_error".into(),
                failure_signature: "sha256:late-agent-error".into(),
                next_observation_at: Utc::now().timestamp_millis(),
                resume_cursor: None,
            },
        )
        .unwrap_err()
        .to_string()
        .contains("terminal"));
        assert_eq!(
            store.get(&cancelled.id).await.unwrap().unwrap().status,
            ObjectiveStatus::Cancelled
        );
    }

    #[tokio::test]
    async fn completed_objective_releases_managed_workspace_lease_atomically() {
        let pool = pool().await;
        crate::agent::delivery_run::ensure_schema(&pool)
            .await
            .unwrap();
        crate::agent::execution_workspace::ensure_schema(&pool)
            .await
            .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-completed-workspace".into(),
                kind: ObjectiveKind::Informational,
                session_id: None,
                root_turn_id: None,
                domain: RecoveryDomain::Chat,
                requested_acceptance: "informational_answer".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO execution_workspaces
             (id, objective_id, repo_identity, repo_root, git_common_dir,
              worktree_path, worktree_identity, branch_name, base_ref, base_sha,
              head_sha, state, lease_owner, lease_expires_at, created_at, updated_at)
             VALUES ('workspace-completed', ?, 'repo-completed', '/tmp/source-completed',
                     '/tmp/common-completed', '/tmp/worktree-completed',
                     'gitdir-completed', 'codefactory/objective-completed', 'origin/main',
                     'base-completed', 'head-completed', 'active', 'process-completed',
                     ?, ?, ?)",
        )
        .bind(&objective.id)
        .bind(now + 120_000)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let evidence = ObjectiveEvidence {
            id: "evidence-completed-workspace".into(),
            kind: EvidenceKind::InformationalAnswer,
            scope: "objective-completed-workspace".into(),
            digest: "sha256:completed-workspace".into(),
            evidence_ref: "db:test/completed-workspace".into(),
            observed_at: now,
            reached_acceptance: "informational_answer".into(),
        };
        let completed = CompletionArbiter::decide(&objective, &[evidence]).unwrap();

        let completed = store
            .apply_decision(objective.revision, completed)
            .await
            .unwrap();

        assert_eq!(completed.status, ObjectiveStatus::Completed);
        let workspace_terminal: (String, Option<String>, Option<i64>) = sqlx::query_as(
            "SELECT state, lease_owner, lease_expires_at
             FROM execution_workspaces WHERE objective_id=?",
        )
        .bind(&objective.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(workspace_terminal, ("cleanup_pending".into(), None, None));
    }

    async fn recovery_ceiling_objective(pool: &SqlitePool, id: &str) -> ObjectiveSnapshot {
        ObjectiveStore::new(pool.clone())
            .create(CreateObjective {
                id: id.into(),
                kind: ObjectiveKind::Informational,
                session_id: Some(format!("session-{id}")),
                root_turn_id: Some(format!("turn-{id}")),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "informational_answer".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap()
    }

    async fn seed_legacy_parked_chat_incident(pool: &SqlitePool, id: &str) -> ObjectiveSnapshot {
        let objective = recovery_ceiling_objective(pool, id).await;
        let now = Utc::now().timestamp_millis();
        let root_turn_id = objective.root_turn_id.as_deref().unwrap();
        sqlx::query(
            "INSERT INTO objective_bindings
             (id, objective_id, domain, resource_kind, resource_id,
              resource_generation, identity_digest, created_at, updated_at)
             VALUES (?, ?, 'chat', 'chat_root_turn', ?, 1, ?, ?, ?)",
        )
        .bind(format!("binding-{id}"))
        .bind(&objective.id)
        .bind(root_turn_id)
        .bind(objective_binding_digest(
            &objective.id,
            "chat_root_turn",
            root_turn_id,
        ))
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE objectives SET revision=revision+1, status='waiting_system',
               decision_type='failed_internal', failure_code=?,
               failure_signature='sha256:legacy-exhausted',
               recovery_owner=?, remediation_id=NULL, next_observation_at=NULL,
               lease_owner=NULL, lease_expires_at=NULL, updated_at=?
             WHERE id=?",
        )
        .bind(TECHNICAL_RECOVERY_EXHAUSTED)
        .bind(OBJECTIVE_INCIDENT_CONTROLLER)
        .bind(now)
        .bind(&objective.id)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO objective_incidents
             (id, objective_id, status, failure_code, failure_signature,
              owner, resume_cursor, opened_at, updated_at)
             VALUES (?, ?, 'open', ?, 'sha256:legacy-exhausted', ?, ?, ?, ?)",
        )
        .bind(format!("incident-{id}"))
        .bind(&objective.id)
        .bind(TECHNICAL_RECOVERY_EXHAUSTED)
        .bind(OBJECTIVE_INCIDENT_CONTROLLER)
        .bind(&objective.root_turn_id)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .unwrap();
        ObjectiveStore::new(pool.clone())
            .get(&objective.id)
            .await
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn capability_revision_reactivates_a_legacy_incident_without_user_reprompt() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let parked = seed_legacy_parked_chat_incident(&pool, "objective-capability-rearm").await;

        store.sync_recovery_capabilities().await.unwrap();
        let reactivated = store
            .reactivate_eligible_incidents(8)
            .await
            .unwrap();

        assert_eq!(reactivated.len(), 1);
        assert_eq!(reactivated[0].id, parked.id);
        assert_eq!(reactivated[0].status, ObjectiveStatus::WaitingSystem);
        assert_ne!(
            reactivated[0].recovery_owner.as_deref(),
            Some(OBJECTIVE_INCIDENT_CONTROLLER)
        );
        assert!(reactivated[0].remediation_id.is_some());
        assert!(reactivated[0].next_observation_at.is_some());
        assert_eq!(
            reactivated[0].recovery_generation,
            parked.recovery_generation
        );

        let incident: (String, String, i64) = sqlx::query_as(
            "SELECT status, reactivation_status, reactivation_count
             FROM objective_incidents WHERE objective_id=?",
        )
        .bind(&parked.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(incident, ("resolved".into(), "admitted".into(), 1));

        let second = store
            .reactivate_eligible_incidents(8)
            .await
            .unwrap();
        assert!(
            second.is_empty(),
            "one capability revision may rearm only once"
        );
    }

    #[tokio::test]
    async fn current_capability_revision_does_not_reactivate_a_new_incident() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let parked = seed_legacy_parked_chat_incident(&pool, "objective-current-capability").await;
        store.sync_recovery_capabilities().await.unwrap();
        sqlx::query(
            "UPDATE objective_incidents
             SET blocked_capability_revision=? WHERE objective_id=?",
        )
        .bind(RECOVERY_CAPABILITY_REVISION)
        .bind(&parked.id)
        .execute(&pool)
        .await
        .unwrap();

        let reactivated = store
            .reactivate_eligible_incidents(8)
            .await
            .unwrap();
        assert!(
            reactivated.is_empty(),
            "restart under the same recovery contract must not buy another budget"
        );
        assert_eq!(
            store.get(&parked.id).await.unwrap().unwrap().status,
            ObjectiveStatus::WaitingSystem,
            "the retired reactivation gate leaves the legacy row for start-up convergence"
        );
        store.reclassify_synthetic_technical_handbacks().await.unwrap();
        assert_parked_system_incident(&store.get(&parked.id).await.unwrap().unwrap());
    }

    #[tokio::test]
    async fn capability_registry_is_domain_scoped_and_requires_a_revision_bump() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        store.sync_recovery_capabilities().await.unwrap();
        let capabilities: Vec<(String, i64)> = sqlx::query_as(
            "SELECT domain, executable FROM recovery_capabilities
             WHERE executable=1 ORDER BY domain",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(capabilities, vec![("chat".into(), 1)]);

        sqlx::query(
            "UPDATE recovery_capabilities SET contract_digest='sha256:drift'
             WHERE domain='chat'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let error = store
            .sync_recovery_capabilities()
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("revision bump"), "{error}");
    }

    #[tokio::test]
    async fn capability_registry_fails_closed_on_binary_downgrade() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        store.sync_recovery_capabilities().await.unwrap();
        sqlx::query(
            "UPDATE recovery_capabilities
             SET revision=?, contract_digest='sha256:newer-contract'
             WHERE domain='chat'",
        )
        .bind(RECOVERY_CAPABILITY_REVISION + 1)
        .execute(&pool)
        .await
        .unwrap();

        let error = store
            .sync_recovery_capabilities()
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("downgrade capability revision"), "{error}");
    }

    #[tokio::test]
    async fn unresolved_side_effect_fences_legacy_incident_reactivation() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let parked = seed_legacy_parked_chat_incident(&pool, "objective-unsafe-rearm").await;
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, revision, action_fingerprint, idempotency_key,
              status, created_at, observed_at)
             VALUES ('unsafe-rearm-receipt', ?, 1, 'sha256:action', 'sha256:key',
                     'unknown', ?, ?)",
        )
        .bind(&parked.id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        store.sync_recovery_capabilities().await.unwrap();
        let reactivated = store
            .reactivate_eligible_incidents(8)
            .await
            .unwrap();
        assert!(reactivated.is_empty());

        // The retired gate must not rearm the legacy row here. Convergence then
        // finishes it as an honest failure, and the unknown receipt is left
        // exactly as it was: nothing was replayed.
        assert_eq!(
            store.get(&parked.id).await.unwrap().unwrap().status,
            ObjectiveStatus::WaitingSystem
        );
        store.reclassify_synthetic_technical_handbacks().await.unwrap();
        let current = store.get(&parked.id).await.unwrap().unwrap();
        assert_parked_system_incident(&current);
        let incident: (String, String) = sqlx::query_as(
            "SELECT status, reactivation_status FROM objective_incidents WHERE objective_id=?",
        )
        .bind(&parked.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(incident, ("resolved".into(), "resolved".into()));
        let receipt: String = sqlx::query_scalar(
            "SELECT status FROM side_effect_receipts WHERE id='unsafe-rearm-receipt'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(receipt, "unknown", "an unknown side effect is never replayed");
    }

    /// R4: start-up convergence turns the retired limbo into the honest failure
    /// terminal, keeps the preserved work visible, and is idempotent — a second
    /// start writes nothing.
    #[tokio::test]
    async fn startup_convergence_finishes_legacy_limbo_exactly_once() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let parked = seed_legacy_parked_chat_incident(&pool, "objective-limbo-converge").await;
        let session_id = parked.session_id.clone().unwrap();
        let root_turn_id = parked.root_turn_id.clone().unwrap();
        let now = Utc::now().timestamp_millis();

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS sessions
             (id TEXT PRIMARY KEY, updated_at INTEGER NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO sessions (id, updated_at) VALUES (?, ?)")
            .bind(&session_id)
            .bind(now - 10_000)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS messages (
               id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT NOT NULL,
               content TEXT NOT NULL, completion_state TEXT, created_at INTEGER NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS chat_turn_state (
               root_turn_id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
               revision INTEGER NOT NULL DEFAULT 1, phase TEXT NOT NULL DEFAULT 'working',
               status TEXT NOT NULL, recent_activity_kind TEXT, recent_activity_label TEXT,
               waiting_reason TEXT, updated_at INTEGER NOT NULL DEFAULT 0,
               completed_at INTEGER, terminal_reason TEXT, turn_settled_at INTEGER,
               stream_closed_at INTEGER, terminal_revision INTEGER,
               objective_revision INTEGER, visible_final_message_id TEXT,
               visible_final_kind TEXT, next_action TEXT, objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, session_id, status, next_action, objective_id)
             VALUES (?, ?, 'waiting_system', 'await_system_recovery', ?)",
        )
        .bind(&root_turn_id)
        .bind(&session_id)
        .bind(&parked.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS delivery_runs (
               id TEXT PRIMARY KEY, objective_id TEXT NOT NULL, status TEXT NOT NULL,
               wait_class TEXT, next_action TEXT,
               next_action_authorized INTEGER NOT NULL DEFAULT 0,
               lease_owner TEXT, lease_expires_at INTEGER,
               failure_code TEXT, failure_class TEXT,
               last_observed_at INTEGER, updated_at INTEGER NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO delivery_runs
             (id, objective_id, status, wait_class, next_action, lease_owner,
              lease_expires_at, last_observed_at, updated_at)
             VALUES ('delivery-limbo-converge', ?, 'platform_incident',
                     'delivery_identity_conflict', 'await_system_capability_change',
                     'owner-a', ?, ?, ?)",
        )
        .bind(&parked.id)
        .bind(now + 60_000)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            store.reclassify_synthetic_technical_handbacks().await.unwrap(),
            1
        );
        assert_parked_system_incident(&store.get(&parked.id).await.unwrap().unwrap());

        let incident: String =
            sqlx::query_scalar("SELECT status FROM objective_incidents WHERE objective_id=?")
                .bind(&parked.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(incident, "resolved");

        let turn: (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT status, terminal_reason, next_action FROM chat_turn_state WHERE root_turn_id=?",
        )
        .bind(&root_turn_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            turn,
            ("completed".into(), Some("objective_failed".into()), None)
        );

        let run: (String, Option<String>, Option<String>, i64) = sqlx::query_as(
            "SELECT status, wait_class, next_action, next_action_authorized
             FROM delivery_runs WHERE id='delivery-limbo-converge'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(run, ("failed".into(), None, None, 0));

        let notice: String = sqlx::query_scalar(
            "SELECT content FROM messages WHERE session_id=? AND role='assistant' LIMIT 1",
        )
        .bind(&session_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(notice.contains("这件事没做成"), "{notice}");
        crate::agent::failure_summary::assert_no_internal_vocabulary(&notice).unwrap();

        assert_eq!(
            store.reclassify_synthetic_technical_handbacks().await.unwrap(),
            0
        );
        let messages_after: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE session_id=?")
                .bind(&session_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            messages_after, 1,
            "a second start must not write another notice"
        );
    }

    async fn claimable_remediations(pool: &SqlitePool, objective_id: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_remediations
             WHERE objective_id=? AND status NOT IN ('completed','cancelled','superseded')",
        )
        .bind(objective_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// How many durable recovery attempts the system actually bought. Each one
    /// is a real re-run of the model, so this is the number the ceiling exists
    /// to bound.
    async fn total_remediations(pool: &SqlitePool, objective_id: &str) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM objective_remediations WHERE objective_id=?")
            .bind(objective_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Drive one system-owned technical failure round and return the durable
    /// objective it settled into.
    async fn route_technical_failure(
        store: &ObjectiveStore,
        current: &ObjectiveSnapshot,
        failure_code: &str,
        failure_signature: &str,
    ) -> ObjectiveSnapshot {
        let decision = DecisionRouter::route(
            current,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Chat,
                failure_code: failure_code.into(),
                failure_signature: failure_signature.into(),
                next_observation_at: Utc::now().timestamp_millis() - 1,
                resume_cursor: current.root_turn_id.clone(),
            },
        )
        .unwrap();
        let waiting = store
            .apply_decision(current.revision, decision)
            .await
            .unwrap();
        if waiting.status == ObjectiveStatus::WaitingSystem
            && waiting.failure_code.as_deref() != Some(TECHNICAL_RECOVERY_EXHAUSTED)
        {
            // A repeating signature is now scheduled with a growing backoff, so
            // driving several rounds in a test has to stand in for the wait the
            // supervisor would really sit through. Pull the observation time
            // forward — the ceiling itself is left exactly as strict as before.
            sqlx::query(
                "UPDATE objective_remediations SET next_observation_at=?
                 WHERE objective_id=? AND status IN ('queued','waiting')",
            )
            .bind(Utc::now().timestamp_millis() - 1)
            .bind(&waiting.id)
            .execute(&store.pool)
            .await
            .unwrap();
            let claims = store
                .claim_due_remediations("recovery-ceiling-test", 1, 30_000)
                .await
                .unwrap();
            assert_eq!(claims.len(), 1, "one queued recovery round must be claimed");
            assert!(store
                .charge_claimed_remediation_attempt(
                    &claims[0].objective.id,
                    &claims[0].remediation_id,
                    "recovery-ceiling-test",
                    claims[0].claim_epoch,
                )
                .await
                .unwrap());
        }
        store.get(&waiting.id).await.unwrap().unwrap()
    }

    /// Exhaustion is the honest failure terminal: the objective is finished,
    /// not parked. It holds no owner, no claimable retry and no future
    /// wake-up — nothing about a later build changes this outcome.
    fn assert_parked_system_incident(objective: &ObjectiveSnapshot) {
        assert_eq!(
            objective.status,
            ObjectiveStatus::Failed,
            "exhausted recovery is a real failure terminal"
        );
        assert!(
            objective.status.is_terminal(),
            "the failure terminal sits beside completed/cancelled"
        );
        assert_eq!(objective.decision_type, DecisionType::FailedInternal);
        assert!(
            !objective.requires_user_action,
            "a technical ceiling cannot manufacture a user-input requirement"
        );
        assert_eq!(
            objective.failure_code.as_deref(),
            Some(TECHNICAL_RECOVERY_EXHAUSTED)
        );
        assert!(objective.request_key.is_none());
        assert!(
            objective.recovery_owner.as_deref() == Some("objective-incident-controller")
                && objective.remediation_id.is_none(),
            "the terminal report names the controller but keeps no claimable retry"
        );
        assert!(
            objective.next_observation_at.is_none(),
            "nothing will ever wake this objective again"
        );
    }

    /// The 2026-08-13 incident: a completion-evidence rejection re-queued a
    /// durable remediation every ~13s forever, each round paying for a real
    /// model call that returned the very same answer.
    #[tokio::test]
    async fn failures_older_than_the_window_do_not_count_toward_the_ceiling() {
        // The ceiling is meant to detect "not making progress right now". Without
        // a window it also remembers that the same error happened once days ago,
        // so an Objective can be condemned by history it already recovered from.
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let mut current = recovery_ceiling_objective(&pool, "objective-window").await;
        let signature = "sha256:one-error-spread-over-days";

        for _ in 0..(MAX_SIGNATURE_RECOVERY_ATTEMPTS - 1) {
            current = route_technical_failure(&store, &current, "adapter_error", signature).await;
        }
        assert_ne!(
            current.failure_code.as_deref(),
            Some(TECHNICAL_RECOVERY_EXHAUSTED),
            "the ceiling must not have been reached yet"
        );

        // Age every attempt so far well past the window, as if they happened on
        // previous days, then drive several more rounds.
        sqlx::query(
            "UPDATE objective_remediations SET created_at=? WHERE objective_id=?",
        )
        .bind(Utc::now().timestamp_millis() - (SIGNATURE_RECOVERY_WINDOW_MS * 4))
        .bind(&current.id)
        .execute(&pool)
        .await
        .unwrap();

        for _ in 0..(MAX_SIGNATURE_RECOVERY_ATTEMPTS - 1) {
            current = route_technical_failure(&store, &current, "adapter_error", signature).await;
        }
        assert_ne!(
            current.failure_code.as_deref(),
            Some(TECHNICAL_RECOVERY_EXHAUSTED),
            "attempts older than the window must not condemn the Objective"
        );
    }

    #[test]
    fn a_dead_endpoint_is_not_told_that_no_input_is_needed() {
        // Saying "你不需要补充输入" is true for a technical ceiling the system will
        // retry on its own. For an unreachable endpoint it is the opposite of
        // true: nothing moves until a reachable model is chosen.
        let generic = parked_incident_message(Some("agent_loop_error"));
        assert!(generic.contains("你不需要补充输入"), "{generic}");

        let dead = parked_incident_message(Some(PROVIDER_ENDPOINT_UNAVAILABLE));
        assert!(!dead.contains("你不需要补充输入"), "{dead}");
        assert!(
            !dead.contains("切换到另一个可用模型"),
            "U25: switching models is an offer, not a demand the system hands back: {dead}"
        );
        assert!(dead.contains("换成另一个可用模型"), "{dead}");
        assert!(dead.contains("进度和上下文都已保留"), "{dead}");

        // U25: the transport case says what can actually be done about it —
        // wait for the network, then pick the same task back up.
        let transport = parked_incident_message(Some(PROVIDER_TRANSPORT_UNREACHABLE));
        assert!(!transport.contains("你不需要补充输入"), "{transport}");
        assert!(!transport.contains("换"), "no model switch is needed: {transport}");
        assert!(transport.contains("继续"), "{transport}");
    }

    /// U25: seed the durable failure series a transport outage leaves behind —
    /// the same row `record_failure_detail` writes when a turn dies with
    /// `error sending request for url (…)`.
    async fn seed_transport_failure(
        pool: &SqlitePool,
        objective_id: &str,
        url: &str,
        created_at: i64,
    ) {
        let detail = serde_json::json!({
            "failure_signature": "sha256:transport",
            "error_text": format!("HTTP error: error sending request for url ({url})"),
        })
        .to_string();
        sqlx::query(
            "INSERT INTO objective_events
             (id, objective_id, revision, event_type, status, decision_type,
              domain, failure_code, detail_json, created_at)
             SELECT ?, id, revision, 'technical_failure_detail', status,
                    decision_type, 'chat', ?, ?, ? FROM objectives WHERE id=?",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(PROVIDER_TRANSPORT_UNREACHABLE)
        .bind(detail)
        .bind(created_at)
        .bind(objective_id)
        .execute(pool)
        .await
        .unwrap();
    }

    /// A local listening socket the probe can reach: the outage is over. No
    /// external network is involved.
    fn reachable_local_route() -> (std::net::TcpListener, String) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        (
            listener,
            format!("http://127.0.0.1:{port}/v1/chat/completions"),
        )
    }

    /// A local port nobody listens on: `connect` is refused at once, so the
    /// probe cannot reach it. Still no external network.
    fn unreachable_local_route() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        format!("http://127.0.0.1:{port}/v1/chat/completions")
    }

    async fn route_transport_failure(
        store: &ObjectiveStore,
        current: &ObjectiveSnapshot,
    ) -> ObjectiveSnapshot {
        let issued = Utc::now().timestamp_millis();
        let decision = DecisionRouter::route(
            current,
            RouteSignal::TechnicalFailure {
                domain: RecoveryDomain::Chat,
                failure_code: PROVIDER_TRANSPORT_UNREACHABLE.into(),
                failure_signature: "sha256:transport".into(),
                next_observation_at: issued + 5_000,
                resume_cursor: current.resume_cursor.clone(),
            },
        )
        .unwrap();
        store
            .apply_decision(current.revision, decision)
            .await
            .unwrap()
    }

    async fn queued_remediations(pool: &SqlitePool, objective_id: &str) -> i64 {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_remediations
             WHERE objective_id=? AND status IN ('queued','waiting')",
        )
        .bind(objective_id)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// U25 policy table: wait inside the window, retry the moment the path
    /// answers, give up only once the window is over.
    #[test]
    fn transport_outage_policy_waits_then_settles_only_after_the_window() {
        let now = 1_700_000_000_000_i64;
        assert_eq!(
            transport_outage_action(now, now - 20_000, 1, false),
            TransportOutageAction::Wait {
                delay_ms: TRANSPORT_PROBE_INTERVAL_MS
            }
        );
        // Three minutes in — the 2026-10-08 DeepSeek outage: still waiting, not
        // a verdict about the task.
        assert!(matches!(
            transport_outage_action(now, now - 180_000, 3, false),
            TransportOutageAction::Wait { .. }
        ));
        // The path answers again: retry, whatever the clock says.
        assert_eq!(
            transport_outage_action(now, now - 180_000, 3, true),
            TransportOutageAction::Retry
        );
        // Still down after the window: settle, with how long and how often.
        assert_eq!(
            transport_outage_action(now, now - TRANSPORT_OUTAGE_WINDOW_MS - 1_000, 7, false),
            TransportOutageAction::GiveUp {
                unreachable_ms: TRANSPORT_OUTAGE_WINDOW_MS + 1_000,
                attempts: 7,
            }
        );
        // The probe interval never steps past the end of the window.
        let remaining = 5_000;
        assert_eq!(
            transport_outage_action(now, now - (TRANSPORT_OUTAGE_WINDOW_MS - remaining), 2, false),
            TransportOutageAction::Wait { delay_ms: remaining }
        );
        assert!(
            MAX_TRANSPORT_RECOVERY_ATTEMPTS > MAX_SIGNATURE_RECOVERY_ATTEMPTS,
            "one outage must not be charged like the ordinary ladder"
        );
        assert_eq!(
            max_signature_attempts_for(Some(PROVIDER_TRANSPORT_UNREACHABLE)),
            MAX_TRANSPORT_RECOVERY_ATTEMPTS
        );
        assert_eq!(
            recovery_backoff_ms_for(Some(PROVIDER_TRANSPORT_UNREACHABLE), 0),
            TRANSPORT_RETRY_BASE_MS
        );
        assert!(
            recovery_backoff_ms_for(Some(PROVIDER_TRANSPORT_UNREACHABLE), 5) > 60_000,
            "the transport ladder has to grow, not repeat a flat interval"
        );
        assert_eq!(
            recovery_backoff_ms_for(Some(PROVIDER_ENDPOINT_UNAVAILABLE), 5),
            DEAD_ENDPOINT_RETRY_MS,
            "an explicit refusal keeps the short path"
        );
    }

    /// U33 / CF-PFB-R12. "No attempt for more than N minutes is a defect" has to
    /// hold for every schedule this module can produce, not just the transport
    /// one — so the bound is asserted against the whole ladder, and against a
    /// proposal that tried to walk an hour out.
    #[test]
    fn no_wait_schedule_may_exceed_the_upper_bound() {
        let now = 1_777_000_000_000_i64;
        assert_eq!(
            bounded_next_observation_at(now, now + 60 * 60 * 1_000),
            now + MAX_WAIT_BEFORE_NEXT_ATTEMPT_MS,
            "a proposed wait an hour out is clamped to the bound"
        );
        assert_eq!(
            bounded_next_observation_at(now, now + 20_000),
            now + 20_000,
            "an ordinary short wait is left exactly as asked"
        );
        for prior in 0..12 {
            for code in [
                Some(PROVIDER_TRANSPORT_UNREACHABLE),
                Some(PROVIDER_ENDPOINT_UNAVAILABLE),
                None,
            ] {
                let delay = recovery_backoff_ms_for(code, prior);
                assert!(
                    delay <= MAX_WAIT_BEFORE_NEXT_ATTEMPT_MS,
                    "the backoff ladder may never exceed the wait bound ({code:?}, prior={prior}, delay={delay})"
                );
            }
        }
        assert!(
            TRANSPORT_PROBE_INTERVAL_MS <= MAX_WAIT_BEFORE_NEXT_ATTEMPT_MS,
            "the outage probe must be more frequent than the bound"
        );
    }

    #[test]
    fn the_probe_targets_the_route_that_just_failed() {
        assert_eq!(
            probe_target_from_error_text(
                "HTTP error: error sending request for url (https://api.deepseek.com/chat/completions)"
            ),
            Some(("api.deepseek.com".to_string(), 443))
        );
        assert_eq!(
            probe_target_from_error_text(
                "error sending request for url (http://127.0.0.1:8123/v1/chat/completions)"
            ),
            Some(("127.0.0.1".to_string(), 8123))
        );
        assert_eq!(probe_target_from_error_text("connection reset by peer"), None);
    }

    /// The probe is the cheap check that replaces a model turn: a local socket
    /// answers, a closed local port does not. Both are synthetic and local.
    #[tokio::test]
    async fn the_probe_answers_for_a_live_socket_and_not_for_a_closed_port() {
        let (listener, _url) = reachable_local_route();
        let port = listener.local_addr().unwrap().port();
        assert!(probe_transport_reachability("127.0.0.1", port).await);
        drop(listener);
        assert!(!probe_transport_reachability("127.0.0.1", port).await);
    }

    /// U25 requirement 2: while the path is still down, the objective waits and
    /// spends nothing — no model turn, no charged attempt.
    #[tokio::test]
    async fn a_path_that_is_still_down_waits_without_spending_an_attempt() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let current = recovery_ceiling_objective(&pool, "objective-transport-still-down").await;
        let url = unreachable_local_route();
        let now = Utc::now().timestamp_millis();
        for offset in [60_000_i64, 30_000] {
            seed_transport_failure(&pool, &current.id, &url, now - offset).await;
        }

        let after = route_transport_failure(&store, &current).await;

        assert_eq!(after.status, ObjectiveStatus::WaitingSystem, "the task is not over");
        assert_eq!(
            after.failure_code.as_deref(),
            Some(PROVIDER_TRANSPORT_UNREACHABLE)
        );
        assert_ne!(after.failure_code.as_deref(), Some(TECHNICAL_RECOVERY_EXHAUSTED));
        assert_eq!(
            queued_remediations(&pool, &after.id).await,
            0,
            "a path that is still down must not queue a charged attempt"
        );
        let next_observation: i64 = after.next_observation_at.unwrap();
        assert!(
            next_observation > Utc::now().timestamp_millis(),
            "the objective must come back to look again"
        );
    }

    /// U25 requirement 2 and 3: three minutes of transport failure, then the
    /// network returns. The objective must not be failed, and it resumes into a
    /// real retry rather than exhausting anything.
    #[tokio::test]
    async fn a_three_minute_outage_that_recovers_does_not_fail_the_objective() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let current = recovery_ceiling_objective(&pool, "objective-transport-recovers").await;
        let (listener, url) = reachable_local_route();
        let now = Utc::now().timestamp_millis();
        // The outage started three minutes ago, with several failed attempts,
        // and is over now: exactly the 2026-10-08 shape.
        for offset in [180_000_i64, 150_000, 120_000, 60_000] {
            seed_transport_failure(&pool, &current.id, &url, now - offset).await;
        }

        let after = route_transport_failure(&store, &current).await;
        drop(listener);

        assert_ne!(
            after.status,
            ObjectiveStatus::Failed,
            "a one-minute network loss is not every approach having failed"
        );
        assert_ne!(after.failure_code.as_deref(), Some(TECHNICAL_RECOVERY_EXHAUSTED));
        assert_eq!(
            queued_remediations(&pool, &after.id).await,
            1,
            "the recovered path must resume into a real retry"
        );
    }

    /// U25 requirement 4: an outage that outlasts the window settles honestly,
    /// saying how long it was unreachable and how many times it was tried, and
    /// using no internal vocabulary.
    #[tokio::test]
    async fn an_outage_past_the_window_settles_with_long_and_honest_wording() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let current = recovery_ceiling_objective(&pool, "objective-transport-window").await;
        let url = unreachable_local_route();
        let now = Utc::now().timestamp_millis();
        for offset in [16 * 60_000_i64, 10 * 60_000, 3 * 60_000, 30_000] {
            seed_transport_failure(&pool, &current.id, &url, now - offset).await;
        }

        let after = route_transport_failure(&store, &current).await;

        assert_eq!(after.status, ObjectiveStatus::Failed);
        assert_eq!(
            after.failure_code.as_deref(),
            Some(PROVIDER_TRANSPORT_UNREACHABLE),
            "the settled reason must explain itself"
        );
        assert!(!after.requires_user_action);

        let detail: String = sqlx::query_scalar(
            "SELECT detail_json FROM objective_events
             WHERE objective_id=? AND event_type='transport_outage_terminal'
             ORDER BY created_at DESC, rowid DESC LIMIT 1",
        )
        .bind(&after.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&detail).unwrap();
        let message = value["message"].as_str().unwrap();
        assert!(message.contains("16 分钟"), "how long: {message}");
        assert!(message.contains("4 次"), "how many times: {message}");
        assert!(message.contains("保留"), "progress is kept: {message}");
        assert!(message.contains("继续"), "how to go on: {message}");
        crate::agent::failure_summary::assert_no_internal_vocabulary(message).unwrap();
        assert_eq!(value["attempts"].as_i64(), Some(4));
    }

    #[tokio::test]
    async fn a_dead_endpoint_converges_fast_instead_of_waiting_out_the_transient_ladder() {
        // The growing ladder exists to give a *transient* condition room to
        // clear. An endpoint that is answering "unavailable" is not transient in
        // that sense: waiting it out just shows "等待中" for twelve minutes while
        // nothing can change without the user picking another model. Measured on
        // a real run: 10:07:06 -> 10:19:15 on a dead DeepSeek endpoint.
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let mut current = recovery_ceiling_objective(&pool, "objective-dead-endpoint").await;

        let mut delays = Vec::new();
        for _ in 0..4 {
            let issued = Utc::now().timestamp_millis();
            let decision = DecisionRouter::route(
                &current,
                RouteSignal::TechnicalFailure {
                    domain: RecoveryDomain::Chat,
                    failure_code: PROVIDER_ENDPOINT_UNAVAILABLE.into(),
                    failure_signature: "sha256:endpoint-is-down".into(),
                    next_observation_at: issued + 5_000,
                    resume_cursor: current.resume_cursor.clone(),
                },
            )
            .unwrap();
            current = store.apply_decision(current.revision, decision).await.unwrap();
            let next: i64 = sqlx::query_scalar(
                "SELECT next_observation_at FROM objective_remediations
                 WHERE objective_id=? ORDER BY created_at DESC, rowid DESC LIMIT 1",
            )
            .bind(&current.id)
            .fetch_one(&pool)
            .await
            .unwrap();
            delays.push(next - issued);
        }

        assert!(
            delays.iter().all(|delay| *delay <= 15_000),
            "a dead endpoint must not be granted the transient ladder, got {delays:?}",
        );
    }

    /// 2026-09-08: four Objectives exhausted their recovery budget on
    /// `agent_loop_error`, and one signature — `sha256:5facdc14…` — showed up in
    /// two unrelated sessions, so it was systematic. Every `decision_applied`
    /// row carried a NULL `detail_json` and only one of the two chat call sites
    /// logged the text, so nothing on the machine could say what the error WAS.
    /// A signature is a fingerprint, not a diagnosis.
    #[tokio::test]
    async fn a_technical_failure_keeps_a_bounded_redacted_copy_of_its_error_text() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let objective = recovery_ceiling_objective(&pool, "objective-detail").await;

        store
            .record_failure_detail(
                &objective.id,
                RecoveryDomain::Chat,
                "agent_loop_error",
                "sha256:deadbeef",
                &format!(
                    "upstream 502 with api_key=sk-live-not-a-real-secret while streaming {}",
                    "x".repeat(4_000)
                ),
            )
            .await
            .unwrap();

        let detail: String = sqlx::query_scalar(
            "SELECT detail_json FROM objective_events
             WHERE objective_id=? AND event_type='technical_failure_detail'",
        )
        .bind(&objective.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let detail: serde_json::Value = serde_json::from_str(&detail).unwrap();
        let text = detail["error_text"].as_str().unwrap();
        assert!(text.contains("upstream 502"), "{text}");
        assert!(
            !text.contains("sk-live-not-a-real-secret"),
            "a stored diagnostic must never carry a credential: {text}"
        );
        assert!(
            text.chars().count() < 2_000,
            "the copy is bounded, not the whole stream: {} chars",
            text.chars().count()
        );
        assert_eq!(detail["failure_signature"], "sha256:deadbeef");

        // Recording a diagnostic is not a decision: it must not move the
        // Objective or spend any part of the recovery budget.
        let after = store.get(&objective.id).await.unwrap().unwrap();
        assert_eq!(after.revision, objective.revision);
        assert_eq!(after.status, objective.status);
    }

    /// 2026-09-08 audit of the production database: five side-effect receipts
    /// had sat at `unknown` for 17 to 20 days, on Objectives cancelled weeks
    /// earlier during a manual "unfreeze". Nothing can ever settle them — the
    /// recovery ladder that owned them is gone — yet the mutation fence counts
    /// them, so the Objective stays poisoned and the only fix on record was
    /// editing the database by hand.
    #[tokio::test]
    async fn unsettled_receipts_on_a_terminal_objective_are_swept() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let live = recovery_ceiling_objective(&pool, "objective-live").await;
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO objectives
             (id, revision, kind, status, decision_type, domain,
              requested_acceptance, cancellation_provenance, completed_at,
              created_surface, created_at, updated_at)
             VALUES ('objective-gone', 1, 'informational', 'cancelled',
                     'cancelled', 'chat', 'informational_answer',
                     'explicit_cancel', ?, 'test', ?, ?)",
        )
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        for (objective_id, key) in [
            ("objective-gone", "dead-1"),
            ("objective-gone", "dead-2"),
            (live.id.as_str(), "live-1"),
        ] {
            sqlx::query(
                "INSERT INTO side_effect_receipts
                 (id, objective_id, revision, action_fingerprint, idempotency_key,
                  status, created_at, observed_at)
                 VALUES (?, ?, 1, ?, ?, 'unknown', ?, ?)",
            )
            .bind(format!("receipt-{key}"))
            .bind(objective_id)
            .bind(key)
            .bind(key)
            .bind(now - 20 * 24 * 3_600_000)
            .bind(now - 20 * 24 * 3_600_000)
            .execute(&pool)
            .await
            .unwrap();
        }

        assert_eq!(store.cancel_receipts_on_terminal_objectives().await.unwrap(), 2);

        let still_uncertain: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM side_effect_receipts
             WHERE objective_id='objective-gone' AND status IN ('started','unknown')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            still_uncertain, 0,
            "a terminal Objective must stop poisoning its own id"
        );

        // A live Objective's unsettled receipt is a real open question and must
        // survive: assuming it never landed is how an effect happens twice.
        let live_status: String = sqlx::query_scalar(
            "SELECT status FROM side_effect_receipts WHERE id='receipt-live-1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(live_status, "unknown");

        assert_eq!(
            store.cancel_receipts_on_terminal_objectives().await.unwrap(),
            0,
            "the sweep is idempotent"
        );
    }

    /// 2026-09-08: one session stopped mid-turn and stayed `active` — no lease,
    /// no queued remediation, `next_observation_at` NULL. The supervisor only
    /// claims due remediations, so nothing owned it and nothing would ever wake
    /// it; 83 minutes later the UI still said 进行中. Not failed, not finished,
    /// just lost. A stalled Objective has to become a real failure so the
    /// recovery ladder — and then the user — can see it.
    #[tokio::test]
    async fn an_abandoned_active_objective_is_reaped_into_a_visible_failure() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        // The production liveness table, in the shape this query reads.
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
               objective_id TEXT, updated_at INTEGER NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let stalled = recovery_ceiling_objective(&pool, "objective-stalled").await;
        let building = recovery_ceiling_objective(&pool, "objective-building").await;
        let now = Utc::now().timestamp_millis();
        for (objective, heartbeat) in [
            (&stalled, now - 90 * 60_000),
            // A healthy turn twelve minutes into a build still publishes
            // activity; it must be left alone.
            (&building, now - 12 * 60_000),
        ] {
            sqlx::query(
                "INSERT INTO chat_turn_state
                 (root_turn_id, session_id, objective_id, updated_at)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(objective.root_turn_id.as_deref().unwrap())
            .bind(objective.session_id.as_deref().unwrap())
            .bind(&objective.id)
            .bind(heartbeat)
            .execute(&pool)
            .await
            .unwrap();
        }

        let reaped = store
            .reap_stalled_active_objectives(STALLED_ACTIVE_OBJECTIVE_MS)
            .await
            .unwrap();
        assert_eq!(
            reaped.iter().map(|o| o.id.as_str()).collect::<Vec<_>>(),
            vec![stalled.id.as_str()],
            "only the Objective nothing will wake again may be reaped"
        );

        let settled = store.get(&stalled.id).await.unwrap().unwrap();
        assert_ne!(
            settled.status,
            ObjectiveStatus::Active,
            "a reaped Objective must stop claiming to be running"
        );
        assert_eq!(
            settled.failure_code.as_deref(),
            Some(OBJECTIVE_PROGRESS_STALLED)
        );

        let untouched = store.get(&building.id).await.unwrap().unwrap();
        assert_eq!(untouched.status, ObjectiveStatus::Active);
        assert_eq!(
            untouched.revision, building.revision,
            "a live turn must not even be revised by the sweep"
        );

        assert!(
            store
                .reap_stalled_active_objectives(STALLED_ACTIVE_OBJECTIVE_MS)
                .await
                .unwrap()
                .is_empty(),
            "reaping is idempotent: a settled Objective is not reaped twice"
        );
    }

    #[tokio::test]
    async fn a_repeating_failure_backs_off_instead_of_burning_the_budget_in_seconds() {
        // Every repeat of the same signature is scheduled at a flat now+5s, so
        // the whole five-attempt budget is spent in well under a minute. A
        // transient condition that would clear on its own never gets the
        // chance, and the objective parks permanently.
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let mut current = recovery_ceiling_objective(&pool, "objective-backoff").await;

        let mut delays = Vec::new();
        for _ in 0..4 {
            let issued = Utc::now().timestamp_millis();
            let decision = DecisionRouter::route(
                &current,
                RouteSignal::TechnicalFailure {
                    domain: RecoveryDomain::Chat,
                    failure_code: "agent_loop_error".into(),
                    failure_signature: "sha256:the-same-error-every-time".into(),
                    next_observation_at: issued + 5_000,
                    resume_cursor: current.resume_cursor.clone(),
                },
            )
            .unwrap();
            current = store.apply_decision(current.revision, decision).await.unwrap();
            let next: i64 = sqlx::query_scalar(
                "SELECT next_observation_at FROM objective_remediations
                 WHERE objective_id=? ORDER BY created_at DESC, rowid DESC LIMIT 1",
            )
            .bind(&current.id)
            .fetch_one(&pool)
            .await
            .unwrap();
            delays.push(next - issued);
        }

        for window in delays.windows(2) {
            assert!(
                window[1] > window[0],
                "a repeating signature must back off, got delays {delays:?}",
            );
        }
        assert!(
            *delays.last().unwrap() >= 30_000,
            "by the fourth repeat the retry must be minutes away, got {delays:?}",
        );
    }

    /// M30(a)(d): declaring a task over must stop its live run first — the
    /// durable `cancel_requested_at` plus the process-local flag the running
    /// loop actually observes — and a settlement that is *not* ending the task
    /// must not stop anything.
    #[tokio::test]
    async fn terminal_settlement_requests_the_live_run_stop_first() {
        let pool = pool().await;
        let objective = recovery_ceiling_objective(&pool, "objective-m30").await;
        let now = Utc::now().timestamp_millis();
        let run_instance_id = format!("run-m30-{}", uuid::Uuid::new_v4());
        // The half the running AgentLoop reads, registered exactly the way a
        // real chat run registers itself.
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        crate::register_chat_run_stop_flag(&run_instance_id, &cancel);
        sqlx::query(
            "INSERT INTO chat_run_controls
             (run_instance_id, session_id, root_turn_id, objective_id,
              objective_revision, status, created_process_instance,
              created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 'active', 'process-m30', ?, ?)",
        )
        .bind(&run_instance_id)
        .bind(objective.session_id.as_deref().unwrap())
        .bind(objective.root_turn_id.as_deref().unwrap())
        .bind(&objective.id)
        .bind(objective.revision)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());

        store
            .request_live_run_stop_before_settlement("objective-m30", false)
            .await;
        assert!(
            !cancel.load(std::sync::atomic::Ordering::SeqCst),
            "a settlement that is not ending the task must not stop a live run"
        );
        let untouched: String =
            sqlx::query_scalar("SELECT status FROM chat_run_controls WHERE run_instance_id=?")
                .bind(&run_instance_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(untouched, "active");

        store
            .request_live_run_stop_before_settlement("objective-m30", true)
            .await;
        assert!(
            cancel.load(std::sync::atomic::Ordering::SeqCst),
            "the live run must be told to stop; a durable row nobody reads stops nothing"
        );
        let (status, cancel_requested_at): (String, Option<i64>) = sqlx::query_as(
            "SELECT status, cancel_requested_at FROM chat_run_controls
             WHERE run_instance_id=?",
        )
        .bind(&run_instance_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(
            cancel_requested_at.is_some(),
            "(d) a settled run must carry cancel_requested_at"
        );
        assert_eq!(
            status, "active",
            "the pending-cancel status is handed back with the stop already carried \
             out, so the durable-cancellation fence cannot refuse the terminal write; \
             the request itself survives as cancel_requested_at"
        );
    }

    #[tokio::test]
    async fn repeated_failure_signature_parks_a_system_owned_incident() {
        let pool = pool().await;
        crate::agent::delivery_run::ensure_schema(&pool)
            .await
            .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let mut current = recovery_ceiling_objective(&pool, "objective-recovery-ceiling").await;
        let delivery_process = crate::agent::delivery_run::ProcessIdentity::new(
            "process-recovery-ceiling-delivery",
            "test",
            "test",
        );
        let delivery = crate::agent::delivery_run::NewDeliveryRun {
            id: "delivery-recovery-ceiling".into(),
            objective_id: current.id.clone(),
            run_kind: "deliver_changes".into(),
            session_id: current.session_id.clone(),
            root_turn_id: current.root_turn_id.clone(),
            task_segment_id: Some("segment-recovery-ceiling".into()),
            task_id: None,
            workspace_path: "/workspace".into(),
            worktree_identity: "worktree:recovery-ceiling".into(),
            repo_identity: "repo:recovery-ceiling".into(),
            base_branch: "main".into(),
            head_branch: "feature/recovery-ceiling".into(),
            change_set_digest: "sha256:recovery-ceiling".into(),
            expected_head_sha: "abc".into(),
            canonical_pr_number: Some(411),
            canonical_pr_url: Some("https://example.invalid/pull/411".into()),
            canonical_head_sha: Some("abc".into()),
            requested_ceiling: "through_release".into(),
            reached_ceiling: "pr_open".into(),
            stage: "takeover_reconciliation".into(),
            status: "platform_incident".into(),
            wait_class: Some("external_state_uncertain".into()),
            next_action: Some("observe_only_reconcile".into()),
            next_action_authorized: true,
            autonomous_completion: true,
        };
        crate::agent::delivery_run::create_delivery_run(
            &pool,
            &delivery,
            &delivery_process,
            Utc::now().timestamp_millis(),
            90_000,
        )
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE messages (
               id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT NOT NULL,
               content TEXT NOT NULL, completion_state TEXT, created_at INTEGER NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages(id, session_id, role, content, created_at)
             VALUES (?, ?, 'user', 'synthetic task', ?)",
        )
        .bind(current.root_turn_id.as_deref().unwrap())
        .bind(current.session_id.as_deref().unwrap())
        .bind(Utc::now().timestamp_millis())
        .execute(&pool)
        .await
        .unwrap();
        // The sidebar orders by sessions.updated_at, so a session the recovery
        // ceiling just wrote a visible notice into must not keep sorting as if
        // nothing happened. Seed it deliberately stale.
        sqlx::query("CREATE TABLE sessions (id TEXT PRIMARY KEY, updated_at INTEGER NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        let stale_session_updated_at = Utc::now().timestamp_millis() - 3 * 24 * 60 * 60 * 1000;
        sqlx::query("INSERT INTO sessions(id, updated_at) VALUES (?, ?)")
            .bind(current.session_id.as_deref().unwrap())
            .bind(stale_session_updated_at)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
               revision INTEGER NOT NULL DEFAULT 1, phase TEXT NOT NULL,
               status TEXT NOT NULL, recent_activity_kind TEXT,
               recent_activity_label TEXT, waiting_reason TEXT,
               updated_at INTEGER NOT NULL, completed_at INTEGER,
               terminal_reason TEXT, objective_id TEXT,
               turn_settled_at INTEGER, stream_closed_at INTEGER,
               terminal_revision INTEGER, objective_revision INTEGER,
               visible_final_message_id TEXT,
               visible_final_kind TEXT, next_action TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, session_id, phase, status, updated_at, objective_id)
             VALUES (?, ?, 'recovering', 'active', ?, ?)",
        )
        .bind(current.root_turn_id.as_deref().unwrap())
        .bind(current.session_id.as_deref().unwrap())
        .bind(Utc::now().timestamp_millis())
        .bind(&current.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_run_controls
             (run_instance_id, session_id, root_turn_id, objective_id,
              objective_revision, status, created_process_instance,
              created_at, updated_at)
             VALUES ('run-recovery-ceiling', ?, ?, ?, ?, 'active',
                     'process-recovery-ceiling', ?, ?)",
        )
        .bind(current.session_id.as_deref().unwrap())
        .bind(current.root_turn_id.as_deref().unwrap())
        .bind(&current.id)
        .bind(current.revision)
        .bind(Utc::now().timestamp_millis())
        .bind(Utc::now().timestamp_millis())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE tool_calls (
               id TEXT PRIMARY KEY, objective_id TEXT,
               status TEXT NOT NULL, result TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tool_calls(id, objective_id, status, result)
             VALUES ('waiting-read-only-command', ?, 'waiting', 'external_state_uncertain')",
        )
        .bind(&current.id)
        .execute(&pool)
        .await
        .unwrap();
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO side_effect_receipts
             (id, objective_id, revision, action_fingerprint, idempotency_key,
              status, created_at, observed_at)
             VALUES ('recovery-ceiling-unknown-receipt', ?, ?,
                     'sha256:unknown', 'sha256:unknown-key', 'unknown', ?, ?)",
        )
        .bind(&current.id)
        .bind(current.revision)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        let signature = format!("{}:Finished:none", current.id);

        let mut queued_rounds = 0_i64;
        for _ in 0..(MAX_SIGNATURE_RECOVERY_ATTEMPTS + 4) {
            if current.failure_code.as_deref() == Some(TECHNICAL_RECOVERY_EXHAUSTED) {
                break;
            }
            current = route_technical_failure(
                &store,
                &current,
                "completion_evidence_incomplete",
                &signature,
            )
            .await;
            if current.status == ObjectiveStatus::WaitingSystem
                && current.failure_code.as_deref() != Some(TECHNICAL_RECOVERY_EXHAUSTED)
            {
                queued_rounds += 1;
            }
        }

        assert_eq!(
            queued_rounds,
            max_signature_attempts_for(Some(COMPLETION_EVIDENCE_INCOMPLETE)),
            "system recovery must stop re-queueing at the global ceiling"
        );
        assert_parked_system_incident(&current);
        assert_eq!(
            claimable_remediations(&pool, &current.id).await,
            0,
            "the supervisor must have nothing left to claim"
        );
        let session_touched: i64 = sqlx::query_scalar("SELECT updated_at FROM sessions WHERE id=?")
            .bind(current.session_id.as_deref().unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(
            session_touched > stale_session_updated_at,
            "writing the incident notice must advance sessions.updated_at so the sidebar \
             does not bury a session it just wrote to",
        );
        let delivery_projection: (String, Option<String>, i64, Option<String>, Option<i64>) =
            sqlx::query_as(
                "SELECT status, wait_class, next_action_authorized,
                        lease_owner, lease_expires_at
                 FROM delivery_runs WHERE id='delivery-recovery-ceiling'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            delivery_projection,
            ("failed".into(), None, 0, None, None),
            "Objective recovery exhaustion must atomically fail its linked DeliveryRun"
        );
        let projection: (
            String,
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT status, turn_settled_at, stream_closed_at,
                    terminal_revision, visible_final_message_id,
                    visible_final_kind, next_action
             FROM chat_turn_state WHERE objective_id=?",
        )
        .bind(&current.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(projection.0, "completed");
        assert!(
            projection.1.is_some() && projection.2.is_some(),
            "the failure terminal must durably settle the turn and close its stream"
        );
        assert_eq!(projection.3, Some(current.revision));
        assert!(projection
            .4
            .as_deref()
            .is_some_and(|value| !value.is_empty()));
        assert_eq!(projection.5.as_deref(), Some("assistant_final"));
        assert_eq!(projection.6, None, "a terminal turn queues no next action");
        let visible_final: (String, String) =
            sqlx::query_as("SELECT role, content FROM messages WHERE id=?")
                .bind(projection.4.as_deref().unwrap())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(visible_final.0, "assistant");
        // U1b: this Objective parked with a canonical PR already open, and the
        // only unconfirmed thing is that the checks were rerun. The honest
        // terminal states that fact — denying the work would be false.
        assert!(
            visible_final.1.contains("改动已交付到 PR #411"),
            "{}",
            visible_final.1
        );
        assert!(
            !visible_final.1.contains("这件事没做成"),
            "a delivered change must not be reported as not done: {}",
            visible_final.1
        );
        crate::agent::failure_summary::assert_no_internal_vocabulary(&visible_final.1).unwrap();
        assert!(
            !visible_final.1.contains(TECHNICAL_RECOVERY_EXHAUSTED),
            "the visible failure report must not expose an internal reason code"
        );
        let open_incidents: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_incidents
             WHERE objective_id=? AND status='open'",
        )
        .bind(&current.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            open_incidents, 0,
            "the failure terminal leaves no incident waiting for anything"
        );
        let run_control: (String, Option<i64>) = sqlx::query_as(
            "SELECT status, settled_at FROM chat_run_controls
             WHERE objective_id=?",
        )
        .bind(&current.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(run_control.0, "completed");
        assert!(
            run_control.1.is_some(),
            "handback must release the run lock"
        );
        let tool: (String, Option<String>) =
            sqlx::query_as("SELECT status, result FROM tool_calls WHERE objective_id=?")
                .bind(&current.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            tool.0, "blocked",
            "no tool may keep a live clock after handback"
        );
        assert!(
            !tool
                .1
                .unwrap_or_default()
                .contains("external_state_uncertain"),
            "the terminal tool projection must not expose an internal recovery code"
        );
        let receipt_status: String = sqlx::query_scalar(
            "SELECT status FROM side_effect_receipts
             WHERE id='recovery-ceiling-unknown-receipt'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            receipt_status, "unknown",
            "handback must not rewrite unresolved external-state audit truth"
        );
    }

    /// A recovery ceiling bounds system-owned retries; it must not permanently
    /// poison the same Objective after the user explicitly supplies a new turn.
    /// The opaque Objective identity and audit history stay intact, while the
    /// new user-driven generation receives its own bounded recovery budget.
    #[tokio::test]
    async fn user_reprompt_after_exhaustion_starts_a_new_bounded_recovery_generation() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE messages (
               id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT NOT NULL,
               content TEXT NOT NULL, completion_state TEXT, created_at INTEGER NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               revision INTEGER NOT NULL DEFAULT 1,
               phase TEXT NOT NULL DEFAULT 'working',
               status TEXT NOT NULL,
               recent_activity_kind TEXT,
               recent_activity_label TEXT,
               waiting_reason TEXT,
               updated_at INTEGER NOT NULL DEFAULT 0,
               completed_at INTEGER,
               terminal_reason TEXT,
               turn_settled_at INTEGER,
               stream_closed_at INTEGER,
               terminal_revision INTEGER,
               objective_revision INTEGER,
               visible_final_message_id TEXT,
               visible_final_kind TEXT,
               next_action TEXT,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let mut current = recovery_ceiling_objective(&pool, "objective-user-reprompt").await;
        let signature = "sha256:user-reprompt-same-failure";

        while current.failure_code.as_deref() != Some(TECHNICAL_RECOVERY_EXHAUSTED) {
            current = route_technical_failure(
                &store,
                &current,
                "completion_evidence_incomplete",
                signature,
            )
            .await;
        }
        assert_parked_system_incident(&current);

        let original_root_turn_id = current.root_turn_id.clone().unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, status, objective_id)
             VALUES (?, ?, 'waiting_system', ?),
                    ('turn-user-reprompt', ?, 'active', NULL)",
        )
        .bind(&original_root_turn_id)
        .bind(current.session_id.as_deref().unwrap())
        .bind(&current.id)
        .bind(current.session_id.as_deref().unwrap())
        .execute(&pool)
        .await
        .unwrap();

        let reopened = store
            .ensure_or_continue_chat_objective(
                current.session_id.as_deref().unwrap(),
                "turn-user-reprompt",
                Some(&original_root_turn_id),
                current.kind,
                &current.requested_acceptance,
            )
            .await
            .unwrap();
        assert_eq!(
            reopened.id, current.id,
            "the business Objective stays stable"
        );
        assert_eq!(reopened.status, ObjectiveStatus::Active);

        let first_new_failure = route_technical_failure(
            &store,
            &reopened,
            "completion_evidence_incomplete",
            signature,
        )
        .await;
        assert_eq!(
            first_new_failure.status,
            ObjectiveStatus::WaitingSystem,
            "old exhausted attempts must not consume the user-driven generation's budget"
        );
        assert_eq!(first_new_failure.recovery_generation, 1);
        assert_eq!(claimable_remediations(&pool, &reopened.id).await, 1);

        let mut current_generation = first_new_failure;
        while current_generation.failure_code.as_deref() != Some(TECHNICAL_RECOVERY_EXHAUSTED) {
            current_generation = route_technical_failure(
                &store,
                &current_generation,
                "completion_evidence_incomplete",
                signature,
            )
            .await;
        }
        assert_parked_system_incident(&current_generation);
        assert_eq!(current_generation.recovery_generation, 1);
        assert_eq!(claimable_remediations(&pool, &reopened.id).await, 0);
        let attempts_by_generation: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT recovery_generation, SUM(execution_attempt_index)
             FROM objective_remediations WHERE objective_id=?
             GROUP BY recovery_generation ORDER BY recovery_generation",
        )
        .bind(&reopened.id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            attempts_by_generation,
            vec![
                (
                    0,
                    max_signature_attempts_for(Some(COMPLETION_EVIDENCE_INCOMPLETE))
                ),
                (
                    1,
                    max_signature_attempts_for(Some(COMPLETION_EVIDENCE_INCOMPLETE))
                )
            ],
            "each user-authorized generation stays independently bounded"
        );
    }

    /// A second user-driven generation may retry the work, but it must not
    /// re-print the same system-incident notice every time it exhausts again.
    /// The visible message is written once per Objective; later generations
    /// reuse it instead of spamming the transcript.
    #[tokio::test]
    async fn reprompt_after_exhaustion_does_not_repeat_the_incident_notice() {
        let pool = pool().await;
        sqlx::query(
            "CREATE TABLE messages (
               id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT NOT NULL,
               content TEXT NOT NULL, completion_state TEXT, created_at INTEGER NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               revision INTEGER NOT NULL DEFAULT 1,
               phase TEXT NOT NULL DEFAULT 'working',
               status TEXT NOT NULL,
               recent_activity_kind TEXT,
               recent_activity_label TEXT,
               waiting_reason TEXT,
               updated_at INTEGER NOT NULL DEFAULT 0,
               completed_at INTEGER,
               terminal_reason TEXT,
               turn_settled_at INTEGER,
               stream_closed_at INTEGER,
               terminal_revision INTEGER,
               objective_revision INTEGER,
               visible_final_message_id TEXT,
               visible_final_kind TEXT,
               next_action TEXT,
               objective_id TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let mut current = recovery_ceiling_objective(&pool, "objective-notice-once").await;
        let signature = "sha256:notice-once-same-failure";

        while current.failure_code.as_deref() != Some(TECHNICAL_RECOVERY_EXHAUSTED) {
            current = route_technical_failure(
                &store,
                &current,
                "completion_evidence_incomplete",
                signature,
            )
            .await;
        }
        assert_parked_system_incident(&current);

        let original_root_turn_id = current.root_turn_id.clone().unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, status, objective_id)
             VALUES (?, ?, 'waiting_system', ?),
                    ('turn-notice-once-reprompt', ?, 'active', NULL)",
        )
        .bind(&original_root_turn_id)
        .bind(current.session_id.as_deref().unwrap())
        .bind(&current.id)
        .bind(current.session_id.as_deref().unwrap())
        .execute(&pool)
        .await
        .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let reopened = store
            .ensure_or_continue_chat_objective(
                current.session_id.as_deref().unwrap(),
                "turn-notice-once-reprompt",
                Some(&original_root_turn_id),
                current.kind,
                &current.requested_acceptance,
            )
            .await
            .unwrap();
        assert_eq!(reopened.status, ObjectiveStatus::Active);

        let mut current_generation = reopened;
        while current_generation.failure_code.as_deref()
            != Some(TECHNICAL_RECOVERY_EXHAUSTED)
        {
            current_generation = route_technical_failure(
                &store,
                &current_generation,
                "completion_evidence_incomplete",
                signature,
            )
            .await;
        }
        assert_parked_system_incident(&current_generation);

        let notice_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM messages
             WHERE content LIKE '这件事没做成%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            notice_count, 1,
            "a settled turn writes one report, never a duplicate"
        );
        let distinct_notices: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT visible_final_message_id) FROM chat_turn_state
             WHERE objective_id=? AND visible_final_kind='assistant_final'
               AND visible_final_message_id IS NOT NULL",
        )
        .bind(&current_generation.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            distinct_notices, notice_count,
            "each settled turn points at its own report instead of reusing a stale one"
        );
    }

    /// U1b (2026-10-08). A change that already reached a PR, where the only gap
    /// is that the completion gate could not confirm the checks were rerun, must
    /// be reported as delivered with the specific unmet checks — never as
    /// "这件事没做成".
    #[tokio::test]
    async fn a_delivered_change_with_unconfirmed_checks_is_reported_as_delivered() {
        let pool = pool().await;
        settlement_tables(&pool).await;
        crate::agent::delivery_run::ensure_schema(&pool).await.unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let mut current = recovery_ceiling_objective(&pool, "objective-u1b-delivered").await;
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "INSERT INTO objective_events
             (id, objective_id, revision, event_type, status, decision_type, domain,
              failure_code, detail_json, created_at)
             VALUES ('event-u1b-verdict', ?, 3, 'completion_gate_verdict', 'active',
                     'apply_recommended', 'chat', ?, ?, ?)",
        )
        .bind(&current.id)
        .bind(COMPLETION_EVIDENCE_INCOMPLETE)
        .bind(
            serde_json::json!({
                "verdict": "completion_evidence_incomplete",
                "outcome_count": 3,
                "blockers": [{
                    "kind": "failed_verification",
                    "message": "修改后没有重跑测试",
                    "check": "check X",
                    "command": "cargo test --lib",
                }],
            })
            .to_string(),
        )
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO delivery_runs
             (id, objective_id, run_kind, requested_ceiling, reached_ceiling, stage,
              status, canonical_pr_url, canonical_pr_number, last_observed_at,
              last_progress_at, app_version, app_build, process_instance,
              created_at, updated_at)
             VALUES ('run-u1b-delivered', ?, 'deliver_changes', 'pr_only', 'pr_only',
                     'delivery', 'waiting', ?, 572, ?, ?, '1.82.5', 'u1b-build',
                     'u1b-process', ?, ?)",
        )
        .bind(&current.id)
        .bind("https://github.com/BumStill/CodeFactory/pull/572")
        .bind(now)
        .bind(now)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, session_id, status, objective_id, updated_at)
             VALUES (?, ?, 'waiting_system', ?, ?)",
        )
        .bind(current.root_turn_id.as_deref().unwrap())
        .bind(format!("session-{}", current.id))
        .bind(&current.id)
        .bind(now)
        .execute(&pool)
        .await
        .unwrap();

        let signature = "sha256:u1b-delivered-checks";
        while current.failure_code.as_deref() != Some(TECHNICAL_RECOVERY_EXHAUSTED) {
            current = route_technical_failure(
                &store,
                &current,
                COMPLETION_EVIDENCE_INCOMPLETE,
                signature,
            )
            .await;
        }
        assert_parked_system_incident(&current);

        let content: String = sqlx::query_scalar(
            "SELECT content FROM messages WHERE session_id=?
             ORDER BY created_at DESC, rowid DESC LIMIT 1",
        )
        .bind(format!("session-{}", current.id))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(content.contains("PR #572"), "{content}");
        assert!(content.contains("check X"), "{content}");
        assert!(!content.contains("没做成"), "{content}");
        assert!(!content.contains("目标没有达成"), "{content}");
        crate::agent::failure_summary::assert_no_internal_vocabulary(&content).unwrap();
    }

    /// U1b negative control: the same verification gap with nothing delivered
    /// keeps the existing honest failure terminal word for word.
    #[tokio::test]
    async fn an_undelivered_verification_gap_keeps_the_honest_failure_wording() {
        let pool = pool().await;
        settlement_tables(&pool).await;
        let store = ObjectiveStore::new(pool.clone());
        let mut current = recovery_ceiling_objective(&pool, "objective-u1b-undelivered").await;
        sqlx::query(
            "INSERT INTO objective_events
             (id, objective_id, revision, event_type, status, decision_type, domain,
              failure_code, detail_json, created_at)
             VALUES ('event-u1b-undelivered', ?, 3, 'completion_gate_verdict', 'active',
                     'apply_recommended', 'chat', ?, ?, ?)",
        )
        .bind(&current.id)
        .bind(COMPLETION_EVIDENCE_INCOMPLETE)
        .bind(
            serde_json::json!({
                "verdict": "completion_evidence_incomplete",
                "outcome_count": 3,
                "blockers": [{
                    "kind": "failed_verification",
                    "message": "修改后没有重跑测试",
                    "check": "check X",
                    "command": "cargo test --lib",
                }],
            })
            .to_string(),
        )
        .bind(Utc::now().timestamp_millis())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, session_id, status, objective_id, updated_at)
             VALUES (?, ?, 'waiting_system', ?, ?)",
        )
        .bind(current.root_turn_id.as_deref().unwrap())
        .bind(format!("session-{}", current.id))
        .bind(&current.id)
        .bind(Utc::now().timestamp_millis())
        .execute(&pool)
        .await
        .unwrap();

        let signature = "sha256:u1b-undelivered-checks";
        while current.failure_code.as_deref() != Some(TECHNICAL_RECOVERY_EXHAUSTED) {
            current = route_technical_failure(
                &store,
                &current,
                COMPLETION_EVIDENCE_INCOMPLETE,
                signature,
            )
            .await;
        }
        assert_parked_system_incident(&current);

        let content: String = sqlx::query_scalar(
            "SELECT content FROM messages WHERE session_id=?
             ORDER BY created_at DESC, rowid DESC LIMIT 1",
        )
        .bind(format!("session-{}", current.id))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(content.contains("这件事没做成"), "{content}");
        assert!(!content.contains("改动已交付到"), "{content}");
    }

    async fn exhausted_reprompt_compatibility_fixture(
        user_reprompt_driver: Option<&str>,
    ) -> (SqlitePool, ObjectiveStore, String) {
        let pool = pool().await;
        sqlx::query("CREATE TABLE sessions (id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE messages (
               id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
               role TEXT NOT NULL, content TEXT NOT NULL,
               completion_state TEXT, created_at INTEGER NOT NULL
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE chat_turn_state (
               root_turn_id TEXT PRIMARY KEY, session_id TEXT NOT NULL,
               revision INTEGER NOT NULL DEFAULT 1, phase TEXT NOT NULL,
               status TEXT NOT NULL, recent_activity_kind TEXT,
               recent_activity_label TEXT, waiting_reason TEXT,
               updated_at INTEGER NOT NULL, completed_at INTEGER,
               terminal_reason TEXT, objective_id TEXT,
               user_reprompt_driver TEXT,
               turn_settled_at INTEGER, stream_closed_at INTEGER,
               terminal_revision INTEGER, objective_revision INTEGER,
               visible_final_message_id TEXT,
               visible_final_kind TEXT, next_action TEXT
             )",
        )
        .execute(&pool)
        .await
        .unwrap();
        let session_id = "session-exhausted-reprompt-compat";
        let root_turn_id = "turn-exhausted-reprompt-compat";
        sqlx::query("INSERT INTO sessions(id) VALUES (?)")
            .bind(session_id)
            .execute(&pool)
            .await
            .unwrap();
        let store = ObjectiveStore::new(pool.clone());
        let objective = store
            .create(CreateObjective {
                id: "objective-exhausted-reprompt-compat".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some(session_id.into()),
                root_turn_id: Some("turn-original".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        sqlx::query(
            "UPDATE objectives SET revision=7, status='waiting_core_input',
               decision_type='core_input_required', requires_user_action=1,
               request_key=?, failure_code=?, failure_signature='sha256:old',
               recovery_owner=NULL, remediation_id=NULL, resume_cursor=?,
               recovery_generation=0, completed_at=? WHERE id=?",
        )
        .bind(format!("{TECHNICAL_RECOVERY_EXHAUSTED}:{}", objective.id))
        .bind(TECHNICAL_RECOVERY_EXHAUSTED)
        .bind(root_turn_id)
        .bind(Utc::now().timestamp_millis())
        .bind(&objective.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO messages(id, session_id, role, content, created_at)
             VALUES (?, ?, 'user', '继续', ?)",
        )
        .bind(root_turn_id)
        .bind(session_id)
        .bind(Utc::now().timestamp_millis())
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, session_id, phase, status, recent_activity_kind,
              recent_activity_label, waiting_reason, updated_at, completed_at,
              terminal_reason, objective_id, user_reprompt_driver)
             VALUES (?, ?, 'waiting', 'waiting_core_input', ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(root_turn_id)
        .bind(session_id)
        .bind(TECHNICAL_RECOVERY_EXHAUSTED)
        .bind("系统多轮自动恢复没有进展")
        .bind(TECHNICAL_RECOVERY_EXHAUSTED)
        .bind(Utc::now().timestamp_millis())
        .bind(Utc::now().timestamp_millis())
        .bind(TECHNICAL_RECOVERY_EXHAUSTED)
        .bind(&objective.id)
        .bind(user_reprompt_driver)
        .execute(&pool)
        .await
        .unwrap();
        (pool, store, objective.id)
    }

    #[tokio::test]
    async fn startup_recovers_a_v1819_exhausted_reprompt_exactly_once() {
        let (pool, store, objective_id) =
            exhausted_reprompt_compatibility_fixture(Some("core_input_response")).await;

        assert_eq!(
            store
                .reconcile_unconsumed_exhausted_chat_reprompts()
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .reconcile_unconsumed_exhausted_chat_reprompts()
                .await
                .unwrap(),
            0,
            "a restart must not buy the compatibility generation twice"
        );
        let recovered = store.get(&objective_id).await.unwrap().unwrap();
        assert_eq!(recovered.status, ObjectiveStatus::WaitingSystem);
        assert_eq!(recovered.recovery_generation, 1);
        assert_eq!(
            recovered.resume_cursor.as_deref(),
            Some("turn-exhausted-reprompt-compat")
        );
        assert_eq!(claimable_remediations(&pool, &objective_id).await, 1);
        let turn: (String, String, Option<String>) = sqlx::query_as(
            "SELECT status, phase, terminal_reason FROM chat_turn_state
             WHERE root_turn_id='turn-exhausted-reprompt-compat'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(turn, ("waiting_system".into(), "recovering".into(), None));
    }

    #[tokio::test]
    async fn startup_does_not_reopen_exhaustion_without_an_unconsumed_user_reprompt() {
        let (pool, store, objective_id) = exhausted_reprompt_compatibility_fixture(None).await;
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, session_id, phase, status, recent_activity_kind,
              recent_activity_label, updated_at, completed_at, terminal_reason,
              objective_id)
             VALUES ('turn-historical', 'session-exhausted-reprompt-compat',
                     'finalizing', 'completed', 'objective_completed',
                     '历史回合已完成', 1, 1, 'complete', ?)",
        )
        .bind(&objective_id)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            store
                .reconcile_unconsumed_exhausted_chat_reprompts()
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .reclassify_synthetic_technical_handbacks()
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .reclassify_synthetic_technical_handbacks()
                .await
                .unwrap(),
            0,
            "the compatibility migration is idempotent"
        );
        let untouched = store.get(&objective_id).await.unwrap().unwrap();
        assert_parked_system_incident(&untouched);
        assert_eq!(untouched.recovery_generation, 0);
        let historical_status: String = sqlx::query_scalar(
            "SELECT status FROM chat_turn_state WHERE root_turn_id='turn-historical'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(historical_status, "completed");
    }

    /// Adapter failures defer the same row instead of creating a new one. The
    /// ceiling must count those real claims or a single remediation can loop
    /// forever across process restarts.
    #[tokio::test]
    async fn same_remediation_claim_defer_reclaim_hits_the_recovery_ceiling() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let current = recovery_ceiling_objective(&pool, "objective-same-row-reclaim").await;
        let waiting =
            route_technical_failure(&store, &current, "adapter_error", "sha256:same-row-reclaim")
                .await;
        let remediation_id = waiting.remediation_id.clone().unwrap();

        // route_technical_failure bought claim #1. Repeated adapter failures
        // keep deferring exactly that row.
        for expected_claim in 1..MAX_SIGNATURE_RECOVERY_ATTEMPTS {
            store
                .defer_claimed_remediation(
                    &waiting.id,
                    &remediation_id,
                    "recovery-ceiling-test",
                    expected_claim,
                    1_000,
                )
                .await
                .unwrap();
            sqlx::query("UPDATE objective_remediations SET next_observation_at=? WHERE id=?")
                .bind(Utc::now().timestamp_millis() - 1)
                .bind(&remediation_id)
                .execute(&pool)
                .await
                .unwrap();
            let claim = store
                .claim_due_remediations("recovery-ceiling-test", 1, 30_000)
                .await
                .unwrap();
            assert_eq!(claim.len(), 1);
            assert_eq!(claim[0].claim_epoch, expected_claim + 1);
            assert!(store
                .charge_claimed_remediation_attempt(
                    &waiting.id,
                    &remediation_id,
                    "recovery-ceiling-test",
                    claim[0].claim_epoch,
                )
                .await
                .unwrap());
        }

        store
            .defer_claimed_remediation(
                &waiting.id,
                &remediation_id,
                "recovery-ceiling-test",
                MAX_SIGNATURE_RECOVERY_ATTEMPTS,
                1_000,
            )
            .await
            .unwrap();
        sqlx::query("UPDATE objective_remediations SET next_observation_at=? WHERE id=?")
            .bind(Utc::now().timestamp_millis() - 1)
            .bind(&remediation_id)
            .execute(&pool)
            .await
            .unwrap();

        let terminal_poll = store
            .claim_due_remediation_batch("replacement-process", 1, 30_000)
            .await
            .unwrap();
        assert!(terminal_poll.claims.is_empty());
        assert_eq!(terminal_poll.terminal_transitions.len(), 1);
        assert_parked_system_incident(&terminal_poll.terminal_transitions[0]);
        let exhausted = store.get(&waiting.id).await.unwrap().unwrap();
        assert_parked_system_incident(&exhausted);
        assert_eq!(claimable_remediations(&pool, &waiting.id).await, 0);
    }

    /// A changed failure code must not buy another full budget: the tally is a
    /// lifetime per-signature count, not a consecutive streak that churn resets.
    #[tokio::test]
    async fn alternating_failure_codes_cannot_reset_the_recovery_ceiling() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let mut current = recovery_ceiling_objective(&pool, "objective-recovery-churn").await;
        let signatures = [
            format!("{}:Finished:none", current.id),
            format!("{}:FailedInternal:none", current.id),
        ];

        let mut rounds = 0_usize;
        while current.failure_code.as_deref() != Some(TECHNICAL_RECOVERY_EXHAUSTED) && rounds < 64 {
            let index = rounds % signatures.len();
            current = route_technical_failure(
                &store,
                &current,
                if index == 0 {
                    "completion_evidence_incomplete"
                } else {
                    "failed_internal"
                },
                &signatures[index],
            )
            .await;
            rounds += 1;
        }

        assert_parked_system_incident(&current);
        assert!(
            (rounds as i64) <= MAX_SIGNATURE_RECOVERY_ATTEMPTS * signatures.len() as i64 + 1,
            "alternating two signatures bought {rounds} rounds; the per-signature \
             tally must survive an intervening different failure code"
        );
        assert_eq!(claimable_remediations(&pool, &current.id).await, 0);
    }

    /// Signature churn (a failure signature that embeds varying error text)
    /// must still terminate — otherwise the ceiling is trivially bypassed.
    #[tokio::test]
    async fn churning_failure_signatures_still_reach_the_objective_recovery_ceiling() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let mut current = recovery_ceiling_objective(&pool, "objective-recovery-unique").await;

        let mut rounds = 0_i64;
        while current.failure_code.as_deref() != Some(TECHNICAL_RECOVERY_EXHAUSTED)
            && rounds < MAX_OBJECTIVE_RECOVERY_ATTEMPTS * 4
        {
            current = route_technical_failure(
                &store,
                &current,
                "agent_loop_error",
                &format!("sha256:unique-{rounds}"),
            )
            .await;
            rounds += 1;
        }

        assert_parked_system_incident(&current);
        assert_eq!(
            total_remediations(&pool, &current.id).await,
            MAX_OBJECTIVE_RECOVERY_ATTEMPTS,
            "a never-repeating signature must still hit the per-objective backstop"
        );
        assert_eq!(claimable_remediations(&pool, &current.id).await, 0);
    }

    /// Requirement 3 verified against the real routing path, not a hand-built
    /// envelope: a `Finished` run whose evidence never satisfies the objective
    /// kind produces the identical signature every round, so re-running the
    /// same prompt for the same answer is no progress and must count.
    #[tokio::test]
    async fn completion_evidence_gate_rejection_counts_as_no_progress() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let mut current = store
            .create(CreateObjective {
                id: "objective-evidence-gate-loop".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-evidence-gate-loop".into()),
                root_turn_id: Some("turn-evidence-gate-loop".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();

        // The model answers every round; the gate rejects every round because
        // no ChangeSet/PostChangeValidation evidence ever appears.
        let rerun = || RunOutcome {
            final_text: "同一段答案".into(),
            final_message_id: Some("assistant-repeated-answer".into()),
            completion_evidence: CompletionEvidence::default(),
            input_tokens: 12,
            output_tokens: 34,
            stop_reason: StopReason::Finished,
        };

        let mut signatures = std::collections::HashSet::new();
        let mut rounds = 0_i64;
        while current.failure_code.as_deref() != Some(TECHNICAL_RECOVERY_EXHAUSTED)
            && rounds < MAX_SIGNATURE_RECOVERY_ATTEMPTS * 4
        {
            let decision = decision_for_run_outcome(&current, &rerun()).unwrap();
            assert_eq!(
                decision.failure_code.as_deref(),
                Some("completion_evidence_incomplete")
            );
            signatures.insert(decision.failure_signature.clone().unwrap());
            current = store
                .apply_decision(current.revision, decision)
                .await
                .unwrap();
            if current.status == ObjectiveStatus::WaitingSystem
                && current.failure_code.as_deref() != Some(TECHNICAL_RECOVERY_EXHAUSTED)
            {
                sqlx::query("UPDATE objective_remediations SET next_observation_at=? WHERE id=?")
                    .bind(Utc::now().timestamp_millis() - 1)
                    .bind(current.remediation_id.as_deref().unwrap())
                    .execute(&pool)
                    .await
                    .unwrap();
                let claims = store
                    .claim_due_remediations("evidence-gate-test", 1, 30_000)
                    .await
                    .unwrap();
                assert_eq!(claims.len(), 1);
                assert!(store
                    .charge_claimed_remediation_attempt(
                        &claims[0].objective.id,
                        &claims[0].remediation_id,
                        "evidence-gate-test",
                        claims[0].claim_epoch,
                    )
                    .await
                    .unwrap());
                current = store.get(&current.id).await.unwrap().unwrap();
            }
            rounds += 1;
        }

        assert_eq!(
            signatures.len(),
            1,
            "re-running the same prompt for the same answer must keep one signature"
        );
        assert_eq!(
            total_remediations(&pool, &current.id).await,
            MAX_STAGNANT_COMPLETION_ATTEMPTS,
            "an unchanged answer against unchanged evidence may buy only the \
             stagnation bound of model re-runs, not the full transient ladder"
        );
        assert_parked_system_incident(&current);
        assert_eq!(claimable_remediations(&pool, &current.id).await, 0);
    }

    /// The other half of the stagnation bound. A round that GATHERED something
    /// must not be charged like a round that changed nothing.
    ///
    /// The signature used to be `{objective_id}:Finished:{terminal_reason}` —
    /// constant for the life of the Objective — so progress and stagnation were
    /// literally indistinguishable to the budget: on 2026-09-08 five attempts
    /// went on re-sending one answer, and a genuinely improving turn would have
    /// been condemned exactly as fast. Folding the evidence state in is what
    /// lets the tight stagnation bound exist without ever shortening real work.
    #[tokio::test]
    async fn new_completion_evidence_mints_a_new_signature_and_a_fresh_budget() {
        let pool = pool().await;
        // A LocalMutation whose evidence never satisfies the arbiter: the same
        // shape the field report produced, so every round really does route a
        // technical failure rather than completing.
        let objective = ObjectiveStore::new(pool.clone())
            .create(CreateObjective {
                id: "objective-evidence-signature".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-evidence-signature".into()),
                root_turn_id: Some("turn-evidence-signature".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();

        let outcome_for = |verification: Option<u64>| RunOutcome {
            final_text: "同一段答案".into(),
            final_message_id: Some("assistant-answer".into()),
            completion_evidence: CompletionEvidence {
                last_successful_verification_sequence: verification,
                ..CompletionEvidence::default()
            },
            input_tokens: 1,
            output_tokens: 1,
            stop_reason: StopReason::Finished,
        };
        let signature_for = |verification: Option<u64>| {
            decision_for_run_outcome(&objective, &outcome_for(verification))
                .unwrap()
                .failure_signature
                .unwrap()
        };

        assert_eq!(
            signature_for(None),
            signature_for(None),
            "an unchanged evidence set is the same failure and must be charged once"
        );
        assert_ne!(
            signature_for(None),
            signature_for(Some(7)),
            "a round that gathered verification evidence is progress, not a repeat"
        );
        assert_ne!(
            signature_for(Some(7)),
            signature_for(Some(9)),
            "further evidence is further progress"
        );
    }

    /// The ceiling must never swallow a real capability restoration: that is
    /// the user changing the world, not the system spinning on itself.
    #[tokio::test]
    async fn restored_capability_is_never_terminalized_by_the_recovery_ceiling() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let mut current = recovery_ceiling_objective(&pool, "objective-recovery-restored").await;

        for _ in 0..(MAX_OBJECTIVE_RECOVERY_ATTEMPTS + 8) {
            let decision = DecisionRouter::route(
                &current,
                RouteSignal::CapabilityRestored {
                    domain: RecoveryDomain::Auth,
                    reason: "authorization_restored".into(),
                    next_observation_at: Utc::now().timestamp_millis(),
                    resume_cursor: current.root_turn_id.clone(),
                },
            )
            .unwrap();
            current = store
                .apply_decision(current.revision, decision)
                .await
                .unwrap();
            assert_eq!(current.status, ObjectiveStatus::WaitingSystem);
            assert!(!current.requires_user_action);
        }
        assert_eq!(claimable_remediations(&pool, &current.id).await, 1);
    }

    #[tokio::test]
    async fn update_safe_point_observation_does_not_spend_the_recovery_budget() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let mut current = store
            .create(CreateObjective {
                id: "objective-update-safe-point-wait".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: None,
                root_turn_id: None,
                domain: RecoveryDomain::Update,
                requested_acceptance: "installed_exact_update".into(),
                created_surface: "updater".into(),
            })
            .await
            .unwrap();
        let signature = "sha256:update-safe-point-wait";
        current = store
            .apply_decision(
                current.revision,
                DecisionRouter::route(
                    &current,
                    RouteSignal::TechnicalFailure {
                        domain: RecoveryDomain::Update,
                        failure_code: UPDATE_SAFE_POINT_PENDING.into(),
                        failure_signature: signature.into(),
                        next_observation_at: Utc::now().timestamp_millis() - 1,
                        resume_cursor: None,
                    },
                )
                .unwrap(),
            )
            .await
            .unwrap();

        for round in 0..(MAX_SIGNATURE_RECOVERY_ATTEMPTS + 3) {
            sqlx::query(
                "UPDATE objective_remediations SET next_observation_at=? WHERE objective_id=?
                 AND status IN ('queued','waiting')",
            )
            .bind(Utc::now().timestamp_millis() - 1)
            .bind(&current.id)
            .execute(&pool)
            .await
            .unwrap();
            let claim = store
                .claim_due_remediations("update-safe-point-test", 1, 30_000)
                .await
                .unwrap()
                .pop()
                .unwrap_or_else(|| panic!("safe-point observation {round} exhausted recovery"));
            let permit = codefactory_agent_loop::tool::MutationPermit {
                objective_id: claim.objective.id.clone(),
                remediation_id: claim.remediation_id,
                owner: "update-safe-point-test".into(),
                claim_epoch: claim.claim_epoch,
                binding_id: claim.binding_id,
                resource_generation: claim.resource_generation,
            };
            current = claim.objective;
            let decision = DecisionRouter::route(
                &current,
                RouteSignal::TechnicalFailure {
                    domain: RecoveryDomain::Update,
                    failure_code: UPDATE_SAFE_POINT_PENDING.into(),
                    failure_signature: signature.into(),
                    next_observation_at: Utc::now().timestamp_millis() - 1,
                    resume_cursor: None,
                },
            )
            .unwrap();
            current = store
                .apply_claimed_decision(current.revision, decision, &permit)
                .await
                .unwrap();
            assert_eq!(current.status, ObjectiveStatus::WaitingSystem);
            assert!(!current.requires_user_action);
        }
    }

    /// Permission authorization writes its remediation directly, with no
    /// decision row to join against. A user clicking Allow is progress and must
    /// not quietly spend the system's recovery budget.
    #[tokio::test]
    async fn authorized_permission_remediations_do_not_spend_the_recovery_budget() {
        let pool = pool().await;
        let store = ObjectiveStore::new(pool.clone());
        let current = recovery_ceiling_objective(&pool, "objective-recovery-permission").await;
        let now = Utc::now().timestamp_millis();
        for index in 0..(MAX_OBJECTIVE_RECOVERY_ATTEMPTS + 4) {
            sqlx::query(
                "INSERT INTO objective_remediations
                 (id, objective_id, domain, status, failure_code, failure_signature,
                  strategy, approach_index, attempt_index, next_observation_at,
                  created_at, updated_at)
                 VALUES (?, ?, 'permission', 'superseded', 'authorization_restored', ?,
                         'resume_authorized_action', 0, 0, ?, ?, ?)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(&current.id)
            .bind(format!("permission:intent-{index}:authorized"))
            .bind(now)
            .bind(now)
            .bind(now)
            .execute(&pool)
            .await
            .unwrap();
        }

        let queued = route_technical_failure(
            &store,
            &current,
            "completion_evidence_incomplete",
            &format!("{}:Finished:none", current.id),
        )
        .await;
        assert_eq!(
            queued.status,
            ObjectiveStatus::WaitingSystem,
            "user-authorized permissions must not exhaust system recovery"
        );
    }

    /// U18/R4: startup turn convergence is the fake-running repair. A live
    /// Objective with a ghost turn keeps exactly its current turn; an Objective
    /// that is already terminal owns no live turn at all; and a second start
    /// writes nothing.
    #[tokio::test]
    async fn startup_turn_convergence_closes_ghost_turns_exactly_once() {
        let pool = pool().await;
        settlement_tables(&pool).await;
        let store = ObjectiveStore::new(pool.clone());

        let live = store
            .create(CreateObjective {
                id: "objective-ghost-live".into(),
                kind: ObjectiveKind::LocalMutation,
                session_id: Some("session-ghost-live".into()),
                root_turn_id: Some("turn-ghost-old".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "validated_change".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        sqlx::query("UPDATE objectives SET resume_cursor=? WHERE id=?")
            .bind("turn-ghost-new")
            .bind(&live.id)
            .execute(&pool)
            .await
            .unwrap();
        let terminal = store
            .create(CreateObjective {
                id: "objective-ghost-terminal".into(),
                kind: ObjectiveKind::Informational,
                session_id: Some("session-ghost-terminal".into()),
                root_turn_id: Some("turn-ghost-terminal".into()),
                domain: RecoveryDomain::Chat,
                requested_acceptance: "informational_answer".into(),
                created_surface: "test".into(),
            })
            .await
            .unwrap();
        let now = Utc::now().timestamp_millis();
        sqlx::query(
            "UPDATE objectives SET status='cancelled', decision_type='cancelled',
               cancellation_provenance='explicit_cancel', completed_at=?
             WHERE id=?",
        )
        .bind(now)
        .bind(&terminal.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, session_id, status, waiting_reason, objective_id)
             VALUES ('turn-ghost-old', 'session-ghost-live', 'waiting_system',
                     'await_system_recovery', ?),
                    ('turn-ghost-new', 'session-ghost-live', 'active', NULL, ?),
                    ('turn-ghost-terminal', 'session-ghost-terminal', 'active', NULL, ?)",
        )
        .bind(&live.id)
        .bind(&live.id)
        .bind(&terminal.id)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(store.reconcile_stale_chat_turns().await.unwrap(), 2);

        let live_turns: Vec<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT root_turn_id, status, terminal_reason FROM chat_turn_state
             WHERE objective_id=? ORDER BY root_turn_id",
        )
        .bind(&live.id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            live_turns,
            vec![
                (
                    "turn-ghost-new".to_string(),
                    "active".to_string(),
                    None
                ),
                (
                    "turn-ghost-old".to_string(),
                    "completed".to_string(),
                    Some("superseded_at_startup".to_string())
                ),
            ],
            "the Objective keeps exactly its current turn and closes the rest"
        );
        let terminal_status: (String, Option<String>) = sqlx::query_as(
            "SELECT status, terminal_reason FROM chat_turn_state WHERE root_turn_id=?",
        )
        .bind("turn-ghost-terminal")
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            terminal_status,
            (
                "completed".to_string(),
                Some("objective_already_terminal".to_string())
            ),
            "a terminal Objective owns no live turn"
        );

        // Deliberately different second run so it is not mistaken for a repeat.
        let second = store.reconcile_stale_chat_turns().await.unwrap();
        assert_eq!(second, 0, "convergence is idempotent");
    }

    /// U18/R1+R2: a user message during system recovery takes the Objective
    /// over atomically — the queued remediation is superseded, the cursor moves
    /// to the user's turn, the superseded turn reaches a terminal state in the
    /// same transaction, and the user turn stays the only live one.
    #[tokio::test]
    async fn user_reprompt_during_system_recovery_settles_the_superseded_turn() {
        let pool = pool().await;
        settlement_tables(&pool).await;
        let store = ObjectiveStore::new(pool.clone());
        let current = recovery_ceiling_objective(&pool, "objective-u18-steer").await;
        let original_root_turn_id = current.root_turn_id.clone().unwrap();
        let session_id = current.session_id.clone().unwrap();

        let waiting = route_technical_failure(
            &store,
            &current,
            "completion_evidence_incomplete",
            &format!("{}:steer:waiting", current.id),
        )
        .await;
        assert_eq!(waiting.status, ObjectiveStatus::WaitingSystem);
        assert_eq!(claimable_remediations(&pool, &current.id).await, 1);

        sqlx::query(
            "INSERT INTO chat_turn_state
             (root_turn_id, session_id, status, next_action, objective_id)
             VALUES (?, ?, 'waiting_system', 'resume_automatically', ?)",
        )
        .bind(&original_root_turn_id)
        .bind(&session_id)
        .bind(&current.id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chat_turn_state(root_turn_id, session_id, status, objective_id)
             VALUES ('turn-u18-user-steer', ?, 'active', NULL)",
        )
        .bind(&session_id)
        .execute(&pool)
        .await
        .unwrap();

        let taken_over = store
            .ensure_or_continue_chat_objective(
                &session_id,
                "turn-u18-user-steer",
                Some(&original_root_turn_id),
                current.kind,
                &current.requested_acceptance,
            )
            .await
            .unwrap();

        assert_eq!(taken_over.id, current.id, "the Objective stays stable");
        assert_eq!(taken_over.status, ObjectiveStatus::Active);
        assert_eq!(
            taken_over.resume_cursor.as_deref(),
            Some("turn-u18-user-steer"),
            "the Objective's live turn is the user's message"
        );
        assert_eq!(
            claimable_remediations(&pool, &current.id).await,
            0,
            "the queued system remediation is superseded"
        );
        let superseded: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_remediations
             WHERE objective_id=? AND status='superseded'",
        )
        .bind(&current.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(superseded, 1);
        let turns: Vec<(String, String)> = sqlx::query_as(
            "SELECT root_turn_id, status FROM chat_turn_state
             WHERE objective_id=? ORDER BY root_turn_id",
        )
        .bind(&current.id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            turns,
            vec![
                (
                    original_root_turn_id.clone(),
                    "completed".to_string()
                ),
                ("turn-u18-user-steer".to_string(), "active".to_string()),
            ],
            "exactly one turn stays live, in the same transaction"
        );
        let audit: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM objective_events
             WHERE objective_id=? AND event_type='user_steer_superseded_remediation'",
        )
        .bind(&current.id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audit, 1, "the takeover is auditable");
    }

    /// U18/R3: an identity disagreement that reconciliation cannot resolve is
    /// deterministic, so it settles the honest failure terminal on the first
    /// occurrence instead of being retried until the budget runs out.
    #[tokio::test]
    async fn unreconcilable_identity_settles_the_failure_terminal_immediately() {
        let pool = pool().await;
        settlement_tables(&pool).await;
        let store = ObjectiveStore::new(pool.clone());
        let current = recovery_ceiling_objective(&pool, "objective-u18-identity").await;

        let settled = route_technical_failure(
            &store,
            &current,
            CHAT_IDENTITY_UNRECONCILABLE,
            "sha256:identity-unreconcilable-synthetic",
        )
        .await;

        assert_eq!(
            settled.status,
            ObjectiveStatus::Failed,
            "no second attempt is spent on a deterministic identity failure"
        );
        assert_eq!(
            settled.failure_code.as_deref(),
            Some(CHAT_IDENTITY_UNRECONCILABLE)
        );
        assert_eq!(
            claimable_remediations(&pool, &settled.id).await,
            0,
            "a terminal identity failure queues no remediation"
        );
    }

    /// U27 / CF-WSC-R8: a deterministic identity signature must not burn the
    /// recovery budget. The first occurrence settles the objective and queues no
    /// remediation at all, so the repeated signature the production loop showed
    /// has nothing to retry — and the user gets a plain-language reason that
    /// says the workspace still holds their changes.
    #[tokio::test]
    async fn an_identity_failure_settles_once_and_never_queues_a_repeat() {
        let pool = pool().await;
        settlement_tables(&pool).await;
        let store = ObjectiveStore::new(pool.clone());
        let current = recovery_ceiling_objective(&pool, "objective-u27-identity-repeat").await;

        let settled = route_technical_failure(
            &store,
            &current,
            CHAT_IDENTITY_UNRECONCILABLE,
            "sha256:identity-signature-repeats",
        )
        .await;
        assert_eq!(settled.status, ObjectiveStatus::Failed);
        assert_eq!(claimable_remediations(&pool, &settled.id).await, 0);

        // The repeated signature the production loop showed (~every 5 minutes)
        // has nothing to spend: the objective is already terminal — a further
        // decision is refused outright ("terminal objective cannot accept
        // another decision") — and no remediation was ever queued for the
        // deterministic code, so no attempt index exists to grow.
        let stored = store.get(&settled.id).await.unwrap().unwrap();
        assert_eq!(stored.status, ObjectiveStatus::Failed);
        assert_eq!(stored.failure_code.as_deref(), Some(CHAT_IDENTITY_UNRECONCILABLE));
        let queued: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM objective_remediations WHERE objective_id=?")
                .bind(&stored.id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            queued, 0,
            "the deterministic signature queues no remediation to retry"
        );
        assert_eq!(claimable_remediations(&pool, &stored.id).await, 0);

        let summary =
            crate::agent::failure_summary::plain_failure_reason(stored.failure_code.as_deref());
        crate::agent::failure_summary::assert_no_internal_vocabulary(&summary)
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            summary.contains("工作区"),
            "the reason must say where the user's work is: {summary}"
        );
    }
}
