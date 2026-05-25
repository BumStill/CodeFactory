// SPDX-License-Identifier: Apache-2.0
//
// Git first-class citizen commands.
//
// Hybrid strategy:
//   * Read operations (status, log, branches, diff) use `git2` (libgit2)
//     for fast, structured, async-friendly access.
//   * Write operations (commit, checkout, push, pull) shell out to the
//     `git` CLI for simpler error handling and to avoid libgit2's
//     authentication/credential complexity. Git CLI is universally
//     installed on developer machines.

use serde::Serialize;
use std::process::Command;

use git2::{BranchType, DiffFormat, DiffOptions, Repository, Status, StatusOptions};
use crate::util::no_window::NoWindow;

// ── Types serialized to the frontend ────────────────────────────────────────

#[derive(Serialize, Debug, Clone)]
pub struct GitStatus {
    pub branch: String,           // "main" or "(detached)"
    pub upstream: Option<String>, // "origin/main"
    pub ahead: usize,
    pub behind: usize,
    pub staged: Vec<FileChange>,
    pub unstaged: Vec<FileChange>,
    pub untracked: Vec<String>,
    pub is_repo: bool, // false if cwd is not a git repo
}

#[derive(Serialize, Debug, Clone)]
pub struct FileChange {
    pub path: String,
    pub status: String, // "modified" | "added" | "deleted" | "renamed" | "typechange"
}

#[derive(Serialize, Debug, Clone)]
pub struct GitCommit {
    pub hash: String,         // full SHA
    pub short_hash: String,   // first 7 chars
    pub author: String,
    pub email: String,
    pub timestamp: i64,       // unix epoch (seconds)
    pub message: String,      // first line only
    pub message_body: String, // full message
}

#[derive(Serialize, Debug, Clone)]
pub struct GitBranch {
    pub name: String,
    pub is_current: bool,
    pub is_remote: bool,
    pub upstream: Option<String>,
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn empty_status() -> GitStatus {
    GitStatus {
        branch: String::new(),
        upstream: None,
        ahead: 0,
        behind: 0,
        staged: Vec::new(),
        unstaged: Vec::new(),
        untracked: Vec::new(),
        is_repo: false,
    }
}

/// Try to open the repository starting at `cwd` and walking up the tree.
fn open_repo(cwd: &str) -> Result<Repository, String> {
    Repository::discover(cwd).map_err(|e| format!("Not a git repository ({}): {}", cwd, e.message()))
}

/// Convert a libgit2 status code for the index (staged) into a label.
fn classify_index_status(s: Status) -> Option<&'static str> {
    if s.contains(Status::INDEX_NEW) {
        Some("added")
    } else if s.contains(Status::INDEX_MODIFIED) {
        Some("modified")
    } else if s.contains(Status::INDEX_DELETED) {
        Some("deleted")
    } else if s.contains(Status::INDEX_RENAMED) {
        Some("renamed")
    } else if s.contains(Status::INDEX_TYPECHANGE) {
        Some("typechange")
    } else {
        None
    }
}

/// Convert a libgit2 status code for the working tree (unstaged) into a label.
fn classify_workdir_status(s: Status) -> Option<&'static str> {
    if s.contains(Status::WT_MODIFIED) {
        Some("modified")
    } else if s.contains(Status::WT_DELETED) {
        Some("deleted")
    } else if s.contains(Status::WT_RENAMED) {
        Some("renamed")
    } else if s.contains(Status::WT_TYPECHANGE) {
        Some("typechange")
    } else if s.contains(Status::WT_NEW) {
        // Note: WT_NEW alone is normally a fresh untracked file. Callers that
        // want untracked-vs-unstaged distinction should check IGNORED/conflict
        // first. We treat WT_NEW here as unstaged "added" only when the entry
        // also has an INDEX_* mark (i.e. partially staged — handled separately).
        Some("added")
    } else {
        None
    }
}

/// Convenience: run `git` CLI with given args in `cwd`, returning stdout on
/// success or a combined-error string on failure.
fn run_git(cwd: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git").no_window()
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(|e| format!("Failed to spawn git: {}", e))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let mut msg = stderr.trim().to_string();
        if msg.is_empty() {
            msg = stdout.trim().to_string();
        }
        if msg.is_empty() {
            msg = format!("git exited with status {}", output.status);
        }
        Err(msg)
    }
}

// ── Read commands (git2) ────────────────────────────────────────────────────

