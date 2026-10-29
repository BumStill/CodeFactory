// SPDX-License-Identifier: Apache-2.0
//! Invisible plan/act dispatch for the chat surface.
//!
//! The chat panel runs [`AgentMode::Interactive`] only when the user's current
//! intent is to discuss, inspect, ask a question, or explicitly request a plan.
//! Direct execution requests ("修复", "上线", "直接改", "implement", "ship")
//! switch that single turn to [`AgentMode::Execute`] even on the first message.
//! The problem that motivated this module: an old broad plan-first contract made
//! the agent answer clear commands with another plan plus "Ready to proceed?".
//! Intent now wins over ceremony.
//!
//! The fix is deliberately **invisible**: there is no user-facing plan/act
//! toggle. The framework decides each turn's contract from the conversation
//! itself — `(previous assistant message, current user message)` — and flips
//! that single turn to [`AgentMode::Execute`] when the user has approved a
//! pending proposal. Because the *framework* makes the call (not the model),
//! a model with weak instruction-following can't sabotage it: on an Execute
//! turn the "plan-first / ask to proceed" instruction simply isn't present
//! for it to latch onto.
//!
//! Detection is intentionally cross-language (the user converses in Chinese
//! and English interchangeably) and conservative: when intent is ambiguous
//! we stay Interactive, which is the pre-existing behaviour — so this can
//! only *reduce* spurious re-confirmations, never introduce a regression.
//!
//! Matching convention: CJK cues are matched as substrings (no word
//! boundaries in Chinese); English cues are matched as whole tokens so
//! "go" doesn't fire on "google" and "approve" doesn't fire on "disapprove".

use super::{AgentMode, TurnCapability};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChatContract {
    pub mode: AgentMode,
    pub capability: TurnCapability,
    pub grants: TurnGrants,
}

/// Narrow user-authored grants that do not widen the turn's code capability.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TurnGrants {
    pub browser_read: bool,
    /// M130: structured read-only or an explicit current read-only directive.
    /// Wording may lower authority, never grant remote delivery.
    pub explicit_read_only: bool,
}

/// A structured, caller-supplied turn intent.
///
/// This is the only source of a hard read-only turn. The wording of the user's
/// message may choose the POSTURE (discuss first vs. act) and may add
/// instructions for the model, but it can never grant or revoke capability —
/// see `docs/specs/architecture-constraints.md` §意图推断约束 and the phase-1
/// forensics in `docs/plans/explicit-read-only-inference-*`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TurnIntent {
    pub structured_read_only: bool,
}

/// Negation cues — substring-matched (CJK). If any appears we refuse to read
/// the message as approval. A false negative just makes the agent ask (safe);
/// a false positive would execute against the user's wishes (unsafe).
const NEGATIONS_CJK: &[&str] = &[
    "不",
    "别",
    "甭",
    "暂停",
    "先别",
    "等等",
    "等一下",
    "稍等",
    "再想",
    "再说",
];
/// Negation cues — whole-token-matched (English).
const NEGATIONS_EN: &[&str] = &[
    "no", "not", "dont", "stop", "wait", "hold", "cancel", "nope", "nah",
];

/// Unambiguous go-ahead phrases (CJK, substring). Fire even inside a longer
/// sentence — "我同意执行并要求输出 ppt" is carried by "同意".
const STRONG_CJK: &[&str] = &[
    "同意",
    "批准",
    "执行吧",
    "开始吧",
    "动手",
    "做吧",
    "搞吧",
    "去吧",
    "开始改",
    "开始修",
    "赶紧开始",
    "赶紧修",
    "修复上线",
    "搞定",
    "落地执行",
    "直到上线",
    "完成上面",
    "继续执行",
    "继续修复",
    "继续交付",
    "继续开发",
    "继续完成",
    "就这样",
    "就这么",
    "可以执行",
    "照这个",
    "按这个",
    "确认执行",
    // Bare confirmations: after a draft the user types "确认/确定" to mean go.
    // Promoted from the weak tier so they execute even without a trailing "?".
    "确认",
    "确定",
];
/// Strong approvals (English, whole multi-word phrases — substring is fine
/// because the phrase itself is unambiguous).
const STRONG_EN_PHRASES: &[&str] = &[
    "go ahead",
    "do it",
    "ship it",
    "make it so",
    "let's go",
    "lets go",
    "go for it",
    "sounds good",
    "looks good",
];
/// Strong approvals (English, single word — whole-token so "approve" never
/// fires on "disapprove").
const STRONG_EN_WORDS: &[&str] = &[
    "approve",
    "approved",
    "proceed",
    "lgtm",
    "confirm",
    "confirmed",
];

/// Weak/standalone approvals — only count when the message is essentially
/// *just* the approval. "好的，但是…" is a refinement, not a green light.
const WEAK_CJK: &[&str] = &["好", "可以", "行", "嗯", "继续", "对", "是的", "可", "成"];
const WEAK_EN: &[&str] = &[
    "ok", "okay", "k", "yes", "yeah", "yep", "yup", "sure", "fine", "go", "right",
];

/// Explicit planning / analysis-only cues. These deliberately keep the turn in
/// Interactive even if the sentence mentions implementation words.
const PLAN_ONLY_CJK: &[&str] = &[
    "先给我方案",
    "给个方案",
    "出个方案",
    "分析一下",
    "评估一下",
    "只分析",
    "只评估",
    "不要改",
    "别改",
    "不要修改",
    "别修改",
    "先别修改",
    "不要执行",
    "别执行",
    "不要动代码",
    "别动代码",
    "继续分析",
    "继续审视",
    "继续评估",
    "继续讨论",
    "继续看看",
];
const PLAN_ONLY_EN_PHRASES: &[&str] = &[
    "what's the plan",
    "whats the plan",
    "give me a plan",
    "propose a plan",
    "analyze the options",
    "explain the options",
    "do not implement",
    "don't implement",
    "do not change",
    "don't change",
    "do not execute",
    "don't execute",
    "without changing anything",
];
const PLAN_ONLY_EN_WORDS: &[&str] = &["analyze", "analyse", "explain", "evaluate"];

/// Direct execution cues. These are not approvals of an earlier proposal; they
/// are first-class user intent to act now.
const DIRECT_EXEC_CJK: &[&str] = &[
    "修复",
    "解决",
    "上线",
    "发布",
    "直接改",
    "改掉",
    "删掉",
    "删除",
    "实现",
    "只创建",
    "创建文件",
    "新建文件",
    "加上",
    "开始搞",
    "开始做",
    "开始实施",
    "批准的执行",
    "赶紧处理",
    "赶紧修",
    "搞定",
    "落地",
    "处理一下",
];
const DIRECT_EXEC_EN_WORDS: &[&str] = &[
    "fix",
    "repair",
    "implement",
    "build",
    "ship",
    "release",
    "publish",
    "deploy",
    "remove",
    "delete",
    "change",
    "update",
];

/// A visible defect in the product AS IT STANDS — something the user can point
/// at. Deliberately excludes the speaker's own uncertainty ("不清楚 / not
/// sure"), which is lexically similar but asks for an explanation instead.
const DEFECT_CJK: &[&str] = &[
    "太丑",
    "很丑",
    "丑了",
    "难看",
    "不好看",
    "太挤",
    "太乱",
    "很乱",
    "太紧",
    "太宽",
    "太窄",
    "太小",
    "太大",
    "太长",
    "不好用",
    "难用",
    "不方便",
    "不对",
    "不一致",
    "不一样",
    "不统一",
    "不整齐",
    "没对齐",
    "不对齐",
    "不直观",
    "不明显",
    "不合理",
    "看不到",
    "看不清",
    "找不到",
    "显示不全",
    "别扭",
    "怪怪的",
    "有点怪",
];
/// Multi-word phrases only: unambiguous enough to match as substrings, unlike a
/// bare word such as "off" (which would fire on "offered").
const DEFECT_EN_PHRASES: &[&str] = &[
    "too small",
    "too big",
    "too wide",
    "too narrow",
    "too tight",
    "too cramped",
    "not aligned",
    "hard to read",
    "hard to see",
    "can't see",
    "cannot see",
    "doesn't match",
    "does not match",
    "looks off",
    "looks wrong",
];
const DEFECT_EN_WORDS: &[&str] = &["ugly", "inconsistent", "misaligned", "awkward"];

/// The TARGET state the user wants instead — the second half of a work order.
/// "对齐" is intentionally absent: it is a substring of the defect cues
/// "不对齐 / 没对齐", so including it would make a bare complaint self-satisfy
/// both halves by accident.
const TARGET_CJK: &[&str] = &[
    "是不是应该",
    "是不是可以",
    "是不是该",
    "应该",
    "该改",
    "能不能",
    "可不可以",
    "改成",
    "换成",
    "挪到",
    "移到",
    "放到",
    "放在",
    "调成",
    "调整成",
    "统一成",
    "最好",
    "更好",
    "建议",
    "希望",
];
const TARGET_EN_PHRASES: &[&str] = &[
    "should be",
    "should match",
    "should we",
    "shouldn't it",
    "can we",
    "could we",
    "would be better",
    "better to",
    "instead of",
    "make it",
    "move it",
];

/// Asking HOW to change something is a request for a plan, even when the
/// message also names a defect and a target.
const METHOD_QUESTION_CJK: &[&str] = &[
    "怎么改",
    "怎样改",
    "如何改",
    "怎么做",
    "怎么办",
    "怎么处理",
    "怎么弄",
];
const METHOD_QUESTION_EN: &[&str] = &[
    "how should",
    "how do we",
    "how would",
    "what's the best way",
    "whats the best way",
];

const DELIVERY_CJK: &[&str] = &[
    "提交代码",
    "提交 pr",
    "开 pr",
    "创建 pr",
    "合并 pr",
    "合并并发布",
    "发布上线",
    "上线",
    "发布",
    "推送",
    "交付",
];
const DELIVERY_EN_WORDS: &[&str] = &["ship", "release", "publish", "deploy", "push", "merge"];
const DELIVERY_EN_PHRASES: &[&str] = &[
    "open a pr",
    "create a pr",
    "create pr",
    "merge the pr",
    "pull request",
];

