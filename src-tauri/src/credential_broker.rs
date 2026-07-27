// SPDX-License-Identifier: Apache-2.0
//! Process-wide, lazy credential access.
//!
//! A Keychain read can outlive an async caller (for example while macOS waits
//! for user authorization). The broker keeps that one blocking read alive,
//! shares it across callers, and caches the typed result so a timeout or denial
//! cannot create a prompt loop on the next turn.

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{oneshot, Mutex};

const DEFAULT_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialErrorKind {
    Unavailable,
    Store,
}

#[derive(Clone, Debug)]
pub struct CredentialError {
    pub kind: CredentialErrorKind,
    pub message: String,
}

type CredentialResult = Result<Option<String>, CredentialError>;
type Reader = Arc<dyn Fn(&str) -> CredentialResult + Send + Sync>;

enum CredentialSlot {
    Loading(Vec<oneshot::Sender<CredentialResult>>),
    Ready(CredentialResult),
}

pub struct CredentialBroker {
    slots: Arc<Mutex<HashMap<String, CredentialSlot>>>,
    reader: Reader,
    timeout: Duration,
}

static GLOBAL: Lazy<Arc<CredentialBroker>> = Lazy::new(|| {
    Arc::new(CredentialBroker::new(
        Arc::new(|key_ref| {
            crate::secrets::get_key(key_ref).map_err(|error| CredentialError {
                kind: CredentialErrorKind::Store,
                message: error.to_string(),
            })
        }),
        DEFAULT_LOOKUP_TIMEOUT,
    ))
});

impl CredentialBroker {
    fn new(reader: Reader, timeout: Duration) -> Self {
        Self {
            slots: Arc::new(Mutex::new(HashMap::new())),
            reader,
            timeout,
        }
    }

    pub fn global() -> Arc<Self> {
        GLOBAL.clone()
    }

    pub async fn get(&self, key_ref: &str) -> CredentialResult {
        let (receiver, launch) = {
            let mut slots = self.slots.lock().await;
            match slots.get_mut(key_ref) {
                Some(CredentialSlot::Ready(result)) => return result.clone(),
                Some(CredentialSlot::Loading(waiters)) => {
                    let (sender, receiver) = oneshot::channel();
                    waiters.push(sender);
                    (receiver, false)
                }
                None => {
                    let (sender, receiver) = oneshot::channel();
                    slots.insert(key_ref.to_string(), CredentialSlot::Loading(vec![sender]));
                    (receiver, true)
                }
            }
        };

        if launch {
            let slots = self.slots.clone();
            let reader = self.reader.clone();
            let lookup_ref = key_ref.to_string();
            tokio::spawn(async move {
                let blocking_ref = lookup_ref.clone();
                let result = tokio::task::spawn_blocking(move || reader(&blocking_ref))
                    .await
                    .unwrap_or_else(|error| {
                        Err(CredentialError {
                            kind: CredentialErrorKind::Store,
                            message: format!("凭据读取任务异常：{error}"),
                        })
                    });
                let waiters = {
                    let mut slots = slots.lock().await;
                    match slots.insert(lookup_ref, CredentialSlot::Ready(result.clone())) {
                        Some(CredentialSlot::Loading(waiters)) => waiters,
                        _ => Vec::new(),
                    }
                };
                for waiter in waiters {
                    let _ = waiter.send(result.clone());
                }
            });
        }

        match tokio::time::timeout(self.timeout, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(CredentialError {
                kind: CredentialErrorKind::Store,
                message: "凭据读取任务提前结束".into(),
            }),
            Err(_) => Err(CredentialError {
                kind: CredentialErrorKind::Unavailable,
                message: "凭据读取仍在等待系统授权".into(),
            }),
        }
    }

    pub async fn put(&self, key_ref: &str, value: &str) -> Result<(), CredentialError> {
        let write_ref = key_ref.to_string();
        let write_value = value.to_string();
        tokio::task::spawn_blocking(move || crate::secrets::set_key(&write_ref, &write_value))
            .await
            .map_err(|error| CredentialError {
                kind: CredentialErrorKind::Store,
                message: format!("凭据保存任务异常：{error}"),
            })?
            .map_err(|error| CredentialError {
                kind: CredentialErrorKind::Store,
                message: error.to_string(),
            })?;
        self.slots.lock().await.insert(
            key_ref.to_string(),
            CredentialSlot::Ready(Ok(Some(value.to_string()))),
        );
        Ok(())
    }

    pub async fn delete(&self, key_ref: &str) -> Result<(), CredentialError> {
        let delete_ref = key_ref.to_string();
        tokio::task::spawn_blocking(move || crate::secrets::delete_key(&delete_ref))
            .await
            .map_err(|error| CredentialError {
                kind: CredentialErrorKind::Store,
                message: format!("凭据删除任务异常：{error}"),
            })?
            .map_err(|error| CredentialError {
                kind: CredentialErrorKind::Store,
                message: error.to_string(),
            })?;
        self.slots.lock().await.remove(key_ref);
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn invalidate(&self, key_ref: &str) {
        self.slots.lock().await.remove(key_ref);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn concurrent_and_later_reads_share_one_store_lookup() {
        let calls = Arc::new(AtomicUsize::new(0));
        let reader_calls = calls.clone();
        let broker = Arc::new(CredentialBroker::new(
            Arc::new(move |_| {
                reader_calls.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(25));
                Ok(Some("secret".into()))
            }),
            Duration::from_secs(1),
        ));
        let mut tasks = Vec::new();
        for _ in 0..20 {
            let broker = broker.clone();
            tasks.push(tokio::spawn(async move { broker.get("deepseek").await }));
        }
        for task in tasks {
            assert_eq!(task.await.unwrap().unwrap().as_deref(), Some("secret"));
        }
        assert_eq!(
            broker.get("deepseek").await.unwrap().as_deref(),
            Some("secret")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn timeout_keeps_the_single_lookup_alive_for_the_next_turn() {
        let calls = Arc::new(AtomicUsize::new(0));
        let reader_calls = calls.clone();
        let broker = CredentialBroker::new(
            Arc::new(move |_| {
                reader_calls.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(80));
                Ok(Some("secret".into()))
            }),
            Duration::from_millis(10),
        );
        assert_eq!(
            broker.get("deepseek").await.unwrap_err().kind,
            CredentialErrorKind::Unavailable
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            broker.get("deepseek").await.unwrap().as_deref(),
            Some("secret")
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
