// SPDX-License-Identifier: Apache-2.0
//! What a browser action is allowed to do without asking.
//!
//! A browser driving a persistent profile acts *as the signed-in user*. That
//! makes it a different risk class from the file and shell tools, which are
//! bounded by the project directory: here the blast radius is every account the
//! user has ever logged into in that profile. So this classifier runs ahead of
//! the generic allow/ask/deny policy — including ahead of `full_access` — the
//! same way `shell_policy` does for `bash`.
//!
//! Three rules:
//!
//!   1. **Non-web schemes are denied outright.** `file://` through a browser
//!      would read arbitrary local files, quietly bypassing the path sandbox
//!      that `read`/`write` enforce; `chrome://` reaches browser internals.
//!      Neither has a legitimate use here, so neither is a prompt — it's a no.
//!   2. **Reading a signed-in site asks once per host.** The user should know
//!      which of their accounts the agent is about to look at, but re-asking
//!      for every page of the same site is noise that trains them to click
//!      through. Ask on the first page of a host, remember it for the session.
//!   3. **Acting always asks.** Clicking and typing as the signed-in user can
//!      send, post, buy, or delete. There is no reliable way to tell an inert
//!      button from a destructive one from a DOM reference, so the honest
//!      default is to confirm every time rather than guess.
//!
//! An ephemeral profile carries no identity, so plain public-web browsing in a
//! throwaway session doesn't prompt at all.

use super::profile::ProfileScope;

/// What the agent is trying to do with a browser session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserAction {
    /// Navigate, extract text, search within the page, screenshot. No side effects.
    Read,
    /// Click, fill, press. Acts with the profile's identity.
    Act,
    /// Open/close a session. Gated by the navigation that follows, not itself.
    Lifecycle,
}

impl BrowserAction {
    pub fn from_tool_action(action: &str) -> Self {
        match action {
            "click" | "fill" | "press" => Self::Act,
            "attach" | "close" => Self::Lifecycle,
            _ => Self::Read,
        }
    }
}

/// The gate's verdict for one browser call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserPermission {
    Allow,
    /// Prompt the user. `subject` is what the prompt should name.
    Ask {
        subject: String,
    },
    Deny {
        reason: String,
    },
}

/// Hosts already confirmed for this chat session, so repeat reads don't re-ask.
pub type GrantedHosts = std::collections::BTreeSet<String>;

/// Classify one browser call.
///
/// `url` is the page being navigated to, or `None` for actions on the page
/// that's already open.
pub fn classify(
    action: BrowserAction,
    url: Option<&str>,
    scope: &ProfileScope,
    granted: &GrantedHosts,
) -> BrowserPermission {
    if let Some(url) = url {
        match scheme_of(url).as_deref() {
            Some("http") | Some("https") => {}
            Some(other) => {
                return BrowserPermission::Deny {
                    reason: format!(
                        "browser sessions only open http(s) pages; '{other}:' is not allowed \
                         (local files must go through the file tools, which stay inside the project)"
                    ),
                }
            }
            None => {
                return BrowserPermission::Deny {
                    reason: "browser sessions need an absolute http(s) URL".into(),
                }
            }
        }
    }

    match action {
        BrowserAction::Lifecycle => BrowserPermission::Allow,
        BrowserAction::Act => BrowserPermission::Ask {
            subject: match url.and_then(host_of) {
                Some(host) => format!("act as your signed-in account on {host}"),
                None => "act as your signed-in account on the open page".into(),
            },
        },
        // A throwaway profile is signed in to nothing, so reading it is no more
        // sensitive than fetching a public page. Acting still asks above.
        BrowserAction::Read if !scope.is_persistent() => BrowserPermission::Allow,
        BrowserAction::Read => match url.and_then(host_of) {
            Some(host) if granted.contains(&host) => BrowserPermission::Allow,
            Some(host) => BrowserPermission::Ask {
                subject: format!("read {host} using your signed-in session"),
            },
            // Reading the already-open page: the host was cleared on navigation.
            None => BrowserPermission::Allow,
        },
    }
}

