// SPDX-License-Identifier: Apache-2.0
//! M105: built-in, approval-free closeout actions for a task's own artifacts.
//!
//! Background (real machines, 2026-10-17): after a PR merged, the agent cleaned
//! up its own leftover build directory with
//! `rm -rf "$PWD/.codefactory-cache/cargo-target-m103"` and was prompted for
//! approval — in a session the user had already put in trusted mode. The
//! workspace-local proof in [`super::trusted_workspace`] answers `false` for
//! that spelling on purpose: `$`, `"`, `&&` and absolute paths are unprovable
//! syntax, so the normal gate applies.
//!
//! Widening that parser would widen *every* `rm -rf` the model can write. This
//! module is the narrow alternative the spec asks for: a dedicated closeout
//! action that deletes exactly three things, each with its own ownership proof,
//! and nothing else.
//!
//! * [`CloseoutArtifact::WorkspaceCache`] — a path under
//!   `<workspace>/.codefactory-cache/`. Must be relative, must exist, must
//!   canonicalize strictly inside the workspace (no `..`, no symlink escape,
//!   never the cache root itself).
//! * [`CloseoutArtifact::MergedRemoteBranch`] — a PR head branch that the
//!   caller has already confirmed `merged=true` for. Never the default branch.
//! * [`CloseoutArtifact::OwnedTempFile`] — a file directly inside the system
//!   temp directory whose name carries this task's ownership token. A file
//!   whose ownership cannot be proven is refused, not deleted.
//!
//! Everything outside those three shapes keeps the existing confirmation: the
//! general `rm -rf` rule is untouched.

use std::path::{Component, Path, PathBuf};

/// Directory, inside the workspace, that holds per-task build/output artifacts.
pub(crate) const TASK_CACHE_DIR: &str = ".codefactory-cache";

/// What the closeout action was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloseoutArtifact {
    /// Workspace-relative path under `.codefactory-cache/`.
    WorkspaceCache(String),
    /// Remote branch of a PR the caller proved `merged=true`.
    MergedRemoteBranch {
        branch: String,
        default_branch: String,
        merged: bool,
        /// The caller compared the PR's `head.ref` with `branch`.
        pr_head_matches: bool,
    },
    /// A file the task created directly in the system temp directory.
    OwnedTempFile { path: PathBuf, owner_token: String },
}

/// Classification result. `Allowed` is the only value that may skip approval;
/// the reason string is what the agent should tell the user when it is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloseoutOutcome {
    Allowed,
    RequiresApproval(&'static str),
}

impl CloseoutOutcome {
    pub(crate) fn is_allowed(&self) -> bool {
        matches!(self, CloseoutOutcome::Allowed)
    }
}

/// Decide one closeout artifact against the live workspace root.
pub(crate) fn classify(root: &Path, artifact: &CloseoutArtifact) -> CloseoutOutcome {
    match artifact {
        CloseoutArtifact::WorkspaceCache(raw) => workspace_cache(root, raw),
        CloseoutArtifact::MergedRemoteBranch {
            branch,
            default_branch,
            merged,
            pr_head_matches,
        } => remote_branch(branch, default_branch, *merged, *pr_head_matches),
        CloseoutArtifact::OwnedTempFile { path, owner_token } => temp_file(path, owner_token),
    }
}

/// Delete a proven workspace-cache artifact. Refuses (without touching the
/// filesystem) whenever [`classify`] does not answer `Allowed`.
pub(crate) fn remove(root: &Path, artifact: &CloseoutArtifact) -> std::io::Result<bool> {
    if !classify(root, artifact).is_allowed() {
        return Ok(false);
    }
    match artifact {
        CloseoutArtifact::WorkspaceCache(raw) => {
            let Ok(root) = std::fs::canonicalize(root) else {
                return Ok(false);
            };
            let target = root.join(raw);
            let Ok(real) = std::fs::canonicalize(&target) else {
                return Ok(false);
            };
            if real == root || !real.starts_with(&root) {
                return Ok(false);
            }
            if real.is_dir() {
                std::fs::remove_dir_all(&real)?;
            } else {
                std::fs::remove_file(&real)?;
            }
            Ok(true)
        }
        CloseoutArtifact::OwnedTempFile { path, .. } => {
            let Ok(real) = std::fs::canonicalize(path) else {
                return Ok(false);
            };
            if real.is_dir() {
                std::fs::remove_dir_all(&real)?;
            } else {
                std::fs::remove_file(&real)?;
            }
            Ok(true)
        }
        // Branch deletion is performed through the authenticated GitHub client
        // by the caller; this module only proves that it is safe to skip the
        // prompt.
        CloseoutArtifact::MergedRemoteBranch { .. } => Ok(true),
    }
}

