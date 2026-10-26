// SPDX-License-Identifier: Apache-2.0
//! R1: old rows remain legacy; new sessions explicitly select thin.
pub(crate) async fn is_thin(db: &sqlx::SqlitePool, session: &str) -> bool {
    sqlx::query_scalar::<_, String>("SELECT delivery_mode FROM sessions WHERE id=?")
        .bind(session)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .as_deref()
        == Some("thin")
}

pub(crate) fn external_pending(pr: &serde_json::Value) -> bool {
    pr["state"] == "OPEN"
        || pr["statusCheckRollup"].as_array().is_some_and(|checks| {
            checks.iter().any(|check| {
                check["status"]
                    .as_str()
                    .is_some_and(|status| status != "COMPLETED")
                    || check["state"]
                        .as_str()
                        .is_some_and(|state| matches!(state, "PENDING" | "EXPECTED"))
            })
        })
}

/// Observe GitHub once, never persist a second source of delivery truth.
pub(crate) async fn external_wait_hint(db: &sqlx::SqlitePool, session: &str) -> Option<String> {
    let reference: Option<(String, i64)> =
        sqlx::query_as("SELECT branch,pr_number FROM thin_delivery_refs WHERE session_id=?")
            .bind(session)
            .fetch_optional(db)
            .await
            .ok()
            .flatten();
    let Some((_branch, number)) = reference else {
        return None;
    };
    let cwd: String = sqlx::query_scalar("SELECT cwd FROM sessions WHERE id=?")
        .bind(session)
        .fetch_one(db)
        .await
        .ok()?;
    let raw = tokio::task::spawn_blocking(move || {
        super::delivery::bounded_delivery_output(
            super::delivery::dev_command("gh").current_dir(cwd).args([
                "pr",
                "view",
                &number.to_string(),
                "--json",
                "state,statusCheckRollup",
            ]),
        )
    })
    .await;
    tracing::info!(
        session_id = session,
        gh_calls = 1,
        step = "completion_observe",
        "thin_delivery"
    );
    let pending = match raw {
        Ok(Ok(raw)) => serde_json::from_str::<serde_json::Value>(&raw)
            .map(|pr| external_pending(&pr))
            .unwrap_or(true),
        _ => true,
    };
    super::background_tasks::external_wait_completion_hint(db, session, pending).await
}

/// R5：外部事项（PR/CI/发版/部署/流水线）是**模型自己能监视**的对象
/// （`gh pr view`、`gh run watch`、`background_task_start`、`schedule_wakeup`）。
/// 它们绝不能走 R3 的「等一个人」分类——那会把「等 CI」升级成给用户的问题
/// （v1.91.1 会话 6f5e87a2 的现场：03:33 停在 waiting_user，
/// 问题写着「我在等「交付 PR / CI」，但这不是我能自己确认的对象」）。
pub(crate) fn is_external_object(what: &str) -> bool {
    let lower = what.to_lowercase();
    const CJK: [&str; 8] = [
        "交付",
        "发版",
        "发布",
        "部署",
        "流水线",
        "工作流",
        "合并",
        "上线",
    ];
    if CJK.iter().any(|token| lower.contains(token)) {
        return true;
    }
    const ASCII: [&str; 10] = [
        "pr", "prs", "ci", "mr", "release", "deploy", "merge", "pipeline", "workflow", "actions",
    ];
    lower
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '#')
        .map(|token| token.trim_matches('#'))
        .any(|token| ASCII.contains(&token))
}

/// Pending work observed by a tool, not inferred from a legacy failure code.
pub(crate) async fn record_unwatched_wait(
    db: &sqlx::SqlitePool,
    session: &str,
    waiting_for: &str,
) -> sqlx::Result<()> {
    sqlx::raw_sql(include_str!("../../migrations/0023_thin_wait_notes.sql"))
        .execute(db)
        .await?;
    sqlx::query("INSERT INTO thin_wait_notes(session_id,waiting_for) VALUES (?,?) ON CONFLICT(session_id) DO UPDATE SET waiting_for=excluded.waiting_for")
        .bind(session).bind(waiting_for).execute(db).await?;
    Ok(())
}

pub(crate) async fn unwatched_wait(db: &sqlx::SqlitePool, session: &str) -> Option<String> {
    sqlx::query_scalar("SELECT waiting_for FROM thin_wait_notes WHERE session_id=?")
        .bind(session)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
}

pub(crate) fn resume_prompt(at: i64, summary: &str) -> String {
    let time = chrono::DateTime::from_timestamp_millis(at)
        .map(|time| time.with_timezone(&chrono::Local).to_rfc3339())
        .unwrap_or_else(|| at.to_string());
    format!("你在 {time} 被中断，最后一步是 {summary}，可能已执行也可能未执行。先用 git status、git log、gh pr view 等确认现状，再决定下一步。")
}

#[cfg(test)]
mod tests {
    /// R5：外部事项（模型自己能监视）与「等一个人/一次确认」必须可判定，
    /// 否则 R3 会把「等 CI」误判成给用户的问题。
    #[test]
    fn external_objects_are_not_person_waits() {
        for what in [
            "交付 PR / CI",
            "PR #42 的 CI",
            "等 GitHub Actions",
            "release 构建",
            "等发版",
            "部署流水线",
            "等合并结果",
        ] {
            assert!(
                super::is_external_object(what),
                "should be external: {what}"
            );
        }
        for what in [
            "等你确认用哪个文件夹",
            "等客户回复",
            "等你确认是否删除",
            "等你提供 API key",
        ] {
            assert!(
                !super::is_external_object(what),
                "should be a person wait: {what}"
            );
        }
    }

