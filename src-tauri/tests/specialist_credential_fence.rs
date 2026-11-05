// SPDX-License-Identifier: Apache-2.0
#[path = "../src/specialist_credential_fence.rs"]
mod fence;
#[tokio::test(flavor = "current_thread")]
async fn mutation_invalidates_old_observation_and_serializes_commit() {
    let first = fence::revision().unwrap();
    {
        let _mutation = fence::mutation().unwrap();
    }
    assert!(fence::commit(first, async { Ok(()) }).await.is_err());
    let revision = fence::revision().unwrap();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let commit = tokio::spawn(fence::commit(revision, async move {
        entered_tx.send(()).unwrap();
        release_rx.await.unwrap();
        Ok(())
    }));
    entered_rx.await.unwrap();
    assert!(
        fence::mutation().is_err(),
        "direct async callers must fail promptly"
    );
    let mut mutation = tokio::task::spawn_blocking(|| fence::mutation().is_err());
    let early = tokio::time::timeout(std::time::Duration::from_millis(200), &mut mutation).await;
    release_tx.send(()).unwrap();
    commit.await.unwrap().unwrap();
    // Old blocking mutation times out, but releasing above still lets cleanup finish.
    let refused_promptly = match early {
        Ok(value) => value.unwrap(),
        Err(_) => {
            mutation.await.unwrap();
            false
        }
    };
    assert!(
        refused_promptly,
        "synchronous callers must not block runtime workers"
    );
    {
        let _retry = fence::mutation().unwrap();
    }
    cancelled_caller_keeps_owned_config_guard().await;
}
// Keep in the same test process sequence: the fence epoch is process-global.
async fn cancelled_caller_keeps_owned_config_guard() {
    let settings = std::sync::Arc::new(tokio::sync::RwLock::new(()));
    let guard = settings.clone().read_owned().await;
    let revision = fence::revision().unwrap();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let caller = tokio::spawn(fence::commit(revision, async move {
        entered_tx.send(()).unwrap();
        release_rx.await.unwrap();
        drop(guard);
        finished_tx.send(()).unwrap();
        Ok(())
    }));
    entered_rx.await.unwrap();
    caller.abort();
    let _ = caller.await;
    assert!(settings.try_write().is_err());
    release_tx.send(()).unwrap();
    finished_rx.await.unwrap();
    assert!(settings.try_write().is_ok());
}