fn workspace_cache(root: &Path, raw: &str) -> CloseoutOutcome {
    let raw = raw.trim();
    if raw.is_empty() {
        return CloseoutOutcome::RequiresApproval("empty closeout path");
    }
    let candidate = Path::new(raw);
    if candidate.is_absolute() || raw.starts_with('~') || raw.contains('$') {
        return CloseoutOutcome::RequiresApproval("closeout path is not workspace-relative");
    }
    for component in candidate.components() {
        match component {
            Component::CurDir | Component::Normal(_) => {}
            _ => return CloseoutOutcome::RequiresApproval("closeout path escapes the workspace"),
        }
    }
    let mut normal_components = candidate
        .components()
        .filter(|component| matches!(component, Component::Normal(_)));
    match normal_components.next() {
        Some(Component::Normal(first)) if first == TASK_CACHE_DIR => {}
        _ => {
            return CloseoutOutcome::RequiresApproval(
                "only .codefactory-cache artifacts are closed out automatically",
            )
        }
    }
    // `.codefactory-cache` alone would wipe the shared build cache.
    if normal_components.next().is_none() {
        return CloseoutOutcome::RequiresApproval("refusing to delete the whole task cache root");
    }
    let Ok(root) = std::fs::canonicalize(root) else {
        return CloseoutOutcome::RequiresApproval("workspace root is not resolvable");
    };
    let Ok(real) = std::fs::canonicalize(root.join(raw)) else {
        return CloseoutOutcome::RequiresApproval("closeout artifact does not exist");
    };
    if real == root || !real.starts_with(&root) {
        return CloseoutOutcome::RequiresApproval("closeout path escapes the workspace");
    }
    if real != root.join(raw) {
        return CloseoutOutcome::RequiresApproval("closeout artifact resolves through a symlink");
    }
    CloseoutOutcome::Allowed
}

fn remote_branch(
    branch: &str,
    default_branch: &str,
    merged: bool,
    pr_head_matches: bool,
) -> CloseoutOutcome {
    let branch = branch.trim();
    if branch.is_empty() {
        return CloseoutOutcome::RequiresApproval("empty branch name");
    }
    if branch == default_branch || branch == "main" || branch == "master" {
        return CloseoutOutcome::RequiresApproval("refusing to delete the default branch");
    }
    if !merged {
        return CloseoutOutcome::RequiresApproval("the PR is not confirmed merged");
    }
    if !pr_head_matches {
        return CloseoutOutcome::RequiresApproval("the branch is not this task's own PR head");
    }
    CloseoutOutcome::Allowed
}

fn temp_file(path: &Path, owner_token: &str) -> CloseoutOutcome {
    let owner_token = owner_token.trim();
    if owner_token.len() < 6 {
        return CloseoutOutcome::RequiresApproval("no task ownership token: refusing to delete");
    }
    if !path.is_absolute() {
        return CloseoutOutcome::RequiresApproval("temp artifact path is not absolute");
    }
    let Ok(temp) = std::fs::canonicalize(std::env::temp_dir()) else {
        return CloseoutOutcome::RequiresApproval("system temp directory is not resolvable");
    };
    // Direct child of the system temp directory only: a nested directory could
    // be somebody else's tree.
    let Some(parent) = path.parent() else {
        return CloseoutOutcome::RequiresApproval("temp artifact has no parent");
    };
    let Ok(parent) = std::fs::canonicalize(parent) else {
        return CloseoutOutcome::RequiresApproval("temp artifact parent is not resolvable");
    };
    if parent != temp {
        return CloseoutOutcome::RequiresApproval(
            "temp artifact is outside the system temp directory",
        );
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return CloseoutOutcome::RequiresApproval("temp artifact name is not valid UTF-8");
    };
    if !name.contains(owner_token) {
        return CloseoutOutcome::RequiresApproval("temp artifact does not carry this task's token");
    }
    if !path.exists() {
        return CloseoutOutcome::RequiresApproval("temp artifact does not exist");
    }
    CloseoutOutcome::Allowed
}

