// SPDX-License-Identifier: Apache-2.0

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellCommandPolicy {
    Allow { risk: ShellRisk },
    Ask { risk: ShellRisk, reason: &'static str },
    Deny { reason: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellRisk {
    Low,
    High,
}

impl ShellRisk {
    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::High => "high",
        }
    }
}

impl ShellCommandPolicy {
    pub fn risk(&self) -> ShellRisk {
        match self {
            Self::Allow { risk } | Self::Ask { risk, .. } => *risk,
            Self::Deny { .. } => ShellRisk::High,
        }
    }
}

pub fn classify_command(command: &str) -> ShellCommandPolicy {
    let normalized = normalize(command);

    if is_direct_playwright_command(&normalized) {
        return deny("direct Playwright CLI is not allowed; use the native browser_session tool so CodeFactory can own and clean up the browser process");
    }

    if is_unbounded_github_polling(&normalized) {
        return deny(
            "continuous GitHub polling is not allowed: `--watch` refreshes every 10s until CI ends, \
             so one command can spend hundreds of API requests and exhaust the shared quota \
             (which then breaks releases). Wait for CI through `deliver_changes` with \
             ceiling=through_ci_green — it backs off 10s→60s — or poll once per call, or loop with \
             `sleep 60` or longer",
        );
    }

    if let Some(reason) = contains_any(
        &normalized,
        &[
            ("rm -rf /", "matches permanent deny 'rm -rf /'"),
            ("format ", "matches permanent deny 'format'"),
            ("del /f /s /q c:\\", "matches permanent deny 'del /f /s /q c:\\'"),
            ("rd /s /q c:\\", "matches permanent deny 'rd /s /q c:\\'"),
            ("reg delete hklm", "matches permanent deny 'reg delete hklm'"),
            ("shutdown", "matches permanent deny 'shutdown'"),
        ],
    ) {
        return deny(reason);
    }

    if normalized.contains("remove-item")
        && (normalized.contains("-recurse") || normalized.contains("-force"))
    {
        return ask("high risk shell command: recursive or forced remove-item");
    }

    if let Some(reason) = contains_any(
        &normalized,
        &[
            ("git reset --hard", "high risk shell command: git reset --hard"),
            ("git clean -", "high risk shell command: git clean"),
            ("rm -rf", "high risk shell command: recursive rm"),
            ("rd /s", "high risk shell command: recursive rd"),
            ("rmdir /s", "high risk shell command: recursive rmdir"),
            ("del /s", "high risk shell command: recursive del"),
            ("reg delete", "high risk shell command: registry delete"),
            ("set-executionpolicy", "high risk shell command: execution policy change"),
            ("invoke-expression", "high risk shell command: invoke-expression"),
            ("| iex", "high risk shell command: pipe to iex"),
            ("|iex", "high risk shell command: pipe to iex"),
            ("diskpart", "high risk shell command: diskpart"),
            ("bcdedit", "high risk shell command: boot configuration edit"),
        ],
    ) {
        return ask(reason);
    }

    ShellCommandPolicy::Allow {
        risk: ShellRisk::Low,
    }
}

/// Does this command poll GitHub faster than roughly once a minute, without
/// bound?
///
/// Two shapes, both measured in the 2026-08-03/04 trajectory data:
/// - `gh … --watch` / `gh run watch`: self-refreshing every 10s until the run
///   ends. Recorded as ONE tool call, worth 60–96 API requests.
/// - a shell loop around `gh` with a short `sleep`.
///
/// The cadence is what is banned, not the intent: a loop that sleeps ≥60s is
/// allowed, because a gate with no sanctioned alternative just pushes the model
/// onto the next bypass.
fn is_unbounded_github_polling(command: &str) -> bool {
    let touches_gh = command.contains("gh ");
    if !touches_gh {
        return false;
    }
    if command.contains("--watch") || command.contains("gh run watch") {
        return true;
    }
    let loops = command.contains("until ") || command.contains("while ");
    if !loops {
        return false;
    }
    // A loop with no sleep at all is the worst case; with a sleep, judge it.
    match sleep_seconds(command) {
        Some(seconds) => seconds < 60,
        None => true,
    }
}

/// The first `sleep <n>` in the command, in seconds.
fn sleep_seconds(command: &str) -> Option<u32> {
    let rest = command.split("sleep ").nth(1)?;
    let digits: String = rest
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn is_direct_playwright_command(command: &str) -> bool {
    command.contains("playwright-cli")
        || command.contains("@playwright/cli")
        || command.contains("playwright-core/lib/entry/clidaemon")
        || command.contains("playwright_cli.sh")
}

pub fn audit_footer(cwd: &std::path::Path, exit_code: Option<i32>, risk: ShellRisk) -> String {
    audit_footer_with_quota(cwd, exit_code, risk, None)
}

/// Same footer, plus the GitHub quota line when this command touched `gh`.
///
/// Callers were flying blind: the first sign of exhaustion was a 403, by which
/// point an unrelated release had already failed on a throttled query
/// (2026-08-04). Reading `/rate_limit` is free — GitHub does not count that
/// endpoint against the limit — so the number can simply be shown.
pub fn audit_footer_with_quota(
    cwd: &std::path::Path,
    exit_code: Option<i32>,
    risk: ShellRisk,
    quota: Option<GithubQuota>,
) -> String {
    let exit = exit_code
        .map(|code| code.to_string())
        .unwrap_or_else(|| "terminated".into());
    let mut footer = format!(
        "[shell-audit] cwd={} exit_code={} risk={}",
        cwd.display(),
        exit,
        risk.label()
    );
    if let Some(quota) = quota {
        footer.push_str(&format!(
            "\n[github-quota] {}/{} remaining{}",
            quota.remaining,
            quota.limit,
            if quota.is_low() {
                " — running low; prefer deliver_changes over repeated gh calls"
            } else {
                ""
            }
        ));
    }
    footer
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GithubQuota {
    pub remaining: u32,
    pub limit: u32,
}

impl GithubQuota {
    /// Below 10% is the point where a delivery run (~13 polls) plus its release
    /// verification can still finish, but a second one probably cannot.
    pub fn is_low(self) -> bool {
        self.limit > 0 && self.remaining * 10 < self.limit
    }
}

/// Does this command warrant showing the quota at all?
pub fn touches_github_api(command: &str) -> bool {
    let normalized = normalize(command);
    normalized.contains("gh ") || normalized.contains("api.github.com")
}

fn normalize(command: &str) -> String {
    command.to_lowercase().replace('`', "")
}

fn contains_any<'a>(command: &str, needles: &[(&str, &'a str)]) -> Option<&'a str> {
    needles
        .iter()
        .find_map(|(needle, reason)| command.contains(needle).then_some(*reason))
}

