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

/// How long the browser process gets to print its DevTools endpoint.
///
/// This has to be handed to chromiumoxide explicitly. Its own default is 20s,
/// which fired long before the watchdog below and left this budget as dead
/// code: a `windows-latest` runner whose virus scanner is reading a freshly
/// unpacked `chrome.exe` has been measured silent past 20s with the process
/// still alive and healthy — an empty `BrowserStderr` and a `LaunchTimeout`
/// rather than an exit. That turned a slow start into a hard failure in
/// roughly one CI run in three.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(60);
/// Backstop around the whole of `Browser::launch`. Resolving the endpoint is
/// only its first half — the websocket connect that follows has no timeout of
/// its own. Deliberately longer than `LAUNCH_TIMEOUT`, so a launch that merely
/// ran slow reports as a launch failure naming the binary rather than as this.
const LAUNCH_WATCHDOG: Duration = Duration::from_secs(75);
/// Any single page operation. Generous enough for a slow site, short enough
/// that a wedged page surfaces as an error instead of hanging the turn.
const OP_TIMEOUT: Duration = Duration::from_secs(30);

/// The marker every "the browser would not start" message carries.
///
/// Shared by [`launch_failure`] and [`is_launch_failure`] so the two cannot
/// drift apart: rewording the message without moving this constant is a
/// compile-time impossibility rather than a silently broken classifier.
const LAUNCH_FAILURE_MARKER: &str = "Could not start the browser";

/// The one place a failed launch becomes an error.
///
/// Naming the binary is the point. Which Chrome we resolved is the first thing
/// anyone needs when a launch fails, and it is the one fact the underlying
/// error never carries — its stderr is routinely empty when the process dies,
/// or just stays silent, during startup.
fn launch_failure(executable: &Path, detail: impl std::fmt::Display) -> AppError {
    AppError::Other(format!(
        "{LAUNCH_FAILURE_MARKER} at {}: {detail}",
        executable.display()
    ))
}

/// Whether a message came from [`launch_failure`] — i.e. this machine could not
/// produce a working browser at all.
///
/// The browser-session smoke uses this to tell an unusable runner apart from
/// the lifecycle regressions it actually asserts. Nothing else may be retried.
pub fn is_launch_failure(message: &str) -> bool {
    message.contains(LAUNCH_FAILURE_MARKER)
}

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

/// An explicitly configured binary, when it actually exists.
///
/// Separate from the env lookup so it can be tested without mutating process
/// state that other tests in the same binary would see.
fn override_executable(raw: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(raw?);
    // A stale or misspelled override must not shadow a working install: fall
    // through to detection rather than failing with "no such file".
    path.is_file().then_some(path)
}

/// Whether to launch without a window. Off unless explicitly asked for.
fn headless_requested() -> bool {
    matches!(
        std::env::var("CODEFACTORY_BROWSER_HEADLESS").as_deref(),
        Ok("1") | Ok("true")
    )
}

/// A Chrome the machine already has.
///
/// Chromium-family only. These all speak the DevTools protocol the driver is
/// built on; Firefox and Safari do not, so listing them here would produce a
/// launch that fails later instead of an honest "not found" now.
fn system_chrome() -> Option<PathBuf> {
    if let Some(path) = override_executable(std::env::var_os("CODEFACTORY_CHROME")) {
        return Some(path);
    }
    system_chrome_candidates().into_iter().find(|path| path.is_file())
}

#[cfg(target_os = "macos")]
fn system_chrome_candidates() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/Applications")];
    if let Some(home) = dirs::home_dir() {
        // A per-user install is just as valid, and common on managed Macs where
        // /Applications is not writable.
        roots.push(home.join("Applications"));
    }
    let apps = [
        ("Google Chrome.app", "Google Chrome"),
        ("Chromium.app", "Chromium"),
        ("Microsoft Edge.app", "Microsoft Edge"),
    ];
    roots
        .iter()
        .flat_map(|root| {
            apps.iter()
                .map(move |(bundle, binary)| root.join(bundle).join("Contents/MacOS").join(binary))
        })
        .collect()
}