/// Does `command` delete only `.codefactory-cache` artifacts of `root`?
///
/// This is the bridge used by the trusted-mode gate for the two spellings that
/// actually happened on real machines — `rm -rf .codefactory-cache/<task>` and
/// `rm -rf "$PWD/.codefactory-cache/<task>"` — and it accepts nothing beyond
/// them: one `rm`-family program, reviewed flags, and operands that are
/// `.codefactory-cache/<something>` after `$PWD/` / `${PWD}/` expansion.
pub(crate) fn bash_cache_delete(root: &Path, command: &str) -> bool {
    let Some(argv) = tokenize_closeout(command) else {
        return false;
    };
    let Some((program, operands)) = argv.split_first() else {
        return false;
    };
    let program = program.rsplit('/').next().unwrap_or(program);
    let program = program.to_ascii_lowercase();
    if !matches!(program.as_str(), "rm" | "rmdir" | "unlink") {
        return false;
    }
    let mut targets = 0usize;
    for operand in operands {
        if let Some(flag) = operand.strip_prefix('-') {
            let reviewed = matches!(
                flag.to_ascii_lowercase().as_str(),
                "r" | "f" | "d" | "v" | "rf" | "fr" | "recurse" | "recursive" | "force"
            );
            if !reviewed {
                return false;
            }
            continue;
        }
        let relative = operand
            .strip_prefix("${PWD}/")
            .or_else(|| operand.strip_prefix("$PWD/"))
            .unwrap_or(operand);
        if !classify(
            root,
            &CloseoutArtifact::WorkspaceCache(relative.to_string()),
        )
        .is_allowed()
        {
            return false;
        }
        targets += 1;
    }
    targets > 0
}

