// SPDX-License-Identifier: Apache-2.0
//! M159: conservative proof + durable, non-destructive Git snapshot.
use std::{path::{Component, Path}, process::{Command, Stdio}, io::Write};
use crate::util::no_window::NoWindow;

/// M172-R1: ask for the M159 proof/snapshot only when an **executable segment**
/// runs a Git action that can discard local work.
///
/// The previous version scanned the whole command text for words such as
/// `clean`, `restore` or `git fetch`. That made two kinds of read-only call
/// prompt on 2026-10-26 in a trusted session: `git fetch --prune origin main`
/// together with `git merge-base --is-ancestor` (both listed as git *queries*
/// in `agent-core`), and `node -e '<script>'` whose script *data* mentioned one
/// of those words — quoted text is data, not a statement.
///
/// So: tokenize into segments (quotes keep their content inside one word),
/// then judge the segment's own subcommand. Anything we cannot parse safely
/// stays sensitive (fail closed).
pub(super) fn sensitive(command: &str) -> bool {
    let Some(segments) = executable_segments(command) else { return true };
    segments.iter().any(|words| segment_is_sensitive(words))
}

/// Git subcommands that move the working tree, the index or a local branch.
const DESTRUCTIVE_GIT_SUBCOMMANDS: &[&str] = &[
    "reset", "restore", "checkout", "clean", "stash", "rebase", "merge",
    "filter-branch", "filter-repo", "worktree", "cherry-pick", "revert", "am", "apply",
];

/// Words that only wrap another command.
const WRAPPER_WORDS: &[&str] = &["sudo", "command", "exec", "env", "nohup", "time", "builtin"];

/// Interpreters that take inline shell code and therefore must be re-scanned.
const SHELL_INTERPRETERS: &[&str] = &["bash", "sh", "zsh", "dash", "ksh", "ash"];

/// Interpreters that take inline code in another language: their payload is
/// data unless it visibly shells out to a destructive Git command.
const INLINE_CODE_INTERPRETERS: &[&str] = &["node", "nodejs", "deno", "bun", "python", "python3", "perl", "ruby"];

fn executable_segments(command: &str) -> Option<Vec<Vec<String>>> {
    fn finish_word(words: &mut Vec<String>, word: &mut String, in_word: &mut bool) {
        if *in_word {
            words.push(std::mem::take(word));
            *in_word = false;
        }
    }
    let mut segments: Vec<Vec<String>> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    for ch in command.chars() {
        if let Some(opening) = quote {
            if ch == opening {
                quote = None;
            } else {
                word.push(ch);
            }
            in_word = true;
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                in_word = true;
            }
            ';' | '|' | '&' | '>' | '<' | '\n' | '\r' => {
                finish_word(&mut words, &mut word, &mut in_word);
                if !words.is_empty() {
                    segments.push(std::mem::take(&mut words));
                }
            }
            c if c.is_whitespace() => finish_word(&mut words, &mut word, &mut in_word),
            c => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        return None;
    }
    finish_word(&mut words, &mut word, &mut in_word);
    if !words.is_empty() {
        segments.push(words);
    }
    Some(segments)
}

