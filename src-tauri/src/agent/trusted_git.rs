// SPDX-License-Identifier: Apache-2.0
//! M159: conservative proof + durable, non-destructive Git snapshot.
use std::{path::{Component, Path}, process::{Command, Stdio}, io::Write};
use crate::util::no_window::NoWindow;

// The very same policy source used by standalone shell calls; also available
// to the standalone integration harnesses that include this proof module.
#[path = "../tools/shell_policy.rs"]
mod standalone_shell_policy;

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
    if command.contains("<<") {
        return !python_heredoc_proved(None, command);
    }
    let Some(segments) = executable_segments(command) else { return true };
    segments.iter().any(|words| segment_is_sensitive(words))
}

/// M198: recognize one complete, expansion-disabled Python stdin program.
/// No shell suffix, custom executable, wrapper, substitution or redirection is
/// accepted. The validator parses AST only; it never evaluates model code.
pub(super) fn python_heredoc_proved(root: Option<&Path>, command: &str) -> bool {
    python_heredoc_proof(root, command).is_ok()
}

/// M215: the same proof, but a refusal carries the reason (including the script
/// line the prover could not prove) so the approval text can name it instead of
/// only saying "unproved".
pub(super) fn python_heredoc_proof(root: Option<&Path>, command: &str) -> Result<(), String> {
    if command.len() > 256 * 1024 { return Err("脚本超过 256 KiB".into()); }
    let Some((header, rest)) = command.split_once('\n') else { return Err("heredoc 缺少换行".into()); };
    // M215: `cd <this workspace> &&` is how a heredoc actually starts; the
    // prefix is proved against the registered root, never ignored.
    let header = heredoc_header_prefix(header.trim(), root)?;
    let Some((runner, delimiter)) = header.split_once(" <<") else { return Err("缺少 `<<` heredoc 起始符".into()); };
    let node = ["node", "node -"].contains(&runner);
    if !node && !["python3 -", "python -"].contains(&runner) {
        return Err(format!("不接受的解释器入口：{runner}"));
    }
    let Some(delimiter) = delimiter.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) else {
        return Err("heredoc 分隔符必须用单引号关闭展开".into());
    };
    if delimiter.is_empty() || !delimiter.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err("heredoc 分隔符不合法".into());
    }
    let mut body = String::new();
    let mut lines = rest.lines();
    let mut closed = false;
    while let Some(line) = lines.next() {
        if line == delimiter {
            closed = true;
            if lines.any(|l| !l.trim().is_empty()) { return Err("heredoc 之后仍有 shell 内容".into()); }
            break;
        }
        body.push_str(line);
        body.push('\n');
    }
    if !closed { return Err("heredoc 分隔符未闭合".into()); }
    let mut cmd = Command::new(if cfg!(windows) { "python" } else { "python3" });
    cmd.args(["-I", "-c", include_str!("trusted_python_ast.py")]);
    if let Some(root) = root { cmd.arg(root); } else { cmd.arg(""); }
    if node { cmd.arg("node"); }
    let Ok(mut child) = cmd.no_window().stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn() else {
        return Err(format!("无法启动{}证明进程", if node { " node " } else { " python " }));
    };
    let Some(mut input) = child.stdin.take() else { return Err("无法写入证明脚本".into()); };
    if input.write_all(body.as_bytes()).is_err() { return Err("证明脚本写入失败".into()); }
    drop(input);
    let Ok(output) = child.wait_with_output() else { return Err("证明进程异常退出".into()); };
    if output.status.success() { return Ok(()); }
    let reason = decode_prover_escapes(String::from_utf8_lossy(&output.stderr).trim());
    Err(if reason.is_empty() { "脚本包含无法静态证明的结构".into() } else { reason })
}