/// Split a closeout command into words, or `None` for any syntax this module
/// does not fully understand. Double quotes are accepted only around a single
/// whitespace-free word (the `"$PWD/..."` spelling); everything else asks.
fn tokenize_closeout(command: &str) -> Option<Vec<String>> {
    const REJECTED: &[&str] = &[
        ";", "&&", "||", "|", "&", ">", "<", "`", "(", ")", "{", "}", "*", "?", "[", "]", "~",
        "\n", "\r", "'", "\\", "!", "#", "$(", "$HOME", "$0",
    ];
    if command.trim().is_empty() {
        return None;
    }
    // `${PWD}` / `$PWD` are the only expansions accepted here: they name the
    // workspace root the caller already proved. Every other `$` still fails
    // closed, so `$HOME`, `$(pwd)` and friends keep asking.
    let probe = command.replace("${PWD}", "").replace("$PWD", "");
    if probe.contains('$') || REJECTED.iter().any(|token| probe.contains(token)) {
        return None;
    }
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in command.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
            }
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if quoted {
        return None;
    }
    if !current.is_empty() {
        words.push(current);
    }
    if words.is_empty() {
        None
    } else {
        Some(words)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: PathBuf,
        root: PathBuf,
        outside: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "cf-m105-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            let root = dir.join("workspace");
            let outside = dir.join("checkout");
            std::fs::create_dir_all(root.join(".codefactory-cache/cargo-target-m105")).unwrap();
            std::fs::create_dir_all(&outside).unwrap();
            std::fs::write(
                root.join(".codefactory-cache/cargo-target-m105/out.bin"),
                "x",
            )
            .unwrap();
            std::fs::write(root.join("notes.txt"), "draft\n").unwrap();
            Fixture { dir, root, outside }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// R2/R5: the task's own cache artifact closes out without approval.
    #[test]
    fn r5_own_cache_artifact_is_allowed() {
        let fixture = Fixture::new("own-cache");
        for command in [
            "rm -rf .codefactory-cache/cargo-target-m105",
            "rm -rf \"$PWD/.codefactory-cache/cargo-target-m105\"",
            "rm -rf ${PWD}/.codefactory-cache/cargo-target-m105",
            "rm -r ./.codefactory-cache/cargo-target-m105",
        ] {
            assert!(
                bash_cache_delete(&fixture.root, command),
                "own cache closeout must not prompt: {command}"
            );
        }
        assert!(classify(
            &fixture.root,
            &CloseoutArtifact::WorkspaceCache(".codefactory-cache/cargo-target-m105".into())
        )
        .is_allowed());
    }

    /// R3/R5: outside paths, the cache root itself and user files still ask.
    #[test]
    fn r3_outside_and_user_paths_still_ask() {
        let fixture = Fixture::new("outside");
        let outside = fixture.outside.display().to_string();
        for command in [
            format!("rm -rf {outside}"),
            "rm -rf ../checkout".to_string(),
            format!("rm -rf \"$PWD\"/{outside}"),
            "rm -rf .codefactory-cache".to_string(),
            "rm -rf .codefactory-cache/".to_string(),
            "rm -rf notes.txt".to_string(),
            "rm -rf src".to_string(),
            "rm -rf $HOME".to_string(),
            "rm -rf .codefactory-cache/cargo-target-m105; rm -rf ../checkout".to_string(),
            "rm -rf .codefactory-cache/cargo-target-m105 && gh api -X DELETE /x".to_string(),
            "sudo rm -rf .codefactory-cache/cargo-target-m105".to_string(),
            "rm -rf .codefactory-cache/not-there".to_string(),
            "rm -rf *".to_string(),
        ] {
            assert!(
                !bash_cache_delete(&fixture.root, &command),
                "must still ask: {command}"
            );
        }
        assert_eq!(
            classify(
                &fixture.root,
                &CloseoutArtifact::WorkspaceCache("../checkout".into())
            ),
            CloseoutOutcome::RequiresApproval("closeout path escapes the workspace")
        );
        assert!(matches!(
            classify(
                &fixture.root,
                &CloseoutArtifact::WorkspaceCache(TASK_CACHE_DIR.into())
            ),
            CloseoutOutcome::RequiresApproval(_)
        ));
    }

    /// R3/R5: only a merged PR's own head branch closes out silently.
    #[test]
    fn r3_branch_requires_merged_own_head() {
        let ok = CloseoutArtifact::MergedRemoteBranch {
            branch: "codefactory/objective-m105".into(),
            default_branch: "main".into(),
            merged: true,
            pr_head_matches: true,
        };
        assert!(classify(Path::new("."), &ok).is_allowed());

        for (artifact, reason) in [
            (
                CloseoutArtifact::MergedRemoteBranch {
                    branch: "codefactory/objective-m105".into(),
                    default_branch: "main".into(),
                    merged: false,
                    pr_head_matches: true,
                },
                "the PR is not confirmed merged",
            ),
            (
                CloseoutArtifact::MergedRemoteBranch {
                    branch: "codefactory/someone-else".into(),
                    default_branch: "main".into(),
                    merged: true,
                    pr_head_matches: false,
                },
                "the branch is not this task's own PR head",
            ),
            (
                CloseoutArtifact::MergedRemoteBranch {
                    branch: "main".into(),
                    default_branch: "main".into(),
                    merged: true,
                    pr_head_matches: true,
                },
                "refusing to delete the default branch",
            ),
        ] {
            assert_eq!(
                classify(Path::new("."), &artifact),
                CloseoutOutcome::RequiresApproval(reason)
            );
        }
    }

    /// R2/R5: temp files are deleted only when this task's token is in the name.
    #[test]
    fn r3_temp_file_requires_ownership_token() {
        let fixture = Fixture::new("temp");
        let token = format!("m105-{}", std::process::id());
        let owned = std::env::temp_dir().join(format!("pr681-body-{token}.md"));
        std::fs::write(&owned, "body\n").unwrap();

        let outcome = classify(
            &fixture.root,
            &CloseoutArtifact::OwnedTempFile {
                path: owned.clone(),
                owner_token: token.clone(),
            },
        );
        assert_eq!(
            outcome,
            CloseoutOutcome::Allowed,
            "owned temp file must close out: {owned:?}"
        );

        assert!(matches!(
            classify(
                &fixture.root,
                &CloseoutArtifact::OwnedTempFile {
                    path: owned.clone(),
                    owner_token: String::new(),
                }
            ),
            CloseoutOutcome::RequiresApproval(_)
        ));
        assert!(matches!(
            classify(
                &fixture.root,
                &CloseoutArtifact::OwnedTempFile {
                    path: owned.clone(),
                    owner_token: "m999-task".into(),
                }
            ),
            CloseoutOutcome::RequiresApproval(_)
        ));

        // A path nested below the temp root is not provably ours.
        let nested = std::env::temp_dir()
            .join(format!("cf-m105-owned-{token}"))
            .join("body.md");
        std::fs::create_dir_all(nested.parent().unwrap()).unwrap();
        std::fs::write(&nested, "x").unwrap();
        assert!(matches!(
            classify(
                &fixture.root,
                &CloseoutArtifact::OwnedTempFile {
                    path: nested.clone(),
                    owner_token: token.clone(),
                }
            ),
            CloseoutOutcome::RequiresApproval(_)
        ));

        let _ = std::fs::remove_file(&owned);
        let _ = std::fs::remove_dir_all(nested.parent().unwrap());
    }

    /// R2: `remove` actually deletes what it proved, and refuses the rest
    /// without touching the filesystem.
    #[test]
    fn r2_remove_executes_only_proven_artifacts() {
        let fixture = Fixture::new("remove");
        assert!(remove(
            &fixture.root,
            &CloseoutArtifact::WorkspaceCache(".codefactory-cache/cargo-target-m105".into())
        )
        .unwrap());
        assert!(!fixture
            .root
            .join(".codefactory-cache/cargo-target-m105")
            .exists());
        assert!(fixture.root.join("notes.txt").exists());

        assert!(!remove(
            &fixture.root,
            &CloseoutArtifact::WorkspaceCache("notes.txt".into())
        )
        .unwrap());
        assert!(fixture.root.join("notes.txt").exists());
    }
}
