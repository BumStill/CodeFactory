// SPDX-License-Identifier: Apache-2.0
//! Browser profiles — where a session's cookies and logins live.
//!
//! The whole point of the feature is that the user signs in to a site once and
//! the agent can read it afterwards, so a profile is a *persistent* Chromium
//! user-data directory that outlives the chat session. That makes it the most
//! sensitive thing this subsystem owns: whoever can drive a persistent profile
//! is, as far as every site is concerned, the user.
//!
//! Two rules follow, and both are enforced here rather than left to callers:
//!
//!   1. **Anonymous chats never touch a persistent profile.** An anonymous
//!      session promises to leave no trace; handing it the user's logged-in
//!      cookie jar would break that promise in the most damaging direction —
//!      it would let a no-trace conversation read the user's real accounts.
//!      [`ProfileScope::for_session`] resolves anonymous to ephemeral, and
//!      there is no argument that overrides it.
//!   2. **One live browser per profile directory.** Chromium takes an
//!      exclusive lock on its user-data dir; a second launch against the same
//!      directory either fails obscurely or corrupts the profile. We take our
//!      own lock first so the failure is a clear message instead.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A lock older than this is treated as abandoned by a crashed process. Matches
/// the browser-session lease TTL so the two reclaim on the same clock.
const LOCK_TTL: Duration = Duration::from_secs(20 * 60);

/// The kind of chat a browser session is being opened for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Project,
    Standalone,
    /// No-trace chat: never persisted, never listed.
    Anonymous,
}

impl SessionKind {
    /// Parse the `kind` column the sessions table stores. Unknown values are
    /// treated as project sessions, matching the rest of the app's default.
    pub fn from_db(kind: Option<&str>) -> Self {
        match kind {
            Some("anonymous") => Self::Anonymous,
            Some("quick") => Self::Standalone,
            _ => Self::Project,
        }
    }
}

/// Where a browser session keeps its cookies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileScope {
    /// A named, on-disk profile. Logins survive across chats and restarts.
    Persistent { name: String },
    /// A throwaway directory, discarded when the session closes. No logins.
    Ephemeral,
}

impl ProfileScope {
    /// Resolve the scope a session is allowed to use.
    ///
    /// Anonymous chats are forced to [`ProfileScope::Ephemeral`] regardless of
    /// what was requested — see the module docs.
    pub fn for_session(kind: SessionKind, requested: Option<&str>) -> Self {
        if kind == SessionKind::Anonymous {
            return Self::Ephemeral;
        }
        match requested.map(str::trim).filter(|name| !name.is_empty()) {
            Some(name) => Self::Persistent {
                name: sanitize_name(name),
            },
            None => Self::Persistent {
                name: "default".into(),
            },
        }
    }

    pub fn is_persistent(&self) -> bool {
        matches!(self, Self::Persistent { .. })
    }
}

/// Keep profile names to a single safe path segment — they reach the filesystem
/// and are partly model-chosen, so `../` and separators must not survive.
fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('-').to_ascii_lowercase();
    if cleaned.is_empty() {
        "default".into()
    } else {
        cleaned.chars().take(64).collect()
    }
}

/// Root for all persistent profiles: `~/.codefactory/browser/profiles`.
pub fn profiles_root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".codefactory").join("browser").join("profiles"))
}

/// Directory backing a persistent profile. `None` for ephemeral scopes (the
/// driver allocates a temp dir instead) or when the home dir is unresolvable.
pub fn profile_dir(scope: &ProfileScope) -> Option<PathBuf> {
    match scope {
        ProfileScope::Ephemeral => None,
        ProfileScope::Persistent { name } => Some(profiles_root()?.join(name)),
    }
}

