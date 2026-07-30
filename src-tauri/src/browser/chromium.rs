// SPDX-License-Identifier: Apache-2.0
//! The local Chromium backend, driven over the DevTools Protocol.
//!
//! Owns browser *processes*, which is the part that has bitten this project
//! before: a session whose owner crashed once left a headless Chrome running at
//! full CPU for five days. So every launch is paired with an explicit kill on
//! close, the process is `kill_on_drop` as a backstop, and a session that fails
//! part-way through setup tears down what it already started rather than
//! leaking it.
//!
//! Refs are the other thing worth reading carefully. Element references are
//! minted by our own page script as `ref_<n>` and merely echoed back by the
//! model, so [`validated_ref`] rejects anything that isn't that exact shape.
//! With that check in front, building an attribute selector from a ref is
//! provably safe — which lets us use real CDP mouse and keyboard events
//! instead of JavaScript `.click()`, and real events are what make form
//! submission and framework handlers behave like a user did it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::page::Page;
use futures::StreamExt;
use tokio::sync::Mutex;

use super::install::{self, InstallState};
use super::page as page_script;
use super::profile::{self, LockOutcome, ProfileScope};
use super::{BrowserDriver, PageContent};
use crate::errors::{AppError, Result};

/// Launching has to cover a cold profile and a first paint.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(60);
/// Any single page operation. Generous enough for a slow site, short enough
/// that a wedged page surfaces as an error instead of hanging the turn.
const OP_TIMEOUT: Duration = Duration::from_secs(30);

/// One launched browser and the page we drive in it.
struct LiveSession {
    browser: Browser,
    page: Page,
    /// Drives the CDP connection; aborted on close.
    handler: tokio::task::JoinHandle<()>,
    /// Held so an ephemeral profile's directory is removed when it ends.
    _scratch: Option<tempfile::TempDir>,
    /// Set for persistent profiles, so close releases the right lock.
    profile_dir: Option<PathBuf>,
}

/// Chromium running under CodeFactory's control.
#[derive(Default)]
pub struct ChromiumDriver {
    sessions: Arc<Mutex<HashMap<String, LiveSession>>>,
}

impl ChromiumDriver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve the downloaded browser, or explain what to do about it.
    fn executable() -> Result<PathBuf> {
        let platform = install::Platform::current().ok_or_else(|| {
            AppError::Other(
                "Browser control isn't available on this platform — no Chromium build is \
                 published for it."
                    .into(),
            )
        })?;
        let root = install::install_root()
            .ok_or_else(|| AppError::Other("Could not resolve the home directory".into()))?;
        match install::detect(&root, platform) {
            InstallState::Ready(found) => Ok(found.binary),
            // Actionable rather than a bare failure: the caller turns this into
            // the download prompt instead of showing the user a dead end.
            InstallState::Missing { previous: None } => Err(AppError::Other(
                "Chromium isn't installed yet. Enable browser control in Settings to download \
                 it (about 150 MB, one time)."
                    .into(),
            )),
            InstallState::Missing { previous: Some(_) } => Err(AppError::Other(
                "The downloaded Chromium is incomplete. Re-run the download from Settings to \
                 repair it."
                    .into(),
            )),
        }
    }

    /// Run `f` against the session's page, or say the session is unknown.
    async fn with_page<T, F, Fut>(&self, session_id: &str, f: F) -> Result<T>
    where
        F: FnOnce(Page) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let page = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(session_id)
                .map(|live| live.page.clone())
                .ok_or_else(|| {
                    AppError::Other(format!(
                        "Browser session {session_id} is not open. Open one first."
                    ))
                })?
        };
        f(page).await
    }

    /// Make sure the page script is present, then evaluate `expression`.
    ///
    /// The script is re-installed every time because navigation wipes it and
    /// the driver cannot see every navigation a page performs on its own. The
    /// script short-circuits when already present, so this is cheap and — the
    /// property the tests pin — does not renumber existing refs.
    async fn eval_with_script(page: &Page, expression: String) -> Result<String> {
        timeout(page.evaluate(page_script::install_expression())).await?;
        let value: String = timeout(page.evaluate(expression))
            .await?
            .into_value()
            .map_err(|error| AppError::Other(format!("Page script returned no value: {error}")))?;
        Ok(value)
    }

    /// Resolve a ref to an element, using a selector built only from a
    /// validated `ref_<n>` so real input events can be used safely.
    async fn element(page: &Page, reference: &str) -> Result<chromiumoxide::element::Element> {
        let reference = validated_ref(reference)?;
        timeout(page.evaluate(page_script::install_expression())).await?;
        let selector = format!("[data-cf-ref='{reference}']");
        timeout(page.find_element(selector)).await.map_err(|_| {
            AppError::Other(format!(
                "{reference} is no longer on the page — take a fresh snapshot and use the new ref."
            ))
        })
    }
}