/// Explicit delivery-authorization declarations (M48 R1).
///
/// These are *permission* statements, not posture hints: "授权交付" tells the
/// agent it may commit, push, open a PR and merge. Wording around them — a
/// question, a hesitation, "先核实…能否…" — must never downgrade them, because
/// only posture may be guessed from phrasing; permission may not.
const DELIVERY_AUTHORIZATION_CJK: &[&str] = &[
    "授权交付",
    "授权发布",
    "授权合并",
    "授权提交",
    "授权上线",
    "交付授权",
    "允许交付",
    "允许发布",
    "批准交付",
    "准许交付",
];
const DELIVERY_AUTHORIZATION_EN_PHRASES: &[&str] = &[
    "delivery authorized",
    "delivery is authorized",
    "delivery authorization",
    "authorized to deliver",
    "authorize delivery",
    "authorized to ship",
    "authorized to publish",
    "authorized to merge",
    "authorized to release",
];

/// A `ceiling:` declaration is itself a delivery instruction — the user is
/// telling the agent how far it may carry the work, not asking whether it may.
const DELIVERY_CEILINGS: &[&str] = &[
    "pr_only",
    "through_ci_green",
    "through_merge",
    "through_release",
    "merged",
];

/// An explicit read-only directive is a *permission* constraint, so it still
/// vetoes an authorization sentence in the same message. Anything in
/// [`PLAN_ONLY_CJK`] that is merely a posture cue ("给个方案 / 分析一下") is
/// deliberately absent: it changes how the turn talks, not what it may write.
const HARD_READ_ONLY_CJK: &[&str] = &[
    "只分析",
    "只评估",
    "只审视",
    "只读",
    "不要改",
    "别改",
    "不要修改",
    "别修改",
    "先别修改",
    "不要执行",
    "别执行",
    "不要实现",
    "别实现",
    "先别动代码",
    "不要动代码",
    "别动代码",
];
const HARD_READ_ONLY_EN_PHRASES: &[&str] = &[
    "only analyze",
    "only analyse",
    "only review",
    "do not implement",
    "don't implement",
    "do not change",
    "don't change",
    "do not execute",
    "don't execute",
    "without changing anything",
];

/// Narrow file-scope limits ("不要修改其他文件") bound the requested mutation;
/// they do not revoke it. Removed before any read-only scan so the scoped form
/// never reads as a global directive.
const SCOPED_FILE_LIMITS: &[&str] = &[
    "不要修改其他文件",
    "别修改其他文件",
    "不要改其他文件",
    "别改其他文件",
    "不要动其他文件",
    "别动其他文件",
    "do not modify other files",
    "don't modify other files",
    "do not change other files",
    "don't change other files",
];

fn without_scoped_file_limits(lowercased: &str) -> String {
    SCOPED_FILE_LIMITS
        .iter()
        .fold(lowercased.to_string(), |text, scope| {
            text.replace(scope, "")
        })
}

/// M175: pasted specifications, templates and logs are context, not the user's
/// action directive. Use the first instruction paragraph; quoted/fenced/heading
/// blocks end that paragraph even when the paste omitted a blank separator.
fn instruction_paragraph(message: &str) -> &str {
    let message = message.trim();
    let mut end = 0;
    for line in message.split_inclusive('\n') {
        let text = line.trim();
        if text.is_empty() || text.starts_with('>') || text.starts_with("```")
            || text.starts_with('#') {
            break;
        }
        end += line.len();
    }
    message[..end].trim()
}

fn has_hard_read_only_directive(user_msg: &str) -> bool {
    let text = without_scoped_file_limits(&instruction_paragraph(user_msg).to_ascii_lowercase());
    // Conditional constraints govern their branch, not the entire turn.
    let text = text.split([ '。', '；', ';' ]).filter(|clause| {
        let clause = clause.trim_start();
        !clause.starts_with("如果") && !clause.starts_with("若") && !clause.starts_with("if ")
    }).collect::<Vec<_>>().join("。");
    HARD_READ_ONLY_CJK.iter().chain(HARD_READ_ONLY_EN_PHRASES.iter())
        .any(|cue| text.contains(cue) && !text.contains(&format!("别{cue}")))
}

fn ceiling_declaration(lowercased: &str) -> bool {
    tokens(lowercased)
        .windows(2)
        .any(|pair| pair[0] == "ceiling" && DELIVERY_CEILINGS.contains(&pair[1]))
        || declared_ceiling_word(lowercased).is_some()
}

/// M116-R2: read the ceiling word that follows `ceiling` without discarding its
/// value. `tokens()` splits on every non-alphanumeric character, which is why a
/// compound label (`through_ci_green`) could never be read from a message and a
/// dispatch saying `ceiling: merged` only ever produced a boolean grant.
fn declared_ceiling_word(lowercased: &str) -> Option<&'static str> {
    const WORDS: &[(&str, &str)] = &[
        ("through_release", "through_release"),
        ("through_ci_green", "through_ci_green"),
        ("through_merge", "through_merge"),
        ("pr_only", "pr_only"),
        ("merged", "through_merge"),
    ];
    let mut widest: Option<&'static str> = None;
    for (index, _) in lowercased.match_indices("ceiling") {
        let rest = lowercased[index + "ceiling".len()..].trim_start_matches(|c: char| {
            c.is_whitespace() || matches!(c, ':' | '：' | '=' | '(' | '（')
        });
        let Some((_, label)) = WORDS.iter().find(|(word, _)| {
            rest.starts_with(word)
                && rest[word.len()..]
                    .chars()
                    .next()
                    .map(|next| !next.is_alphanumeric())
                    .unwrap_or(true)
        }) else {
            continue;
        };
        let label = *label;
        let rank = |value: &str| {
            crate::config::settings::DeliveryCeiling::from_label(value)
                .map(|c| c.rank())
                .unwrap_or(0)
        };
        if widest.map_or(true, |current| rank(label) > rank(current)) {
            widest = Some(label);
        }
    }
    widest
}

/// M116-R2: the delivery ceiling this message actually declared, e.g.
/// "授权交付（ceiling: merged）". Only an explicit delivery grant may declare a
/// ceiling — a question about a ceiling is not an authorization — and the widest
/// declaration in the message wins.
pub fn declared_delivery_ceiling(user_msg: &str) -> Option<crate::config::settings::DeliveryCeiling> {
    if !is_explicit_delivery_authorization(user_msg) {
        return None;
    }
    let lowercased = user_msg.trim().to_lowercase();
    // Preserve underscores in labels; do not match `nonceiling` or label suffixes.
    let words: Vec<&str> = lowercased
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|word| !word.is_empty())
        .collect();
    words.windows(2).filter(|pair| pair[0] == "ceiling")
        .filter_map(|pair| crate::config::settings::DeliveryCeiling::from_label(pair[1]))
        .max_by_key(|ceiling| ceiling.rank())
}

/// Did the user explicitly authorize delivery in THIS message? (M48 R1/R2)
///
/// The 10-09 field report: a kickoff message said "授权交付，ceiling: pr_only"
/// and then asked "先核实个人账户仓库能否用 merge queue"; the answer came back
/// "not authorized" because a posture cue in the same sentence vetoed the
/// delivery cue. That inverted the rule this module exists to enforce — the
/// wording may guess a *posture*, never a *permission*. An explicit grant wins
/// over questions, hesitation and posture cues; only an explicit revocation or
/// an explicit read-only directive can override it.
pub fn is_explicit_delivery_authorization(user_msg: &str) -> bool {
    let m = user_msg.trim().to_lowercase();
    if m.is_empty() || is_delivery_revocation(&m) {
        return false;
    }
    let declared = DELIVERY_AUTHORIZATION_CJK.iter().any(|cue| m.contains(cue))
        || DELIVERY_AUTHORIZATION_EN_PHRASES
            .iter()
            .any(|cue| m.contains(cue))
        || ceiling_declaration(&m);
    if !declared {
        return false;
    }
    let scoped = without_scoped_file_limits(&m);
    !(HARD_READ_ONLY_CJK.iter().any(|cue| scoped.contains(cue))
        || HARD_READ_ONLY_EN_PHRASES
            .iter()
            .any(|cue| scoped.contains(cue)))
}