/// Host that a granted read applies to, so callers can record the approval.
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = authority.split(':').next()?.trim().to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

fn scheme_of(url: &str) -> Option<String> {
    let (scheme, _) = url.split_once("://")?;
    let scheme = scheme.trim().to_ascii_lowercase();
    (!scheme.is_empty()
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-'))
    .then_some(scheme)
}

/// Build the permission key for a browser call from the tool's arguments.
///
/// The key is what the user sees in the prompt and what they can allow-list, so
/// it is `<action>` or `<action>:<host>` — readable, and stable enough that
/// "always allow reading github.com" keeps working across pages.
pub fn permission_cmd(args: &serde_json::Value) -> Option<String> {
    let action = args.get("action")?.as_str()?;
    match args.get("url").and_then(|url| url.as_str()).and_then(host_of) {
        Some(host) => Some(format!("{action}:{host}")),
        None => Some(action.to_string()),
    }
}

/// Classify a call from the key [`permission_cmd`] produced.
///
/// The gate that calls this is a pure function with no database access, so it
/// cannot tell a signed-in profile from a throwaway one. It therefore judges
/// every call by the stricter, signed-in rules: the cost is an extra prompt when
/// browsing a throwaway profile, and the alternative — assuming throwaway —
/// would skip prompts on the profile that holds the user's accounts.
pub fn classify_cmd(cmd: &str) -> BrowserPermission {
    let (action, host) = match cmd.split_once(':') {
        Some((action, host)) => (action, Some(host)),
        None => (cmd, None),
    };
    // Reconstruct a URL only to reuse one scheme/host rule for both paths.
    let url = host.map(|host| format!("https://{host}/"));
    let mut verdict = classify(
        BrowserAction::from_tool_action(action),
        url.as_deref(),
        &ProfileScope::Persistent {
            name: "unknown".into(),
        },
        &GrantedHosts::new(),
    );
    // The key carries a host, not a scheme, so a non-web scheme has to be
    // caught where the raw URL is still available — in the tool. Reading the
    // reconstructed https URL back would always look fine.
    if let BrowserPermission::Ask { subject } = &verdict {
        if host.is_none() {
            verdict = BrowserPermission::Ask {
                subject: subject.clone(),
            };
        }
    }
    verdict
}

#[cfg(test)]
mod tests {
    use super::*;

    fn persistent() -> ProfileScope {
        ProfileScope::Persistent {
            name: "default".into(),
        }
    }

    #[test]
    fn file_urls_are_denied_not_merely_prompted() {
        // Reading local files through the browser would bypass the project
        // sandbox the file tools enforce — that's a no, not a confirmation.
        for url in ["file:///etc/passwd", "chrome://settings", "devtools://x"] {
            let verdict = classify(
                BrowserAction::Read,
                Some(url),
                &persistent(),
                &GrantedHosts::new(),
            );
            assert!(
                matches!(verdict, BrowserPermission::Deny { .. }),
                "{url} should be denied, got {verdict:?}"
            );
        }
    }

    #[test]
    fn a_relative_or_schemeless_url_is_denied() {
        let verdict = classify(
            BrowserAction::Read,
            Some("example.com/news"),
            &persistent(),
            &GrantedHosts::new(),
        );
        assert!(matches!(verdict, BrowserPermission::Deny { .. }));
    }

    #[test]
    fn reading_a_signed_in_host_asks_the_first_time_only() {
        let mut granted = GrantedHosts::new();
        let first = classify(
            BrowserAction::Read,
            Some("https://mail.example.com/inbox"),
            &persistent(),
            &granted,
        );
        assert!(matches!(first, BrowserPermission::Ask { .. }));

        granted.insert("mail.example.com".into());
        let second = classify(
            BrowserAction::Read,
            Some("https://mail.example.com/thread/2"),
            &persistent(),
            &granted,
        );
        assert_eq!(second, BrowserPermission::Allow);
    }

    #[test]
    fn approving_one_host_does_not_approve_another() {
        let mut granted = GrantedHosts::new();
        granted.insert("mail.example.com".into());
        let verdict = classify(
            BrowserAction::Read,
            Some("https://bank.example.com/"),
            &persistent(),
            &granted,
        );
        assert!(matches!(verdict, BrowserPermission::Ask { .. }));
    }

    #[test]
    fn acting_as_the_user_asks_every_time_even_on_a_granted_host() {
        // Reading a site is not consent to post, send, or buy on it — and a DOM
        // reference doesn't tell us which button is which.
        let mut granted = GrantedHosts::new();
        granted.insert("mail.example.com".into());
        let verdict = classify(BrowserAction::Act, None, &persistent(), &granted);
        assert!(matches!(verdict, BrowserPermission::Ask { .. }));
    }

    #[test]
    fn a_throwaway_profile_browses_the_public_web_without_prompting() {
        let verdict = classify(
            BrowserAction::Read,
            Some("https://example.com/"),
            &ProfileScope::Ephemeral,
            &GrantedHosts::new(),
        );
        assert_eq!(verdict, BrowserPermission::Allow);

        // …but the scheme rule still applies, identity or not.
        let denied = classify(
            BrowserAction::Read,
            Some("file:///etc/passwd"),
            &ProfileScope::Ephemeral,
            &GrantedHosts::new(),
        );
        assert!(matches!(denied, BrowserPermission::Deny { .. }));

        assert!(matches!(
            classify(
                BrowserAction::Act,
                None,
                &ProfileScope::Ephemeral,
                &GrantedHosts::new(),
            ),
            BrowserPermission::Ask { .. }
        ));
    }

    #[test]
    fn tool_actions_map_to_the_right_risk_class() {
        for action in ["click", "fill", "press"] {
            assert_eq!(
                BrowserAction::from_tool_action(action),
                BrowserAction::Act,
                "{action} acts as the user"
            );
        }
        for action in ["open", "read", "find", "snapshot", "screenshot"] {
            assert_eq!(BrowserAction::from_tool_action(action), BrowserAction::Read);
        }
        for action in ["attach", "close"] {
            assert_eq!(
                BrowserAction::from_tool_action(action),
                BrowserAction::Lifecycle
            );
        }
    }

    #[test]
    fn the_permission_key_is_readable_and_host_scoped() {
        // What the user sees in the prompt, and what they can allow-list.
        let args = serde_json::json!({"action": "open", "url": "https://Mail.Example.com:443/x"});
        assert_eq!(permission_cmd(&args).as_deref(), Some("open:mail.example.com"));

        // Actions on an already-open page carry no host.
        let args = serde_json::json!({"action": "read", "session_id": "codefactory-1"});
        assert_eq!(permission_cmd(&args).as_deref(), Some("read"));

        assert_eq!(permission_cmd(&serde_json::json!({})), None);
    }

    #[test]
    fn the_gate_judges_every_call_by_the_signed_in_rules() {
        // The gate cannot see which profile is in play, so reading asks and
        // acting asks — erring toward a prompt rather than toward silence.
        assert!(matches!(
            classify_cmd("open:mail.example.com"),
            BrowserPermission::Ask { .. }
        ));
        assert!(matches!(classify_cmd("click"), BrowserPermission::Ask { .. }));
        // Closing a session is not an action on a site.
        assert_eq!(classify_cmd("close"), BrowserPermission::Allow);
    }

    #[test]
    fn host_extraction_ignores_port_credentials_and_case() {
        assert_eq!(
            host_of("https://User:pw@Mail.Example.com:8443/x?y#z").as_deref(),
            Some("mail.example.com")
        );
        assert_eq!(
            host_of("https://example.com").as_deref(),
            Some("example.com")
        );
        assert_eq!(host_of("not-a-url"), None);
    }
}
