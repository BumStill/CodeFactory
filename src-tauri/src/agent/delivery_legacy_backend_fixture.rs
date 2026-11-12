// SPDX-License-Identifier: Apache-2.0
// Only included by tool_backend::tests. Continue an already allocated test
// Objective through the real native host; never synthesize ownership receipts.
pub(super) async fn r2_native_write_existing_fixture_inner(
    pool: &sqlx::SqlitePool,
    root: &std::path::Path,
    path: &str,
    content: &str,
) {
    let mut executor = backend().await;
    executor.db = pool.clone();
    let id = format!("legacy-native-write-{}", uuid::Uuid::new_v4());
    let args = serde_json::json!({"path": path, "content": content});
    let call = call_with_args(&id, "write_file", &args);
    register_tool_call(&executor, &call, &args).await;
    let result = executor.execute(&call, &args, &objective_ctx(root))
        .await.expect("native write executes under the existing Objective");
    assert_eq!(result.status, ToolExecutionStatus::Done, "{result:?}");
    assert!(!result.is_error, "{result:?}");
    crate::trajectory::record_tool_call_finished(
        &executor.db, TEST_SESSION_ID, &id, "done",
        Some("native legacy fixture write completed"), None, 0,
    ).await.unwrap();
}