/// Accept only refs our own page script mints.
///
/// The model never invents a ref; it echoes one back. Anything else is either a
/// mistake or an attempt to reach an element we never offered, and both should
/// be refused before the value can reach a selector.
fn validated_ref(reference: &str) -> Result<String> {
    let ok = reference
        .strip_prefix("ref_")
        .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()));
    if ok {
        Ok(reference.to_string())
    } else {
        Err(AppError::Other(format!(
            "'{reference}' is not an element reference from a snapshot (expected ref_<number>)."
        )))
    }
}

/// Apply the standard operation timeout and flatten the error.
async fn timeout<T, E: std::fmt::Display>(
    future: impl std::future::Future<Output = std::result::Result<T, E>>,
) -> Result<T> {
    match tokio::time::timeout(OP_TIMEOUT, future).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(AppError::Other(format!("Browser operation failed: {error}"))),
        Err(_) => Err(AppError::Other(format!(
            "Browser operation timed out after {}s",
            OP_TIMEOUT.as_secs()
        ))),
    }
}

#[async_trait]
impl BrowserDriver for ChromiumDriver {
    async fn open(
        &self,
        session_id: &str,
        url: &str,
        scope: &ProfileScope,
    ) -> Result<PageContent> {
        if self.sessions.lock().await.contains_key(session_id) {
            return Err(AppError::Other(format!(
                "Browser session {session_id} is already open."
            )));
        }
        let executable = Self::executable()?;

        // Persistent profiles are exclusive; an ephemeral one gets a scratch
        // directory that is deleted with the session.
        let (user_data_dir, scratch, profile_dir) = match profile::profile_dir(scope) {
            Some(dir) => {
                match profile::acquire_lock(&dir, session_id)
                    .map_err(|error| AppError::Other(format!("Profile is unusable: {error}")))?
                {
                    LockOutcome::Acquired => {}
                    LockOutcome::Busy { holder } => {
                        return Err(AppError::Other(format!(
                            "That browser profile is already in use by session {holder}. Close \
                             it first, or use a different profile."
                        )))
                    }
                }
                (dir.clone(), None, Some(dir))
            }
            None => {
                let scratch = tempfile::tempdir().map_err(|error| {
                    AppError::Other(format!("Could not create a scratch profile: {error}"))
                })?;
                (scratch.path().to_path_buf(), Some(scratch), None)
            }
        };

        let config = BrowserConfig::builder()
            .chrome_executable(&executable)
            .user_data_dir(&user_data_dir)
            // Headed: the user has to be able to complete a sign-in, including
            // any 2FA, in this window. A headless browser cannot be signed in
            // to anything by a person.
            .with_head()
            .build()
            .map_err(|error| AppError::Other(format!("Invalid browser configuration: {error}")))?;

        let launched = tokio::time::timeout(LAUNCH_TIMEOUT, Browser::launch(config)).await;
        let (mut browser, mut handler) = match launched {
            Ok(Ok(pair)) => pair,
            Ok(Err(error)) => {
                release_profile(&profile_dir, session_id);
                return Err(AppError::Other(format!("Could not start Chromium: {error}")));
            }
            Err(_) => {
                release_profile(&profile_dir, session_id);
                return Err(AppError::Other("Chromium did not start in time".into()));
            }
        };

        let handler = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    break;
                }
            }
        });

        // From here on, every failure path must stop the process we just
        // started — this is exactly where a leaked browser would come from.
        let page = match timeout(browser.new_page(url)).await {
            Ok(page) => page,
            Err(error) => {
                let _ = tokio::time::timeout(OP_TIMEOUT, browser.close()).await;
                let _ = tokio::time::timeout(Duration::from_secs(5), browser.kill()).await;
                handler.abort();
                release_profile(&profile_dir, session_id);
                return Err(error);
            }
        };

        let mut live = LiveSession {
            browser,
            page,
            handler,
            _scratch: scratch,
            profile_dir: profile_dir.clone(),
        };

        let content = match Self::eval_with_script(&live.page, page_script::readable_expression())
            .await
            .and_then(|json| {
                page_script::parse_readable(&json).ok_or_else(|| {
                    AppError::Other("Could not extract readable content from the page".into())
                })
            }) {
            Ok(content) => content,
            Err(error) => {
                teardown(&mut live, session_id).await;
                return Err(error);
            }
        };

        self.sessions
            .lock()
            .await
            .insert(session_id.to_string(), live);
        Ok(content)
    }

    async fn read(&self, session_id: &str) -> Result<PageContent> {
        self.with_page(session_id, |page| async move {
            let json = Self::eval_with_script(&page, page_script::readable_expression()).await?;
            page_script::parse_readable(&json).ok_or_else(|| {
                AppError::Other("Could not extract readable content from the page".into())
            })
        })
        .await
    }

    async fn find(&self, session_id: &str, query: &str) -> Result<Vec<String>> {
        let query = query.to_string();
        self.with_page(session_id, |page| async move {
            let json =
                Self::eval_with_script(&page, page_script::find_expression(&query, 20)).await?;
            Ok(page_script::parse_find(&json))
        })
        .await
    }

    async fn snapshot(&self, session_id: &str) -> Result<String> {
        self.with_page(session_id, |page| async move {
            Self::eval_with_script(&page, page_script::snapshot_expression(100)).await
        })
        .await
    }

    async fn click(&self, session_id: &str, target: &str) -> Result<String> {
        let target = target.to_string();
        self.with_page(session_id, |page| async move {
            let element = Self::element(&page, &target).await?;
            timeout(element.click()).await?;
            Ok(format!("Clicked {target}."))
        })
        .await
    }

    async fn fill(&self, session_id: &str, target: &str, text: &str) -> Result<String> {
        let (target, text) = (target.to_string(), text.to_string());
        self.with_page(session_id, |page| async move {
            let element = Self::element(&page, &target).await?;
            timeout(element.click()).await?;
            timeout(element.type_str(&text)).await?;
            Ok(format!("Typed into {target}."))
        })
        .await
    }

    async fn press(&self, session_id: &str, key: &str) -> Result<String> {
        let key = key.to_string();
        self.with_page(session_id, |page| async move {
            // Keys go to whatever holds focus, which is what a user pressing a
            // key does; no element reference is involved.
            let element = timeout(page.find_element("body")).await?;
            timeout(element.press_key(&key)).await?;
            Ok(format!("Pressed {key}."))
        })
        .await
    }

    async fn screenshot(&self, session_id: &str, path: &Path) -> Result<String> {
        let path = path.to_path_buf();
        self.with_page(session_id, |page| async move {
            let bytes = timeout(page.screenshot(
                chromiumoxide::page::ScreenshotParams::builder().build(),
            ))
            .await?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    AppError::Other(format!("Could not create screenshot directory: {error}"))
                })?;
            }
            std::fs::write(&path, bytes).map_err(|error| {
                AppError::Other(format!("Could not write the screenshot: {error}"))
            })?;
            Ok(format!("Saved a screenshot to {}", path.display()))
        })
        .await
    }

    async fn close(&self, session_id: &str) -> Result<()> {
        // Safe to call twice: an already-closed session is a no-op, not an
        // error, so a cleanup path can always call it.
        let Some(mut live) = self.sessions.lock().await.remove(session_id) else {
            return Ok(());
        };
        teardown(&mut live, session_id).await;
        Ok(())
    }
}