/// Tokenize on non-alphanumeric boundaries for whole-word English matching.
/// CJK runs survive as multi-char tokens but we match those by substring, so
/// the tokenization only matters for the ASCII cues.
fn tokens(m: &str) -> Vec<&str> {
    m.split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Did the previous assistant turn end by proposing something and waiting on
/// the user? Cross-language signal: a trailing `?` / `？`. Covers the English
/// "Ready to proceed?" marker our own `SYSTEM_PROMPT` emits *and* a localized
/// "可以开始吗？" without depending on the model echoing any exact string. Any
/// trailing question works: if the agent asked and the user approved,
/// executing is the right move.
pub fn is_pending_proposal(prev_assistant: &str) -> bool {
    let t = prev_assistant.trim_end();
    t.ends_with('?') || t.ends_with('？')
}

/// An unambiguous, imperative go-ahead ("做吧 / 执行吧 / 确认 / do it / proceed").
/// Strong enough to act on even when our previous turn didn't end with a
/// question — the user is plainly telling us to act, not answering one.
/// Negations still veto.
pub fn is_strong_approval(user_msg: &str) -> bool {
    let m = user_msg.trim().to_lowercase();
    if m.is_empty() {
        return false;
    }
    let toks = tokens(&m);
    if NEGATIONS_CJK.iter().any(|n| m.contains(n)) {
        return false;
    }
    if toks.iter().any(|t| NEGATIONS_EN.contains(t)) {
        return false;
    }
    if STRONG_CJK.iter().any(|s| m.contains(s)) {
        return true;
    }
    if STRONG_EN_PHRASES.iter().any(|s| m.contains(s)) {
        return true;
    }
    toks.iter().any(|t| STRONG_EN_WORDS.contains(t))
}

/// Does the user's message read as a go-ahead to execute?
///
/// A strong approval counts anywhere; weak approvals ("好 / ok") count only when
/// the whole message is basically "yes" (so "好的，但是…" is a refinement).
pub fn is_approval(user_msg: &str) -> bool {
    if is_strong_approval(user_msg) {
        return true;
    }
    let m = user_msg.trim().to_lowercase();
    let toks = tokens(&m);
    if NEGATIONS_CJK.iter().any(|n| m.contains(n)) {
        return false;
    }
    if toks.iter().any(|t| NEGATIONS_EN.contains(t)) {
        return false;
    }
    // Weak approvals — only when the whole message is basically "yes".
    let short = m.chars().count() <= 12;
    if !short {
        return false;
    }
    if WEAK_CJK.iter().any(|w| m.contains(w)) {
        return true;
    }
    toks.iter().any(|t| WEAK_EN.contains(t))
}

/// Wording-level POSTURE cue: "should this turn discuss first?" — and nothing
/// else. Since β it never grants or revokes a capability.
///
/// Renamed from `is_explicit_planning_request` (2026-09-28). The old name read
/// like a permission verdict, callers treated it as one, and that is how a
/// conditional aside — "如果 CI 再遇到环境抖动，不要改代码或验收脚本" — revoked a
/// whole delivery turn's write capability (field report, 2026-09-24).
fn planning_cue_present(user_msg: &str) -> bool {
    let m = user_msg.trim().to_lowercase();
    if m.is_empty() {
        return false;
    }
    let explicitly_leaves_review = [
        "别只分析",
        "不要只分析",
        "别只审视",
        "不要只审视",
        "别再分析",
        "不要再分析",
        "别再审视",
        "不要再审视",
        "stop analyzing",
        "stop reviewing",
        "don't just analyze",
        "do not just analyze",
        "don't just review",
        "do not just review",
    ]
    .iter()
    .any(|cue| m.contains(cue))
        && [
            "继续执行",
            "继续实施",
            "开始执行",
            "开始实施",
            "执行完",
            "实施完",
            "动手",
            "implement",
            "execute",
            "proceed",
            "continue",
        ]
        .iter()
        .any(|cue| m.contains(cue));
    if explicitly_leaves_review {
        return false;
    }
    // A requested write may carry a narrow boundary such as "不要修改其他文件".
    // That limits the requested mutation; it does not revoke the mutation itself.
    // Remove only explicit "other files" scopes before looking for a global
    // read-only directive. The unscoped forms ("不要修改 / don't change") stay
    // fail-closed and continue to select ReviewOnly.
    let review_text = without_scoped_file_limits(&m);
    let toks = tokens(&review_text);
    PLAN_ONLY_CJK.iter().any(|cue| review_text.contains(cue))
        || PLAN_ONLY_EN_PHRASES
            .iter()
            .any(|cue| review_text.contains(cue))
        || toks.iter().any(|token| PLAN_ONLY_EN_WORDS.contains(token))
}

fn is_direct_execution_request(user_msg: &str) -> bool {
    let m = user_msg.trim().to_lowercase();
    if m.is_empty() || planning_cue_present(&m) {
        return false;
    }
    let toks = tokens(&m);
    if toks.iter().any(|t| NEGATIONS_EN.contains(t)) {
        return false;
    }
    DIRECT_EXEC_CJK.iter().any(|cue| m.contains(cue))
        || toks
            .iter()
            .any(|token| DIRECT_EXEC_EN_WORDS.contains(token))
}

fn is_delivery_request(user_msg: &str) -> bool {
    let m = user_msg.trim().to_lowercase();
    if m.is_empty() || is_delivery_revocation(&m) {
        return false;
    }
    // M48 R1/R2: an explicit authorization declaration is a permission
    // statement. It is evaluated BEFORE the posture cue table, so a question or
    // a "先分析一下" in the same message can no longer downgrade it.
    if is_explicit_delivery_authorization(&m) {
        return true;
    }
    if planning_cue_present(&m) {
        return false;
    }
    let toks = tokens(&m);
    DELIVERY_CJK.iter().any(|cue| m.contains(cue))
        || DELIVERY_EN_PHRASES.iter().any(|cue| m.contains(cue))
        || toks.iter().any(|token| DELIVERY_EN_WORDS.contains(token))
}

fn is_explicit_continuation_request(user_msg: &str) -> bool {
    let m = user_msg.trim().to_lowercase();
    if m.is_empty() || planning_cue_present(&m) {
        return false;
    }
    let diagnostic = ["为什么", "为何", "原因", "怎么会", "why", "how"]
        .iter()
        .any(|cue| m.contains(cue));
    if diagnostic {
        return false;
    }
    let compact = m
        .chars()
        .filter(|character| !character.is_whitespace() && !"，。！？,.!?".contains(*character))
        .collect::<String>();
    compact == "继续"
        || compact == "可以继续"
        || compact == "好的继续"
        || compact == "开始"
        || m.contains("继续做")
        || m.contains("接着做")
        || m.contains("继续执行")
        || m.contains("继续实施")
        || m.contains("继续完成")
        || m.contains("resume")
        || m.contains("continue")
}

/// A change request that never reaches for an execution verb.
///
/// [`is_direct_execution_request`] only recognizes a user who writes like a
/// ticket ("修复 / 实现 / 改掉 / fix"). Real messages on this surface are
/// colloquial: the user points at something in their own product, says what is
/// wrong with it, and proposes what it should be instead — "布局太丑了…是不是
/// 应该把模型选择放这里啊". That is a work order wearing a question mark, and
/// classifying it `ReviewOnly` is what produced the 2026-08-05 dead end: the
/// write gate refused the edit, the denial forbade asking, and the agent told
/// the user to switch to an "implementation 模式" that does not exist.
///
/// Every previous repair of this class added more imperative verbs to the cue
/// tables (#37 → #204 → #261 → #265), which can never converge because the
/// missing signal is not a verb. It is the pairing:
///
/// 1. a complaint about the product's CURRENT state, and
/// 2. the TARGET state the user wants instead.
///
/// Both halves are required — that is what keeps a plain question out — and
/// asking for the *method* ("应该怎么改比较好") vetoes, because that is a
/// request for a plan.
///
/// Precision over recall on purpose. A miss now costs one polite question,
/// because an ambiguity-default review turn carries the route back to
/// implementation in its denial (`policy::review_only_denial`). A false
/// positive starts editing against the user's wishes and has no such cheap
/// undo.
pub fn is_change_request(user_msg: &str) -> bool {
    let m = user_msg.trim().to_lowercase();
    if m.is_empty() || planning_cue_present(&m) {
        return false;
    }
    let toks = tokens(&m);
    if METHOD_QUESTION_CJK.iter().any(|cue| m.contains(cue))
        || METHOD_QUESTION_EN.iter().any(|cue| m.contains(cue))
    {
        return false;
    }
    let names_a_defect = DEFECT_CJK.iter().any(|cue| m.contains(cue))
        || DEFECT_EN_PHRASES.iter().any(|cue| m.contains(cue))
        || toks.iter().any(|token| DEFECT_EN_WORDS.contains(token));
    let names_a_target = TARGET_CJK.iter().any(|cue| m.contains(cue))
        || TARGET_EN_PHRASES.iter().any(|cue| m.contains(cue));
    names_a_defect && names_a_target
}

pub fn proposal_capability(previous_assistant: &str) -> Option<TurnCapability> {
    let text = previous_assistant.trim().to_lowercase();
    if text.is_empty() {
        return None;
    }
    if [
        "开 pr",
        "创建 pr",
        "合并 pr",
        "发布上线",
        "发布。",
        "推送",
        "交付",
        "open a pr",
        "create a pr",
        "merge the pr",
        "release",
        "publish",
        "deploy",
    ]
    .iter()
    .any(|cue| text.contains(cue))
    {
        return Some(TurnCapability::Deliver);
    }
    let actionable = is_pending_proposal(&text)
        || [
            "可实施",
            "实施方案",
            "修复方案",
            "落地方案",
            "实现步骤",
            "改动范围",
            "下一步修改",
            "开始实施",
            "我会修复",
            "我会修改",
            "我会实现",
            "implementation plan",
            "next step",
            "i will fix",
            "i will implement",
        ]
        .iter()
        .any(|cue| text.contains(cue));
    actionable.then_some(TurnCapability::Implement)
}

pub fn is_contextual_approval(user_msg: &str) -> bool {
    is_explicit_continuation_request(user_msg) || is_approval(user_msg)
}

/// Explicit revocation of a previously granted delivery authorization. A
/// session that once said "提交上线" keeps `Deliver` capability on later
/// turns (see [`with_persisted_delivery_authorization`]); these phrasings
/// turn that back off so the user is not stuck with standing delivery
/// permission they no longer want.
pub fn is_delivery_revocation(user_msg: &str) -> bool {
    let text = user_msg.to_ascii_lowercase();
    [
        "别推了",
        "先不要提交到远端",
        "不要推送",
        "别推送",
        "don't push",
        "do not push",
        "取消交付",
        "取消发布",
        "先不发布",
        "先别发布",
        "不要发布",
        "不要提交",
        "别提交",
        "别发布",
        "不要合并",
        "别合并",
        "停止交付",
        "暂停交付",
        "撤回发布",
        "不用上线",
        "先不上线",
        "don't publish",
        "do not publish",
        "don't release",
        "do not release",
        "don't deploy",
        "do not deploy",
        "don't submit",
        "cancel the release",
        "cancel delivery",
    ]
    .iter()
    .any(|cue| text.contains(cue))
}

/// Apply a session-persisted delivery authorization to the per-turn contract.
///
/// `decide_chat_contract` derives capability from the CURRENT message alone,
/// so a user who said "提交上线" on an earlier turn finds the next turn back
/// at `Implement` and gets asked to re-confirm delivery again — field report.
/// Once a session has granted delivery, later messages retain that grant so
/// fixing follow-up issues then shipping works without a repeat confirmation.
/// A current explicit read-only constraint pauses mutation for the requested
/// action, and [`is_delivery_revocation`] clears the durable grant.
pub fn with_persisted_delivery_authorization(
    contract: ChatContract,
    delivery_authorized: bool,
) -> ChatContract {
    // A default question/diagnostic classification is not a revocation of an
    // already-authorized delivery task. Only an explicit current constraint
    // ("只分析 / 不要修改") can pause mutations. This is an action-intent
    // decision, not a root-turn lock.
    if delivery_authorized && !contract.grants.explicit_read_only {
        ChatContract {
            mode: AgentMode::Execute,
            capability: TurnCapability::Deliver,
            grants: contract.grants,
        }
    } else {
        contract
    }
}

/// What a single user message does to the session's durable delivery grant.
///
/// The GUI input path (`send_message`) and the anonymous quick-chat path both
/// persisted `sessions.delivery_authorized` with their own copy of the same
/// if/else. M48 extracted the decision so a wording fix cannot land in one
/// path only — the 10-09 session that lost its authorization went through the
/// GUI path while the background dispatch entry passed the flag structurally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryAuthorizationUpdate {
    /// The message explicitly revoked delivery; persist 0.
    Revoked,
    /// The message explicitly authorized delivery; persist 1.
    Granted,
    /// Nothing to persist; keep whatever the session row holds.
    Unchanged,
}