fn lock_path(dir: &Path) -> PathBuf {
    dir.join(".codefactory-lock")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Outcome of trying to claim a profile for a new browser session.
#[derive(Debug, PartialEq, Eq)]
pub enum LockOutcome {
    Acquired,
    /// Another live session already holds this profile.
    Busy { holder: String },
}

/// Claim a persistent profile for `session_id`.
///
/// Re-claiming a profile whose lock has gone stale (owner crashed without
/// releasing) succeeds: an abandoned lock must not strand the profile forever.
pub fn acquire_lock(dir: &Path, session_id: &str) -> std::io::Result<LockOutcome> {
    std::fs::create_dir_all(dir)?;
    let path = lock_path(dir);
    if let Ok(raw) = std::fs::read_to_string(&path) {
        if let Some((holder, stamp)) = parse_lock(&raw) {
            let age = now_secs().saturating_sub(stamp);
            if holder != session_id && age < LOCK_TTL.as_secs() {
                return Ok(LockOutcome::Busy { holder });
            }
        }
    }
    std::fs::write(&path, format!("{session_id}\n{}", now_secs()))?;
    Ok(LockOutcome::Acquired)
}

/// Refresh the lock timestamp so a long-running session isn't reclaimed under it.
pub fn touch_lock(dir: &Path, session_id: &str) {
    let _ = std::fs::write(lock_path(dir), format!("{session_id}\n{}", now_secs()));
}

/// Release a profile. Only the holder may release it, so a stale reclaim by a
/// new owner isn't undone when the old owner finally exits.
pub fn release_lock(dir: &Path, session_id: &str) {
    let path = lock_path(dir);
    if let Ok(raw) = std::fs::read_to_string(&path) {
        if let Some((holder, _)) = parse_lock(&raw) {
            if holder != session_id {
                return;
            }
        }
    }
    let _ = std::fs::remove_file(path);
}

fn parse_lock(raw: &str) -> Option<(String, u64)> {
    let mut lines = raw.lines();
    let holder = lines.next()?.trim().to_string();
    let stamp = lines.next()?.trim().parse().ok()?;
    Some((holder, stamp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anonymous_sessions_never_get_a_persistent_profile() {
        // The load-bearing rule: a no-trace chat must not be able to read the
        // user's logged-in accounts, even if it asks for a named profile.
        let scope = ProfileScope::for_session(SessionKind::Anonymous, Some("default"));
        assert_eq!(scope, ProfileScope::Ephemeral);
        assert!(profile_dir(&scope).is_none());
    }

    #[test]
    fn ordinary_sessions_default_to_the_shared_profile() {
        for kind in [SessionKind::Project, SessionKind::Standalone] {
            assert_eq!(
                ProfileScope::for_session(kind, None),
                ProfileScope::Persistent {
                    name: "default".into()
                }
            );
        }
    }

    #[test]
    fn profile_names_cannot_escape_the_profiles_directory() {
        let scope = ProfileScope::for_session(SessionKind::Project, Some("../../etc/passwd"));
        let ProfileScope::Persistent { name } = scope else {
            panic!("expected a persistent profile");
        };
        assert!(!name.contains('/'));
        assert!(!name.contains(".."));
        assert_eq!(name, "etc-passwd");
    }

    #[test]
    fn blank_or_symbol_only_names_fall_back_to_default() {
        for requested in ["", "   ", "///", "..."] {
            assert_eq!(
                ProfileScope::for_session(SessionKind::Project, Some(requested)),
                ProfileScope::Persistent {
                    name: "default".into()
                }
            );
        }
    }

    #[test]
    fn a_profile_can_only_be_held_by_one_session_at_a_time() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            acquire_lock(dir.path(), "session-a").unwrap(),
            LockOutcome::Acquired
        );
        assert_eq!(
            acquire_lock(dir.path(), "session-b").unwrap(),
            LockOutcome::Busy {
                holder: "session-a".into()
            }
        );
    }

    #[test]
    fn the_holder_can_reclaim_its_own_profile() {
        let dir = tempfile::tempdir().expect("tempdir");
        acquire_lock(dir.path(), "session-a").unwrap();
        assert_eq!(
            acquire_lock(dir.path(), "session-a").unwrap(),
            LockOutcome::Acquired
        );
    }

    #[test]
    fn an_abandoned_lock_does_not_strand_the_profile() {
        // Owner crashed without releasing: the lock is older than the TTL, so a
        // new session may take it rather than the profile being lost forever.
        let dir = tempfile::tempdir().expect("tempdir");
        let stale = now_secs() - LOCK_TTL.as_secs() - 1;
        std::fs::write(lock_path(dir.path()), format!("dead-session\n{stale}")).unwrap();

        assert_eq!(
            acquire_lock(dir.path(), "session-b").unwrap(),
            LockOutcome::Acquired
        );
    }

    #[test]
    fn releasing_is_scoped_to_the_holder() {
        // A crashed owner exiting late must not release a profile that has
        // since been reclaimed by someone else.
        let dir = tempfile::tempdir().expect("tempdir");
        acquire_lock(dir.path(), "session-a").unwrap();
        release_lock(dir.path(), "session-b");
        assert_eq!(
            acquire_lock(dir.path(), "session-c").unwrap(),
            LockOutcome::Busy {
                holder: "session-a".into()
            }
        );

        release_lock(dir.path(), "session-a");
        assert_eq!(
            acquire_lock(dir.path(), "session-c").unwrap(),
            LockOutcome::Acquired
        );
    }
}