#[cfg(windows)]
fn system_chrome_candidates() -> Vec<PathBuf> {
    // Chrome installs per-machine or per-user, and a 32-bit install on a 64-bit
    // box lands in a third place — so all three roots have to be checked.
    let roots = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"];
    let relative = [
        r"Google\Chrome\Application\chrome.exe",
        r"Chromium\Application\chrome.exe",
        r"Microsoft\Edge\Application\msedge.exe",
    ];
    roots
        .iter()
        .filter_map(|key| std::env::var_os(key))
        .flat_map(|root| {
            relative
                .iter()
                .map(move |tail| PathBuf::from(&root).join(tail))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn system_chrome_candidates() -> Vec<PathBuf> {
    let names = [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "microsoft-edge",
    ];
    // PATH first so a user's own build wins over a distro package, then the
    // usual install roots for the case where PATH is not inherited.
    let mut roots: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();
    roots.extend([PathBuf::from("/usr/bin"), PathBuf::from("/usr/local/bin"), PathBuf::from("/snap/bin")]);
    roots
        .iter()
        .flat_map(|root| names.iter().map(move |name| root.join(name)))
        .collect()
}

impl ChromiumDriver {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolve a browser to drive, or explain what to do about it.
    ///
    /// Order matters and is a product decision: the managed download wins when
    /// it is there, because the user opted into it deliberately and its version
    /// is known. But nobody should be forced through a 150 MB download to use
    /// browser control when Chrome is already sitting on the machine, so a
    /// system install is the fallback rather than a dead end. Either way the
    /// launch below pins an isolated `--user-data-dir`, so driving the user's
    /// own Chrome binary still never touches their signed-in profile — reading
    /// pages they are already signed into is what the extension path is for.
    fn executable() -> Result<PathBuf> {
        let platform = install::Platform::current();
        let state = platform.and_then(|platform| {
            let root = install::install_root()?;
            Some(install::detect(&root, platform))
        });
        if let Some(InstallState::Ready(found)) = state {
            return Ok(found.binary);
        }
        if let Some(binary) = system_chrome() {
            return Ok(binary);
        }

        // A half-written managed install with no Chrome to fall back on: repair
        // is the one useful instruction, so don't bury it under the generic one.
        if matches!(state, Some(InstallState::Missing { previous: Some(_) })) {
            return Err(AppError::Other(
                "The downloaded Chromium is incomplete, and no installed Chrome was found to \
                 use instead. Re-run the download from Settings to repair it."
                    .into(),
            ));
        }

        // Actionable rather than a bare failure: the caller turns this into the
        // download prompt instead of showing the user a dead end.
        if platform.is_none() {
            return Err(AppError::Other(
                "Browser control needs Chrome. No Chromium build is published for this \
                 platform, and no installed Chrome, Chromium or Edge was found."
                    .into(),
            ));
        }
        Err(AppError::Other(
            "Browser control needs Chrome. Install Chrome (or Chromium/Edge) and it will be \
             used automatically, or download the managed browser from Settings (about 150 MB, \
             one time)."
                .into(),
        ))
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

        let builder = BrowserConfig::builder()
            .chrome_executable(&executable)
            .user_data_dir(&user_data_dir)
            // Without this the crate's 20s default applies, not ours.
            .launch_timeout(LAUNCH_TIMEOUT);
        // Headed by default: the user has to be able to complete a sign-in,
        // including any 2FA, in this window. A headless browser cannot be
        // signed in to anything by a person. The override exists for automated
        // environments with no desktop session (CI smokes), where there is no
        // person to sign in either way.
        let builder = if headless_requested() {
            // Automated environments only. A CI account has no desktop session,
            // no GPU and a sandbox that Chrome cannot enter, and each of those
            // makes it exit before it ever prints its DevTools endpoint — which
            // surfaces as an empty stderr and a websocket timeout. None of this
            // is applied to the headed path a real user gets.
            builder.args(vec![
                "--no-sandbox",
                "--disable-gpu",
                "--disable-dev-shm-usage",
                "--no-first-run",
                "--no-default-browser-check",
            ])
        } else {
            builder.with_head()
        };
        let config = builder
            .build()
            .map_err(|error| AppError::Other(format!("Invalid browser configuration: {error}")))?;

        let launched = tokio::time::timeout(LAUNCH_WATCHDOG, Browser::launch(config)).await;
        let (mut browser, mut handler) = match launched {
            Ok(Ok(pair)) => pair,
            Ok(Err(error)) => {
                release_profile(&profile_dir, session_id);
                return Err(launch_failure(&executable, error));
            }
            Err(_) => {
                release_profile(&profile_dir, session_id);
                return Err(launch_failure(
                    &executable,
                    format!("it did not respond within {}s", LAUNCH_WATCHDOG.as_secs()),
                ));
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
    fn the_crate_default_launch_timeout_never_preempts_ours() {
        // The bug this pins: chromiumoxide's own 20s default fired first and
        // made LAUNCH_TIMEOUT unreachable, so a merely-slow Chrome on a cold CI
        // runner failed the browser smoke about one run in three. The watchdog
        // has to stay strictly the longer of the two, or a slow launch reports
        // as "did not respond" instead of naming the binary and its real cause.
        assert!(
            LAUNCH_TIMEOUT > Duration::from_secs(20),
            "our budget must exceed the crate default it now overrides"
        );
        assert!(
            LAUNCH_WATCHDOG > LAUNCH_TIMEOUT,
            "the watchdog must not preempt the launch budget it wraps"
        );
    }

    #[test]
    fn a_launch_failure_is_recognisable_to_the_smoke() {
        // Built exactly the way `open` builds it, so this cannot keep passing
        // while the real message drifts out from under `is_launch_failure`.
        let message = launch_failure(
            Path::new(r"C:\Program Files\Google\Chrome\Application\chrome.exe"),
            "Timeout while resolving websocket URL from browser process, \
             stderr: BrowserStderr(\"\")",
        )
        .to_string();
        assert!(is_launch_failure(&message), "message was: {message}");
        assert!(
            message.contains("chrome.exe"),
            "a launch failure has to name the binary it tried: {message}"
        );
    }

    #[test]
    fn nothing_the_smoke_asserts_looks_like_a_launch_failure() {
        // The retry added for slow CI runners must never re-roll one of these.
        // Each is a real regression in code we own, and gets exactly one shot.
        for message in [
            "smoke did not receive a session id",
            "Could not extract readable content from the page",
            "Browser session codefactory-1 is already open.",
            "browser_session session is unknown or has already been reclaimed",
            "That browser profile is already in use by session codefactory-1.",
        ] {
            assert!(
                !is_launch_failure(message),
                "{message} must fail loudly, not be retried"
            );
        }
    }

    #[test]
    fn an_override_only_counts_when_the_binary_is_really_there() {
        let real = std::env::current_exe().expect("test binary exists");
        assert_eq!(
            override_executable(Some(real.clone().into_os_string())),
            Some(real)
        );

        // A stale override must fall through to detection instead of becoming a
        // launch that fails with "no such file" later.
        assert_eq!(
            override_executable(Some("/nope/not-a-chrome".into())),
            None,
            "a path that does not exist must not shadow a working install"
        );
        assert_eq!(override_executable(None), None);

        // A directory is not a binary — `exists()` would wrongly accept it.
        let dir = std::env::temp_dir();
        assert_eq!(override_executable(Some(dir.into_os_string())), None);
    }

    #[test]
    fn the_candidate_list_names_only_browsers_that_speak_devtools() {
        let candidates = system_chrome_candidates();
        assert!(
            !candidates.is_empty(),
            "every supported platform must look somewhere"
        );

        // Firefox and Safari cannot be driven by this driver at all, so finding
        // one would produce a launch that fails instead of an honest "not
        // found" — they must never be candidates.
        for path in &candidates {
            let name = path.to_string_lossy().to_lowercase();
            assert!(
                !name.contains("firefox") && !name.contains("safari"),
                "non-DevTools browser in the candidate list: {}",
                path.display()
            );
            assert!(
                name.contains("chrome") || name.contains("chromium") || name.contains("edge"),
                "unexpected candidate: {}",
                path.display()
            );
        }
    }

    #[test]
    fn headless_is_off_unless_explicitly_asked_for() {
        // The default has to stay headed: a user completing a 2FA sign-in in an
        // invisible window is the one failure they cannot diagnose.
        assert!(
            !headless_requested(),
            "tests run without the override, so this must be false"
        );
    }

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