/// Resolve one turn's contract together with the grant update it implies.
///
/// `persisted` is the session row's current `delivery_authorized` value. A
/// message with no delivery wording keeps the standing grant (so follow-up
/// fixes still end in shipping), an explicit revocation clears it, and an
/// explicit authorization wins over question/hesitation cues (M48 R1).
pub fn resolve_turn_delivery_authorization(
    prev_assistant: Option<&str>,
    user_msg: &str,
    intent: TurnIntent,
    persisted: bool,
) -> (ChatContract, DeliveryAuthorizationUpdate) {
    let instruction = instruction_paragraph(user_msg);
    let mut contract = decide_chat_contract_with_intent(prev_assistant, instruction, intent);
    // M175 R1/R3: a structured session grant outranks wording inference, but
    // never an explicitly structured review-only subagent boundary.
    let revoked = is_delivery_revocation(instruction);
    if persisted && !revoked && !intent.structured_read_only {
        if contract.grants.explicit_read_only {
            tracing::info!(
                kind = "authorized_delivery_posture_conflict",
                resolution = "structured_delivery_authorization_wins",
                "M175: wording inferred read-only; keeping authorized implementation"
            );
        }
        contract.mode = AgentMode::Execute;
        contract.capability = TurnCapability::Deliver;
        contract.grants.explicit_read_only = false;
    } else if contract.grants.explicit_read_only {
        contract.capability = TurnCapability::ReviewOnly;
    } else {
        contract.capability = TurnCapability::Implement;
    }
    (contract, if revoked { DeliveryAuthorizationUpdate::Revoked } else { DeliveryAuthorizationUpdate::Unchanged })
}

/// A mid-run user steer can change the current action intent at the next safe
/// round boundary. This is separate from normal permission approval: it
/// changes the user's objective and therefore must reach policy evaluation.
pub fn steer_capability_override(user_msg: &str) -> Option<TurnCapability> {
    has_hard_read_only_directive(user_msg).then_some(TurnCapability::ReviewOnly)
}

/// The same steer, read against what the session already holds.
///
/// [`steer_capability_override`] answers only "what is the user asking for
/// right now", so it necessarily lands on `Implement` for a bare `继续` — the
/// word carries no delivery verb. Applied to a session that had ALREADY been
/// granted delivery, that reads as a revocation: `policy::capability_denial`
/// then refuses every commit, PR and release with "当前意图只授权本地实施".
/// On 2026-09-08 seven resumed sessions hit exactly this, so the one word the
/// user typed to keep the work moving was what stopped it from finishing.
///
/// A steer changes the current ACTION, not the standing grant. Only an
/// explicit read-only steer (handled above, and preserved here) or an explicit
/// revocation ([`is_delivery_revocation`], applied upstream where the durable
/// flag is cleared) may lower it — the same rule
/// [`with_persisted_delivery_authorization`] already applies at turn entry.
pub fn steer_capability_override_with_authorization(
    user_msg: &str,
    _delivery_authorized: bool,
) -> Option<TurnCapability> {
    if is_delivery_revocation(user_msg) {
        Some(TurnCapability::Implement)
    } else {
        steer_capability_override(user_msg)
    }
}

fn grants_browser_read(user_msg: &str) -> bool {
    let text = user_msg.to_ascii_lowercase();
    if [
        "不要打开浏览器",
        "别打开浏览器",
        "不要读浏览器",
        "别读浏览器",
        "不用打开浏览器",
        "无需打开浏览器",
        "do not open the browser",
        "don't open the browser",
        "dont open the browser",
        "do not read the browser",
    ]
    .iter()
    .any(|cue| text.contains(cue))
    {
        return false;
    }

    let existing_browser_target = [
        "本机 chrome",
        "本机chrome",
        "我的 chrome",
        "我的chrome",
        "chrome 里",
        "chrome里",
        "本机浏览器",
        "我的浏览器",
        "浏览器里",
        "浏览器登录态",
        "existing chrome",
        "my chrome",
        "local chrome",
        "current browser",
        "signed-in browser",
        "logged-in browser",
    ]
    .iter()
    .any(|cue| text.contains(cue));
    let read_action = [
        "读一下",
        "读取",
        "看看",
        "查看",
        "打开",
        "访问",
        "走查",
        "检查页面",
        "read",
        "inspect",
        "browse",
        "open",
        "visit",
        "check the page",
    ]
    .iter()
    .any(|cue| text.contains(cue));
    existing_browser_target && read_action
}

/// Wording constraints become INSTRUCTIONS for the model, never permissions.
///
/// A message like "如果 CI 再遇到环境抖动，不要改代码或验收脚本" yields an
/// instruction block for this turn and nothing else: capability is untouched,
/// and the caller records the constraint as a durable `objective_events` row
/// for audit. Detection reuses the existing posture cue table — this adds no
/// cue entry and no regex.
pub fn constraint_instruction_block(user_msg: &str) -> Option<String> {
    if !planning_cue_present(user_msg) {
        return None;
    }
    Some(
        "<turn-constraint>\n\
         本轮用户消息里带有一条措辞层面的改动约束。按约束执行，不要扩大改动范围；\n\
         若该约束附带条件（例如某个假设情形），只在条件成立时生效。\n\
         这条约束不改变本轮的授权能力。\n\
         </turn-constraint>"
            .to_string(),
    )
}

/// Durable audit payload for a wording constraint: the constraint was seen and
/// turned into an instruction — never into a capability change. Pure, so the
/// runtime can persist it into `objective_events` without a second detection
/// pass.
pub fn constraint_audit_payload(user_msg: &str) -> Option<serde_json::Value> {
    let block = constraint_instruction_block(user_msg)?;
    Some(serde_json::json!({
        "kind": "turn_wording_constraint",
        "capability_effect": "none",
        "instruction_block": block,
    }))
}

pub fn decide_chat_contract(prev_assistant: Option<&str>, user_msg: &str) -> ChatContract {
    decide_chat_contract_with_intent(prev_assistant, user_msg, TurnIntent::default())
}

/// [`decide_chat_contract`] with the caller's STRUCTURED intent.
///
/// β (2026-09-28): the wording of `user_msg` decides the posture (`mode`) and
/// the instructions handed to the model, never the capability. A hard read-only
/// turn exists only when the caller says so through
/// [`TurnIntent::structured_read_only`] — set by `delegate_tasks` for a
/// review-only subagent or by an acceptance-scenario declaration.
pub fn decide_chat_contract_with_intent(
    prev_assistant: Option<&str>,
    user_msg: &str,
    intent: TurnIntent,
) -> ChatContract {
    let user_msg = instruction_paragraph(user_msg);
    let read_only = intent.structured_read_only || has_hard_read_only_directive(user_msg);
    let grants = TurnGrants {
        browser_read: grants_browser_read(user_msg),
        explicit_read_only: read_only,
    };
    if read_only {
        return ChatContract {
            mode: AgentMode::Interactive,
            capability: TurnCapability::ReviewOnly,
            grants,
        };
    }
    if is_delivery_request(user_msg) {
        return ChatContract {
            mode: AgentMode::Execute,
            capability: TurnCapability::Implement,
            grants,
        };
    }
    if is_direct_execution_request(user_msg) || is_change_request(user_msg) {
        return ChatContract {
            mode: AgentMode::Execute,
            capability: TurnCapability::Implement,
            grants,
        };
    }
    if is_explicit_continuation_request(user_msg) {
        return ChatContract {
            mode: AgentMode::Execute,
            capability: TurnCapability::Implement,
            grants,
        };
    }
    let approved_proposal = prev_assistant
        .and_then(proposal_capability)
        .filter(|_| is_approval(user_msg));
    let approval = is_strong_approval(user_msg) || approved_proposal.is_some();
    if approval {
        return ChatContract {
            mode: AgentMode::Execute,
            capability: TurnCapability::Implement,
            grants,
        };
    }
    // Ambiguity picks a POSTURE, never a permission.
    //
    // `mode` and `capability` answer two different questions, and until
    // 2026-08-05 this fallthrough answered both with the same guess:
    //
    // - `mode` — should the turn discuss first, or act? A wrong guess costs one
    //   sentence and the model self-corrects. That is what this module was
    //   built for (#37) and it stays inferred.
    // - `capability` — may the turn write at all? A wrong guess is
    //   unrecoverable: `policy::capability_denial` refuses every write and
    //   tells the model not to re-ask, so the agent dead-ends and (field
    //   report, 2026-08-05) invents a nonexistent "implementation 模式" for the
    //   user to switch to.
    //
    // A cue-table guess is nowhere near accurate enough to carry an
    // unrecoverable decision — published intent-classification benchmarks put
    // regex baselines near 53%, and no shipping agent (Claude Code's plan mode,
    // Cursor's mode selector) infers write permission from how a sentence is
    // phrased; the user holds that switch. So the hard read-only gate is now
    // reachable ONLY from an explicit user constraint, checked at the top of
    // this function. Ambiguity keeps the discuss-first posture and leaves the
    // real safety net where it belongs: the per-action permission gateway,
    // which already answers `Ask` for `write_file`/`edit_file` outside trusted
    // mode.
    ChatContract {
        mode: AgentMode::Interactive,
        capability: TurnCapability::Implement,
        grants,
    }
}

