// SPDX-License-Identifier: Apache-2.0
//! Linearizes in-process secret mutation with a local observation commit.
//! Epoch is deliberately process-scoped: after restart history remains readable,
//! but observing again requires a fresh explicit binding. External keychain edits
//! are not controlled by this lock and never imply execution permission.
use anyhow::{ensure, Result};
use once_cell::sync::Lazy;
use std::{
    future::Future,
    sync::{Mutex, MutexGuard},
};
static EPOCH: Lazy<Mutex<String>> = Lazy::new(|| Mutex::new(uuid::Uuid::new_v4().to_string()));
pub fn revision() -> Result<String> {
    Ok(EPOCH
        .try_lock()
        .map_err(|_| anyhow::anyhow!("credential state busy or unavailable"))?
        .clone())
}
pub fn mutation() -> Result<MutexGuard<'static, String>> {
    let mut guard = EPOCH
        .try_lock()
        .map_err(|_| anyhow::anyhow!("credential update busy; retry after observation commit"))?;
    // Invalidate even a failed or ambiguous write; never resurrect an old grant.
    *guard = uuid::Uuid::new_v4().to_string();
    Ok(guard)
}
pub async fn commit<T, F>(expected: String, action: F) -> Result<T>
where
    T: Send + 'static,
    F: Future<Output = Result<T>> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let guard = EPOCH
            .lock()
            .map_err(|_| anyhow::anyhow!("credential fence poisoned"))?;
        ensure!(*guard == expected, "credential epoch changed");
        tokio::runtime::Handle::current().block_on(action)
    })
    .await?
}
