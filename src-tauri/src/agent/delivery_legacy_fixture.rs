// SPDX-License-Identifier: Apache-2.0
// Included only inside delivery::tests. These remote-state-machine fixtures
// enter through a real managed workspace and native write/edit host receipts.
// No existing file is adopted and no ownership receipt is inserted by the test.
struct OwnedDeliveryFixture {
    db: sqlx::SqlitePool,
    _temp: tempfile::TempDir,
    root: PathBuf,
    objective_id: String,
}

impl OwnedDeliveryFixture {
    async fn new() -> Self {
        let (db, temp, root, objective_id) =
            crate::agent::tool_backend::r2_native_delivery_fixture(false).await;
        Self { db, _temp: temp, root, objective_id }
    }

    async fn write(&self, path: &str, content: &str) {
        crate::agent::tool_backend::r2_native_write_existing_fixture(
            &self.db, &self.root, path, content,
        ).await;
    }

    async fn scope(&self) -> TaskDeliveryScope {
        TaskDeliveryScope::load(&self.db, &self.objective_id, &self.root, &[])
            .await.expect("the production host receipts prove this fixture's exact content")
    }

    async fn options(&self) -> DeliverOpts {
        DeliverOpts {
            // This helper is only for tests of remote state transitions, not
            // tests of implicit title selection on arbitrary branch names.
            title: Some("fix: exercise owned delivery state transitions".into()),
            task_scope: Some(self.scope().await),
            ..DeliverOpts::default()
        }
    }
}
