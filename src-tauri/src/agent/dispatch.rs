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
    /// The current message explicitly constrains mutations. This is separate
    /// from a default question/diagnostic classification.
    pub explicit_read_only: bool,
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

fn is_explicit_planning_request(user_msg: &str) -> bool {
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
    let toks = tokens(&m);
    PLAN_ONLY_CJK.iter().any(|cue| m.contains(cue))
        || PLAN_ONLY_EN_PHRASES.iter().any(|cue| m.contains(cue))
        || toks.iter().any(|token| PLAN_ONLY_EN_WORDS.contains(token))
}

fn is_direct_execution_request(user_msg: &str) -> bool {
    let m = user_msg.trim().to_lowercase();
    if m.is_empty() || is_explicit_planning_request(&m) {
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
    if m.is_empty() || is_explicit_planning_request(&m) {
        return false;
    }
    let toks = tokens(&m);
    DELIVERY_CJK.iter().any(|cue| m.contains(cue))
        || DELIVERY_EN_PHRASES.iter().any(|cue| m.contains(cue))
        || toks.iter().any(|token| DELIVERY_EN_WORDS.contains(token))
}

fn is_explicit_continuation_request(user_msg: &str) -> bool {
    let m = user_msg.trim().to_lowercase();
    if m.is_empty() || is_explicit_planning_request(&m) {
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
    if m.is_empty() || is_explicit_planning_request(&m) {
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

/// A mid-run user steer can change the current action intent at the next safe
/// round boundary. This is separate from normal permission approval: it
/// changes the user's objective and therefore must reach policy evaluation.
pub fn steer_capability_override(user_msg: &str) -> Option<TurnCapability> {
    if is_explicit_planning_request(user_msg) {
        return Some(TurnCapability::ReviewOnly);
    }
    if is_delivery_request(user_msg) {
        return Some(TurnCapability::Deliver);
    }
    if is_direct_execution_request(user_msg)
        || is_change_request(user_msg)
        || is_explicit_continuation_request(user_msg)
        || is_strong_approval(user_msg)
    {
        return Some(TurnCapability::Implement);
    }
    None
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

pub fn decide_chat_contract(prev_assistant: Option<&str>, user_msg: &str) -> ChatContract {
    let explicit_read_only = is_explicit_planning_request(user_msg);
    let grants = TurnGrants {
        browser_read: grants_browser_read(user_msg),
        explicit_read_only,
    };
    if explicit_read_only {
        return ChatContract {
            mode: AgentMode::Interactive,
            capability: TurnCapability::ReviewOnly,
            grants,
        };
    }
    if is_delivery_request(user_msg) {
        return ChatContract {
            mode: AgentMode::Execute,
            capability: TurnCapability::Deliver,
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
            capability: prev_assistant
                .and_then(proposal_capability)
                .unwrap_or(TurnCapability::Implement),
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
            capability: approved_proposal
                .or_else(|| prev_assistant.and_then(proposal_capability))
                .unwrap_or(TurnCapability::Implement),
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
        assert_eq!(
            decide_chat_contract(None, "先系统分析，不要修改代码").capability,
            TurnCapability::ReviewOnly
        );
        assert_eq!(
            decide_chat_contract(None, "修复这个重复验证问题").capability,
            TurnCapability::Implement
        );
        assert_eq!(
            decide_chat_contract(None, "修复完成后提交 PR、合并并发布上线").capability,
            TurnCapability::Deliver
        );
    }

    #[test]
    fn short_approval_inherits_delivery_only_from_the_approved_proposal() {
        assert_eq!(
            decide_chat_contract(Some("我会修复、开 PR 并发布。可以开始吗？"), "做吧").capability,
            TurnCapability::Deliver
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
    fn persisted_delivery_authorization_never_overrides_explicit_planning() {
        let planning = decide_chat_contract(None, "先系统分析，不要修改代码");
        assert_eq!(planning.capability, TurnCapability::ReviewOnly);
        // Even with a standing grant, an explicit planning request stays
        // review-only — the user's current intent wins.
        let kept = with_persisted_delivery_authorization(planning, true);
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
            TurnCapability::Deliver,
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
        for message in ["先分析一下，不要改代码", "只评估风险，别执行"] {
            let contract = decide_chat_contract(None, message);
            assert!(contract.grants.explicit_read_only, "{message:?}");
            assert_eq!(
                contract.capability,
                TurnCapability::ReviewOnly,
                "{message:?}"
            );
        }
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
    fn an_explicit_read_only_request_still_wins_over_a_change_request() {
        for message in [
            "先分析一下这个布局为什么这么丑，不要改代码",
            "别改，先给个方案：这两个圆角不一致，应该统一成 8px",
        ] {
            let contract = decide_chat_contract(None, message);
            assert_eq!(contract.capability, TurnCapability::ReviewOnly);
            assert!(contract.grants.explicit_read_only, "{message:?}");
        }
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
        let constrained = decide_chat_contract(None, "先分析一下这个布局，不要改代码");
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
            Some(TurnCapability::Implement),
        );
    }

    #[test]
    fn explicit_mid_turn_correction_can_change_the_active_capability() {
        assert_eq!(
            steer_capability_override("先把之前批准的执行了，再回来分析原因"),
            Some(TurnCapability::Implement),
        );
        assert_eq!(
            steer_capability_override("别只审视了，继续实施，把刚才这个方案执行完。"),
            Some(TurnCapability::Implement),
        );
        assert_eq!(
            steer_capability_override("不要只分析了，继续执行"),
            Some(TurnCapability::Implement),
        );
        assert_eq!(
            steer_capability_override("继续"),
            Some(TurnCapability::Implement),
        );
        assert_eq!(
            steer_capability_override("继续发布上线"),
            Some(TurnCapability::Deliver),
        );
        assert_eq!(
            steer_capability_override("先别修改，继续分析"),
            Some(TurnCapability::ReviewOnly),
        );
    }
}
