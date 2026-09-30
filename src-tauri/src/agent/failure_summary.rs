// SPDX-License-Identifier: Apache-2.0

//! The plain-language failure report a user sees when system-owned recovery
//! reaches its ceiling.
//!
//! Product rule: when the system cannot finish a task by itself, the user gets
//! exactly two honest outcomes — it keeps trying by another route, or it says
//! "this did not work" and explains what it tried, what it kept, and how to go
//! on from here. There is no third "quietly waiting for a future capability"
//! state the user cannot act on.
//!
//! The report is rendered from structured data (attempts, preserved changes,
//! delivery state) so that step two — "automatically try a different
//! approach" — can reuse the same per-approach structure instead of parsing
//! prose.
//!
//! Banned vocabulary: this text is shown verbatim to users, so it must never
//! contain internal machine words such as recovery/generation/incident/
//! objective/remediation/exhausted. `assert_no_internal_vocabulary` is the
//! executable guard and is asserted by tests and before the message is stored.

use std::fmt::Write as _;

/// One approach the system already tried, merged across repeats of the same
/// underlying problem. Kept as a list element (not a sentence) so a later
/// "try something else" step can pick a different entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttemptSummary {
    /// Plain-language name of what was attempted.
    pub approach: String,
    /// How many consecutive attempts hit this same problem.
    pub attempts: i64,
    /// Plain-language reason each attempt stopped.
    pub reason: String,
}

/// One file kept from the work in progress, relative to the baseline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreservedChange {
    pub path: String,
    pub added: i64,
    pub removed: i64,
    /// A brand-new file that has no baseline entry yet.
    pub untracked: bool,
}

/// What survived the failed task and can be built on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreservedWork {
    pub branch: Option<String>,
    pub location: Option<String>,
    pub changes: Vec<PreservedChange>,
    /// Total number of changed paths, including the ones not listed.
    pub total_changed_files: i64,
    pub pr_url: Option<String>,
    pub pr_state: Option<String>,
}

/// Everything the report needs, gathered before rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureReport {
    /// One sentence naming the goal that was not achieved.
    pub goal: String,
    pub attempts: Vec<AttemptSummary>,
    pub work: PreservedWork,
}

/// How many changed files are listed before the rest are summarised.
pub const MAX_LISTED_FILES: usize = 10;

/// Words that must never reach the user-facing message.
pub const BANNED_INTERNAL_WORDS: [&str; 10] = [
    "恢复耗尽",
    "安全上限",
    "系统故障",
    "已登记",
    "能力更新",
    "incident",
    "objective",
    "remediation",
    "generation",
    "recovery",
];

/// Executable guard for the banned-vocabulary rule above.
pub fn assert_no_internal_vocabulary(text: &str) -> Result<(), String> {
    let lowered = text.to_lowercase();
    for word in BANNED_INTERNAL_WORDS {
        if lowered.contains(&word.to_lowercase()) {
            return Err(format!("failure summary leaks internal vocabulary: {word}"));
        }
    }
    Ok(())
}

/// Plain-language description of a stable failure code. Unknown codes must not
/// leak a raw identifier into the message, so they collapse to a generic
/// phrase that is still honest about being unspecified.
pub fn plain_failure_reason(failure_code: Option<&str>) -> String {
    let code = failure_code.unwrap_or_default();
    if code.is_empty() {
        return "遇到了一个没说明的技术问题".to_string();
    }
    let text = match code {
        "provider_endpoint_unavailable" => "所选模型服务一直连不上",
        "provider_route_unavailable" => "没有可用的模型线路",
        "provider_rate_limited" => "模型服务持续拒绝请求",
        "provider_auth_expired" => "模型服务的登录状态已失效",
        "completion_evidence_incomplete" => "给出的结论缺少可核对的依据",
        "external_state_uncertain" => "外部改动是否已经生效无法确认",
        "tool_observation_contract_missing" => "这一步缺少可核对结果的执行方式",
        "tool_timeout" => "有一个工具执行超时",
        "tool_panic" => "有一个工具执行时崩溃",
        "permission_timed_out" => "等待授权超时",
        "permission_channel_closed" => "授权通道在中途关闭",
        "context_compaction_exhausted" => "可用的对话上下文已经用完",
        "run_budget_exhausted" => "这一轮可用的执行预算已经用完",
        "agent_loop_error" => "执行过程本身出错",
        "delivery_identity_conflict" => "交付身份与已有记录冲突",
        "platform_incident" => "运行环境出现了持续问题",
        "test_failure" | "verification_failed" => "修改后测试没有通过",
        _ => "遇到了一个没说明的技术问题",
    };
    text.to_string()
}

