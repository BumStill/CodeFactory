// SPDX-License-Identifier: Apache-2.0
// Real production preflight/execute and durable claim; no forged file contract.
#[cfg(unix)]
async fn r2_shell_assert_unknown_is_not_replayed(expected_bytes_present: bool) {
    let backend = objective_backend(false).await;
    let dir = tempfile::tempdir().unwrap();
    let args = serde_json::json!({"command":"printf '%s' planned > state.txt"});
    let ctx = objective_ctx(dir.path());
    let receipt =
        prime_file_receipt_without_dispatch(&backend, &ctx, "shell-before-crash", "bash", &args)
            .await;
    if expected_bytes_present {
        std::fs::write(dir.path().join("state.txt"), b"planned").unwrap();
    }
    claim_file_recovery(&backend, 2).await;
    let mut recovery_ctx = ctx.clone();
    recovery_ctx.mutation_permit = Some(current_permit(2));
    let resumed = call_with_args("shell-after-crash", "bash", &args);
    register_tool_call(&backend, &resumed, &args).await;
    let out = backend
        .execute(&resumed, &args, &recovery_ctx)
        .await
        .unwrap();
    assert_ne!(out.status,ToolExecutionStatus::Done,"file evidence alone must not authorize shell redispatch or synthesize whole-command success: {out:?}");
    assert_eq!(
        dir.path().join("state.txt").exists(),
        expected_bytes_present,
        "uncertain command must not execute"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM side_effect_receipts WHERE id=?")
        .bind(&receipt)
        .fetch_one(&backend.db)
        .await
        .unwrap();
    assert!(matches!(status.as_str(), "started" | "unknown"));
    let policy: Option<String> =
        sqlx::query_scalar("SELECT replay_policy FROM tool_recovery_contracts WHERE receipt_id=?")
            .bind(&receipt)
            .fetch_optional(&backend.db)
            .await
            .unwrap();
    assert_eq!(policy.as_deref(), Some("never_after_dispatch"));
}
#[cfg(unix)]
#[tokio::test]
async fn r2_shell_recovery_unchanged_files_never_redispatch() {
    r2_shell_assert_unknown_is_not_replayed(false).await;
}
#[cfg(unix)]
#[tokio::test]
async fn r2_shell_recovery_expected_files_do_not_claim_whole_command_done() {
    r2_shell_assert_unknown_is_not_replayed(true).await;
}