/// The framework's per-turn contract decision for a chat message.
///
/// `prev_assistant` is the most recent assistant message already in history
/// (None on the first turn). `user_msg` is the message being sent now.
pub fn decide_chat_mode(prev_assistant: Option<&str>, user_msg: &str) -> AgentMode {
    decide_chat_contract(prev_assistant, user_msg).mode
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m175_authorized_inline_template_cannot_remove_write_authority() {
        let message = "开始实施：修复合成任务，授权交付（ceiling: merged）。\n\n# 合成规格\n只读审查一下，不要修改代码；only review, do not implement.\n\n# 合成模板\n不要推送；规划角色只分析。";
        for instruction in [message, "只读审查一下，不要修改代码"] {
            let (contract, update) = resolve_turn_delivery_authorization(None, instruction, TurnIntent::default(), true);
            assert_eq!(contract.mode, AgentMode::Execute);
            assert_eq!(contract.capability, TurnCapability::Deliver);
            assert!(!contract.grants.explicit_read_only);
            assert_eq!(update, DeliveryAuthorizationUpdate::Unchanged);
        }
        let (review, _) = resolve_turn_delivery_authorization(None, "只读审查一下", TurnIntent::default(), false);
        assert_eq!(review.capability, TurnCapability::ReviewOnly);
        let (structured, _) = resolve_turn_delivery_authorization(None, message, TurnIntent { structured_read_only: true }, true);
        assert_eq!(structured.capability, TurnCapability::ReviewOnly);
    }

    #[test]
    fn m175_only_the_first_instruction_paragraph_selects_posture() {
        let message = "修复合成问题。\n\n> 不要修改代码\n```text\nonly review\n```";
        let contract = decide_chat_contract(None, message);
        assert_eq!(contract.mode, AgentMode::Execute);
        assert_eq!(contract.capability, TurnCapability::Implement);
        let (revoked, update) = resolve_turn_delivery_authorization(None, "不要推送，修复后只在本地验证。", TurnIntent::default(), true);
        assert_eq!(revoked.capability, TurnCapability::Implement);
        assert_eq!(update, DeliveryAuthorizationUpdate::Revoked);
    }

    #[test]
    fn browser_read_grant_comes_only_from_the_current_user_message() {
        let contract = decide_chat_contract(
            Some("我可以读取 Chrome，是否开始？"),
            "你去读一下我本机 Chrome 里的产品现网看看",
        );
        assert_eq!(contract.mode, AgentMode::Interactive);
        assert!(contract.grants.browser_read);

        assert!(
            !decide_chat_contract(Some("我可以读取 Chrome，是否开始？"), "只分析现有截图")
                .grants
                .browser_read
        );
        assert!(
            !decide_chat_contract(None, "不要打开浏览器，只分析截图")
                .grants
                .browser_read
        );
        assert!(
            !decide_chat_contract(None, "打开这个公开网页看看")
                .grants
                .browser_read,
            "a public-page request must not authorize attaching signed-in Chrome"
        );
    }

    #[test]
    fn pending_proposal_detects_trailing_question_both_scripts() {
        assert!(is_pending_proposal(
            "…here's the plan.\n\nReady to proceed?"
        ));
        assert!(is_pending_proposal("…方案如上，可以开始吗？"));
        assert!(is_pending_proposal("Ready to proceed?   \n")); // trailing ws
        assert!(!is_pending_proposal("Done. Files changed: a.rs, b.rs."));
        assert!(!is_pending_proposal("I shipped it.")); // statement, no question
    }

    #[test]
    fn strong_approvals_fire_even_in_a_longer_sentence() {
        assert!(is_approval("我同意执行并要求输出 ppt"));
        assert!(is_approval("approved, go ahead and build it"));
        assert!(is_approval("就这样做吧"));
        assert!(is_approval("proceed"));
        assert!(is_approval("lgtm, ship it"));
    }

    #[test]
    fn weak_approvals_only_count_when_short() {
        assert!(is_approval("好的"));
        assert!(is_approval("ok"));
        assert!(is_approval("可以"));
        assert!(is_approval("yes"));
        // Long message that merely starts with an approval word is a refinement.
        assert!(!is_approval("好的，但是先把数据库那块改成 Postgres 再说"));
        assert!(!is_approval(
            "ok but first rename the module and add a test"
        ));
    }

    #[test]
    fn negations_veto_approval() {
        assert!(!is_approval("先别执行"));
        assert!(!is_approval("不可以"));
        assert!(!is_approval("no, hold on"));
        assert!(!is_approval("don't do that yet"));
        assert!(!is_approval("等一下，再想想"));
        assert!(!is_approval("我不同意")); // 不 vetoes even though 同意 present
    }

    #[test]
    fn whole_word_english_avoids_false_positives() {
        // "disapprove" must NOT read as "approve".
        assert!(!is_approval("i disapprove of this approach entirely"));
        // "another" / "google" contain ascii approval substrings but aren't tokens.
        assert!(!is_approval("show me another option"));
    }

    #[test]
    fn empty_and_neutral_messages_are_not_approval() {
        assert!(!is_approval(""));
        assert!(!is_approval("   "));
        assert!(!is_approval("what does this function do?"));
        assert!(!is_approval("把第二步再解释一下"));
    }

    #[test]
    fn decide_execute_only_when_pending_plan_and_approval() {
        // The motivating bug: agent proposed, user approved + named a deliverable.
        assert_eq!(
            decide_chat_mode(Some("…建议如上。Ready to proceed?"), "同意执行并输出 ppt"),
            AgentMode::Execute
        );
        // Approval but no pending proposal (agent's last turn was a statement)
        // → stay interactive; nothing was on the table to execute.
        assert_eq!(
            decide_chat_mode(Some("I refactored the parser."), "好的"),
            AgentMode::Interactive
        );
        // Pending proposal but the user is refining, not approving.
        assert_eq!(
            decide_chat_mode(Some("Ready to proceed?"), "先把第一步换成别的库"),
            AgentMode::Interactive
        );
        // First turn of a session.
        assert_eq!(
            decide_chat_mode(None, "帮我做个 ppt"),
            AgentMode::Interactive
        );
    }

    #[test]
    fn strong_approval_executes_without_a_pending_question() {
        // The motivating bug: the agent produced a draft that did NOT end with
        // "?", and the user confirms — yet it stayed interactive and re-planned.
        // "确认 / 做吧" now execute anyway.
        assert!(is_strong_approval("确认"));
        assert!(is_strong_approval("确定"));
        assert!(!is_strong_approval("好的")); // weak — still needs a pending "?"
        assert!(!is_strong_approval("先别确认")); // negation vetoes
        assert_eq!(
            decide_chat_mode(Some("这是 PPT 初稿，你看看。"), "确认"),
            AgentMode::Execute
        );
        assert_eq!(
            decide_chat_mode(Some("方案如下：第一步…第二步…"), "做吧"),
            AgentMode::Execute
        );
        // A bare weak "好" after a no-question statement still stays interactive.
        assert_eq!(
            decide_chat_mode(Some("我已经重构了解析器。"), "好"),
            AgentMode::Interactive
        );
    }

    #[test]
    fn intent_first_direct_execution_requests_execute_without_prior_plan() {
        for instruction in [
            "修复这个重复总结问题",
            "赶紧修复上线吧，已经严重影响使用了",
            "直接改，不要问 ready to proceed",
            "把这个按钮删掉并发布",
            "fix this bug and ship it",
            "please implement the endpoint now",
        ] {
            assert_eq!(
                decide_chat_mode(None, instruction),
                AgentMode::Execute,
                "{instruction:?} is a direct execution request and must not trigger plan-first confirmation"
            );
        }
    }

    #[test]
    fn explicit_planning_or_analysis_requests_stay_interactive() {
        for instruction in [
            "先给我方案，不要改代码",
            "分析一下解决方案",
            "只评估风险，别执行",
            "what's the plan before changing anything?",
            "explain the options, do not implement yet",
        ] {
            assert_eq!(
                decide_chat_mode(None, instruction),
                AgentMode::Interactive,
                "{instruction:?} explicitly asks for planning/analysis instead of execution"
            );
        }
    }

    #[test]
    fn turn_capability_separates_review_implementation_and_delivery() {
        // β: wording alone never selects ReviewOnly…
        assert_eq!(
            decide_chat_contract(None, "先系统分析，不要修改代码").capability,
            TurnCapability::ReviewOnly
        );
        // …a hard read-only turn comes from a structured intent instead.
        assert_eq!(
            decide_chat_contract_with_intent(
                None,
                "先系统分析，不要修改代码",
                TurnIntent {
                    structured_read_only: true
                }
            )
            .capability,
            TurnCapability::ReviewOnly
        );
        assert_eq!(
            decide_chat_contract(None, "修复这个重复验证问题").capability,
            TurnCapability::Implement
        );
        assert_eq!(
            decide_chat_contract(None, "修复完成后提交 PR、合并并发布上线").capability,
            TurnCapability::Implement
        );
    }

    #[test]
    fn short_approval_inherits_delivery_only_from_the_approved_proposal() {
        assert_eq!(
            decide_chat_contract(Some("我会修复、开 PR 并发布。可以开始吗？"), "做吧").capability,
            TurnCapability::Implement
        );
        assert_eq!(
            decide_chat_contract(Some("我会在本地修复并验证。可以开始吗？"), "做吧").capability,
            TurnCapability::Implement
        );
        // A bare "好的" after a read-only summary approves nothing in
        // particular, so the turn keeps the discuss-first posture. It does NOT
        // lose write capability — that is a permission decision and nobody
        // asked for read-only here.
        let acknowledged = decide_chat_contract(Some("这是只读审视方案。"), "好的");
        assert_eq!(acknowledged.mode, AgentMode::Interactive);
        assert_ne!(acknowledged.capability, TurnCapability::ReviewOnly);
    }

    #[test]
    fn persisted_delivery_authorization_keeps_deliver_on_followup_turns() {
        // A follow-up "fix this too" turn alone would be Implement…
        let followup = decide_chat_contract(None, "顺便把这两个问题也修复了");
        assert_eq!(followup.capability, TurnCapability::Implement);
        // …but once the session granted delivery, it inherits Deliver.
        let inherited = with_persisted_delivery_authorization(followup, true);
        assert_eq!(inherited.capability, TurnCapability::Deliver);
        assert_eq!(inherited.mode, followup.mode);
        // No standing grant → the original contract is untouched.
        let untouched = with_persisted_delivery_authorization(followup, false);
        assert_eq!(untouched.capability, TurnCapability::Implement);
    }

    #[test]
    fn persisted_delivery_authorization_never_overrides_a_structured_read_only_turn() {
        // β: a wording cue no longer revokes anything, so a standing grant still
        // applies to it.
        let wording_only = decide_chat_contract(None, "先系统分析，不要修改代码");
        assert_eq!(wording_only.capability, TurnCapability::ReviewOnly);
        assert_eq!(
            with_persisted_delivery_authorization(wording_only, true).capability,
            TurnCapability::ReviewOnly
        );
        // A STRUCTURED read-only turn stays review-only whatever the session
        // holds.
        let structured = decide_chat_contract_with_intent(
            None,
            "先系统分析，不要修改代码",
            TurnIntent {
                structured_read_only: true,
            },
        );
        assert_eq!(structured.capability, TurnCapability::ReviewOnly);
        let kept = with_persisted_delivery_authorization(structured, true);
        assert_eq!(kept.capability, TurnCapability::ReviewOnly);
    }

    #[test]
    fn ordinary_diagnostic_does_not_revoke_an_active_delivery_intent() {
        let diagnostic = decide_chat_contract(None, "你为啥没权限，没人限制你啊");
        assert_eq!(diagnostic.mode, AgentMode::Interactive);

        let continued = with_persisted_delivery_authorization(diagnostic, true);
        assert_eq!(continued.capability, TurnCapability::Deliver);
        assert_eq!(continued.mode, AgentMode::Execute);
    }

    #[test]
    fn delivery_revocation_phrasings_are_recognized() {
        assert!(is_delivery_revocation("取消发布，先等等"));
        assert!(is_delivery_revocation("先别提交"));
        assert!(is_delivery_revocation("don't publish yet"));
        assert!(is_delivery_revocation("取消交付"));
        assert!(!is_delivery_revocation("修复后提交上线"));
        assert!(!is_delivery_revocation("继续修复"));
    }

    #[test]
    fn direct_delivery_instructions_use_the_execute_contract() {
        for instruction in [
            "开始改吧",
            "输出完整方案，然后赶紧开始修复上线",
            "搞定上面所有方案里的内容，直到上线发布",
            "ok，把卡死的问题和新的视觉设计一起落地执行",
            "继续修复并完成发布",
        ] {
            assert_eq!(
                decide_chat_mode(Some("问题和方案已经分析清楚。"), instruction),
                AgentMode::Execute,
                "{instruction:?} must not fall back to the 30-round interactive contract"
            );
        }
    }

    #[test]
    fn continuation_is_an_explicit_state_transition_not_a_question_heuristic() {
        for instruction in ["继续", "可以，继续", "继续做", "接着做"] {
            assert_eq!(
                decide_chat_contract(
                    Some("这是已经审视完成的可实施方案。下一步按上述步骤修改并验证。"),
                    instruction,
                )
                .capability,
                TurnCapability::Implement,
                "{instruction:?} must enter implementation even when the proposal does not end in a question"
            );
        }
        assert_eq!(
            decide_chat_contract(Some("我会修复、开 PR、合并并发布。"), "可以，继续",).capability,
            TurnCapability::Implement,
        );
    }

    // ── Colloquial change requests ──────────────────────────────────────────
    //
    // Field report, 2026-08-05, twice in one day. Both sessions carried exactly
    // ONE user message, neither asked for read-only, and both were classified
    // `ReviewOnly` by the fallthrough at the bottom of `decide_chat_contract`:
    // the cue tables above only recognize a user who speaks like a ticket
    // ("修复 / 实现 / 改掉"), while the user actually points at something in
    // their own app, says what is wrong with it, and proposes what it should
    // be instead. The first session cost an extra "好，改" round-trip. The
    // second dead-ended: the write gate refused the edit and the denial forbade
    // asking, so the agent told the user to switch to an "implementation 模式"
    // that does not exist. Both messages below are verbatim from those sessions.

    #[test]
    fn colloquial_defect_reports_are_work_orders_not_questions() {
        for message in [
            // Verbatim, session 074ac81c (the dead end).
            "新建页面的这个窗口布局太丑了，另外也不能很直观的看到用什么模型，是不是应该把模型选择在新建页面放在这里啊",
            // Verbatim, session 03c77092 (the wasted round-trip).
            "你这个聊天输入框区域的底色为啥跟上面会话信息框的底色不一样啊？应该保持一样更好看一些",
            // Same class, other phrasings of "wrong now, this instead".
            "侧栏太挤了，能不能把间距放大一点",
            "这两个按钮的圆角不一致，应该统一成 8px",
            "会话标题看不清，最好加粗一点",
            "this spacing is inconsistent, it should match the header",
        ] {
            assert_eq!(
                decide_chat_contract(None, message).capability,
                TurnCapability::Implement,
                "{message:?} names a defect AND the target state — that is a work order, \
                 not a request for analysis"
            );
        }
    }

    /// The invariant the 2026-08-05 dead end cost us: the hard read-only gate is
    /// reachable ONLY from a user who actually asked for it. Everything else —
    /// including a message the framework cannot classify — keeps write
    /// capability, so a misclassification can never strand the turn.
    ///
    /// `policy::review_only_denial` says "显式只读意图" and forbids re-asking.
    /// That sentence is only honest, and that instruction only correct, while
    /// this holds.
    #[test]
    fn the_hard_read_only_gate_comes_only_from_an_explicit_user_constraint() {
        for message in [
            // Change requests, questions, diagnostics, bare noise — none of
            // these asked for read-only, so none of them may lose write access.
            "这个布局太丑了，是不是应该改成横排",
            "这个 store 是干嘛的",
            "我应该先看哪个文件",
            "这个页面为什么会闪一下",
            "你为啥没权限，没人限制你啊",
            "嗯",
            "",
        ] {
            let contract = decide_chat_contract(None, message);
            assert!(
                !contract.grants.explicit_read_only,
                "{message:?} never constrained the turn"
            );
            assert_ne!(
                contract.capability,
                TurnCapability::ReviewOnly,
                "{message:?} must not lose write capability on a guess"
            );
        }
        // β (2026-09-28): the same read-only wording no longer revokes write
        // capability. It keeps the discuss-first posture and hands the model an
        // instruction; the capability stays Implement, and the audit payload
        // records that the constraint had no capability effect.
        for message in ["先分析一下，不要改代码", "只评估风险，别执行"] {
            let contract = decide_chat_contract(None, message);
            assert!(contract.grants.explicit_read_only, "{message:?}");
            assert_eq!(contract.capability, TurnCapability::ReviewOnly, "{message:?}");
            assert_eq!(contract.mode, AgentMode::Interactive, "{message:?}");
            let block = constraint_instruction_block(message).expect("instruction block");
            assert!(block.contains("turn-constraint"), "{message:?}");
            let payload = constraint_audit_payload(message).expect("audit payload");
            assert_eq!(payload["capability_effect"], "none", "{message:?}");
        }

        // The hard gate is still reachable — from a structured source only.
        let structured = decide_chat_contract_with_intent(
            None,
            "先分析一下，不要改代码",
            TurnIntent {
                structured_read_only: true,
            },
        );
        assert!(structured.grants.explicit_read_only);
        assert_eq!(structured.capability, TurnCapability::ReviewOnly);
    }

    #[test]
    fn scoped_file_and_delivery_constraints_do_not_turn_a_mutation_order_read_only() {
        let message = "请只创建 managed-proof.txt，内容为 managed workspace live proof，确认文件存在后结束。不要提交、推送、开 PR，也不要修改其他文件。";

        let contract = decide_chat_contract(None, message);

        assert_eq!(contract.mode, AgentMode::Execute);
        assert_eq!(contract.capability, TurnCapability::Implement);
        assert!(
            !contract.grants.explicit_read_only,
            "a scope boundary for an explicitly requested write is not a global mutation ban"
        );
    }

    #[test]
    fn questions_without_a_desired_change_keep_the_discuss_first_posture() {
        for message in [
            // No complaint, no target — a plain question about the code.
            "这个 store 是干嘛的",
            "我应该先看哪个文件",
            "为什么应该用 Postgres 而不是 SQLite",
            // A complaint the user wants *explained*, not changed yet.
            "这个页面为什么会闪一下",
            // Asking for the method IS asking for a plan. (An English message
            // carrying "fix" already reaches Implement through the pre-existing
            // execution table; this change neither widens nor narrows that.)
            "这个布局不好看，应该怎么改比较好",
            "the spacing looks off, how should we handle it",
            // The user's own uncertainty is not a product defect.
            "我不太清楚这个模块应该怎么读",
        ] {
            assert_eq!(
                decide_chat_contract(None, message).mode,
                AgentMode::Interactive,
                "{message:?} asks for understanding or method; the turn should answer \
                 before acting"
            );
        }
    }

    #[test]
    fn a_wording_constraint_no_longer_wins_over_a_change_request() {
        for message in [
            "先分析一下这个布局为什么这么丑，不要改代码",
            "别改，先给个方案：这两个圆角不一致，应该统一成 8px",
        ] {
            // β: the constraint becomes an instruction; the change request
            // keeps its write capability.
            let contract = decide_chat_contract(None, message);
            assert_eq!(contract.capability, TurnCapability::ReviewOnly, "{message:?}");
            assert!(contract.grants.explicit_read_only, "{message:?}");
            assert!(
                constraint_instruction_block(message).is_some(),
                "{message:?} should still be handed to the model as a constraint"
            );
        }
        // Only a structured read-only intent still wins over a change request.
        let structured = decide_chat_contract_with_intent(
            None,
            "先分析一下这个布局为什么这么丑，不要改代码",
            TurnIntent {
                structured_read_only: true,
            },
        );
        assert_eq!(structured.capability, TurnCapability::ReviewOnly);
        assert!(structured.grants.explicit_read_only);
    }

    /// End-to-end reproduction of the field failure, across BOTH modules that
    /// had to agree for it to happen. Each side's own unit tests were green
    /// while the composition dead-ended, so this asserts the composition:
    /// classify the verbatim user message, then ask the real structural gate
    /// whether the edit it wanted is allowed.
    #[test]
    fn the_field_report_message_can_now_reach_the_edit_it_was_denied() {
        use codefactory_agent_core::ToolKind;
        use codefactory_agent_loop::policy::capability_denial;

        // Verbatim, session 074ac81c — the only user message in that session.
        let contract = decide_chat_contract(
            None,
            "新建页面的这个窗口布局太丑了，另外也不能很直观的看到用什么模型，\
是不是应该把模型选择在新建页面放在这里啊",
        );
        // The two writes the agent actually attempted and was refused.
        for (tool, args) in [
            (
                "edit_file",
                serde_json::json!({
                    "path": "src/pages/Workspace/WorkspacePage.draft.test.tsx",
                    "old_string": "a",
                    "new_string": "b",
                }),
            ),
            (
                "write_file",
                serde_json::json!({
                    "path": "src/components/DraftScopeBar.tsx",
                    "content": "// …",
                }),
            ),
        ] {
            assert!(
                capability_denial(
                    contract.capability,
                    tool,
                    &format!("{tool} {}", args["path"]),
                    &ToolKind::Mutation,
                    &args,
                )
                .is_none(),
                "{tool} was structurally denied for a user who never asked for read-only"
            );
        }

        // …while a user who DID ask for read-only still gets the hard gate.
        // β (2026-09-28): the gate is still reachable, but only from a
        // STRUCTURED read-only intent — wording alone no longer sets it.
        let constrained = decide_chat_contract_with_intent(
            None,
            "先分析一下这个布局，不要改代码",
            TurnIntent {
                structured_read_only: true,
            },
        );
        assert!(
            capability_denial(
                constrained.capability,
                "edit_file",
                "edit_file src/components/DraftScopeBar.tsx",
                &ToolKind::Mutation,
                &serde_json::json!({
                    "path": "src/components/DraftScopeBar.tsx",
                    "old_string": "a",
                    "new_string": "b",
                }),
            )
            .is_some(),
            "an explicit read-only request must still block product edits"
        );
    }

    #[test]
    fn a_mid_turn_change_request_reaches_implementation() {
        assert_eq!(
            steer_capability_override("等下，这个间距也太挤了，应该跟上面对齐"),
            None,
        );
    }

    #[test]
    fn explicit_mid_turn_correction_can_change_the_active_capability() {
        for message in ["先把之前批准的执行了，再回来分析原因", "别只审视了，继续实施，把刚才这个方案执行完。", "继续", "继续发布上线"] {
            assert_eq!(steer_capability_override(message), None);
        }
        assert_eq!(steer_capability_override("先别修改，继续分析"), Some(TurnCapability::ReviewOnly));
    }

    /// 2026-09-08 field report. Seven sessions were resumed with the single
    /// word `继续`; every one of them lost its standing delivery grant on the
    /// spot, because the steer path routed `继续` to `Implement` and
    /// `policy::capability_denial` then refused every commit, PR and release
    /// with "当前意图只授权本地实施". The word the user typed to keep the work
    /// moving was the reason it could not finish. A steer changes the current
    /// ACTION, and only an explicit read-only steer or an explicit revocation
    /// lowers a grant the session already holds — the same rule
    /// `with_persisted_delivery_authorization` applies at turn entry.
    #[test]
    fn a_continuation_steer_keeps_a_standing_delivery_grant() {
        for message in ["继续", "接着做", "继续实施", "go on"] {
            assert_eq!(
                steer_capability_override_with_authorization(message, true),
                steer_capability_override(message).map(|_| TurnCapability::Deliver),
                "{message} must not revoke a delivery grant the session already holds"
            );
        }
        // Without a grant nothing is inherited: the steer still means Implement.
        assert_eq!(
            steer_capability_override_with_authorization("继续", false),
            None,
        );
        // An explicit read-only steer still wins over a standing grant, and an
        // explicit revocation is handled by `is_delivery_revocation` upstream.
        // β: wording no longer revokes, so this steer leaves the standing grant
        // alone instead of dropping the turn to ReviewOnly.
        assert_eq!(
            steer_capability_override_with_authorization("先别修改，继续分析", true),
            Some(TurnCapability::ReviewOnly),
        );
        assert!(is_delivery_revocation("先别发布"));
        // A message that steers nothing still steers nothing.
        assert_eq!(steer_capability_override_with_authorization("嗯", true), None);
    }

    /// β fixture 1 — the 2026-09-24 shape: a delivery request that ends with a
    /// constraint about a hypothetical case. Synthesized, not the field text.
    #[test]
    fn a_conditional_aside_never_revokes_the_turn_it_travels_with() {
        let message =
            "先合并主干并跑测试，然后调用交付工具；如果新提交的 CI 再遇到环境抖动，不要改代码或验收脚本。";
        let contract = decide_chat_contract(None, message);
        assert_ne!(contract.capability, TurnCapability::ReviewOnly, "{message:?}");
        assert!(!contract.grants.explicit_read_only, "{message:?}");
        let payload = constraint_audit_payload(message).expect("audit payload");
        assert_eq!(payload["capability_effect"], "none");
        assert!(payload["instruction_block"]
            .as_str()
            .is_some_and(|block| block.contains("turn-constraint")));
    }

    /// β fixture 2 — a pure read-only sentence: discuss-first posture, write
    /// capability retained, instruction handed to the model.
    #[test]
    fn a_read_only_sentence_keeps_write_capability_and_becomes_an_instruction() {
        let message = "先分析一下这个布局，不要改代码";
        let contract = decide_chat_contract(None, message);
        assert_eq!(contract.mode, AgentMode::Interactive, "{message:?}");
        assert_eq!(contract.capability, TurnCapability::ReviewOnly, "{message:?}");
        assert!(contract.grants.explicit_read_only, "{message:?}");
        assert!(constraint_instruction_block(message).is_some(), "{message:?}");
    }

    /// β fixture 3 — an explicitly read-only delegated subagent is the only
    /// thing that yields ReviewOnly, and it must still be able to hand back
    /// evidence: read-only tools stay allowed while mutations stay refused.
    #[test]
    fn a_structured_read_only_subagent_turn_can_still_hand_back_evidence() {
        use codefactory_agent_core::ToolKind;
        use codefactory_agent_loop::policy::capability_denial;

        let contract = decide_chat_contract_with_intent(
            None,
            "只读审查当前分支的实现，运行必要的聚焦测试并返回证据，不要修改文件。",
            TurnIntent {
                structured_read_only: true,
            },
        );
        assert_eq!(contract.capability, TurnCapability::ReviewOnly);
        assert!(contract.grants.explicit_read_only);
        // Reading and read-only shell probes still work…
        for (tool, kind, args) in [
            (
                "read_file",
                ToolKind::ReadOnly,
                serde_json::json!({ "path": "src/lib.rs" }),
            ),
            (
                "bash",
                ToolKind::ReadOnly,
                serde_json::json!({ "command": "git status --porcelain" }),
            ),
        ] {
            assert!(
                capability_denial(contract.capability, tool, tool, &kind, &args).is_none(),
                "{tool} must stay available so a review turn can hand back evidence"
            );
        }
        // …while a mutation is still refused.
        assert!(
            capability_denial(
                contract.capability,
                "edit_file",
                "edit_file src/a.rs",
                &ToolKind::Mutation,
                &serde_json::json!({ "path": "src/a.rs", "old_string": "a", "new_string": "b" }),
            )
            .is_some(),
            "a review-only turn must not edit product code"
        );
    }

    /// β fixture 4 — a delegated brief that carries ONLY read-only wording (no
    /// structured field) keeps write capability.
    #[test]
    fn a_delegated_brief_with_only_read_only_wording_keeps_write_capability() {
        let message = "只读审查当前分支的实现，不要修改文件。";
        let contract = decide_chat_contract(None, message);
        assert!(contract.grants.explicit_read_only, "{message:?}");
        assert_eq!(contract.capability, TurnCapability::ReviewOnly, "{message:?}");
    }

    /// β fixture 5 — a structured read-only turn must still hand back evidence:
    /// running the project's own tests is allowed, editing is not.
    #[test]
    fn a_structured_read_only_turn_may_run_project_tests_but_not_edit() {
        use codefactory_agent_core::ToolKind;
        use codefactory_agent_loop::policy::capability_denial;

        let contract = decide_chat_contract_with_intent(
            None,
            "只读审查当前分支的实现并运行项目测试。",
            TurnIntent {
                structured_read_only: true,
            },
        );
        assert_eq!(contract.capability, TurnCapability::ReviewOnly);
        for command in ["cargo test --lib dispatch", "pnpm test", "npm test"] {
            assert!(
                capability_denial(
                    contract.capability,
                    "bash",
                    command,
                    &ToolKind::ReadOnly,
                    &serde_json::json!({ "command": command }),
                )
                .is_none(),
                "{command} must stay available so a review-only turn can hand back evidence"
            );
        }
        assert!(
            capability_denial(
                contract.capability,
                "edit_file",
                "edit_file src/a.rs",
                &ToolKind::Mutation,
                &serde_json::json!({ "path": "src/a.rs", "old_string": "a", "new_string": "b" }),
            )
            .is_some(),
            "a review-only turn must not edit product code"
        );
    }

    // M48 R1/R3 — an explicit delivery-authorization sentence beats every
    // question/hesitation cue in the same message. Field message 10-09
    // (session d074ba4a) read "授权交付，ceiling: pr_only" and then asked
    // "先核实个人账户仓库能否用 merge queue"; the posture cue table matched
    // and `sessions.delivery_authorized` stayed 0.
    #[test]
    fn explicit_authorization_beats_question_cues_in_the_same_message() {
        for message in [
            "授权交付，ceiling: merged。先核实个人账户仓库能否用 merge queue，再分析一下风险。",
            "先核实个人账户仓库是否可以用 merge queue？授权交付，ceiling: pr_only。",
            "授权交付，ceiling: pr_only。可不可以先看看 CI 再动手？",
        ] {
            assert!(
                is_explicit_delivery_authorization(message),
                "{message} carries an explicit delivery authorization"
            );
            assert_eq!(
                decide_chat_contract(None, message).capability,
                TurnCapability::Implement,
                "{message} must not authorize delivery by wording"
            );
        }
    }

    #[test]
    fn authorization_position_and_language_do_not_matter() {
        for message in [
            // authorization in the trailing sentence
            "先修好问题，最后授权交付。",
            // authorization in the middle, question before and after
            "能否先用 pr_only？这件事已授权交付，ceiling: pr_only，是否可行？",
            // English wording
            "Delivery authorized — ceiling: merged. Can you first check whether the personal account repo supports a merge queue?",
        ] {
            assert_eq!(
                decide_chat_contract(None, message).capability,
                TurnCapability::Implement,
                "{message} must not authorize delivery by wording"
            );
        }
    }

    #[test]
    fn plain_consultation_without_authorization_stays_undelivered() {
        // R3/约束: no authorization sentence anywhere → the default stays
        // "do not deliver", even though the wording asks about shipping.
        for message in [
            "先核实个人账户仓库能否用 merge queue，再分析一下风险。",
            "我们是否应该先看看 CI 的结果，再决定下一步怎么做？",
            "Could you check how the personal account repo is configured for the queue?",
        ] {
            assert!(
                !is_explicit_delivery_authorization(message),
                "{message} carries no explicit delivery authorization"
            );
            let contract = decide_chat_contract(None, message);
            assert_ne!(
                contract.capability,
                TurnCapability::Deliver,
                "{message} must not grant delivery"
            );
        }
    }

    #[test]
    fn explicit_read_only_constraints_still_veto_an_authorization() {
        // R2: posture cues no longer lower permission, but an explicit
        // read-only directive is a permission constraint and stays fail-closed.
        for message in [
            "授权交付，但只分析，不要修改任何文件。",
            "authorized to deliver, but do not implement anything yet.",
        ] {
            assert!(!is_explicit_delivery_authorization(message), "{message}");
            assert_ne!(
                decide_chat_contract(None, message).capability,
                TurnCapability::Deliver,
                "{message} keeps the read-only constraint"
            );
        }
    }

    /// M116-R2/R4: the dispatch message's declared ceiling must survive parsing.
    /// Before this, `ceiling: merged` only produced a boolean grant, so the first
    /// delivery fell back to the global `pr_only` and was recorded there.
    #[test]
    fn m116_declared_ceiling_is_read_from_the_dispatch_message() {
        use crate::config::settings::DeliveryCeiling;
        let cases = [
            ("授权交付（ceiling: merged）。按分支交付。", DeliveryCeiling::ThroughMerge),
            ("开始实施：授权交付（ceiling: through_merge）", DeliveryCeiling::ThroughMerge),
            ("授权交付，ceiling: pr_only。", DeliveryCeiling::PrOnly),
            ("授权交付（ceiling: through_ci_green）", DeliveryCeiling::ThroughCiGreen),
            ("授权交付（ceiling: through_release）", DeliveryCeiling::ThroughRelease),
            ("Delivery authorized — ceiling: merged.", DeliveryCeiling::ThroughMerge),
            // The widest declaration in one message wins.
            ("授权交付，ceiling: pr_only，最终 ceiling: merged", DeliveryCeiling::ThroughMerge),
        ];
        for (message, expected) in cases {
            assert_eq!(
                declared_delivery_ceiling(message),
                Some(expected),
                "{message}"
            );
        }
    }

    /// A message that authorizes delivery but names no ceiling must leave the
    /// previous/default ceiling alone, and an explicit read-only directive must
    /// not produce a ceiling at all.
    #[test]
    fn m116_declared_ceiling_requires_a_writable_delivery_grant() {
        assert_eq!(declared_delivery_ceiling("继续，你被授权交付。"), None);
        assert_eq!(
            declared_delivery_ceiling("授权交付，ceiling: merged，但只分析，不要修改任何文件。"),
            None
        );
        // Existing dispatch contract: a ceiling declaration is itself a grant.
        assert_eq!(
            declared_delivery_ceiling("ceiling: merged"),
            Some(crate::config::settings::DeliveryCeiling::ThroughMerge)
        );
    }
}

