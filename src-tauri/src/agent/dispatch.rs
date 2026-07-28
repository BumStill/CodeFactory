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

use super::AgentMode;

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
    "不要执行",
    "别执行",
    "不要动代码",
    "别动代码",
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
        || toks.iter().any(|token| DIRECT_EXEC_EN_WORDS.contains(token))
}

/// The framework's per-turn contract decision for a chat message.
///
/// `prev_assistant` is the most recent assistant message already in history
/// (None on the first turn). `user_msg` is the message being sent now.
pub fn decide_chat_mode(prev_assistant: Option<&str>, user_msg: &str) -> AgentMode {
    // Intent-first: if the user directly asks us to fix/implement/ship/change,
    // act now. A plan is only the right response when the user explicitly asks
    // for planning/analysis or the request is genuinely not executable yet.
    if is_direct_execution_request(user_msg) {
        return AgentMode::Execute;
    }
    // A clear, imperative go-ahead executes even if our previous turn didn't end
    // with a question — models don't reliably emit "Ready to proceed?", so we
    // don't make the contract hinge on it. Permission policy is evaluated
    // later and never changes this semantic mode.
    if is_strong_approval(user_msg) {
        return AgentMode::Execute;
    }
    match prev_assistant {
        Some(p) if is_pending_proposal(p) && is_approval(user_msg) => AgentMode::Execute,
        _ => AgentMode::Interactive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