#[tauri::command]
pub async fn git_status(cwd: String) -> Result<GitStatus, String> {
    let repo = match open_repo(&cwd) {
        Ok(r) => r,
        Err(_) => return Ok(empty_status()),
    };

    // Branch + detached state
    let (branch_name, head_oid) = match repo.head() {
        Ok(head) => {
            let name = head
                .shorthand()
                .map(str::to_string)
                .unwrap_or_else(|| "(detached)".to_string());
            let oid = head.target();
            (name, oid)
        }
        Err(_) => ("(no commits yet)".to_string(), None),
    };

    // Upstream + ahead/behind
    let mut upstream = None;
    let mut ahead = 0usize;
    let mut behind = 0usize;
    if branch_name != "(detached)" && branch_name != "(no commits yet)" {
        if let Ok(local) = repo.find_branch(&branch_name, BranchType::Local) {
            if let Ok(up) = local.upstream() {
                if let Ok(Some(up_name)) = up.name().map(|n| n.map(str::to_string)) {
                    upstream = Some(up_name);
                }
                if let (Some(local_oid), Some(up_oid)) = (head_oid, up.get().target()) {
                    if let Ok((a, b)) = repo.graph_ahead_behind(local_oid, up_oid) {
                        ahead = a;
                        behind = b;
                    }
                }
            }
        }
    }

    // Working tree status
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .renames_head_to_index(true)
        .renames_index_to_workdir(true);

    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();

    if let Ok(statuses) = repo.statuses(Some(&mut opts)) {
        for entry in statuses.iter() {
            let path = entry.path().unwrap_or("").to_string();
            if path.is_empty() {
                continue;
            }
            let s = entry.status();

            // Pure untracked: WT_NEW with no index marker.
            if s.contains(Status::WT_NEW)
                && !(s.contains(Status::INDEX_NEW)
                    || s.contains(Status::INDEX_MODIFIED)
                    || s.contains(Status::INDEX_DELETED)
                    || s.contains(Status::INDEX_RENAMED)
                    || s.contains(Status::INDEX_TYPECHANGE))
            {
                untracked.push(path.clone());
                continue;
            }

            if let Some(label) = classify_index_status(s) {
                staged.push(FileChange {
                    path: path.clone(),
                    status: label.to_string(),
                });
            }
            if let Some(label) = classify_workdir_status(s) {
                // Skip the WT_NEW case we already routed to "untracked".
                if !(s.contains(Status::WT_NEW)
                    && !(s.contains(Status::INDEX_NEW)
                        || s.contains(Status::INDEX_MODIFIED)
                        || s.contains(Status::INDEX_DELETED)
                        || s.contains(Status::INDEX_RENAMED)
                        || s.contains(Status::INDEX_TYPECHANGE)))
                {
                    unstaged.push(FileChange {
                        path,
                        status: label.to_string(),
                    });
                }
            }
        }
    }

    Ok(GitStatus {
        branch: branch_name,
        upstream,
        ahead,
        behind,
        staged,
        unstaged,
        untracked,
        is_repo: true,
    })
}

#[tauri::command]
pub async fn git_log(cwd: String, limit: u32) -> Result<Vec<GitCommit>, String> {
    let repo = open_repo(&cwd)?;
    let limit = if limit == 0 { 50 } else { limit as usize };

    let mut walker = repo
        .revwalk()
        .map_err(|e| format!("revwalk failed: {}", e.message()))?;

    if walker.push_head().is_err() {
        // Fresh repo with no commits.
        return Ok(Vec::new());
    }

    let mut commits = Vec::with_capacity(limit);
    for oid_res in walker.take(limit) {
        let oid = match oid_res {
            Ok(o) => o,
            Err(_) => continue,
        };
        let commit = match repo.find_commit(oid) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let author = commit.author();
        let full_message = commit.message().unwrap_or("").to_string();
        let first_line = full_message.lines().next().unwrap_or("").to_string();
        let hash = oid.to_string();
        let short_hash = hash.chars().take(7).collect::<String>();
        commits.push(GitCommit {
            hash,
            short_hash,
            author: author.name().unwrap_or("").to_string(),
            email: author.email().unwrap_or("").to_string(),
            timestamp: commit.time().seconds(),
            message: first_line,
            message_body: full_message,
        });
    }

    Ok(commits)
}

#[tauri::command]
pub async fn git_branches(cwd: String) -> Result<Vec<GitBranch>, String> {
    let repo = open_repo(&cwd)?;
    let head_name = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().map(str::to_string));

    let iter = repo
        .branches(None)
        .map_err(|e| format!("branches failed: {}", e.message()))?;

    let mut out = Vec::new();
    for branch_res in iter {
        let (branch, btype) = match branch_res {
            Ok(b) => b,
            Err(_) => continue,
        };
        let name = match branch.name() {
            Ok(Some(n)) => n.to_string(),
            _ => continue,
        };
        let is_remote = matches!(btype, BranchType::Remote);
        let is_current = !is_remote && head_name.as_deref() == Some(name.as_str());
        let upstream = if !is_remote {
            branch
                .upstream()
                .ok()
                .and_then(|u| u.name().ok().flatten().map(str::to_string))
        } else {
            None
        };
        out.push(GitBranch {
            name,
            is_current,
            is_remote,
            upstream,
        });
    }

    // Stable order: current first, then locals (alpha), then remotes (alpha).
    out.sort_by(|a, b| match (a.is_current, b.is_current) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => match (a.is_remote, b.is_remote) {
            (false, true) => std::cmp::Ordering::Less,
            (true, false) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        },
    });

    Ok(out)
}