// --- M200/R2: durable acceptance receipt for a dispatched turn ------------
//
// The dispatch bridge used to answer "已开始" as soon as the request was
// accepted, so a turn that never actually queued anything (row not persisted,
// wake not scheduled) still looked delivered to the caller. The receipt below
// is only produced from persisted state: an exact user row in the target
// session, plus whatever the wake/turn tables really hold.

/// M200/R2: only an exact persisted user row is an acceptance receipt.
#[derive(Debug, serde::Serialize)]
pub struct DurableDispatchReceipt {
    pub session_id: String,
    pub root_turn_id: String,
    pub status: &'static str,
    pub next_at: Option<i64>,
    pub turn_exists: bool,
}

pub(crate) async fn load_delivery_receipt(
    db: &sqlx::SqlitePool,
    session: &str,
    root: &str,
    message: &str,
) -> Result<Option<DurableDispatchReceipt>, crate::errors::AppError> {
    let persisted: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM messages WHERE id=? AND session_id=? AND role='user' AND content=?",
    )
    .bind(root)
    .bind(session)
    .bind(message)
    .fetch_one(db)
    .await?;
    if persisted == 0 {
        return Ok(None);
    }
    super::background_tasks::ensure_wake_schema(db).await?;
    let next_at: Option<i64> = sqlx::query_scalar(
        "SELECT next_at FROM wakeup_schedules WHERE id=? AND session_id=? AND root_turn_id=? AND status='pending'",
    )
    .bind(format!("model-availability-{session}"))
    .bind(session)
    .bind(root)
    .fetch_optional(db)
    .await?;
    let turn_exists: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chat_turn_state WHERE root_turn_id=? AND session_id=?")
            .bind(root)
            .bind(session)
            .fetch_one(db)
            .await?;
    if next_at.is_none() {
        if turn_exists == 0 { return Ok(None); }
        let started: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chat_run_controls WHERE root_turn_id=? AND session_id=? AND status='active' AND created_process_instance=?")
            .bind(root).bind(session).bind(crate::agent::objective::current_process_instance()).fetch_one(db).await?;
        let answered: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages answer JOIN messages user ON user.id=? WHERE answer.session_id=user.session_id AND answer.role='assistant' AND answer.created_at>=user.created_at")
            .bind(root).fetch_one(db).await?;
        if started == 0 && answered == 0 { return Ok(None); }
    }
    Ok(Some(DurableDispatchReceipt {
        session_id: session.into(),
        root_turn_id: root.into(),
        status: if next_at.is_some() {
            "queued_waiting_model"
        } else {
            "started"
        },
        next_at,
        turn_exists: turn_exists > 0,
    }))
}

