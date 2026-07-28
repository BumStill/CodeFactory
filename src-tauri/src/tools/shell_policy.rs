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

fn is_direct_playwright_command(command: &str) -> bool {
    command.contains("playwright-cli")
        || command.contains("@playwright/cli")
        || command.contains("playwright-core/lib/entry/clidaemon")
        || command.contains("playwright_cli.sh")
}

pub fn audit_footer(cwd: &std::path::Path, exit_code: Option<i32>, risk: ShellRisk) -> String {
    let exit = exit_code
        .map(|code| code.to_string())
        .unwrap_or_else(|| "terminated".into());
    format!(
        "[shell-audit] cwd={} exit_code={} risk={}",
        cwd.display(),
        exit,
        risk.label()
    )
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
}