fn segment_is_sensitive(words: &[String]) -> bool {
    let mut start = 0;
    while start < words.len() {
        let word = words[start].as_str();
        let assignment = word
            .split_once('=')
            .is_some_and(|(name, _)| !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        if assignment || WRAPPER_WORDS.contains(&word.to_ascii_lowercase().as_str()) {
            start += 1;
        } else {
            break;
        }
    }
    let rest = &words[start..];
    let Some(binary) = rest.first() else { return false };
    let lowered = rest.iter().map(|w| w.to_ascii_lowercase()).collect::<Vec<_>>();
    // A dynamic command word or argument can expand into anything: keep the
    // snapshot when the segment visibly names Git or a destructive verb.
    let dynamic = rest.iter().any(|w| w.contains('$') || w.contains('`'));
    if dynamic
        && lowered.iter().any(|w| {
            w == "git"
                || w.contains("push")
                || DESTRUCTIVE_GIT_SUBCOMMANDS.contains(&w.as_str())
        })
    {
        return true;
    }
    let name = binary
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(binary)
        .trim_end_matches(".exe")
        .to_ascii_lowercase();
    let args = &rest[1..];
    if name == "git" {
        return git_segment_is_sensitive(args);
    }
    if SHELL_INTERPRETERS.contains(&name.as_str()) {
        return inline_payload(args, &["-c", "-lc", "-ec", "-ic"]).is_some_and(|p| sensitive(&p));
    }
    if INLINE_CODE_INTERPRETERS.contains(&name.as_str()) {
        return inline_payload(args, &["-e", "-c", "-p", "--eval", "--print"])
            .is_some_and(|p| payload_shells_out_to_destructive_git(&p));
    }
    // `cargo clean` deletes build output: previously caught by the "clean"
    // substring, so keep it explicit rather than losing the coverage.
    name == "cargo" && args.iter().any(|a| a.eq_ignore_ascii_case("clean"))
}

fn git_segment_is_sensitive(args: &[String]) -> bool {
    let mut index = 0;
    let mut rewrites_config = false;
    let mut subcommand: Option<&str> = None;
    while index < args.len() {
        // `-C <path>` takes a value and is not a config rewrite; compare the raw
        // argument because lowercasing makes it indistinguishable from `-c`.
        if args[index] == "-C" {
            index += 2;
            continue;
        }
        let lower = args[index].to_ascii_lowercase();
        if lower == "-c" || lower == "--config-env" {
            rewrites_config = true;
            index += 1;
            continue;
        }
        if matches!(lower.as_str(), "--git-dir" | "--work-tree" | "--namespace" | "--exec-path") {
            index += 2;
            continue;
        }
        if lower.starts_with('-') {
            index += 1;
            continue;
        }
        subcommand = Some(args[index].as_str());
        break;
    }
    if rewrites_config {
        return true;
    }
    let Some(subcommand) = subcommand else { return false };
    let lower = subcommand.to_ascii_lowercase();
    if DESTRUCTIVE_GIT_SUBCOMMANDS.contains(&lower.as_str()) {
        return true;
    }
    if lower == "push" {
        let after = &args[index + 1..];
        return after.iter().any(|arg| {
            let arg = arg.to_ascii_lowercase();
            arg.starts_with("--force")
                || arg == "--delete"
                || arg == "--mirror"
                || arg == "--prune"
                || arg == "-f"
                || arg == "-d"
                || arg.starts_with('+')
                || arg.starts_with(':')
        });
    }
    false
}

fn inline_payload(args: &[String], flags: &[&str]) -> Option<String> {
    let mut index = 0;
    while index < args.len() {
        if flags.contains(&args[index].to_ascii_lowercase().as_str()) {
            return args.get(index + 1).cloned();
        }
        index += 1;
    }
    None
}

fn payload_shells_out_to_destructive_git(payload: &str) -> bool {
    let lower = payload.to_ascii_lowercase();
    let shells_out = [
        "execsync", "spawnsync", "exec(", "spawn(", "subprocess", "os.system", "popen",
        "child_process", "system(",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    shells_out
        && [
            "git reset", "git clean", "git checkout", "git restore", "git stash", "git rebase",
            "git merge", "git worktree", "git push", "git -c",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
}
/// M172 regression tests.
#[cfg(test)]
mod m172_trusted_git_sensitive {
    use super::sensitive;

    /// The exact shape that prompted on 2026-10-26 in a trusted session: only
    /// read-only git queries, discovery commands and `gh auth status`.
    const READ_ONLY_EVIDENCE_COMMAND: &str = "git fetch --prune origin main && \
git merge-base --is-ancestor origin/main HEAD; \
find src-tauri/src -iname '*deliver*'; \
grep -R -n 'merge-queue' src-tauri/src | head -90; \
ls src-tauri; \
command -v pnpm; \
command -v cargo; \
gh auth status";

    #[test]
    fn m172_read_only_git_queries_are_not_sensitive() {
        assert!(!sensitive(READ_ONLY_EVIDENCE_COMMAND), "read-only evidence command must not ask");
        assert!(!sensitive("git fetch --prune origin main"));
        assert!(!sensitive("git merge-base --is-ancestor origin/main HEAD"));
        assert!(!sensitive("git log --oneline -5"));
        assert!(!sensitive("git status --short && git rev-parse HEAD"));
    }

    #[test]
    fn m172_quoted_script_data_is_not_a_git_action() {
        // The second prompt on 2026-10-26: a read-only script whose *data*
        // mentions the words the old substring scan treated as destructive.
        let script = "node -e 'const reg=require(\"./docs/testing/scenario-registry.json\");\
console.log(reg.scenarios.map(s=>[s.id,s.change_patterns,s.restore,s.cleanup,s.worktree]).join(\"\\n\"))' | head -20";
        assert!(!sensitive(script), "quoted script data must not trigger the git snapshot");
    }

    #[test]
    fn m172_local_interpreter_and_build_commands_are_not_sensitive() {
        assert!(!sensitive("python3 -"));
        assert!(!sensitive("cargo test --manifest-path src-tauri/Cargo.toml"));
        assert!(!sensitive("pnpm test"));
        assert!(!sensitive("node scripts/run.mjs --dry-run"));
        assert!(!sensitive("git push origin main"));
    }

    #[test]
    fn m172_destructive_git_actions_still_ask() {
        assert!(sensitive("git reset --hard"));
        assert!(sensitive("git checkout -- src/main.rs"));
        assert!(sensitive("git -C /tmp/ws reset --hard HEAD~1"));
        assert!(sensitive("git merge origin/main"));
        assert!(sensitive("git push --force origin main"));
        assert!(sensitive("git push origin :refs/heads/old"));
        assert!(sensitive("cd src-tauri && git clean -fdx"));
        assert!(sensitive("cargo clean"));
    }

    #[test]
    fn m172_shell_payload_is_rescanned_but_plain_script_is_not() {
        assert!(sensitive("bash -c \"git reset --hard\""));
        assert!(sensitive("sh -lc 'git stash drop'"));
        assert!(
            sensitive("node -e 'require(\"child_process\").execSync(\"git reset --hard\")'"),
            "a script that actually shells out to a destructive git command must ask"
        );
    }
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
fn legacy_rewrite(root: &Path, command: &str) -> bool {
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

/// A deliberately small shell grammar: no evaluation, globbing, redirection,
/// subshells or inherited variables. Quotes retain literal vs expansion meaning.
#[derive(Default)]
struct Word { text: String, expanded: bool }
struct Segment { words: Vec<Word>, separator: char }
fn segments(command: &str) -> Option<Vec<Segment>> {
    let mut chars = command.chars().peekable();
    let mut result = Vec::new();
    let mut words = Vec::new();
    let mut word = Word::default();
    let mut started = false;
    let mut quote = None;
    let mut variables = std::collections::HashMap::<String, String>::new();
    while let Some(c) = chars.next() {
        if matches!(c, '\n' | '\r' | '`') { return None; }
        if c == '\\' {
            // A backslash is a path separator on Windows and survives inside
            // quotes, so a quoted Windows worktree path must stay literal.
            // Outside quotes it is an escape or line continuation; inside double
            // quotes the escapes a shell would still act on are refused.
            if quote.is_none() { return None; }
            if quote == Some('"') && matches!(chars.peek(), Some('"') | Some('\\') | Some('$') | Some('`')) { return None; }
            word.text.push(c);
            started = true;
            continue;
        }
        if c == '\'' && quote != Some('"') || c == '"' && quote != Some('\'') {
            quote = if quote == Some(c) { None } else { Some(c) };
            started = true;
            continue;
        }
        if c == '$' && quote != Some('\'') {
            let mut name = String::new();
            while chars.peek().is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_') {
                name.push(chars.next()?);
            }
            let value = variables.get(&name)?;
            if quote.is_none() && value.chars().any(|c| c.is_whitespace() || "*?[]".contains(c)) { return None; }
            word.text.push_str(value);
            word.expanded = true;
            started = true;
            continue;
        }
        if quote.is_none() && (c.is_whitespace() || matches!(c, ';' | '&' | '|')) {
            if started { words.push(std::mem::take(&mut word)); started = false; }
            if c.is_whitespace() { continue; }
            let separator = match c {
                '&' if chars.next() == Some('&') => '&',
                '&' => return None,
                '|' if chars.peek() == Some(&'|') => return None,
                other => other,
            };
            if words.is_empty() { return None; }
            record_assignment(&words, &mut variables)?;
            result.push(Segment { words: std::mem::take(&mut words), separator });
            continue;
        }
        if quote.is_none() && "<>()[{}]*?!#~".contains(c) { return None; }
        word.text.push(c);
        started = true;
    }
    if quote.is_some() { return None; }
    if started { words.push(word); }
    if words.is_empty() { return None; }
    record_assignment(&words, &mut variables)?;
    result.push(Segment { words, separator: ';' });
    Some(result)
}
fn record_assignment(words: &[Word], variables: &mut std::collections::HashMap<String, String>) -> Option<()> {
    if words.len() == 1 {
        if let Some((name, value)) = words[0].text.split_once('=') {
            // Never let a literal assignment change shell/Git execution semantics.
            if words[0].expanded || name.is_empty() || name.starts_with("GIT_")
                || ["PATH", "IFS", "ENV", "BASH_ENV", "CDPATH", "HOME", "SHELLOPTS", "BASHOPTS", "GLOBIGNORE", "PROMPT_COMMAND", "PS4"].contains(&name)
                || !name.chars().enumerate().all(|(i,c)| c == '_' || c.is_ascii_alphabetic() || i > 0 && c.is_ascii_digit()) { return None; }
            variables.insert(name.into(), value.into());
        }
    }
    Some(())
}
fn workspace_at(roots: &[std::path::PathBuf], cwd: &Path, path: &str) -> Option<std::path::PathBuf> {
    let real = cwd.join(path).canonicalize().ok()?;
    roots.iter().find(|root| **root == real).cloned()
}

/// Returns the exact roots needing snapshots and explicit rm paths. The caller
/// supplies only session-owned, identity-verified registrations, never a prefix.
fn prove(roots: &[std::path::PathBuf], cwd: &Path, command: &str) -> Option<Vec<(std::path::PathBuf, Vec<String>)>> {
    let roots: Vec<_> = roots.iter().map(|p| p.canonicalize()).collect::<Result<_, _>>().ok()?;
    let mut cwd = workspace_at(&roots, cwd, ".")?;
    for root in &roots {
        if git(root, &["rev-parse", "--show-toplevel"]).and_then(|p| Path::new(&p).canonicalize().ok()).as_ref() != Some(root)
            || git(root, &["ls-files", "--stage"]).is_none_or(|s| s.lines().any(|l| l.starts_with("160000 "))) { return None; }
    }
    let segments = segments(command)?;
    let mut touched = std::collections::BTreeMap::<std::path::PathBuf, Vec<String>>::new();
    let mut removed = std::collections::BTreeSet::new();
    let mut previous_pipe = false;
    let mut has_git = false;
    for segment in segments {
        let piped = previous_pipe || segment.separator == '|';
        previous_pipe = segment.separator == '|';
        let argv: Vec<_> = segment.words.iter().map(|w| w.text.as_str()).collect();
        if argv.len() == 1 && argv[0].contains('=') {
            if piped || segment.separator != ';' { return None; }
            continue;
        }
        if let ["cd", path] = argv.as_slice() {
            // A failing cd followed by ';' would leave an unknown cwd. Only
            // existing exact registered roots are accepted, never subdirectories.
            if piped { return None; }
            cwd = workspace_at(&roots, &cwd, path)?;
            if removed.contains(&cwd) { return None; }
            continue;
        }
        if let ["head", rest @ ..] = argv.as_slice() {
            if !piped || !(rest.is_empty() || rest.len() == 2 && rest[0] == "-n" && rest[1].parse::<u32>().is_ok()) { return None; }
            continue;
        }
        let (root, args) = match argv.as_slice() {
            ["git", "-C", path, rest @ ..] => (workspace_at(&roots, &cwd, path)?, rest),
            ["git", rest @ ..] => (cwd.clone(), rest),
            ["rm", "-rf", paths @ ..] if !piped && !paths.is_empty() => {
                if paths.iter().any(|p| p.starts_with('-') || !inside(&cwd, p) || cwd.join(p).canonicalize().ok().as_ref() == Some(&cwd)) { return None; }
                touched.entry(cwd.clone()).or_default().extend(paths.iter().map(|p| p.to_string()));
                continue;
            }
            _ => return None,
        };
        if removed.contains(&root) { return None; }
        has_git = true;
        let readonly = match args {
            ["status"] | ["status", "--porcelain"] | ["status", "--short"] => true,
            ["log", rest @ ..] => rest.iter().all(|p| ["-1", "--oneline", "--no-decorate"].contains(p)),
            ["diff", rest @ ..] => rest.iter().all(|p| ["--stat", "--name-only", "--cached", "--staged"].contains(p)),
            _ => false,
        };
        if readonly { continue; }
        if piped { return None; }
        match args {
            ["fetch", "--prune", "origin", "main"] | ["fetch", "origin", "main"]
            | ["merge", "origin/main", "--no-edit"]
            | ["stash"] | ["stash", "push"] | ["stash", "pop"] => (),
            ["stash", "push", "-m", message] if !message.is_empty() => (),
            ["checkout", branch] if argv.get(1) == Some(&"-C") && !branch.starts_with('-') => {
                // Only the current session workspace branch, not arbitrary refs.
                if git(&root, &["branch", "--show-current"]).as_deref() != Some(branch) { return None; }
            }
            ["worktree", "remove", "--force", path] => {
                let target = workspace_at(&roots, &root, path)?;
                if target == root || target == cwd { return None; }
                let common = |r: &Path| git(r, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).and_then(|p| Path::new(&p).canonicalize().ok());
                if common(&root).is_none() || common(&root) != common(&target) { return None; }
                if !removed.insert(target.clone()) { return None; }
                touched.entry(target).or_default().push(".".into());
            }
            _ => {
                let simple = std::iter::once("git").chain(args.iter().copied()).collect::<Vec<_>>().join(" ");
                if !legacy_rewrite(&root, &simple) { return None; }
            }
        }
        touched.entry(root).or_default();
    }
    has_git.then(|| touched.into_iter().collect())
}

pub(super) fn local_rewrite(root: &Path, command: &str) -> bool {
    prove(&[root.into()], root, command).is_some()
}

pub(super) fn registered_rewrite(roots: &[std::path::PathBuf], cwd: &Path, command: &str) -> bool {
    prove(roots, cwd, command).is_some()
}

pub(super) fn backup_registered(roots: &[std::path::PathBuf], cwd: &Path, command: &str) -> Option<String> {
    let targets = prove(roots, cwd, command)?;
    let mut refs = Vec::new();
    for (root, paths) in targets { refs.push(snapshot(&root, &paths)?); }
    // Even read-only proven compositions retain an auditable snapshot.
    if refs.is_empty() { refs.push(snapshot(cwd, &[])?); }
    Some(refs.join(","))
}

/// Snapshot via a private index; never stash away or rewrite the live index.
/// Parent 1 retains all unpublished commits; parent 2 retains staged content.
pub(super) fn backup(root: &Path, command: &str) -> Option<String> {
    backup_registered(&[root.into()], root, command)
}
fn snapshot(root: &Path, paths: &[String]) -> Option<String> {
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
        for path in paths {
            if root.join(path).exists() { run(&["add", "-f", "--", path], None)?; }
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

#[cfg(test)]
mod tests {
    fn words(command: &str) -> Option<Vec<Vec<String>>> {
        super::segments(command).map(|segments| {
            segments.into_iter().map(|s| s.words.into_iter().map(|w| w.text).collect()).collect()
        })
    }

    #[test]
    fn m166_quoted_backslash_paths_stay_literal_words() {
        // Windows worktrees are quoted with backslash separators. Treating a
        // backslash inside quotes as an escape silently demotes the whole
        // composition to an approval prompt.
        let command = r#"cd "C:\Users\runneradmin\AppData\Local\Temp\ws"; git stash push -m prefill; git status --porcelain | head"#;
        let parsed = words(command).expect("quoted Windows path must parse");
        assert_eq!(parsed[0], vec!["cd", r"C:\Users\runneradmin\AppData\Local\Temp\ws"]);
        assert_eq!(parsed[1], vec!["git", "stash", "push", "-m", "prefill"]);
        assert_eq!(parsed[2], vec!["git", "status", "--porcelain"]);
        assert_eq!(parsed[3], vec!["head"]);
        let assigned = words(r#"S="C:\ws\two"; git -C "$S" worktree remove --force "C:\ws\one""#)
            .expect("quoted assignment must parse");
        assert_eq!(assigned[0], vec![r"S=C:\ws\two"]);
        assert_eq!(assigned[1], vec!["git", "-C", r"C:\ws\two", "worktree", "remove", "--force", r"C:\ws\one"]);
    }

    #[test]
    fn m166_backslash_escapes_outside_quotes_stay_rejected() {
        for command in [
            r"cd C:\ws; git status",
            r#"cd "C:\ws\"; git status"#,
            r#"cd "C:\\ws"; git status"#,
            r#"cd "C:$\ws"; git status"#,
        ] {
            assert!(words(command).is_none(), "{command}");
        }
        assert_eq!(
            words(r#"cd "C:\ws" && git status"#).map(|p| p[0].clone()),
            Some(vec!["cd".to_string(), r"C:\ws".to_string()])
        );
    }
}