    #[test]
    fn resumed_bash_always_has_system_prompt_for_success_failure_unknown() {
        for status in ["done", "error", "pending"] {
            let prompt = super::resume_prompt(0, &format!("bash ({status}): synthetic-command"));
            assert!(prompt.contains("可能已执行也可能未执行"));
            assert!(prompt.contains("git status、git log、gh pr view"));
            assert!(prompt.contains(status));
        }
    }
    #[test]
    fn external_wait_tracks_forge_not_mapping_existence() {
        assert!(!super::external_pending(
            &serde_json::json!({"state":"MERGED","statusCheckRollup":[]})
        ));
        assert!(!super::external_pending(
            &serde_json::json!({"state":"CLOSED","statusCheckRollup":[]})
        ));
        assert!(super::external_pending(
            &serde_json::json!({"state":"OPEN","statusCheckRollup":[]})
        ));
        assert!(super::external_pending(
            &serde_json::json!({"state":"MERGED","statusCheckRollup":[{"status":"IN_PROGRESS"}]})
        ));
    }
    #[tokio::test]
    async fn migration_keeps_old_rows_legacy_and_accepts_explicit_thin() {
        let db = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::raw_sql(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY); INSERT INTO sessions VALUES ('old');",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::raw_sql(include_str!("../../migrations/0022_thin_delivery.sql"))
            .execute(&db)
            .await
            .unwrap();
        assert!(!super::is_thin(&db, "old").await);
        sqlx::query("INSERT INTO sessions (id,delivery_mode) VALUES ('new','thin')")
            .execute(&db)
            .await
            .unwrap();
        assert!(super::is_thin(&db, "new").await);
        assert!(!super::is_thin(&db, "missing").await);
        assert!(
            sqlx::query("INSERT INTO sessions (id,delivery_mode) VALUES ('bad','other')")
                .execute(&db)
                .await
                .is_err()
        );
    }
}

#[cfg(test)]
mod phase2_state_tests {
    use crate::agent::thin_state::{self, SessionState, WaitNeed};

    // R8 replay: the final answer exists once and every real-machine case
    // settles into one of the four durable states.
    #[test]
    fn final_answer_replay_cases_settle_four_states() {
        let ended = thin_state::after_final(WaitNeed::None, "本会话记录");
        assert_eq!(ended.state, SessionState::Ended);
        let watched = thin_state::after_final(
            WaitNeed::External {
                what: "PR #774 合并".into(),
                covered: true,
            },
            "",
        );
        assert_eq!(watched.state, SessionState::WaitingWake);
        // R5（v1.91.1 会话 6f5e87a2 现场）：外部事项没有唤醒源时不再向用户发问，
        // 而是停在 `waiting_wake` 并写明该挂什么监视——工具结果层面已经提示过
        // background_task_start / schedule_wakeup；R4 的 30 分钟失效守卫兜底。
        let unwatched = thin_state::after_final(
            WaitNeed::External {
                what: "浏览器只读任务".into(),
                covered: false,
            },
            "",
        );
        assert_eq!(unwatched.state, SessionState::WaitingWake);
        assert!(
            unwatched.question.is_none(),
            "外部事项不得升级成用户问题：{:?}",
            unwatched.question
        );
        assert!(unwatched
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("schedule_wakeup"));
        assert_eq!(unwatched.waiting_for.as_deref(), Some("浏览器只读任务"));
        let self_invented = thin_state::after_final(
            WaitNeed::Unwatched {
                what: "确认范围".into(),
            },
            "",
        );
        assert_eq!(self_invented.state, SessionState::WaitingUser);
        assert!(self_invented.question.is_some());
    }

    #[test]
    fn three_identical_technical_failures_end_and_model_outage_waits_for_its_wake() {
        assert_eq!(
            thin_state::after_failure(1, "bash exited 1").state,
            SessionState::Running
        );
        assert_eq!(
            thin_state::after_failure(2, "bash exited 1").state,
            SessionState::Running
        );
        let ended = thin_state::after_failure(3, "bash exited 1");
        assert_eq!(ended.state, SessionState::Ended);
        assert!(ended
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("bash exited 1"));
        let no_model = thin_state::after_no_model(0);
        assert_eq!(no_model.state, SessionState::WaitingWake);
        assert!(no_model.wake_source.is_some());
    }

    /// M178/R2: a `waiting_wake` row with an empty reason reads as a silent
    /// hang. Every model wait says what is unavailable and when it continues.
    #[test]
    fn m178_a_model_wait_always_states_a_reason() {
        let wait = thin_state::after_no_model(60_000);
        assert_eq!(wait.state, SessionState::WaitingWake);
        assert_eq!(wait.waiting_for.as_deref(), Some("模型恢复可用"));
        let reason = wait.reason.clone().unwrap_or_default();
        assert!(!reason.trim().is_empty(), "waiting_wake 不得没有原因");
        assert!(reason.contains("自动继续"), "{reason}");
        assert!(reason.contains("不需要你操作"), "{reason}");
    }

    #[tokio::test]
    async fn thin_state_round_trips_and_expired_wake_becomes_a_user_question() {
        let db = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        thin_state::ensure(&db).await.unwrap();
        thin_state::save(
            &db,
            "synthetic-session",
            &thin_state::after_final(
                WaitNeed::External {
                    what: "CI 结束".into(),
                    covered: true,
                },
                "",
            ),
        )
        .await
        .unwrap();
        let loaded = thin_state::load(&db, "synthetic-session")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(loaded.state, SessionState::WaitingWake);
        let swept = thin_state::sweep(&db, i64::MAX / 4).await.unwrap();
        assert_eq!(swept, 1);
        let expired = thin_state::load(&db, "synthetic-session")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(expired.state, SessionState::WaitingUser);
        assert!(expired
            .question
            .as_deref()
            .unwrap_or_default()
            .contains("已失效"));
    }
}
