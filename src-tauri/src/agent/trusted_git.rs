// SPDX-License-Identifier: Apache-2.0
//! M159: conservative proof + durable, non-destructive Git snapshot.
use std::{path::{Component, Path}, process::{Command, Stdio}, io::Write};
use crate::util::no_window::NoWindow;

pub(super) fn sensitive(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    let words: Vec<_> = lower.split_whitespace().map(|w| w.trim_matches(['\'', '"'])).collect();
    ["reset", "restore", "checkout", "clean", "stash", "rebase", "filter-branch", "filter-repo"].iter().any(|w| lower.contains(w))
        || (lower.contains("push") && words.iter().any(|w| w.starts_with("--force") || *w == "--delete" || *w == "-f" || *w == "-d" || w.starts_with(':') || w.starts_with('+')))
}
fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").no_window().current_dir(root).args(args).stdin(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}
fn inside(root: &Path, raw: &str) -> bool {
    let path = Path::new(raw);
    if raw.is_empty() || raw.contains(':') || path.is_absolute() { return false; }
    for c in path.components() {
        match c {
            Component::CurDir => (),
            Component::Normal(n) if n != ".git" => (),
            _ => return false,
        }
    }
    let mut probe = root.join(path);
    loop {
        if let Ok(real) = probe.canonicalize() { return real.starts_with(root); }
        if std::fs::symlink_metadata(&probe).is_ok() { return false; }
        match probe.parent() { Some(p) if p.starts_with(root) => probe = p.into(), _ => return false }
    }
}
fn parts(command: &str) -> Option<Vec<Vec<&str>>> {
    if command.contains(['\n', '\r']) { return None; }
    let parts: Vec<Vec<&str>> = command.split("&&").map(|s| s.split_whitespace().collect()).collect();
    if parts.iter().any(|p| p.is_empty()) { return None; }
    for p in &parts {
        for word in p {
            if word.chars().any(|c| !(c.is_ascii_alphanumeric() || "-_/ .".contains(c))) { return None; }
        }
    }
    Some(parts)
}
pub(super) fn local_rewrite(root: &Path, command: &str) -> bool {
    let Some(root) = root.canonicalize().ok() else { return false };
    // No parent repository discovery, submodule effects, or unmerged index.
    if git(&root, &["rev-parse", "--show-toplevel"]).and_then(|p| Path::new(&p).canonicalize().ok()).as_ref() != Some(&root) { return false; }
    if git(&root, &["ls-files", "--stage"]).is_none_or(|s| s.lines().any(|l| l.starts_with("160000 "))) { return false; }
    let Some(parts) = parts(command) else { return false };
    let mut has_git = false;
    for argv in parts {
        let paths: &[&str] = match argv.as_slice() {
            ["git", "reset", "--hard", revision] => {
                has_git = true;
                if revision.starts_with('-') || revision.contains('/') { return false; }
                let Some(target) = git(&root, &["rev-parse", "--verify", &format!("{revision}^{{commit}}")]) else { return false };
                let Some(removed) = git(&root, &["rev-list", &format!("{target}..HEAD")]) else { return false };
                for commit in removed.lines() {
                    if git(&root, &["branch", "-r", "--contains", commit]).is_none_or(|s| !s.is_empty()) { return false; }
                }
                continue;
            }
            ["git", "checkout", "--", rest @ ..] => { has_git = true; rest }
            ["git", "restore", "--staged", rest @ ..] | ["git", "restore", "--", rest @ ..] => { has_git = true; rest }
            ["git", "clean", flags, rest @ ..] if ["-f", "-fq", "-qf", "-fd", "-fdq"].contains(flags) => { has_git = true; rest }
            ["git", "stash"] | ["git", "stash", "push"] => { has_git = true; continue; }
            ["rm", "-rf", rest @ ..] => rest,
            _ => return false,
        };
        if paths.is_empty() || paths.iter().any(|p| p.starts_with('-') || !inside(&root, p)) { return false; }
        // Do not allow shell deletion of the workspace root itself.
        if argv[0] == "rm" && paths.iter().any(|p| root.join(p).canonicalize().ok().as_ref() == Some(&root)) { return false; }
    }
    has_git
}

/// Snapshot via a private index; never stash away or rewrite the live index.
/// Parent 1 retains all unpublished commits; parent 2 retains staged content.
pub(super) fn backup(root: &Path, command: &str) -> Option<String> {
    if !local_rewrite(root, command) { return None; }
    let head = git(root, &["rev-parse", "HEAD"])?;
    let index_tree = git(root, &["write-tree"])?;
    let nonce = format!("{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos());
    let dir = std::env::temp_dir().join(format!("cf-git-backup-{nonce}"));
    std::fs::create_dir(&dir).ok()?;
    let index = dir.join("index");
    let result = (|| {
        let run = |args: &[&str], input: Option<&str>| -> Option<String> {
            let mut child = Command::new("git").no_window().current_dir(root).env("GIT_INDEX_FILE", &index)
                .env("GIT_AUTHOR_NAME", "CodeFactory backup").env("GIT_AUTHOR_EMAIL", "backup@codefactory.invalid")
                .env("GIT_COMMITTER_NAME", "CodeFactory backup").env("GIT_COMMITTER_EMAIL", "backup@codefactory.invalid")
                .args(args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
            if let Some(input) = input { child.stdin.take()?.write_all(input.as_bytes()).ok()?; }
            let out = child.wait_with_output().ok()?;
            out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().into())
        };
        run(&["read-tree", &head], None)?;
        run(&["add", "-A", "--", "."], None)?;
        // Include ignored files explicitly targeted by rm (clean -x is not allowed).
        for argv in parts(command)? {
            if argv.first() == Some(&"rm") {
                for path in &argv[2..] {
                    if root.join(path).exists() { run(&["add", "-f", "--", path], None)?; }
                }
            }
        }
        let tree = run(&["write-tree"], None)?;
        let staged = run(&["commit-tree", &index_tree, "-p", &head], Some("M159 staged backup\n"))?;
        let commit = run(&["commit-tree", &tree, "-p", &head, "-p", &staged], Some("M159 workspace backup\n"))?;
        let reference = format!("refs/codefactory/backup/{nonce}");
        run(&["update-ref", &reference, &commit, "0000000000000000000000000000000000000000"], None)?;
        (git(root, &["rev-parse", &reference])? == commit).then_some(reference)
    })();
    let _ = std::fs::remove_dir_all(dir);
    result
}