fn ask(reason: &'static str) -> ShellCommandPolicy {
    ShellCommandPolicy::Ask {
        risk: ShellRisk::High,
        reason,
    }
}

fn deny(reason: &'static str) -> ShellCommandPolicy {
    ShellCommandPolicy::Deny { reason }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_recursive_remove_item_as_high_risk_ask() {
        assert_eq!(
            classify_command("Remove-Item -Recurse -Force .\\dist"),
            ShellCommandPolicy::Ask {
                risk: ShellRisk::High,
                reason: "high risk shell command: recursive or forced remove-item",
            }
        );
    }

    #[test]
    fn classifies_shutdown_as_permanent_deny() {
        assert_eq!(
            classify_command("shutdown /s /t 0"),
            ShellCommandPolicy::Deny {
                reason: "matches permanent deny 'shutdown'",
            }
        );
    }

    #[test]
    fn rejects_direct_playwright_cli_in_favor_of_managed_sessions() {
        assert!(matches!(
            classify_command("npx --package @playwright/cli playwright-cli open https://example.com"),
            ShellCommandPolicy::Deny { .. }
        ));
    }

    // 2026-08-03/04: CodeFactory burned its GitHub quota into 403s and the
    // rate limiting then broke an unrelated release (auto-release read a
    // throttled query as "no published release exists"). Trajectory data named
    // the amplifier precisely: `gh ... --watch` prints "Refreshing checks
    // status every 10 seconds" and keeps requesting until CI ends — one 10-16
    // minute CI is 60-96 requests from a single command, recorded as ONE tool
    // call. Thirteen of them in a day is ~1000 requests that nothing accounted
    // for. The managed path (`deliver_changes`) already backs off 10→20→40→60s;
    // only this bypass was unbounded.

    #[test]
    fn watch_style_github_polling_is_denied_with_the_managed_alternative() {
        for command in [
            "gh pr checks 291 --watch",
            "gh pr checks --watch --fail-fast",
            "gh run watch 30873050921",
            "GH_PAGER=cat gh run watch 123",
        ] {
            let policy = classify_command(command);
            let ShellCommandPolicy::Deny { reason } = policy else {
                panic!("{command:?} must be denied, got {policy:?}");
            };
            assert!(
                reason.contains("deliver_changes"),
                "the denial must name the managed alternative: {reason}"
            );
        }
    }

    #[test]
    fn tight_polling_loops_around_gh_are_denied_but_patient_ones_are_not() {
        for command in [
            "until gh pr view 291 --json state; do sleep 30; done",
            "while ! gh pr checks 291; do sleep 45; done",
        ] {
            assert!(
                matches!(classify_command(command), ShellCommandPolicy::Deny { .. }),
                "{command:?} polls faster than once a minute and must be denied"
            );
        }
        // A patient loop is the sanctioned escape hatch — deny the cadence, not
        // the intent, or the model just finds another bypass.
        assert!(matches!(
            classify_command("until gh pr view 291 --json state; do sleep 180; done"),
            ShellCommandPolicy::Allow { .. }
        ));
    }

    #[test]
    fn ordinary_github_and_unrelated_commands_stay_allowed() {
        for command in [
            "gh pr view 291 --json state",
            "gh pr list --limit 10",
            "gh release view v1.77.4",
            "gh api rate_limit",
            // `watch` in another context must not trip the rule.
            "cargo test watch_mode",
            "npm run watch",
            "until cargo build; do sleep 5; done",
        ] {
            assert!(
                matches!(classify_command(command), ShellCommandPolicy::Allow { .. }),
                "{command:?} must stay allowed"
            );
        }
    }

    #[test]
    fn the_quota_line_appears_only_for_github_commands_and_warns_when_low() {
        let cwd = std::path::Path::new("/w");
        // Unrelated command: unchanged footer, no noise.
        let plain = audit_footer(cwd, Some(0), ShellRisk::Low);
        assert!(!plain.contains("github-quota"));

        let healthy = audit_footer_with_quota(
            cwd,
            Some(0),
            ShellRisk::Low,
            Some(GithubQuota { remaining: 4800, limit: 5000 }),
        );
        assert!(healthy.contains("4800/5000 remaining"));
        assert!(!healthy.contains("running low"), "no false alarm: {healthy}");

        let low = audit_footer_with_quota(
            cwd,
            Some(0),
            ShellRisk::Low,
            Some(GithubQuota { remaining: 400, limit: 5000 }),
        );
        assert!(low.contains("running low"));
        assert!(low.contains("deliver_changes"), "say what to do instead");

        assert!(touches_github_api("gh pr view 1"));
        assert!(touches_github_api("curl https://api.github.com/rate_limit"));
        assert!(!touches_github_api("cargo test"));
    }
}