#[tauri::command]
pub async fn dispatch_delivery_receipt(
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
    root_turn_id: String,
    message: String,
) -> Result<Option<DurableDispatchReceipt>, crate::errors::AppError> {
    let db = state.db.read().await.clone();
    load_delivery_receipt(&db, &session_id, &root_turn_id, &message).await
}

#[cfg(test)]
mod m200_receipt_tests {
    use super::*;

    #[tokio::test]
    async fn m206_receipt_rejects_persisted_message_without_started_turn() {
        let db = crate::storage::db::connect("sqlite::memory:").await.unwrap();
        sqlx::query("INSERT INTO sessions(id,title,cwd,model_id,created_at,updated_at) VALUES ('receipt','synthetic','/synthetic','model',1,1)").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO messages(id,session_id,role,content,created_at) VALUES ('unstarted','receipt','user','next task',100)").execute(&db).await.unwrap();
        assert!(load_delivery_receipt(&db,"receipt","unstarted","next task").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn m200_receipt_requires_the_exact_persisted_row_and_reports_a_pending_model_wake() {
        let db = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        super::super::background_tasks::ensure_wake_schema(&db)
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE messages (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, role TEXT NOT NULL, content TEXT NOT NULL);
            CREATE TABLE chat_turn_state (root_turn_id TEXT PRIMARY KEY, session_id TEXT NOT NULL);
            INSERT INTO messages VALUES ('root-1','synthetic','user','synthetic task');
            INSERT INTO chat_turn_state VALUES ('root-1','synthetic');
            INSERT INTO wakeup_schedules (id,session_id,root_turn_id,prompt,source,next_at,status,created_at)
              VALUES ('model-availability-synthetic','synthetic','root-1','retry','model_availability',2000000000,'pending',1);",
        )
        .execute(&db)
        .await
        .unwrap();
        let receipt = load_delivery_receipt(&db, "synthetic", "root-1", "synthetic task")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (receipt.status, receipt.next_at, receipt.turn_exists),
            ("queued_waiting_model", Some(2000000000), true)
        );
        assert!(load_delivery_receipt(&db, "synthetic", "root-1", "different task")
            .await
            .unwrap()
            .is_none());
        assert!(load_delivery_receipt(&db, "other", "root-1", "synthetic task")
            .await
            .unwrap()
            .is_none());
    }
}