/// Plain-language name of the approach a domain uses to keep going.
pub fn plain_approach(domain: Option<&str>) -> String {
    let text = match domain.unwrap_or_default() {
        "chat" => "继续同一件事并重试",
        "provider" => "换用同一模型线路重试",
        "auth" => "刷新登录凭据后重试",
        "tool" => "重新执行这一步",
        "permission" => "重新申请授权后继续",
        "task" => "重新排队这个任务",
        "context" => "压缩对话上下文后继续",
        "browser" => "重新连接浏览器后继续",
        "terminal" => "重新接管终端后继续",
        "delivery" => "重新对齐交付状态后继续",
        "release" => "重新对齐发布状态后继续",
        "update" => "等待更新安装完成后继续",
        _ => "再试一次",
    };
    text.to_string()
}

fn render_change_list(work: &PreservedWork) -> Vec<String> {
    if work.changes.is_empty() {
        return vec!["执行工作区里没有留下未提交的改动。".to_string()];
    }
    let mut lines = Vec::new();
    let listed = work.changes.len().min(MAX_LISTED_FILES);
    for change in work.changes.iter().take(listed) {
        let suffix = if change.untracked { "（新文件）" } else { "" };
        lines.push(format!(
            "- {}：新增 {} 行，删除 {} 行{}",
            change.path, change.added, change.removed, suffix
        ));
    }
    let remaining = work.total_changed_files - listed as i64;
    if remaining > 0 {
        lines.push(format!("- 等 {remaining} 个文件"));
    }
    lines
}

/// Render the user-visible failure report.
pub fn render_failure_report(report: &FailureReport) -> String {
    let mut out = String::new();
    let goal = report.goal.trim();
    let goal = if goal.is_empty() { "你交代的这件事" } else { goal };
    let _ = writeln!(out, "这件事没做成：{goal}");
    let _ = writeln!(out);

    if report.attempts.is_empty() {
        let _ = writeln!(out, "试过的办法：还没有形成可复述的重试记录。");
    } else {
        let _ = writeln!(out, "试过的办法，以及每种为什么没成：");
        for attempt in &report.attempts {
            if attempt.attempts > 1 {
                let _ = writeln!(
                    out,
                    "- {} 连续 {} 次都没成：{}。",
                    attempt.approach, attempt.attempts, attempt.reason
                );
            } else {
                let _ = writeln!(
                    out,
                    "- {}，没成：{}。",
                    attempt.approach, attempt.reason
                );
            }
        }
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "保留下来的成果：");
    if let Some(location) = report.work.location.as_deref() {
        if let Some(branch) = report.work.branch.as_deref() {
            let _ = writeln!(out, "- 改动都在 {location}（分支 {branch}），没有丢掉。");
        } else {
            let _ = writeln!(out, "- 改动都在 {location}，没有丢掉。");
        }
    }
    for line in render_change_list(&report.work) {
        let _ = writeln!(out, "{line}");
    }
    if let Some(pr_url) = report.work.pr_url.as_deref() {
        match report.work.pr_state.as_deref() {
            Some(state) if !state.is_empty() => {
                let _ = writeln!(out, "- 已经开好的 PR：{pr_url}（{state}）");
            }
            _ => {
                let _ = writeln!(out, "- 已经开好的 PR：{pr_url}");
            }
        }
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "下一步：");
    let _ = writeln!(
        out,
        "- 直接回一句「继续」，我会在这些改动上接着做；"
    );
    let _ = writeln!(
        out,
        "- 回「换个办法」，我会换一条路重新试；"
    );
    let _ = writeln!(out, "- 回「把这些改动交付」，我会把它们提交并开好 PR。");
    out.trim_end().to_string()
}

/// Build a report from already-collected structured data, then assert the
/// banned-vocabulary rule before anyone can store it.
pub fn build_failure_report(
    goal: &str,
    attempts: Vec<AttemptSummary>,
    work: PreservedWork,
) -> Result<String, String> {
    let report = FailureReport {
        goal: goal.to_string(),
        attempts: merge_attempts(attempts),
        work,
    };
    let text = render_failure_report(&report);
    assert_no_internal_vocabulary(&text)?;
    Ok(text)
}

