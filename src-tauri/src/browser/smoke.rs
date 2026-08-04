// SPDX-License-Identifier: Apache-2.0
//! Verdict and retry rules for the browser-session lifecycle smoke.
//!
//! Split out from the CLI entry point in `lib.rs`, which is `#[cfg(not(test))]`
//! and therefore cannot be unit-tested at all. That mattered once the smoke
//! gained a retry: a retry is exactly the kind of change that quietly starts
//! swallowing the regression it was meant to leave alone. The rule for what
//! earns another attempt, and the rule for what "passed" means, both live here
//! where a test can hold them to it.
//!
//! What this smoke guards is not incidental. A session whose owner died once
//! left a headless Chrome running at full CPU for five days, so the assertions
//! below — a snapshot came back, a logical CLI failure was *detected*, and the
//! lease was reclaimed afterwards — are a leak defence. None of them may be
//! softened to make CI quieter.

use std::time::Duration;

use serde_json::{json, Value};

/// How many times the smoke will try to get a browser open before giving up.
///
/// Three attempts, not "until it works": a runner that genuinely cannot start
/// Chrome still has to fail the job, in bounded time.
pub const MAX_LAUNCH_ATTEMPTS: u32 = 3;

/// Whether a failed `open` earns another attempt.
///
/// Only the environment failing to produce a browser at all is retried, and
/// only while attempts remain. Everything this smoke asserts is a regression in
/// code we own and must go red on its first observation — see the tests, which
/// exist specifically to stop a future edit from widening this predicate.
pub fn should_retry_open(message: &str, attempt: u32) -> bool {
    attempt < MAX_LAUNCH_ATTEMPTS && super::chromium::is_launch_failure(message)
}

/// How long to wait before another launch attempt.
///
/// Linear and short. The failure being retried is a runner momentarily too
/// busy to start a process; the point is to not immediately hand it the same
/// work again, not to wait out anything in particular.
pub fn retry_backoff(attempt: u32) -> Duration {
    Duration::from_secs(2 * u64::from(attempt))
}

/// The smoke's verdict.
///
/// `status` is *derived* from the three observations rather than passed in, so
/// there is no way to write a receipt claiming "passed" beside a failed
/// assertion. CI reads all four fields; they are the contract.
pub fn receipt(
    session_id: &str,
    snapshot_ok: bool,
    failure_detected: bool,
    lease_reclaimed: bool,
    launch_attempts: u32,
) -> Value {
    json!({
        "status": if snapshot_ok && failure_detected && lease_reclaimed {
            "passed"
        } else {
            "failed"
        },
        "native_tool": "browser_session",
        "opened_session": session_id,
        "snapshot_ok": snapshot_ok,
        "failure_detected": failure_detected,
        "lease_reclaimed_after_failure": lease_reclaimed,
        "launch_attempts": launch_attempts,
    })
}

/// A receipt for a smoke that never got far enough to assert anything.
///
/// The old shape wrote a receipt only on the happy path, so a red CI step left
/// behind nothing but an exit code — which is how "cargo run exited 1" stayed
/// unexplained across repeated intermittent failures. Every exit writes one
/// now, and `launch_attempts` says whether the retry was even reached, so the
/// next occurrence arrives as data rather than as another investigation.
pub fn failure_receipt(stage: &str, error: &str, launch_attempts: u32) -> Value {
    json!({
        "status": "failed",
        "native_tool": "browser_session",
        "failed_stage": stage,
        "error": error,
        // Reported as not-observed rather than omitted: CI asserts on each of
        // these, and a missing field would read as an absent gate rather than
        // as a smoke that never got far enough to look.
        "snapshot_ok": false,
        "failure_detected": false,
        "lease_reclaimed_after_failure": false,
        "launch_attempts": launch_attempts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The message shape a runner that cannot start Chrome actually produces.
    fn launch_failure_message() -> String {
        format!(
            "Could not start the browser at C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe: \
             Timeout while resolving websocket URL from browser process, stderr: BrowserStderr(\"\")"
        )
    }

    #[test]
    fn a_slow_runner_gets_another_attempt_but_not_forever() {
        let message = launch_failure_message();
        assert!(should_retry_open(&message, 1));
        assert!(should_retry_open(&message, 2));
        assert!(
            !should_retry_open(&message, MAX_LAUNCH_ATTEMPTS),
            "an unusable runner still has to fail the job"
        );
    }

    #[test]
    fn a_real_lifecycle_failure_is_never_retried() {
        // The boundary the retry must not cross. These are the states the smoke
        // exists to catch; re-rolling one until it passes would turn the leak
        // defence into a coin flip.
        for message in [
            "browser_session session is unknown or has already been reclaimed",
            "Could not extract readable content from the page",
            "smoke did not receive a session id",
        ] {
            assert!(
                !should_retry_open(message, 1),
                "{message} must fail on first observation"
            );
        }
    }

    #[test]
    fn every_failed_assertion_produces_a_red_receipt() {
        // This is the case that proves the retry did not buy CI stability by
        // going quiet: each of the three observations, on its own, must sink
        // the verdict. CI's first check is `status -ne "passed"`.
        let cases = [
            (false, true, true, "a snapshot that never came back"),
            (false, false, true, "a CLI failure that went undetected"),
            (true, true, false, "a lease that outlived its failure"),
            (false, false, false, "nothing working at all"),
        ];
        for (snapshot_ok, failure_detected, lease_reclaimed, what) in cases {
            let receipt = receipt("codefactory-1", snapshot_ok, failure_detected, lease_reclaimed, 1);
            assert_eq!(
                receipt["status"], "failed",
                "{what} must not report as passed"
            );
        }
    }

    #[test]
    fn a_clean_run_passes_and_reports_its_attempts() {
        let receipt = receipt("codefactory-1", true, true, true, 2);
        assert_eq!(receipt["status"], "passed");
        assert_eq!(receipt["opened_session"], "codefactory-1");
        assert_eq!(
            receipt["launch_attempts"], 2,
            "a run that needed a retry has to say so, or the flake stays invisible"
        );
    }

    #[test]
    fn an_early_failure_still_fails_every_gate_ci_checks() {
        let receipt = failure_receipt("open", &launch_failure_message(), MAX_LAUNCH_ATTEMPTS);
        assert_eq!(receipt["status"], "failed");
        assert_eq!(receipt["snapshot_ok"], false);
        assert_eq!(receipt["failure_detected"], false);
        assert_eq!(receipt["lease_reclaimed_after_failure"], false);
        assert_eq!(receipt["failed_stage"], "open");
        assert!(
            receipt["error"]
                .as_str()
                .is_some_and(|error| error.contains("chrome.exe")),
            "the receipt has to carry why, not just that"
        );
    }
}