/// Stop the browser and give back the profile.
///
/// Kill is explicit rather than left to `Drop`: the crate only sets
/// `kill_on_drop`, and the reaping time is not guaranteed — which is how a
/// browser survived a crashed owner here for five days.
async fn teardown(live: &mut LiveSession, session_id: &str) {
    let _ = tokio::time::timeout(OP_TIMEOUT, live.browser.close()).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), live.browser.kill()).await;
    live.handler.abort();
    release_profile(&live.profile_dir, session_id);
}

fn release_profile(dir: &Option<PathBuf>, session_id: &str) {
    if let Some(dir) = dir {
        profile::release_lock(dir, session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_refs_minted_by_our_own_snapshot_are_accepted() {
        assert_eq!(validated_ref("ref_12").unwrap(), "ref_12");

        // Everything else is refused before it can reach a selector: the model
        // never invents a ref, it echoes one back.
        for hostile in [
            "ref_1'] , button",
            "ref_",
            "ref_1a",
            "REF_1",
            "body",
            "*",
            "",
            "ref_1 ref_2",
        ] {
            assert!(
                validated_ref(hostile).is_err(),
                "{hostile:?} must be rejected"
            );
        }
    }

    #[tokio::test]
    async fn acting_on_a_session_that_was_never_opened_is_an_error_not_a_panic() {
        let driver = ChromiumDriver::new();
        assert!(driver.read("codefactory-nope").await.is_err());
        assert!(driver.snapshot("codefactory-nope").await.is_err());
        assert!(driver.click("codefactory-nope", "ref_1").await.is_err());
    }

    #[tokio::test]
    async fn closing_an_unknown_session_succeeds_so_cleanup_can_always_run() {
        let driver = ChromiumDriver::new();
        assert!(driver.close("codefactory-nope").await.is_ok());
        // Twice, too — teardown paths call close defensively.
        assert!(driver.close("codefactory-nope").await.is_ok());
    }
}