#[tauri::command]
pub async fn git_diff(cwd: String, file: Option<String>) -> Result<String, String> {
    let repo = open_repo(&cwd)?;
    let mut opts = DiffOptions::new();
    opts.include_untracked(false).recurse_untracked_dirs(false);
    if let Some(ref path) = file {
        opts.pathspec(path);
    }

    let diff = repo
        .diff_index_to_workdir(None, Some(&mut opts))
        .map_err(|e| format!("diff failed: {}", e.message()))?;

    let mut buf = String::new();
    diff.print(DiffFormat::Patch, |_d, _h, line| {
        let origin = line.origin();
        match origin {
            '+' | '-' | ' ' => buf.push(origin),
            _ => {}
        }
        buf.push_str(&String::from_utf8_lossy(line.content()));
        true
    })
    .map_err(|e| format!("diff print failed: {}", e.message()))?;

    Ok(buf)
}

#[tauri::command]
pub async fn git_file_diff(
    cwd: String,
    file: String,
    staged: bool,
) -> Result<String, String> {
    let repo = open_repo(&cwd)?;
    let mut opts = DiffOptions::new();
    opts.pathspec(&file);

    let diff = if staged {
        // diff between HEAD tree and index
        let head_tree = repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_tree().ok());
        repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut opts))
            .map_err(|e| format!("staged diff failed: {}", e.message()))?
    } else {
        repo.diff_index_to_workdir(None, Some(&mut opts))
            .map_err(|e| format!("diff failed: {}", e.message()))?
    };

    let mut buf = String::new();
    diff.print(DiffFormat::Patch, |_d, _h, line| {
        let origin = line.origin();
        match origin {
            '+' | '-' | ' ' => buf.push(origin),
            _ => {}
        }
        buf.push_str(&String::from_utf8_lossy(line.content()));
        true
    })
    .map_err(|e| format!("diff print failed: {}", e.message()))?;

    Ok(buf)
}

// ── Write commands (git CLI) ────────────────────────────────────────────────

#[tauri::command]
pub async fn git_add(cwd: String, files: Vec<String>) -> Result<(), String> {
    if files.is_empty() {
        return Ok(());
    }
    let mut args: Vec<&str> = vec!["add", "--"];
    for f in &files {
        args.push(f.as_str());
    }
    run_git(&cwd, &args).map(|_| ())
}

#[tauri::command]
pub async fn git_commit(cwd: String, message: String) -> Result<String, String> {
    if message.trim().is_empty() {
        return Err("Commit message cannot be empty".to_string());
    }
    run_git(&cwd, &["commit", "-m", &message])?;
    // Resolve the new HEAD sha
    let head = run_git(&cwd, &["rev-parse", "HEAD"])?;
    Ok(head.trim().to_string())
}

#[tauri::command]
pub async fn git_checkout(cwd: String, target: String) -> Result<(), String> {
    if target.trim().is_empty() {
        return Err("Checkout target cannot be empty".to_string());
    }
    run_git(&cwd, &["checkout", &target]).map(|_| ())
}

#[tauri::command]
pub async fn git_create_branch(
    cwd: String,
    name: String,
    checkout: bool,
) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Branch name cannot be empty".to_string());
    }
    run_git(&cwd, &["branch", &name])?;
    if checkout {
        run_git(&cwd, &["checkout", &name])?;
    }
    Ok(())
}

#[tauri::command]
pub async fn git_push(cwd: String, remote: String, branch: String) -> Result<(), String> {
    let remote = if remote.trim().is_empty() { "origin".to_string() } else { remote };
    if branch.trim().is_empty() {
        return Err("Branch is required for push".to_string());
    }
    run_git(&cwd, &["push", &remote, &branch]).map(|_| ())
}

#[tauri::command]
pub async fn git_pull(cwd: String, remote: String, branch: String) -> Result<(), String> {
    let remote = if remote.trim().is_empty() { "origin".to_string() } else { remote };
    if branch.trim().is_empty() {
        return Err("Branch is required for pull".to_string());
    }
    run_git(&cwd, &["pull", &remote, &branch]).map(|_| ())
}