/// M215: prover diagnostics travel through the host console, and a Windows code
/// page that cannot encode CJK escapes them as `\uXXXX`. Decode those escapes so
/// the approval text names the line in readable Chinese on every platform.
pub(crate) fn decode_prover_escapes(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' || chars.peek() != Some(&'u') {
            out.push(c);
            continue;
        }
        chars.next();
        let mut digits = String::with_capacity(4);
        while digits.len() < 4 {
            match chars.peek() {
                Some(&d) if d.is_ascii_hexdigit() => { digits.push(d); chars.next(); }
                _ => break,
            }
        }
        match u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32) {
            Some(decoded) if digits.len() == 4 => out.push(decoded),
            _ => { out.push('\\'); out.push('u'); out.push_str(&digits); }
        }
    }
    out
}

/// M215: strip an optional `cd <path>` prefix that only re-enters this
/// workspace. Anything else (subdirectory, other root, unparsable) stays
/// unproved.
fn heredoc_header_prefix<'a>(header: &'a str, root: Option<&Path>) -> Result<&'a str, String> {
    let Some(rest) = header.strip_prefix("cd ") else { return Ok(header) };
    let mut split = None;
    for separator in [" && ", "; "] {
        if let Some(parts) = rest.split_once(separator) { split = Some(parts); break; }
    }
    let Some((target, tail)) = split else { return Err("heredoc 头部的 cd 没有可证明的分隔符".into()); };
    let target = target.trim().trim_matches('"').trim_matches('\'');
    let identity = matches!(target, "$(pwd)" | "${PWD}" | "$PWD" | "`pwd`");
    // Compare physical paths: a macOS temp root is reached through a symlink, so
    // the literal root text and its canonical form differ.
    let same_root = root.is_some_and(|root| {
        let Ok(target) = root.join(target).canonicalize() else { return false };
        target == root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
    });
    if !identity && !same_root { return Err(format!("heredoc 头部 cd 目标不是本工作区：{target}")); }
    Ok(tail.trim())
}