/// Merge repeats of the same (approach, reason) pair into one counted entry so
/// the user reads "连续 5 次…" instead of five identical bullets.
pub fn merge_attempts(attempts: Vec<AttemptSummary>) -> Vec<AttemptSummary> {
    let mut merged: Vec<AttemptSummary> = Vec::new();
    for attempt in attempts {
        if let Some(existing) = merged.iter_mut().find(|item| {
            item.approach == attempt.approach && item.reason == attempt.reason
        }) {
            existing.attempts += attempt.attempts.max(1);
        } else {
            merged.push(AttemptSummary {
                attempts: attempt.attempts.max(1),
                ..attempt
            });
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_report() -> FailureReport {
        FailureReport {
            goal: "把导出功能改成流式写入".to_string(),
            attempts: vec![
                AttemptSummary {
                    approach: plain_approach(Some("chat")),
                    attempts: 5,
                    reason: plain_failure_reason(Some("provider_rate_limited")),
                },
                AttemptSummary {
                    approach: plain_approach(Some("tool")),
                    attempts: 1,
                    reason: plain_failure_reason(Some("verification_failed")),
                },
            ],
            work: PreservedWork {
                branch: Some("codefactory/stream-export".to_string()),
                location: Some("工作区".to_string()),
                changes: vec![
                    PreservedChange {
                        path: "src/export.ts".to_string(),
                        added: 42,
                        removed: 7,
                        untracked: false,
                    },
                    PreservedChange {
                        path: "src/new-helper.ts".to_string(),
                        added: 12,
                        removed: 0,
                        untracked: true,
                    },
                ],
                total_changed_files: 2,
                pr_url: Some("https://example.test/pull/1".to_string()),
                pr_state: Some("等待合并".to_string()),
            },
        }
    }

    #[test]
    fn rendered_report_names_goal_tries_keepings_and_next_step() {
        let text = render_failure_report(&sample_report());
        assert!(text.contains("这件事没做成：把导出功能改成流式写入"));
        assert!(text.contains("连续 5 次都没成"));
        assert!(text.contains("src/export.ts：新增 42 行，删除 7 行"));
        assert!(text.contains("（新文件）"));
        assert!(text.contains("已经开好的 PR：https://example.test/pull/1"));
        assert!(text.contains("继续"));
        assert!(text.contains("把这些改动交付"));
    }

    #[test]
    fn rendered_report_never_leaks_internal_vocabulary() {
        let text = render_failure_report(&sample_report());
        assert_no_internal_vocabulary(&text).expect("report must be user-safe");
    }

    #[test]
    fn unknown_failure_code_does_not_leak_the_identifier() {
        let reason = plain_failure_reason(Some("weird_internal_code_7"));
        assert_eq!(reason, "遇到了一个没说明的技术问题");
        assert_no_internal_vocabulary(&reason).expect("reason must be user-safe");
    }

    #[test]
    fn banned_vocabulary_guard_rejects_the_old_limbo_copy() {
        let old = "本回合的自动恢复已达到安全上限，已登记为系统故障。";
        assert!(assert_no_internal_vocabulary(old).is_err());
    }

    #[test]
    fn long_change_lists_are_truncated_with_a_remainder() {
        let mut work = PreservedWork {
            total_changed_files: 14,
            ..PreservedWork::default()
        };
        for index in 0..14 {
            work.changes.push(PreservedChange {
                path: format!("src/file-{index}.ts"),
                added: 1,
                removed: 0,
                untracked: false,
            });
        }
        let report = FailureReport {
            goal: "重构模块".to_string(),
            attempts: vec![],
            work,
        };
        let text = render_failure_report(&report);
        assert!(text.contains("src/file-9.ts"));
        assert!(!text.contains("src/file-10.ts"));
        assert!(text.contains("等 4 个文件"));
        assert_no_internal_vocabulary(&text).expect("report must be user-safe");
    }

    #[test]
    fn repeated_attempts_merge_into_one_counted_entry() {
        let merged = merge_attempts(vec![
            AttemptSummary {
                approach: "再试一次".to_string(),
                attempts: 1,
                reason: "模型服务持续拒绝请求".to_string(),
            },
            AttemptSummary {
                approach: "再试一次".to_string(),
                attempts: 4,
                reason: "模型服务持续拒绝请求".to_string(),
            },
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].attempts, 5);
    }
}