/// Git subcommands that move the working tree, the index or a local branch.
const DESTRUCTIVE_GIT_SUBCOMMANDS: &[&str] = &[
    "reset", "restore", "checkout", "clean", "stash", "rebase", "merge", "switch", "add", "commit", "config",
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
    // M180: `worktree list` reads registrations; it does not move or discard
    // work. Only the listing verb is exempt: remove/prune/add/move/lock still
    // require the session-owned proof and snapshot.
    if lower == "worktree" && args.get(index + 1).is_some_and(|a| a == "list") {
        return false;
    }
    if lower == "branch" {
        return !codefactory_agent_core::command_segment_is_read_only_probe(
            &std::iter::once("git").chain(args[index..].iter().map(String::as_str)).collect::<Vec<_>>().join(" "),
        );
    }
    if lower == "fetch" {
        return !fetch_args(&args[index + 1..].iter().map(String::as_str).collect::<Vec<_>>());
    }
    if DESTRUCTIVE_GIT_SUBCOMMANDS.contains(&lower.as_str()) {
        return true;
    }
    if lower == "push" {
        return args[index + 1..].iter().any(|arg| {
            arg.starts_with("--force") || ["--delete", "--mirror", "--prune", "-f", "-d"].contains(&arg.as_str())
                || arg.starts_with('+') || arg.starts_with(':')
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
        assert!(!sensitive("git push origin main"), "ordinary push uses the separate session delivery approval, never local proof");
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
struct Word { text: String, expanded: bool, redirect: bool }
struct Segment { words: Vec<Word>, separator: char, raw: String }

/// Lossless segment text for the permission gateway: quoted script data is
/// never reconstructed or reinterpreted as shell operators.
pub(super) fn command_segments(command: &str) -> Option<Vec<String>> {
    Some(segments(command)?.into_iter().map(|segment| segment.raw).collect())
}
fn segments(command: &str) -> Option<Vec<Segment>> {
    let mut chars = command.chars().peekable();
    let mut result = Vec::new();
    let mut words = Vec::new();
    let mut word = Word::default();
    let mut started = false;
    let mut quote = None;
    let mut variables = std::collections::HashMap::<String, String>::new();
    let mut raw_start = 0;
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
                '|' if chars.peek() == Some(&'|') => { chars.next(); 'o' },
                other => other,
            };
            if words.is_empty() { return None; }
            record_assignment(&words, &mut variables)?;
            let end = command.len() - chars.clone().map(char::len_utf8).sum::<usize>();
            let delimiter_len = if matches!(separator, '&' | 'o') { 2 } else { 1 };
            result.push(Segment { words: std::mem::take(&mut words), separator, raw: command[raw_start..end - delimiter_len].trim().into() });
            raw_start = end;
            continue;
        }
        if quote.is_none() && c == '>' {
            // Keep redirects as explicit tokens; never execute or erase their target.
            if started {
                if word.text == "2" || word.text == "1" { word = Word::default(); }
                else { words.push(std::mem::take(&mut word)); }
                started = false;
            }
            if chars.peek() == Some(&'&') {
                chars.next();
                if chars.next() != Some('1') { return None; }
                words.push(Word { text: "@stderr-to-stdout".into(), expanded: false, redirect: true });
            } else {
                if chars.peek() == Some(&'>') { chars.next(); }
                words.push(Word { text: "@output".into(), expanded: false, redirect: true });
            }
            continue;
        }
        if quote.is_none() && "<()[{}]*?!#~".contains(c) { return None; }
        word.text.push(c);
        started = true;
    }
    if quote.is_some() { return None; }
    if started { words.push(word); }
    if words.is_empty() { return None; }
    record_assignment(&words, &mut variables)?;
    result.push(Segment { words, separator: ';', raw: command[raw_start..].trim().into() });
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
/// A revision or branch word: never a flag, never a path escape, and no
/// refspec colon that could turn a fetch into a remote write.
fn revision(word: &str) -> bool {
    !word.is_empty()
        && !word.starts_with('-')
        && !word.contains([':', '\\', '*', '?', '['])
        && word.chars().all(|c| c.is_ascii_alphanumeric() || "-_./@{}^~=".contains(c))
}
fn inside_or_here(root: &Path, word: &str) -> bool {
    word == "." && root.is_dir() || inside(root, word)
}

/// M177: the grammar of a provable local rewrite. Every accepted verb keeps the
/// M159 snapshot meaningful: reachable from the workspace, non-interactive and
/// recoverable from the private-index backup.
fn local_rewrite_args(root: &Path, args: &[&str]) -> bool {
    let Some((verb, rest)) = args.split_first() else { return false };
    match *verb {
        // Network read: updates remote-tracking refs only.
        "fetch" => rest.iter().all(|w| {
            ["--prune", "--all", "--tags", "--no-tags", "-q", "--quiet", "--force"].contains(w)
                || revision(w)
        }),
        "stash" => stash_args(root, rest),
        "merge" => merge_args(rest),
        "rebase" => rebase_args(rest),
        "switch" | "checkout" => switch_args(root, rest),
        "restore" => restore_args(root, rest),
        "clean" => clean_args(root, rest),
        "add" => add_args(root, rest),
        "commit" => commit_args(rest),
        _ => false,
    }
}

fn stash_args(root: &Path, rest: &[&str]) -> bool {
    let Some((sub, tail)) = rest.split_first() else { return true }; // bare `git stash` lists
    match *sub {
        "list" | "show" => true,
        // `push`/`save` keep the work: the snapshot plus the stash itself.
        "push" | "save" => stash_push_args(root, tail),
        "pop" | "apply" => tail.iter().all(|w| {
            ["-q", "--quiet", "--index"].contains(w) || revision(w)
        }),
        // `drop`, `clear`, `branch` and `create` discard the only remaining copy.
        _ => false,
    }
}

fn stash_push_args(root: &Path, tail: &[&str]) -> bool {
    let mut index = 0;
    while index < tail.len() {
        let word = tail[index];
        match word {
            "-u" | "--include-untracked" | "-a" | "--all" | "-q" | "--quiet" | "--keep-index"
            | "--no-keep-index" | "--" => (),
            "-m" | "--message" => {
                index += 1;
                if index >= tail.len() { return false; }
            }
            _ if word.starts_with('-') => return false,
            _ if !inside_or_here(root, word) => return false,
            _ => (),
        }
        index += 1;
    }
    true
}

fn merge_args(rest: &[&str]) -> bool {
    let mut index = 0;
    let mut target = false;
    // Merging a finished or aborted state touches no ref.
    let mut state = rest.is_empty();
    while index < rest.len() {
        let word = rest[index];
        match word {
            "--abort" | "--continue" | "--quit" => state = true,
            "--no-edit" | "--no-ff" | "--ff-only" | "--squash" | "--no-commit"
            | "--allow-unrelated-histories" | "--autostash" | "--no-stat" | "--no-autostash"
            | "-q" | "--quiet" => (),
            "-m" | "--message" => {
                index += 1;
                if index >= rest.len() { return false; }
            }
            _ if word.starts_with('-') => return false,
            _ if revision(word) => target = true,
            _ => return false,
        }
        index += 1;
    }
    state || target
}

fn rebase_args(rest: &[&str]) -> bool {
    let mut target = false;
    let mut state = false;
    for word in rest {
        match *word {
            "--abort" | "--continue" | "--skip" | "--quit" => state = true,
            "--autostash" | "--no-autostash" | "--no-edit" | "--keep-empty" | "--no-verify"
            | "-q" | "--quiet" => (),
            // Interactive and exec rebases run arbitrary commands per commit.
            _ if word.starts_with("-i") || *word == "--interactive" || word.starts_with("--exec")
                || *word == "-x" || word.starts_with("-x") => return false,
            _ if word.starts_with('-') => return false,
            _ if revision(word) => target = true,
            _ => return false,
        }
    }
    state || target
}

fn switch_args(root: &Path, rest: &[&str]) -> bool {
    let mut index = 0;
    let mut named = rest.is_empty();
    let mut paths = false;
    let mut pathspec = false;
    while index < rest.len() {
        let word = rest[index];
        if word == "--" {
            pathspec = true;
            index += 1;
            continue;
        }
        match word {
            "-b" | "-B" | "-c" | "-C" | "--create" | "--force-create" => {
                index += 1;
                if index >= rest.len() || !revision(rest[index]) { return false; }
                named = true;
            }
            "-q" | "--quiet" | "-f" | "--force" | "-m" | "--merge" => (),
            _ if word.starts_with('-') => return false,
            _ if pathspec => {
                if !inside_or_here(root, word) { return false; }
                paths = true;
            }
            _ if revision(word) => named = true,
            _ if inside_or_here(root, word) => paths = true,
            _ => return false,
        }
        index += 1;
    }
    paths || named
}

fn restore_args(root: &Path, rest: &[&str]) -> bool {
    let mut index = 0;
    let mut paths = false;
    while index < rest.len() {
        let word = rest[index];
        match word {
            "--" | "--staged" | "--worktree" | "-q" | "--quiet" => (),
            "-s" | "--source" => {
                index += 1;
                if index >= rest.len() || !revision(rest[index]) { return false; }
            }
            _ if word.starts_with('-') => return false,
            _ if inside_or_here(root, word) => paths = true,
            _ => return false,
        }
        index += 1;
    }
    paths
}

fn clean_args(root: &Path, rest: &[&str]) -> bool {
    let mut forced = false;
    let mut paths = false;
    for word in rest {
        let word = *word;
        match word {
            "--" => (),
            "--force" => forced = true,
            "--dry-run" => (),
            _ if word.starts_with('-') && word.len() > 1 => {
                // `-x`/`-X` delete ignored files (build caches, local state) that
                // the snapshot cannot restore, so they stay an approval.
                if !word.chars().skip(1).all(|c| "fdnq".contains(c)) { return false; }
                forced |= word.contains('f');
            }
            _ => {
                if root.join(word).canonicalize().is_ok_and(|p| p == root) { return false; }
                if !inside_or_here(root, word) { return false; }
                paths = true;
            }
        }
    }
    forced && paths
}

fn add_args(root: &Path, rest: &[&str]) -> bool {
    let mut staged = false;
    let mut paths = false;
    for word in rest {
        let word = *word;
        match word {
            "-A" | "--all" | "-u" | "--update" => staged = true,
            "--" | "-n" | "--dry-run" | "-v" | "--verbose" | "-f" | "--force" => (),
            _ if word.starts_with('-') => return false,
            _ if inside_or_here(root, word) => paths = true,
            _ => return false,
        }
    }
    staged || paths
}

fn commit_args(rest: &[&str]) -> bool {
    let mut index = 0;
    while index < rest.len() {
        let word = rest[index];
        match word {
            "-a" | "--all" | "--amend" | "--no-edit" | "--allow-empty"
            | "--allow-empty-message" | "--no-verify" | "-q" | "--quiet" | "-n" => (),
            "-m" | "--message" | "-am" | "-ma" => {
                index += 1;
                if index >= rest.len() { return false; }
            }
            _ if word.starts_with('-') => return false,
            // Every non-flag word of a commit is either consumed as a message
            // above or is an unexplained pathspec; refuse the latter.
            _ => return false,
        }
        index += 1;
    }
    true
}

/// M182: path confinement is independent of Git option ordering.
fn safe_path(root: &Path, raw: &str) -> bool {
    let normalized = raw.replace('\\', "/");
    let lower = normalized.to_ascii_lowercase();
    if lower.split('/').any(|p| p == ".git" || p == ".ssh" || p == ".aws" || p == ".gnupg"
        || p == ".env" || p.starts_with(".env.") || p == "credentials" || p == "id_rsa" || p == "id_ed25519"
        || p.ends_with(".pem") || p.ends_with(".key")) { return false; }
    inside(root, &normalized)
}

fn bounded_args(root: &Path, args: &[&str]) -> bool {
    args.iter().all(|word| {
        let value = word.split_once('=').map_or(*word, |(_, value)| value);
        if value.contains("..") || value.contains(':') || value.starts_with('/') || value.starts_with('\\') { return false; }
        if !word.starts_with('-') || word.contains('=') {
            if !safe_path(root, value) { return false; }
        }
        if value.contains('/') || value.contains('\\') || root.join(value).exists() {
            safe_path(root, value)
        } else { true }
    })
}

fn local_denylist(root: &Path, args: &[&str]) -> bool {
    let Some((verb, rest)) = args.split_first() else { return false };
    // These verbs affect remote authority, command execution or configuration.
    // M198: repository-mandated workspace-local hook setup is not a remote
    // credential/protection rewrite. Exact shape only; --global stays fenced.
    if *verb == "config" {
        return matches!(rest, ["core.hooksPath", ".githooks"] | ["--local", "core.hooksPath", ".githooks"]);
    }
    if ["push", "filter-branch", "filter-repo", "submodule", "send-email", "credential"].contains(verb) { return false; }
    if rest.iter().any(|w| {
        let option = w.split('=').next().unwrap_or(w);
        ["--interactive", "--patch", "--edit", "--exec", "--editor", "--upload-pack", "--receive-pack", "--ext-diff", "--textconv", "--output", "--output-directory", "--git-dir", "--work-tree", "--unsafe-paths", "--pathspec-from-file", "--recurse-submodules", "--separate-git-dir", "--reference", "--alternate-refs-command"].contains(&option)
            || option.starts_with("--inter") || option.starts_with("--exe")
            || ["-i", "-p", "-e"].contains(&option)
            || *verb == "rebase" && (w.starts_with("-i") || w.starts_with("-x"))
    }) { return false; }
    if *verb == "clean" && rest.iter().any(|w| w.starts_with('-') && (w.contains('x') || w.contains('X') || w.contains('i'))) { return false; }
    // Preserve older extra-cautious stash policy: dropping the sole stash is
    // not recoverable by a working-tree snapshot.
    if *verb == "stash" && rest.first().is_some_and(|w| ["drop", "clear"].contains(w)) { return false; }
    let mut message = false;
    let mut checked = Vec::new();
    for word in rest {
        if message { message = false; continue; }
        if ["-m", "--message", "-am", "-ma"].contains(word) { message = true; continue; }
        if word.starts_with("--message=") { continue; }
        checked.push(*word);
    }
    !message && bounded_args(root, &checked)
}

/// Fetch reads a remote, unlike push. Ref specs and URLs are not local paths.
/// Keep executable transport overrides and external remote helpers fenced.
fn fetch_args(rest: &[&str]) -> bool {
    rest.iter().all(|word| {
        if word.starts_with('-') {
            ["--prune", "--all", "--tags", "--no-tags", "--prune-tags", "--no-recurse-submodules", "--force", "-f", "--quiet", "-q", "--verbose", "-v", "--dry-run"].contains(word)
        } else {
            !word.is_empty() && !word.contains("::") && !word.starts_with('|')
                && !word.chars().any(|c| c.is_control() || c.is_whitespace())
        }
    })
}

/// Exact single PR metadata mutation. Compound commands, repository overrides,
/// edits of another resource and implicit current-PR selection are not grants.
pub(super) fn own_pr_metadata_target(command: &str) -> Option<i64> {
    let segments = segments(command)?;
    if segments.len() != 1 || segments[0].words.iter().any(|w| w.redirect) { return None; }
    let words: Vec<_> = segments[0].words.iter().map(|w| w.text.as_str()).collect();
    let ["gh", "pr", action, number, rest @ ..] = words.as_slice() else { return None; };
    let number = number.parse::<i64>().ok().filter(|n| *n > 0)?;
    let mut pairs = rest.chunks_exact(2);
    let allowed = pairs.by_ref().all(|pair| !pair[1].is_empty() && match *action {
        "edit" => ["--add-label", "--remove-label"].contains(&pair[0]),
        "comment" => pair[0] == "--body",
        _ => false,
    });
    (allowed && !rest.is_empty() && pairs.remainder().is_empty()).then_some(number)
}

fn workspace_at(roots: &[std::path::PathBuf], cwd: &Path, path: &str) -> Option<std::path::PathBuf> {
    let real = cwd.join(path).canonicalize().ok()?;
    roots.iter().find(|root| **root == real).cloned()
}

/// M215: `cd "$(pwd)"` re-enters the current directory — `pwd` resolves to the
/// physical path of the same tree — so it neither moves the cwd nor changes
/// which files a relative path names. `segments()` refuses command
/// substitution outright, so the identity prefix is proved and removed here.
/// Every other substitution still fails closed.
fn strip_identity_cd(command: &str) -> &str {
    let command = command.trim_start();
    for prefix in ["cd \"$(pwd)\" && ", "cd \"$(pwd)\"; ", "cd \"$(pwd)\"\n"] {
        if let Some(rest) = command.strip_prefix(prefix) { return rest.trim_start(); }
    }
    command
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
    let segments = segments(strip_identity_cd(command))?;
    let mut touched = std::collections::BTreeMap::<std::path::PathBuf, Vec<String>>::new();
    let mut removed = std::collections::BTreeSet::new();
    let mut previous_pipe = false;
    let mut has_git = false;
    for segment in segments {
        let input_pipe = previous_pipe;
        let piped = input_pipe || segment.separator == '|';
        previous_pipe = segment.separator == '|';
        let mut argv = Vec::new();
        let mut words = segment.words.iter();
        while let Some(word) = words.next() {
            if word.redirect {
                if word.text == "@output" {
                    let target = words.next()?;
                    if target.redirect || !safe_path(&cwd, &target.text) { return None; }
                    touched.entry(cwd.clone()).or_default();
                }
            } else { argv.push(word.text.as_str()); }
        }
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
        // M207: non-Git commands use the same shell safety decision as a
        // standalone trusted call. Do not impose a second path/runner whitelist.
        if argv.first() != Some(&"git") && argv.first() != Some(&"rm") {
            let standalone = segment.raw.as_str();
            if matches!(standalone_shell_policy::classify_command(standalone), standalone_shell_policy::ShellCommandPolicy::Allow { .. })
                && !codefactory_agent_loop::policy::is_delivery_command(standalone)
                && !sensitive(standalone)
            { continue; }
            return None;
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
        // Queries do not need path confinement or a snapshot. Reject execution
        // and output overrides before using the shared Git query classifier.
        let readonly = (codefactory_agent_core::command_segment_is_read_only_probe(&segment.raw)
            || matches!(args, ["stash", "list", ..] | ["stash", "show", ..] | ["worktree", "list", ..])
            || matches!(args, ["config", "core.hooksPath"] | ["config", "--get", "core.hooksPath"] | ["config", "--local", "--get", "core.hooksPath"]))
            && args.first() != Some(&"fetch")
            && !args.iter().any(|word| {
                let option = word.split('=').next().unwrap_or(word);
                ["--ext-diff", "--textconv", "--output", "--output-directory", "--upload-pack", "--receive-pack"].contains(&option)
            });
        if readonly { continue; }
        if args.first() == Some(&"reset") && args.iter().any(|word| *word == "--hard") {
            let revisions: Vec<_> = args[1..].iter().filter(|word| !word.starts_with('-')).copied().collect();
            if revisions.len() > 1 { return None; }
            let revision = revisions.first().copied().unwrap_or("HEAD");
            if !legacy_rewrite(&root, &format!("git reset --hard {revision}")) { return None; }
        }
        if args.first() == Some(&"worktree") {
            match args {
                ["worktree", "remove", flags @ ..] | ["worktree", "add", flags @ ..] => {
                    if piped { return None; }
                    let path_index = flags.iter().position(|w| !w.starts_with('-'))?;
                    let path = flags[path_index];
                    if !flags.iter().enumerate().all(|(index, word)| index == path_index || bounded_args(&root, &[*word])) { return None; }
                    if flags.iter().any(|word| word.starts_with("--orphan") || word.starts_with("--detach=") || word.starts_with("--reason")) { return None; }
                    let target = workspace_at(&roots, &root, path)?;
                    if target == root || target == cwd { return None; }
                    let common = |r: &Path| git(r, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).and_then(|p| Path::new(&p).canonicalize().ok());
                    if common(&root).is_none() || common(&root) != common(&target) { return None; }
                    if args[1] == "remove" {
                        if !removed.insert(target.clone()) { return None; }
                        touched.entry(target).or_default().push(".".into());
                    } else { touched.entry(target).or_default(); }
                    touched.entry(root).or_default();
                    continue;
                }
                _ => return None,
            }
        }
        if let ["fetch", rest @ ..] = args {
            if input_pipe || !fetch_args(rest) { return None; }
            touched.entry(root).or_default();
            continue;
        }
        if !local_denylist(&root, args) { return None; }
        if readonly { continue; }
        if input_pipe { return None; }
        // M182: option ordering is not authority. Confined non-interactive
        // local verbs use the shared denylist and the same durable snapshot.
        if args.first().is_some_and(|verb| ["fetch", "stash", "merge", "rebase", "switch", "checkout", "restore", "clean", "add", "commit", "reset", "cherry-pick", "revert", "am", "apply", "config"].contains(verb)) {
            touched.entry(root).or_default();
            continue;
        }
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
    // An empty target set means the command contains only proved queries.
    // Do not turn an unrelated conflicted index into a read-only approval.
    Some(if refs.is_empty() { "read-only".into() } else { refs.join(",") })
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
