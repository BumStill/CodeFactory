// SPDX-License-Identifier: Apache-2.0
//! M159: conservative proof + durable, non-destructive Git snapshot.
use crate::util::no_window::NoWindow;
use std::{
    io::Write,
    path::{Component, Path},
    process::{Command, Stdio},
};

// The very same policy source used by standalone shell calls; also available
// to the standalone integration harnesses that include this proof module.
#[path = "../tools/shell_policy.rs"]
mod standalone_shell_policy;

// M260: unfold `for` / `while` / `if` / subshell / `$(...)` structures into the
// simple commands they run, so the per-segment judges below can see them.

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
    if has_unquoted_heredoc(command) {
        return !python_heredoc_proved(None, command);
    }
    let Some(segments) = executable_segments(command) else {
        return true;
    };
    segments.iter().any(|words| segment_is_sensitive(words))
}

/// M266 R1: what the *executable positions* of a command say about Git. The
/// approval text has to name the real cause: a command with no git executable
/// anywhere is not "a Git operation we could not prove", and a command we could
/// not unfold is not a Git write we found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum GitEntry {
    /// No git executable in any command position.
    NotGit,
    /// Git runs, but only as read-only queries.
    ReadOnlyGit,
    /// Git runs and a segment writes history or leaves this machine.
    GitWrite,
    /// The command could not be unfolded into simple commands.
    Unprovable,
}

/// M266 R1/R2: judge Git-ness from parsed command positions only. Argument text,
/// a search pattern, a variable name or a path that merely contains `git` is
/// data and never makes this a Git operation.
pub(super) fn git_entry(command: &str) -> GitEntry {
    let Some(segments) = executable_segments(command) else {
        return GitEntry::Unprovable;
    };
    let mut entry = GitEntry::NotGit;
    for words in &segments {
        match segment_git_entry(words) {
            GitEntry::GitWrite => return GitEntry::GitWrite,
            GitEntry::Unprovable => entry = GitEntry::Unprovable,
            GitEntry::ReadOnlyGit if entry == GitEntry::NotGit => entry = GitEntry::ReadOnlyGit,
            _ => {}
        }
    }
    entry
}

fn segment_git_entry(words: &[String]) -> GitEntry {
    let Some((binary, args)) = segment_command_words(words) else {
        return GitEntry::NotGit;
    };
    // A command word that expands at run time can become any program.
    if binary.contains('$') || binary.contains('`') {
        return GitEntry::Unprovable;
    }
    let name = command_name(&binary);
    if name == "git" {
        return if git_segment_is_sensitive(args) {
            GitEntry::GitWrite
        } else {
            GitEntry::ReadOnlyGit
        };
    }
    if SHELL_INTERPRETERS.contains(&name.as_str()) {
        if let Some(payload) = inline_payload(args, &["-c", "-lc", "-ec", "-ic"]) {
            return git_entry(&payload);
        }
    }
    if INLINE_CODE_INTERPRETERS.contains(&name.as_str())
        && inline_payload(args, &["-e", "-c", "-p", "--eval", "--print"])
            .is_some_and(|payload| payload_shells_out_to_destructive_git(&payload))
    {
        return GitEntry::GitWrite;
    }
    GitEntry::NotGit
}

/// M221b: byte offset of the first `<<` that sits outside shell quotes. A grep
/// pattern such as `"heredoc\|<<"` is an argument, not a heredoc — the field
/// sample was a read-only probe refused for `heredoc 缺少换行` because of it.
fn unquoted_double_angle(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut quote: Option<u8> = None;
    while index < bytes.len() {
        let byte = bytes[index];
        match quote {
            Some(opening) => {
                if byte == b'\\' && opening == b'"' {
                    index += 2;
                    continue;
                }
                if byte == opening {
                    quote = None;
                }
                index += 1;
            }
            None => {
                if byte == b'\\' {
                    index += 2;
                    continue;
                }
                if byte == b'\'' || byte == b'"' {
                    quote = Some(byte);
                    index += 1;
                    continue;
                }
                if byte == b'<' && bytes.get(index + 1) == Some(&b'<') {
                    return Some(index);
                }
                index += 1;
            }
        }
    }
    None
}

/// M221b: does this command open a heredoc at all? Only an unquoted `<<` does.
pub(super) fn has_unquoted_heredoc(command: &str) -> bool {
    unquoted_double_angle(command).is_some()
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
    python_heredoc_proof_with_sandbox(root, command, false)
}

/// M248b: the AST still proves effects (no network/process capability), while
/// the live kernel sandbox, not path spelling, confines filesystem writes.
pub(super) fn sandboxed_heredoc_proof(root: &Path, command: &str) -> Result<(), String> {
    python_heredoc_proof_with_sandbox(Some(root), command, true)
}

fn python_heredoc_proof_with_sandbox(
    root: Option<&Path>,
    command: &str,
    sandboxed: bool,
) -> Result<(), String> {
    if command.len() > 256 * 1024 {
        return Err("脚本超过 256 KiB".into());
    }
    // M221: the heredoc may start on a later line (`git add -A` then
    // `git commit -F - <<'MSG'`), so the header is every line up to and
    // including the first one that opens a heredoc.
    let mut header_end = None;
    let mut scanned = 0;
    for line in command.split_inclusive('\n') {
        scanned += line.len();
        if unquoted_double_angle(line).is_some() {
            header_end = Some(scanned);
            break;
        }
    }
    let Some(header_end) = header_end else {
        return Err("heredoc 缺少换行".into());
    };
    let (header, rest) = command.split_at(header_end);
    // M215 R1: resolve the `cd` prefix to a physical path (relative, `..`, `~`,
    // spaces, backslash escapes, macOS `/private` aliasing) and accept it when
    // it lands *inside* this workspace, not only on the workspace root.
    let (effective, header) = heredoc_header_prefix(header.trim(), root)?;
    let base = effective.as_deref().or(root);
    // M215 R1: read-only commands may run before the heredoc (`grep … | head;
    // echo ===; python3 - <<'PY'`). Each earlier segment must be read-only —
    // otherwise the whole command stays unproved. `2>&1` is a redirection, not a
    // command separator, so it is folded away before splitting.
    let header = header
        .replace("2>&1", " ")
        .replace("2>&2", " ")
        .replace(">&1", " ")
        .replace(">&2", " ");
    let segments = split_top_level(&header, &[';', '|', '&', '\n']);
    let Some(heredoc_at) = segments
        .iter()
        .position(|segment| unquoted_double_angle(segment).is_some())
    else {
        return Err("缺少 `<<` heredoc 起始符".into());
    };
    let (prefix, consumers) = (&segments[..heredoc_at], &segments[heredoc_at + 1..]);
    let Some(open) = unquoted_double_angle(segments[heredoc_at]) else {
        return Err("缺少 `<<` heredoc 起始符".into());
    };
    let runner = segments[heredoc_at][..open].trim();
    let delimiter = &segments[heredoc_at][open + 2..];
    let Some(delimiter) = delimiter
        .trim()
        .strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
    else {
        return Err("heredoc 分隔符必须用单引号关闭展开".into());
    };
    if delimiter.is_empty()
        || !delimiter
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Err("heredoc 分隔符不合法".into());
    }
    let mut body = String::new();
    let mut trailer = Vec::new();
    let mut lines = rest.lines();
    let mut closed = false;
    while let Some(line) = lines.next() {
        if line == delimiter {
            closed = true;
            trailer.extend(lines.by_ref());
            break;
        }
        body.push_str(line);
        body.push('\n');
    }
    if !closed {
        return Err("heredoc 分隔符未闭合".into());
    }
    // M221: the lines after the closing delimiter are ordinary shell, so they
    // must stay read-only. The field command ended with `git log --oneline -1`,
    // which used to be refused as "still shell content after the heredoc".
    // M266 R4: descriptor plumbing (`EOF 2>&1 | tail -25`) is folded away first,
    // exactly like the header already does, so it is not read as a command.
    let trailer = strip_fd_duplications(&trailer.join("\n"));
    for segment in split_top_level(&trailer, &[';', '|', '&', '\n']) {
        trailer_segment_is_allowed(base, segment)?;
    }
    // M221: `git commit -F -` / `git tag -F -` read their message from standard
    // input, so such a heredoc carries data — it is neither executed nor
    // resolved as a path. The pipeline spelling (`cat <<'MSG' | git commit -F -`)
    // is the same statement, so both are judged the same way.
    let pipeline_consumer = match consumers {
        [] => None,
        [only] => Some(*only),
        _ => {
            return Err(format!(
                "heredoc 之后不是可证明的单一消费者：{}",
                consumers.join(" | ")
            ))
        }
    };
    if let Some(verb) = git_data_heredoc_consumer(base, runner, pipeline_consumer)? {
        if base.is_none() {
            return Err(format!("没有工作区根目录，无法证明 {verb} 的数据 heredoc"));
        }
        for segment in prefix {
            prefix_segment_is_local(base, segment)?;
        }
        return Ok(());
    }
    for segment in prefix {
        prefix_segment_is_read_only(base, segment)?;
    }
    // M215 R2: a body naming no outside path, no network, no subprocess and no
    // delete call is workspace-contained. A `cat` body is data written into this
    // workspace, so only the paths it mentions are judged.
    if let Some(result) = cat_heredoc_runner(base, runner) {
        result?;
        if let Some(base) = base {
            heredoc_body_contained(base, &body, true)?;
        }
        return Ok(());
    }
    let node = matches!(runner, "node" | "node -");
    if !node && !matches!(runner, "python3 -" | "python -") {
        return Err(format!("不接受的解释器入口：{runner}"));
    }
    // The static AST proof stays the strong path; containment is only the
    // fallback for bodies that proof cannot model (e.g. `json.load`).
    let contained = match base {
        Some(base) if !sandboxed => {
            heredoc_body_contained(base, &body, false)?;
            true
        }
        _ => false,
    };
    let mut cmd = Command::new(if cfg!(windows) { "python" } else { "python3" }).no_window();
    cmd.args(["-I", "-c", include_str!("trusted_python_ast.py")]);
    if let Some(root) = base {
        cmd.arg(root);
    } else {
        cmd.arg("");
    }
    if node {
        cmd.arg("node");
    } else {
        cmd.arg("python");
    }
    if sandboxed {
        cmd.arg("sandboxed");
    }
    let Ok(mut child) = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return Err(format!(
            "无法启动{}证明进程",
            if node { " node " } else { " python " }
        ));
    };
    let Some(mut input) = child.stdin.take() else {
        return Err("无法写入证明脚本".into());
    };
    if input.write_all(body.as_bytes()).is_err() {
        return Err("证明脚本写入失败".into());
    }
    drop(input);
    let Ok(output) = child.wait_with_output() else {
        return Err("证明进程异常退出".into());
    };
    if output.status.success() {
        return Ok(());
    }
    // M215 R2: strict AST proof could not model this body, but containment held.
    if contained {
        return Ok(());
    }
    let reason = decode_prover_escapes(String::from_utf8_lossy(&output.stderr).trim());
    Err(if reason.is_empty() {
        "脚本包含无法静态证明的结构".into()
    } else {
        reason
    })
}

/// M215: prover diagnostics travel through the host console, and a Windows code
/// page that cannot encode CJK escapes them as `\uXXXX`. Decode those escapes so
/// the approval text names the line in readable Chinese on every platform.
/// M248b: filesystem scope belongs to the OS sandbox. Only existing effect
/// classifiers retain authority here: network writes and explicit shell risks.
/// No credential/path/command-word denylist is maintained in this layer.
pub(crate) fn outside_write_sandbox_scope(command: &str) -> bool {
    codefactory_agent_loop::policy::is_delivery_command(command)
        || !matches!(
            standalone_shell_policy::classify_command(command),
            standalone_shell_policy::ShellCommandPolicy::Allow { .. }
        )
}

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
                Some(&d) if d.is_ascii_hexdigit() => {
                    digits.push(d);
                    chars.next();
                }
                _ => break,
            }
        }
        match u32::from_str_radix(&digits, 16)
            .ok()
            .and_then(char::from_u32)
        {
            Some(decoded) if digits.len() == 4 => out.push(decoded),
            _ => {
                out.push('\\');
                out.push('u');
                out.push_str(&digits);
            }
        }
    }
    out
}

/// M215 R1: resolve the `cd` prefix of a heredoc header to a physical directory
/// and accept it when it is the workspace root or any directory inside it.
/// `~`, `$HOME`, relative paths, `..`, quotes and backslash escapes are
/// resolved; anything unresolvable or escaping the workspace stays unproved.
fn heredoc_header_prefix<'a>(
    header: &'a str,
    root: Option<&Path>,
) -> Result<(Option<std::path::PathBuf>, &'a str), String> {
    let root = root.map(canonical);
    let Some(rest) = header.strip_prefix("cd ") else {
        return Ok((root, header));
    };
    let mut split = None;
    for separator in [" && ", "; ", "\n"] {
        if let Some(parts) = rest.split_once(separator) {
            split = Some(parts);
            break;
        }
    }
    let Some((target, tail)) = split else {
        return Err("heredoc 头部的 cd 没有可证明的分隔符".into());
    };
    let target = target.trim();
    // `cd "$(pwd)"` re-enters the directory this session already runs in, so it
    // needs no root to prove: whatever the root is, it stays inside it.
    if matches!(
        target.trim_matches('"').trim_matches('\''),
        "$(pwd)" | "${PWD}" | "$PWD" | "`pwd`"
    ) {
        return Ok((root, tail.trim()));
    }
    let Some(root) = root else {
        return Err("没有工作区根目录，无法证明 heredoc 头部的 cd".into());
    };
    let Some(resolved) = physical(&root, target) else {
        return Err(format!("无法解析 heredoc 头部 cd 目标：{target}"));
    };
    if !within(&root, &resolved) {
        return Err(format!("heredoc 头部 cd 目标不是本工作区：{target}"));
    }
    Ok((Some(resolved), tail.trim()))
}

fn canonical(path: &Path) -> std::path::PathBuf {
    match path.canonicalize() {
        Ok(real) => strip_verbatim_prefix(&real),
        Err(_) => strip_verbatim_prefix(path),
    }
}

/// M215 R1 (Windows): `canonicalize` hands back verbatim paths (`\\?\C:\a`,
/// UNC as `\\?\UNC\srv\share`). They name the same directory as the plain form,
/// so comparisons and `cd` targets are reduced to the plain form first — a
/// workspace root compared against a verbatim temp directory otherwise looks
/// like a different place and the proof fails closed on a legitimate command.
fn plain_windows_text(text: &str) -> String {
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) => rest.to_string(),
        None => text.to_string(),
    }
}

fn strip_verbatim_prefix(path: &Path) -> std::path::PathBuf {
    std::path::PathBuf::from(plain_windows_text(&path.to_string_lossy()))
}

fn within(root: &Path, path: &Path) -> bool {
    let root = strip_verbatim_prefix(root);
    let path = strip_verbatim_prefix(path);
    path == root || path.starts_with(&root)
}

/// Shell word -> physical path. Quotes and backslash escapes are undone, `~`
/// and `$HOME` expand, and a path that does not exist yet keeps its canonical
/// parent (a heredoc may create the file). Anything dynamic is refused.
fn physical(base: &Path, raw: &str) -> Option<std::path::PathBuf> {
    let quoted = raw.trim().trim_matches('"').trim_matches('\'');
    if matches!(quoted, "$(pwd)" | "${PWD}" | "$PWD" | "`pwd`") {
        return Some(base.to_path_buf());
    }
    let text = shell_text(raw)?;
    let home = || {
        std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .ok()
            .map(std::path::PathBuf::from)
    };
    let expanded = if text == "~" || text == "$HOME" || text == "${HOME}" {
        home()?
    } else if let Some(rest) = text
        .strip_prefix("~/")
        .or_else(|| text.strip_prefix("$HOME/"))
        .or_else(|| text.strip_prefix("${HOME}/"))
    {
        home()?.join(rest)
    } else if text.starts_with('/') || windows_absolute(&text) {
        std::path::PathBuf::from(&text)
    } else {
        base.join(&text)
    };
    if let Ok(real) = expanded.canonicalize() {
        return Some(strip_verbatim_prefix(&real));
    }
    let parent = expanded.parent()?.canonicalize().ok()?;
    Some(strip_verbatim_prefix(&parent).join(expanded.file_name()?))
}

fn has_drive(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() > 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

/// `C:\...` (drive) and `\\?\...` / `\\server\share` (verbatim, UNC) are
/// absolute on Windows; they must not be joined onto a base directory.
fn windows_absolute(text: &str) -> bool {
    text.starts_with(r"\\") || has_drive(text)
}

/// Strip matching quotes and undo shell backslash escapes. `$(...)`, backticks
/// and environment references other than `$HOME`/`$PWD` are dynamic: refused.
fn shell_text(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let inner = if raw.len() >= 2
        && ((raw.starts_with('"') && raw.ends_with('"'))
            || (raw.starts_with('\'') && raw.ends_with('\'')))
    {
        &raw[1..raw.len() - 1]
    } else {
        raw
    };
    let mut out = String::with_capacity(inner.len());
    // A Windows path uses `\` as its separator, so undoing escapes there would
    // turn `\\?\C:\a\b` into `\?C:ab`. Such words keep their backslashes.
    if windows_absolute(inner) {
        out.push_str(inner);
    } else {
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                out.push(chars.next()?);
            } else {
                out.push(c);
            }
        }
    }
    let mut rest = out.clone();
    for token in ["${HOME}", "$HOME", "${PWD}", "$PWD"] {
        rest = rest.replace(token, "");
    }
    if rest.contains('$') || rest.contains('`') {
        return None;
    }
    Some(out)
}

const PROTECTED_PARTS: &[&str] = &[
    ".git",
    ".ssh",
    ".aws",
    ".gnupg",
    ".netrc",
    "credentials",
    "id_rsa",
    "id_ed25519",
];

fn protected_path(path: &Path) -> bool {
    path.components().any(|component| match component {
        Component::Normal(part) => {
            let part = part.to_string_lossy();
            PROTECTED_PARTS.contains(&part.as_ref())
                || part.starts_with(".env")
                || part.ends_with(".pem")
                || part.ends_with(".key")
        }
        _ => false,
    })
}

/// M215 R1: `cat >> <workspace file> <<'EOF'` is a data write, not a program.
/// Only the redirect target is judged; flags or extra arguments are refused so
/// the shape stays exactly what it looks like.
fn cat_heredoc_runner(base: Option<&Path>, runner: &str) -> Option<Result<(), String>> {
    let words: Vec<&str> = runner.split_whitespace().collect();
    let name = words
        .first()?
        .rsplit(['/', '\\'])
        .next()?
        .trim_end_matches(".exe");
    if name != "cat" {
        return None;
    }
    match &words[1..] {
        [] => Some(Ok(())),
        [op, path] if matches!(*op, ">" | ">>") => {
            let Some(base) = base else {
                return Some(Err("没有工作区根目录，无法证明 cat heredoc 目标".into()));
            };
            match physical(base, path) {
                Some(target) if within(base, &target) && !protected_path(&target) => Some(Ok(())),
                _ => Some(Err(format!("cat heredoc 目标不在本工作区：{path}"))),
            }
        }
        _ => Some(Err(format!("不接受的 cat heredoc 形式：{runner}"))),
    }
}

/// Capabilities a heredoc program may not hold: network, subprocess, deletion,
/// dynamic environment or reflection.
const HEREDOC_DANGER: &[&str] = &[
    "subprocess",
    "os.system",
    "os.popen",
    "os.exec",
    "os.spawn",
    "socket",
    "urllib",
    "requests",
    "httpx",
    "ftplib",
    "http://",
    "https://",
    "shutil",
    "rmtree",
    "os.remove",
    "os.unlink",
    "unlink(",
    "remove(",
    "child_process",
    "execsync",
    "spawnsync",
    "system(",
    "eval(",
    "exec(",
    "__import__",
    "ctypes",
    "environ",
    "getenv",
    "sys.argv",
    "globals(",
    "locals(",
    "rm -rf",
    "rm -fr",
    "git push",
    "git reset",
    "import os",
    "import sys",
];

/// M215 R2: a body that names no path outside the workspace and holds no
/// network, subprocess or delete capability is treated as workspace-contained.
/// Every path-looking token is resolved; when it cannot be proved the command
/// still asks for approval.
fn heredoc_body_contained(base: &Path, body: &str, data_only: bool) -> Result<(), String> {
    let lower = body.to_ascii_lowercase();
    let line_of = |needle: &str| {
        body.to_ascii_lowercase()
            .lines()
            .position(|l| l.contains(needle))
            .map_or(1, |i| i + 1)
    };
    if !data_only {
        if let Some(needle) = HEREDOC_DANGER
            .iter()
            .find(|needle| lower.contains(**needle))
        {
            return Err(format!(
                "heredoc 正文第 {} 行无法证明：包含越界能力 {needle}",
                line_of(*needle)
            ));
        }
    }
    for token in path_tokens(body) {
        let token = token.trim_matches('"').trim_matches('\'').to_string();
        if token.is_empty() || token.contains("://") {
            continue;
        }
        // `//`, `..` and punctuation runs are prose, not paths.
        if !token.chars().any(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        if !(token.contains('/') || token.contains('\\') || token.starts_with('~')) {
            continue;
        }
        let line = line_of(&token);
        match physical(base, &token) {
            Some(resolved) => {
                if protected_path(&resolved) {
                    return Err(format!(
                        "heredoc 正文第 {line} 行无法证明：涉及受保护路径 {token}"
                    ));
                }
                if !within(base, &resolved) {
                    return Err(format!(
                        "heredoc 正文第 {line} 行无法证明：路径在工作区外 {token}"
                    ));
                }
            }
            /* A relative path that does not exist yet cannot escape; anything
            absolute, expanding or climbing (`..`) must resolve or it stays
            unproved. */
            None if token.starts_with('/')
                || token.starts_with('~')
                || token.contains("..")
                || has_drive(&token) =>
            {
                return Err(format!(
                    "heredoc 正文第 {line} 行无法证明：路径无法解析 {token}"
                ));
            }
            None => {}
        }
    }
    Ok(())
}

/// M266 R4: `2>&1` and its relatives are descriptor plumbing, not commands. The
/// header folds them away already; the same tokens can follow the closing
/// delimiter (`EOF 2>&1 | tail -25`), where they used to be judged as a command
/// literally named `2>`. Only whole `[fd]>&[fd|-]` tokens are dropped, and lines
/// stay separate so a trailer never merges into one fake segment.
fn strip_fd_duplications(text: &str) -> String {
    text.lines()
        .map(|line| {
            line.split_whitespace()
                .filter(|token| {
                    let Some((left, right)) = token.split_once(">&") else {
                        return true;
                    };
                    !(left.chars().all(|c| c.is_ascii_digit())
                        && !right.is_empty()
                        && right.chars().all(|c| c.is_ascii_digit() || c == '-'))
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// M215 R1: split a header on top-level `;`/`|`/`&`/newline. Quotes keep their
/// content together, so a separator inside `'PY'` or `"a;b"` does not split.
fn split_top_level<'a>(input: &'a str, separators: &[char]) -> Vec<&'a str> {
    let mut parts = Vec::new();
    let mut quote: Option<char> = None;
    let mut start = 0usize;
    for (offset, ch) in input.char_indices() {
        if let Some(opening) = quote {
            if ch == opening {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        if separators.contains(&ch) {
            let part = input[start..offset].trim();
            if !part.is_empty() {
                parts.push(part);
            }
            start = offset + ch.len_utf8();
        }
    }
    let tail = input[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts
}

/// Commands that may run before a heredoc without changing the proof: they only
/// read. Anything else keeps the whole command unproved.
const READ_ONLY_PREFIX: &[&str] = &[
    "echo", "printf", "pwd", "ls", "cat", "head", "tail", "wc", "grep", "egrep", "fgrep", "sed",
    "cut", "sort", "uniq", "tr", "nl", "basename", "dirname", "realpath", "stat", "file", "du",
    "df", "date", "diff", "comm", "test", "[", "true", "false", "column", "jq",
];

/// M215 R1: one command placed before the heredoc. It may read files, query Git
/// or re-enter this workspace; anything that writes, deletes or leaves the
/// workspace keeps the approval gate.
fn prefix_segment_is_read_only(base: Option<&Path>, segment: &str) -> Result<(), String> {
    let Some((name, args)) = segment_command(segment) else {
        return Ok(());
    };
    let args = args.as_slice();
    if args.iter().any(|arg| arg.starts_with('>')) {
        return Err(format!("heredoc 之前的命令带有输出重定向：{segment}"));
    }
    if name == "cd" {
        let Some(target) = args.first() else {
            return Err(format!("heredoc 之前的 cd 没有目标：{segment}"));
        };
        let inside = match base {
            Some(base) => physical(base, target).is_some_and(|resolved| within(base, &resolved)),
            None => matches!(*target, "$(pwd)" | "${PWD}" | "$PWD" | "`pwd`"),
        };
        return inside
            .then_some(())
            .ok_or_else(|| format!("heredoc 之前的 cd 目标不可证明：{segment}"));
    }
    if name == "git" {
        let subcommand = args.iter().position(|arg| !arg.starts_with('-'));
        let read_only = subcommand.is_some_and(|at| {
            let rest: Vec<String> = args[at + 1..].iter().map(|arg| arg.to_string()).collect();
            read_only_git_verb(&args[at].to_ascii_lowercase(), &rest)
        });
        return read_only
            .then_some(())
            .ok_or_else(|| format!("heredoc 之前的 Git 不是只读：{segment}"));
    }
    if name == "sed"
        && args
            .iter()
            .any(|arg| arg.starts_with("-i") || arg.starts_with("--in-place"))
    {
        return Err(format!("heredoc 之前的 sed 会写文件：{segment}"));
    }
    if READ_ONLY_PREFIX.contains(&name.as_str()) {
        return Ok(());
    }
    Err(format!("heredoc 之前不是只读命令：{segment}"))
}

/// M221: a segment's binary name and its arguments, skipping the leading
/// `NAME=value` environment assignments the read-only rule already allowed.
fn segment_command(segment: &str) -> Option<(String, Vec<&str>)> {
    let words: Vec<&str> = segment.split_whitespace().collect();
    let mut index = 0;
    while index < words.len() {
        let (name, _) = words[index].split_once('=').unwrap_or((words[index], ""));
        if words[index].contains('=')
            && !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            index += 1;
        } else {
            break;
        }
    }
    let binary = *words.get(index)?;
    let name = binary
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(binary)
        .trim_end_matches(".exe")
        .to_ascii_lowercase();
    Some((name, words[index + 1..].to_vec()))
}

/// M221: parse Git's global arguments down to the subcommand. A `-C` target has
/// to stay inside this workspace; `--git-dir` / `--work-tree` / `--namespace` /
/// `--exec-path` can point the command at another repository and stay unproved.
fn git_subcommand<'a>(
    base: Option<&Path>,
    args: &[&'a str],
) -> Result<(&'a str, Vec<&'a str>), String> {
    let mut index = 0;
    while index < args.len() {
        let arg = args[index];
        match arg {
            "-C" => {
                let Some(target) = args.get(index + 1) else {
                    return Err("Git -C 缺少目录".into());
                };
                let Some(base) = base else {
                    return Err("没有工作区根目录，无法证明 Git -C 目标".into());
                };
                match physical(base, target) {
                    Some(resolved) if within(base, &resolved) => {}
                    _ => return Err(format!("Git -C 目标不在本工作区：{target}")),
                }
                index += 2;
            }
            "-c" => {
                if args.get(index + 1).is_none() {
                    return Err("Git -c 缺少配置项".into());
                }
                index += 2;
            }
            _ if arg.starts_with("--git-dir")
                || arg.starts_with("--work-tree")
                || arg.starts_with("--namespace")
                || arg.starts_with("--exec-path") =>
            {
                return Err(format!("不允许的 Git 全局参数：{arg}"));
            }
            _ if arg.starts_with('-') => index += 1,
            _ => return Ok((arg, args[index + 1..].to_vec())),
        }
    }
    Err("Git 命令缺少子命令".into())
}

/// M221: verbs that only write this workspace's own history, so the session's
/// write permission already covers them once the target is proven local.
const LOCAL_GIT_WRITE: &[&str] = &["add", "commit", "tag"];

/// M221: does this Git segment read its message text from standard input?
fn reads_message_from_stdin(args: &[&str]) -> bool {
    let mut index = 0;
    while index < args.len() {
        match args[index] {
            "-F" | "--file" => return args.get(index + 1) == Some(&"-"),
            "--file=-" | "-F-" => return true,
            _ => {}
        }
        index += 1;
    }
    false
}

fn data_heredoc_verb(base: Option<&Path>, segment: &str) -> Result<Option<String>, String> {
    let Some((name, args)) = segment_command(segment) else {
        return Ok(None);
    };
    if name != "git" {
        return Ok(None);
    }
    let (verb, rest) = git_subcommand(base, &args)?;
    let verb = verb.to_ascii_lowercase();
    if !matches!(verb.as_str(), "commit" | "tag") || !reads_message_from_stdin(&rest) {
        return Ok(None);
    }
    Ok(Some(verb))
}

/// M221: `git commit -F -` / `git tag -F -` turn a heredoc into message data.
/// `cat <<'MSG' | git commit -F -` is the same statement written as a pipeline,
/// so both spellings are accepted — and nothing else is.
fn git_data_heredoc_consumer(
    base: Option<&Path>,
    runner: &str,
    pipeline: Option<&str>,
) -> Result<Option<String>, String> {
    match (data_heredoc_verb(base, runner)?, pipeline) {
        (Some(verb), None) => Ok(Some(verb)),
        // M266 R4: `git commit -F - <<'EOF' 2>&1 | tail -25` pipes the command's
        // *output* through a read-only filter. The heredoc still owns the input,
        // so this is not a second data consumer — but anything that writes or
        // leaves the workspace is still refused.
        (Some(verb), Some(segment)) => match prefix_segment_is_read_only(base, segment) {
            Ok(()) => Ok(Some(verb)),
            Err(_) => Err("heredoc 前后各有一个数据消费者".into()),
        },
        (None, Some(segment)) => match cat_heredoc_runner(base, runner) {
            Some(Ok(())) => data_heredoc_verb(base, segment)?
                .map(Some)
                .ok_or_else(|| format!("heredoc 之后的命令不是可证明的数据消费者：{segment}")),
            _ => Err(format!("不接受的 heredoc 生产者：{runner}")),
        },
        (None, None) => Ok(None),
    }
}

/// M221: what may follow the closing delimiter. Read-only commands and local Git
/// writes are covered by this proof itself; a recognised remote write is handed
/// to its own authority (delivery + ownership, M208/M130), so a plain
/// `&& git push origin HEAD` after the message needs no second proof here.
/// Destructive remote shapes (force, mirror, `--tags`, a `src:dst` refspec to
/// main) are not recognised by `own_remote_actions`, so they still ask.
fn trailer_segment_is_allowed(base: Option<&Path>, segment: &str) -> Result<(), String> {
    if segment.trim().is_empty() {
        return Ok(());
    }
    if own_remote_actions(segment).is_some() {
        return Ok(());
    }
    prefix_segment_is_local(base, segment).map_err(|reason| format!("heredoc 之后：{reason}"))
}

/// M221: the segments in front of a data heredoc may also be local Git writes
/// (`git add -A && git commit -F - <<'MSG'`). Reading a message from standard
/// input there is refused: only the heredoc owns that input.
fn prefix_segment_is_local(base: Option<&Path>, segment: &str) -> Result<(), String> {
    let Some((name, args)) = segment_command(segment) else {
        return Ok(());
    };
    if name == "git" {
        if let Ok((verb, rest)) = git_subcommand(base, &args) {
            let verb = verb.to_ascii_lowercase();
            if LOCAL_GIT_WRITE.contains(&verb.as_str()) {
                if rest
                    .iter()
                    .any(|arg| matches!(*arg, "-F" | "-F-" | "--file" | "--file=-"))
                {
                    return Err(format!(
                        "heredoc 之前的 Git 也从标准输入读取说明：{segment}"
                    ));
                }
                if base.is_none() {
                    return Err(format!(
                        "没有工作区根目录，无法证明本地 Git 写入：{segment}"
                    ));
                }
                return Ok(());
            }
        }
    }
    prefix_segment_is_read_only(base, segment)
}

/// Path-looking words of a body. Quotes and shell punctuation are separators,
/// so `/Users/leo/.ssh` and `"../x"` both survive as single tokens.
fn path_tokens(body: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in body.chars() {
        let keep = ch.is_ascii_alphanumeric()
            || matches!(
                ch,
                '/' | '\\'
                    | '.'
                    | '-'
                    | '_'
                    | '~'
                    | '$'
                    | '{'
                    | '}'
                    | ':'
                    | '"'
                    | '\''
                    | '@'
                    | '+'
                    | '%'
            );
        if keep {
            current.push(ch);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Git subcommands that move the working tree, the index or a local branch.
const DESTRUCTIVE_GIT_SUBCOMMANDS: &[&str] = &[
    "reset",
    "restore",
    "checkout",
    "clean",
    "stash",
    "rebase",
    "merge",
    "switch",
    "add",
    "commit",
    "config",
    "filter-branch",
    "filter-repo",
    "worktree",
    "cherry-pick",
    "revert",
    "am",
    "apply",
];

/// Words that only wrap another command.
const WRAPPER_WORDS: &[&str] = &["sudo", "command", "exec", "env", "nohup", "time", "builtin"];

/// Interpreters that take inline shell code and therefore must be re-scanned.
const SHELL_INTERPRETERS: &[&str] = &["bash", "sh", "zsh", "dash", "ksh", "ash"];

/// Interpreters that take inline code in another language: their payload is
/// data unless it visibly shells out to a destructive Git command.
const INLINE_CODE_INTERPRETERS: &[&str] = &[
    "node", "nodejs", "deno", "bun", "python", "python3", "perl", "ruby",
];

/// M260: structures are unfolded first (`trusted_shell_structure`), then every
/// simple command is judged on its own. `None` means "cannot be enumerated",
/// which every caller already treats as sensitive / keep the snapshot.
fn executable_segments(command: &str) -> Option<Vec<Vec<String>>> {
    trusted_shell_structure::executable_segments(command)
}

fn segment_is_sensitive(words: &[String]) -> bool {
    let Some((binary, args)) = segment_command_words(words) else {
        return false;
    };
    // M266 R2: only the command word can expand into a program. A `$` inside an
    // argument — a search pattern, a path, a variable name — is data, and a word
    // holding an already-unfolded `$(...)` is a placeholder, not a live expansion.
    if binary.contains('$') || binary.contains('`') {
        return true;
    }
    let name = command_name(&binary);
    if name == "git" {
        return git_segment_is_sensitive(args);
    }
    if SHELL_INTERPRETERS.contains(&name.as_str()) {
        return inline_payload(args, &["-c", "-lc", "-ec", "-ic"])
            .is_some_and(|payload| sensitive(&payload));
    }
    if INLINE_CODE_INTERPRETERS.contains(&name.as_str()) {
        return inline_payload(args, &["-e", "-c", "-p", "--eval", "--print"])
            .is_some_and(|payload| payload_shells_out_to_destructive_git(&payload));
    }
    // `cargo clean` deletes build output: previously caught by the "clean"
    // substring, so keep it explicit rather than losing the coverage.
    name == "cargo" && args.iter().any(|arg| arg.eq_ignore_ascii_case("clean"))
}

/// M266 R2: the command word of a segment, after the leading `NAME=value`
/// assignments and wrapper words (`sudo`, `env`, …). Everything after it is an
/// argument, never a program.
fn segment_command_words(words: &[String]) -> Option<(String, &[String])> {
    let mut start = 0;
    while start < words.len() {
        let word = words[start].as_str();
        let assignment = word.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        if assignment || WRAPPER_WORDS.contains(&word.to_ascii_lowercase().as_str()) {
            start += 1;
        } else {
            break;
        }
    }
    words
        .get(start)
        .map(|binary| (binary.clone(), &words[start + 1..]))
}

fn command_name(binary: &str) -> String {
    binary
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(binary)
        .trim_end_matches(".exe")
        .to_ascii_lowercase()
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
        if matches!(
            lower.as_str(),
            "--git-dir" | "--work-tree" | "--namespace" | "--exec-path"
        ) {
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
    let Some(subcommand) = subcommand else {
        return false;
    };
    let lower = subcommand.to_ascii_lowercase();
    // M215 R3: a query verb that can execute a program or write a file is a
    // write, not a read.
    if git_runs_program(&args[index + 1..]) {
        return true;
    }
    // M215 R3: a pure read-only Git invocation needs no workspace-root proof,
    // no registration and no snapshot — wherever it runs. A write verb in the
    // same segment still falls through to the original rules below.
    if read_only_git_verb(&lower, &args[index + 1..]) {
        return false;
    }
    // M180: `worktree list` reads registrations; it does not move or discard
    // work. Only the listing verb is exempt: remove/prune/add/move/lock still
    // require the session-owned proof and snapshot.
    if lower == "worktree" && args.get(index + 1).is_some_and(|a| a == "list") {
        return false;
    }
    if lower == "branch" {
        return !codefactory_agent_core::command_segment_is_read_only_probe(
            &std::iter::once("git")
                .chain(args[index..].iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    if lower == "fetch" {
        return !fetch_args(
            &args[index + 1..]
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
    }
    if DESTRUCTIVE_GIT_SUBCOMMANDS.contains(&lower.as_str()) {
        return true;
    }
    if lower == "push" {
        return args[index + 1..].iter().any(|arg| {
            arg.starts_with("--force")
                || ["--delete", "--mirror", "--prune", "-f", "-d"].contains(&arg.as_str())
                || arg.starts_with('+')
                || arg.starts_with(':')
        });
    }
    false
}

/// M215 R3: options that turn a Git query into program execution or a file
/// write. These keep the workspace-root proof and the snapshot.
fn git_runs_program(rest: &[String]) -> bool {
    rest.iter().any(|arg| {
        let option = arg.split('=').next().unwrap_or(arg);
        [
            "--ext-diff",
            "--textconv",
            "--upload-pack",
            "--receive-pack",
            "--exec",
            "--exec-path",
        ]
        .contains(&option)
    })
}

/// M215 R3: Git verbs that only read local state or a remote. Anything that can
/// execute a program, write a file or rewrite config is excluded here even when
/// the verb is normally a query (`--ext-diff`, `--textconv`, `--output`).
fn read_only_git_verb(subcommand: &str, rest: &[String]) -> bool {
    let lower: Vec<String> = rest.iter().map(|arg| arg.to_ascii_lowercase()).collect();
    if lower.iter().any(|arg| {
        let option = arg.split('=').next().unwrap_or(arg);
        [
            "--ext-diff",
            "--textconv",
            "--output",
            "--output-directory",
            "--upload-pack",
            "--receive-pack",
            "--exec",
            "--exec-path",
        ]
        .contains(&option)
    }) {
        return false;
    }
    match subcommand {
        "status" | "log" | "show" | "diff" | "rev-parse" | "describe" | "ls-files" | "grep"
        | "cat-file" | "merge-base" | "ls-remote" | "blame" | "shortlog" | "for-each-ref"
        | "show-ref" | "rev-list" | "check-ignore" | "count-objects" | "var" | "version"
        | "help" | "name-rev" | "diff-tree" | "diff-index" | "diff-files" | "symbolic-ref"
        | "whatchanged" | "cherry" | "merge-tree" => true,
        "fetch" => fetch_args(&rest.iter().map(String::as_str).collect::<Vec<_>>()),
        "branch" => branch_is_read_only(&lower),
        "remote" => lower
            .iter()
            .all(|arg| matches!(arg.as_str(), "-v" | "--verbose" | "show")),
        "stash" => matches!(lower.first().map(String::as_str), Some("list" | "show")),
        "worktree" => matches!(lower.first().map(String::as_str), Some("list")),
        "reflog" => matches!(
            lower.first().map(String::as_str),
            Some("show" | "list" | "exists")
        ),
        "config" => {
            lower.iter().any(|arg| arg == "core.hookspath")
                && lower.iter().filter(|arg| !arg.starts_with('-')).count() == 1
        }
        _ => false,
    }
}

/// `git branch` is a listing only with a query flag or no arguments at all;
/// a bare positional argument names a branch to create, which is a write.
fn branch_is_read_only(args: &[String]) -> bool {
    const WRITE_FLAGS: &[&str] = &[
        "-d",
        "-D",
        "-m",
        "-M",
        "-c",
        "-C",
        "-f",
        "--force",
        "-u",
        "--set-upstream-to",
        "--unset-upstream",
        "--edit-description",
        "--delete",
        "--move",
        "--copy",
        "--create",
        "--track",
        "--no-track",
    ];
    if args.iter().any(|arg| WRITE_FLAGS.contains(&arg.as_str())) {
        return false;
    }
    args.is_empty() || args.iter().any(|arg| arg.starts_with('-'))
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
        "execsync",
        "spawnsync",
        "exec(",
        "spawn(",
        "subprocess",
        "os.system",
        "popen",
        "child_process",
        "system(",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    shells_out
        && [
            "git reset",
            "git clean",
            "git checkout",
            "git restore",
            "git stash",
            "git rebase",
            "git merge",
            "git worktree",
            "git push",
            "git -c",
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
        assert!(
            !sensitive(READ_ONLY_EVIDENCE_COMMAND),
            "read-only evidence command must not ask"
        );
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
        assert!(
            !sensitive(script),
            "quoted script data must not trigger the git snapshot"
        );
    }

    #[test]
    fn m172_local_interpreter_and_build_commands_are_not_sensitive() {
        assert!(!sensitive("python3 -"));
        assert!(!sensitive(
            "cargo test --manifest-path src-tauri/Cargo.toml"
        ));
        assert!(!sensitive("pnpm test"));
        assert!(!sensitive("node scripts/run.mjs --dry-run"));
        assert!(
            !sensitive("git push origin main"),
            "ordinary push uses the separate session delivery approval, never local proof"
        );
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
    let out = crate::util::host_git::command()
        .no_window()
        .current_dir(root)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}
fn inside(root: &Path, raw: &str) -> bool {
    let path = Path::new(raw);
    if raw.is_empty() || raw.contains(':') || path.is_absolute() {
        return false;
    }
    for c in path.components() {
        match c {
            Component::CurDir => (),
            Component::Normal(n) if n != ".git" => (),
            _ => return false,
        }
    }
    let mut probe = root.join(path);
    loop {
        if let Ok(real) = probe.canonicalize() {
            return real.starts_with(root);
        }
        if std::fs::symlink_metadata(&probe).is_ok() {
            return false;
        }
        match probe.parent() {
            Some(p) if p.starts_with(root) => probe = p.into(),
            _ => return false,
        }
    }
}
fn parts(command: &str) -> Option<Vec<Vec<&str>>> {
    if command.contains(['\n', '\r']) {
        return None;
    }
    let parts: Vec<Vec<&str>> = command
        .split("&&")
        .map(|s| s.split_whitespace().collect())
        .collect();
    if parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    for p in &parts {
        for word in p {
            if word
                .chars()
                .any(|c| !(c.is_ascii_alphanumeric() || "-_/ .".contains(c)))
            {
                return None;
            }
        }
    }
    Some(parts)
}
fn legacy_rewrite(root: &Path, command: &str) -> bool {
    let Some(root) = root.canonicalize().ok() else {
        return false;
    };
    // No parent repository discovery, submodule effects, or unmerged index.
    if git(&root, &["rev-parse", "--show-toplevel"])
        .and_then(|p| Path::new(&p).canonicalize().ok())
        .as_ref()
        != Some(&root)
    {
        return false;
    }
    if git(&root, &["ls-files", "--stage"])
        .is_none_or(|s| s.lines().any(|l| l.starts_with("160000 ")))
    {
        return false;
    }
    let Some(parts) = parts(command) else {
        return false;
    };
    let mut has_git = false;
    for argv in parts {
        let paths: &[&str] = match argv.as_slice() {
            ["git", "reset", "--hard", revision] => {
                has_git = true;
                if revision.starts_with('-') || revision.contains('/') {
                    return false;
                }
                let Some(target) = git(
                    &root,
                    &["rev-parse", "--verify", &format!("{revision}^{{commit}}")],
                ) else {
                    return false;
                };
                let Some(removed) = git(&root, &["rev-list", &format!("{target}..HEAD")]) else {
                    return false;
                };
                for commit in removed.lines() {
                    if git(&root, &["branch", "-r", "--contains", commit])
                        .is_none_or(|s| !s.is_empty())
                    {
                        return false;
                    }
                }
                continue;
            }
            ["git", "checkout", "--", rest @ ..] => {
                has_git = true;
                rest
            }
            ["git", "restore", "--staged", rest @ ..] | ["git", "restore", "--", rest @ ..] => {
                has_git = true;
                rest
            }
            ["git", "clean", flags, rest @ ..]
                if ["-f", "-fq", "-qf", "-fd", "-fdq"].contains(flags) =>
            {
                has_git = true;
                rest
            }
            ["git", "stash"] | ["git", "stash", "push"] => {
                has_git = true;
                continue;
            }
            ["rm", "-rf", rest @ ..] => rest,
            _ => return false,
        };
        if paths.is_empty()
            || paths
                .iter()
                .any(|p| p.starts_with('-') || !inside(&root, p))
        {
            return false;
        }
        // Do not allow shell deletion of the workspace root itself.
        if argv[0] == "rm"
            && paths
                .iter()
                .any(|p| root.join(p).canonicalize().ok().as_ref() == Some(&root))
        {
            return false;
        }
    }
    has_git
}

/// A deliberately small shell grammar: no evaluation, globbing, redirection,
/// subshells or inherited variables. Quotes retain literal vs expansion meaning.
#[derive(Default)]
struct Word {
    text: String,
    expanded: bool,
    redirect: bool,
}
struct Segment {
    words: Vec<Word>,
    separator: char,
    raw: String,
}

/// Lossless segment text for the permission gateway: quoted script data is
/// never reconstructed or reinterpreted as shell operators.
pub(super) fn command_segments(command: &str) -> Option<Vec<String>> {
    Some(
        segments(command)?
            .into_iter()
            .map(|segment| segment.raw)
            .collect(),
    )
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
        if matches!(c, '\n' | '\r' | '`') {
            return None;
        }
        if c == '\\' {
            // A backslash is a path separator on Windows and survives inside
            // quotes, so a quoted Windows worktree path must stay literal.
            // Outside quotes it is an escape or line continuation; inside double
            // quotes the escapes a shell would still act on are refused.
            if quote.is_none() {
                return None;
            }
            if quote == Some('"')
                && matches!(chars.peek(), Some('"') | Some('\\') | Some('$') | Some('`'))
            {
                return None;
            }
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
            while chars
                .peek()
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
            {
                name.push(chars.next()?);
            }
            let value = variables.get(&name)?;
            if quote.is_none()
                && value
                    .chars()
                    .any(|c| c.is_whitespace() || "*?[]".contains(c))
            {
                return None;
            }
            word.text.push_str(value);
            word.expanded = true;
            started = true;
            continue;
        }
        if quote.is_none() && (c.is_whitespace() || matches!(c, ';' | '&' | '|')) {
            if started {
                words.push(std::mem::take(&mut word));
                started = false;
            }
            if c.is_whitespace() {
                continue;
            }
            let separator = match c {
                '&' if chars.next() == Some('&') => '&',
                '&' => return None,
                '|' if chars.peek() == Some(&'|') => {
                    chars.next();
                    'o'
                }
                other => other,
            };
            if words.is_empty() {
                return None;
            }
            record_assignment(&words, &mut variables)?;
            let end = command.len() - chars.clone().map(char::len_utf8).sum::<usize>();
            let delimiter_len = if matches!(separator, '&' | 'o') { 2 } else { 1 };
            result.push(Segment {
                words: std::mem::take(&mut words),
                separator,
                raw: command[raw_start..end - delimiter_len].trim().into(),
            });
            raw_start = end;
            continue;
        }
        if quote.is_none() && c == '>' {
            // Keep redirects as explicit tokens; never execute or erase their target.
            if started {
                if word.text == "2" || word.text == "1" {
                    word = Word::default();
                } else {
                    words.push(std::mem::take(&mut word));
                }
                started = false;
            }
            if chars.peek() == Some(&'&') {
                chars.next();
                if chars.next() != Some('1') {
                    return None;
                }
                words.push(Word {
                    text: "@stderr-to-stdout".into(),
                    expanded: false,
                    redirect: true,
                });
            } else {
                if chars.peek() == Some(&'>') {
                    chars.next();
                }
                words.push(Word {
                    text: "@output".into(),
                    expanded: false,
                    redirect: true,
                });
            }
            continue;
        }
        if quote.is_none() && "<()[{}]*?!#~".contains(c) {
            return None;
        }
        word.text.push(c);
        started = true;
    }
    if quote.is_some() {
        return None;
    }
    if started {
        words.push(word);
    }
    if words.is_empty() {
        return None;
    }
    record_assignment(&words, &mut variables)?;
    result.push(Segment {
        words,
        separator: ';',
        raw: command[raw_start..].trim().into(),
    });
    Some(result)
}
fn record_assignment(
    words: &[Word],
    variables: &mut std::collections::HashMap<String, String>,
) -> Option<()> {
    if words.len() == 1 {
        if let Some((name, value)) = words[0].text.split_once('=') {
            // Never let a literal assignment change shell/Git execution semantics.
            if words[0].expanded
                || name.is_empty()
                || name.starts_with("GIT_")
                || [
                    "PATH",
                    "IFS",
                    "ENV",
                    "BASH_ENV",
                    "CDPATH",
                    "HOME",
                    "SHELLOPTS",
                    "BASHOPTS",
                    "GLOBIGNORE",
                    "PROMPT_COMMAND",
                    "PS4",
                ]
                .contains(&name)
                || !name.chars().enumerate().all(|(i, c)| {
                    c == '_' || c.is_ascii_alphabetic() || i > 0 && c.is_ascii_digit()
                })
            {
                return None;
            }
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
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./@{}^~=".contains(c))
}
fn inside_or_here(root: &Path, word: &str) -> bool {
    word == "." && root.is_dir() || inside(root, word)
}

/// M177: the grammar of a provable local rewrite. Every accepted verb keeps the
/// M159 snapshot meaningful: reachable from the workspace, non-interactive and
/// recoverable from the private-index backup.
fn local_rewrite_args(root: &Path, args: &[&str]) -> bool {
    let Some((verb, rest)) = args.split_first() else {
        return false;
    };
    match *verb {
        // Network read: updates remote-tracking refs only.
        "fetch" => rest.iter().all(|w| {
            [
                "--prune",
                "--all",
                "--tags",
                "--no-tags",
                "-q",
                "--quiet",
                "--force",
            ]
            .contains(w)
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
    let Some((sub, tail)) = rest.split_first() else {
        return true;
    }; // bare `git stash` lists
    match *sub {
        "list" | "show" => true,
        // `push`/`save` keep the work: the snapshot plus the stash itself.
        "push" | "save" => stash_push_args(root, tail),
        "pop" | "apply" => tail
            .iter()
            .all(|w| ["-q", "--quiet", "--index"].contains(w) || revision(w)),
        // `drop`, `clear`, `branch` and `create` discard the only remaining copy.
        _ => false,
    }
}

fn stash_push_args(root: &Path, tail: &[&str]) -> bool {
    let mut index = 0;
    while index < tail.len() {
        let word = tail[index];
        match word {
            "-u"
            | "--include-untracked"
            | "-a"
            | "--all"
            | "-q"
            | "--quiet"
            | "--keep-index"
            | "--no-keep-index"
            | "--" => (),
            "-m" | "--message" => {
                index += 1;
                if index >= tail.len() {
                    return false;
                }
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
            "--no-edit"
            | "--no-ff"
            | "--ff-only"
            | "--squash"
            | "--no-commit"
            | "--allow-unrelated-histories"
            | "--autostash"
            | "--no-stat"
            | "--no-autostash"
            | "-q"
            | "--quiet" => (),
            "-m" | "--message" => {
                index += 1;
                if index >= rest.len() {
                    return false;
                }
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
            _ if word.starts_with("-i")
                || *word == "--interactive"
                || word.starts_with("--exec")
                || *word == "-x"
                || word.starts_with("-x") =>
            {
                return false
            }
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
                if index >= rest.len() || !revision(rest[index]) {
                    return false;
                }
                named = true;
            }
            "-q" | "--quiet" | "-f" | "--force" | "-m" | "--merge" => (),
            _ if word.starts_with('-') => return false,
            _ if pathspec => {
                if !inside_or_here(root, word) {
                    return false;
                }
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
                if index >= rest.len() || !revision(rest[index]) {
                    return false;
                }
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
                if !word.chars().skip(1).all(|c| "fdnq".contains(c)) {
                    return false;
                }
                forced |= word.contains('f');
            }
            _ => {
                if root.join(word).canonicalize().is_ok_and(|p| p == root) {
                    return false;
                }
                if !inside_or_here(root, word) {
                    return false;
                }
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
            "-a"
            | "--all"
            | "--amend"
            | "--no-edit"
            | "--allow-empty"
            | "--allow-empty-message"
            | "--no-verify"
            | "-q"
            | "--quiet"
            | "-n" => (),
            "-m" | "--message" | "-am" | "-ma" => {
                index += 1;
                if index >= rest.len() {
                    return false;
                }
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
    if lower.split('/').any(|p| {
        p == ".git"
            || p == ".ssh"
            || p == ".aws"
            || p == ".gnupg"
            || p == ".env"
            || p.starts_with(".env.")
            || p == "credentials"
            || p == "id_rsa"
            || p == "id_ed25519"
            || p.ends_with(".pem")
            || p.ends_with(".key")
    }) {
        return false;
    }
    inside(root, &normalized)
}

fn bounded_args(root: &Path, args: &[&str]) -> bool {
    args.iter().all(|word| {
        let value = word.split_once('=').map_or(*word, |(_, value)| value);
        if value.contains("..")
            || value.contains(':')
            || value.starts_with('/')
            || value.starts_with('\\')
        {
            return false;
        }
        if !word.starts_with('-') || word.contains('=') {
            if !safe_path(root, value) {
                return false;
            }
        }
        if value.contains('/') || value.contains('\\') || root.join(value).exists() {
            safe_path(root, value)
        } else {
            true
        }
    })
}

fn local_denylist(root: &Path, args: &[&str]) -> bool {
    let Some((verb, rest)) = args.split_first() else {
        return false;
    };
    // These verbs affect remote authority, command execution or configuration.
    // M198: repository-mandated workspace-local hook setup is not a remote
    // credential/protection rewrite. Exact shape only; --global stays fenced.
    if *verb == "config" {
        return matches!(
            rest,
            ["core.hooksPath", ".githooks"] | ["--local", "core.hooksPath", ".githooks"]
        );
    }
    if [
        "push",
        "filter-branch",
        "filter-repo",
        "submodule",
        "send-email",
        "credential",
    ]
    .contains(verb)
    {
        return false;
    }
    if rest.iter().any(|w| {
        let option = w.split('=').next().unwrap_or(w);
        [
            "--interactive",
            "--patch",
            "--edit",
            "--exec",
            "--editor",
            "--upload-pack",
            "--receive-pack",
            "--ext-diff",
            "--textconv",
            "--output",
            "--output-directory",
            "--git-dir",
            "--work-tree",
            "--unsafe-paths",
            "--pathspec-from-file",
            "--recurse-submodules",
            "--separate-git-dir",
            "--reference",
            "--alternate-refs-command",
        ]
        .contains(&option)
            || option.starts_with("--inter")
            || option.starts_with("--exe")
            || ["-i", "-p", "-e"].contains(&option)
            || *verb == "rebase" && (w.starts_with("-i") || w.starts_with("-x"))
    }) {
        return false;
    }
    if *verb == "clean"
        && rest
            .iter()
            .any(|w| w.starts_with('-') && (w.contains('x') || w.contains('X') || w.contains('i')))
    {
        return false;
    }
    // Preserve older extra-cautious stash policy: dropping the sole stash is
    // not recoverable by a working-tree snapshot.
    if *verb == "stash" && rest.first().is_some_and(|w| ["drop", "clear"].contains(w)) {
        return false;
    }
    let mut message = false;
    let mut checked = Vec::new();
    for word in rest {
        if message {
            message = false;
            continue;
        }
        if ["-m", "--message", "-am", "-ma"].contains(word) {
            message = true;
            continue;
        }
        if word.starts_with("--message=") {
            continue;
        }
        checked.push(*word);
    }
    !message && bounded_args(root, &checked)
}

/// Fetch reads a remote, unlike push. Ref specs and URLs are not local paths.
/// Keep executable transport overrides and external remote helpers fenced.
fn fetch_args(rest: &[&str]) -> bool {
    rest.iter().all(|word| {
        if word.starts_with('-') {
            [
                "--prune",
                "--all",
                "--tags",
                "--no-tags",
                "--prune-tags",
                "--no-recurse-submodules",
                "--force",
                "-f",
                "--quiet",
                "-q",
                "--verbose",
                "-v",
                "--dry-run",
            ]
            .contains(word)
        } else {
            !word.is_empty()
                && !word.contains("::")
                && !word.starts_with('|')
                && !word.chars().any(|c| c.is_control() || c.is_whitespace())
        }
    })
}

/// M208: a remote write this session may own. Ownership (which PR, which branch,
/// whether that PR is merged) is proved against the durable delivery records by
/// the caller — this type only carries what the command itself names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum OwnRemoteAction {
    /// A description-level write (`comment`, `ready`, `edit`). `body_files` are
    /// the workspace-relative files read through `--body-file`; the shape prover
    /// already refused anything that could leave the workspace, and the caller
    /// keeps that containment as part of the ownership decision.
    PrMetadata {
        number: i64,
        body_files: Vec<String>,
    },
    PushBranch(String),
    DeleteBranch(String),
}

impl OwnRemoteAction {
    /// M208/M257: a PR description write is answered by a plain confirmation when
    /// it is not owned, never by the remote-write allowance.
    pub(super) fn is_pr_metadata(&self) -> bool {
        matches!(self, Self::PrMetadata { .. })
    }
}

/// M208: the remote writes a command performs on the session's *own* resources.
///
/// Judged segment by segment (M207 principle): every segment is either an own
/// remote write this prover recognises, or an ordinary command that already
/// passes the standalone shell decision on its own. One segment that cannot be
/// placed takes the whole command back to the normal gate, so a mixed command
/// can never launder a foreign write through an owned one.
pub(super) fn own_remote_actions(command: &str) -> Option<Vec<OwnRemoteAction>> {
    let segments = segments(command)?;
    let mut actions = Vec::new();
    for segment in &segments {
        if segment.words.iter().any(|word| word.redirect) {
            return None;
        }
        match own_remote_action(segment) {
            Some(action) => actions.push(action),
            None if ordinary_segment(&segment.raw) => {}
            None => return None,
        }
    }
    (!actions.is_empty()).then_some(actions)
}

/// A segment needing no allowance of its own: the same decision a standalone
/// trusted call would get, minus anything remote or destructive.
fn ordinary_segment(raw: &str) -> bool {
    matches!(
        standalone_shell_policy::classify_command(raw),
        standalone_shell_policy::ShellCommandPolicy::Allow { .. }
    ) && !codefactory_agent_loop::policy::is_delivery_command(raw)
        && !sensitive(raw)
}

fn own_remote_action(segment: &Segment) -> Option<OwnRemoteAction> {
    let words: Vec<&str> = segment.words.iter().map(|w| w.text.as_str()).collect();
    match words.as_slice() {
        ["gh", "pr", action, number, rest @ ..] => {
            let number = number.parse::<i64>().ok().filter(|n| *n > 0)?;
            let body_files = own_pr_metadata_args(action, rest)?;
            Some(OwnRemoteAction::PrMetadata { number, body_files })
        }
        ["git", "push", rest @ ..] => own_branch_push(rest),
        _ => None,
    }
}

/// Exact PR metadata write on an explicit PR number: a comment body, `ready`,
/// and `edit`.
///
/// M257: `edit` is accepted by default. The decision rests on the PR number and
/// the session's delivery authority, not on an ever-growing flag vocabulary, so
/// only the flags that retarget the write or decide the merge are refused —
/// labels (the merge queue's own switch), base/head, and a repository override
/// that would leave the repository the ownership was proved in. `--body-file`
/// names a local file, so the path must stay inside the workspace and is
/// returned for the caller to hold as a condition.
fn own_pr_metadata_args(action: &str, rest: &[&str]) -> Option<Vec<String>> {
    match action {
        "ready" => rest.is_empty().then(Vec::new),
        "comment" => (rest.len() == 2 && rest[0] == "--body" && !rest[1].is_empty()).then(Vec::new),
        "edit" => own_pr_edit_args(rest),
        _ => None,
    }
}

fn own_pr_edit_args(rest: &[&str]) -> Option<Vec<String>> {
    const REFUSED: &[&str] = &[
        "--add-label",
        "--remove-label",
        "--base",
        "--head",
        "--repo",
        "-R",
    ];
    if rest.is_empty() {
        return None;
    }
    let mut body_files = Vec::new();
    let mut value_expected = false;
    let mut index = 0;
    while let Some(word) = rest.get(index) {
        index += 1;
        let (flag, inline) = match word.split_once('=') {
            Some((flag, value)) if flag.starts_with('-') => (flag, Some(value.to_string())),
            _ => (*word, None),
        };
        if flag == "--body-file" {
            let path = match inline {
                Some(value) => value,
                None => {
                    let value = rest.get(index)?.to_string();
                    index += 1;
                    value
                }
            };
            if !workspace_relative(&path) {
                return None;
            }
            body_files.push(path);
            value_expected = false;
            continue;
        }
        if REFUSED.contains(&flag) {
            return None;
        }
        if flag.starts_with('-') && flag != "-" {
            value_expected = true;
            continue;
        }
        // A bare word is a flag's value; the `--body-file` value was consumed
        // above, so a naked word here has no owner.
        if !value_expected {
            return None;
        }
        value_expected = false;
    }
    Some(body_files)
}

/// M257: `--body-file` publishes a local file, so the path must be provably
/// inside the workspace: relative, no home expansion, no absolute prefix and no
/// `..` climb. Anything else keeps the ordinary approval.
fn workspace_relative(path: &str) -> bool {
    !path.is_empty()
        && path != "-"
        && !path.starts_with('-')
        && !path.starts_with('~')
        && !path.starts_with('/')
        && !path.starts_with('\\')
        && !path.contains(':')
        && !path.split(['/', '\\']).any(|part| part == "..")
}

/// `git push` of a plain branch ref: no force, no mirror/all/tags, no explicit
/// `src:dst` refspec, no main/master, origin only. Deleting is carried
/// separately so the caller can require a proven merge.
fn own_branch_push(rest: &[&str]) -> Option<OwnRemoteAction> {
    let mut delete = false;
    let mut remote: Option<&str> = None;
    let mut refspec: Option<&str> = None;
    for word in rest {
        match *word {
            "--delete" | "-d" => {
                delete = true;
                continue;
            }
            "-u" | "--set-upstream" | "--quiet" | "-q" | "--no-verify" => continue,
            _ => {}
        }
        if word.starts_with('-') || word.is_empty() {
            return None;
        }
        if remote.is_none() {
            remote = Some(word);
            continue;
        }
        if refspec.is_some() {
            return None;
        }
        refspec = Some(word);
    }
    if remote != Some("origin") {
        return None;
    }
    let refspec = refspec?;
    if refspec.contains(':') || refspec.contains('+') || refspec.starts_with("refs/tags/") {
        return None;
    }
    let branch = refspec.strip_prefix("refs/heads/").unwrap_or(refspec);
    if matches!(branch, "main" | "master") || branch.is_empty() {
        return None;
    }
    Some(if delete {
        OwnRemoteAction::DeleteBranch(branch.to_string())
    } else {
        OwnRemoteAction::PushBranch(branch.to_string())
    })
}

fn workspace_at(
    roots: &[std::path::PathBuf],
    cwd: &Path,
    path: &str,
) -> Option<std::path::PathBuf> {
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
        if let Some(rest) = command.strip_prefix(prefix) {
            return rest.trim_start();
        }
    }
    command
}

/// Returns the exact roots needing snapshots and explicit rm paths. The caller
/// supplies only session-owned, identity-verified registrations, never a prefix.
fn prove(
    roots: &[std::path::PathBuf],
    cwd: &Path,
    command: &str,
) -> Option<Vec<(std::path::PathBuf, Vec<String>)>> {
    let roots: Vec<_> = roots
        .iter()
        .map(|p| p.canonicalize())
        .collect::<Result<_, _>>()
        .ok()?;
    let mut cwd = workspace_at(&roots, cwd, ".")?;
    for root in &roots {
        if git(root, &["rev-parse", "--show-toplevel"])
            .and_then(|p| Path::new(&p).canonicalize().ok())
            .as_ref()
            != Some(root)
            || git(root, &["ls-files", "--stage"])
                .is_none_or(|s| s.lines().any(|l| l.starts_with("160000 ")))
        {
            return None;
        }
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
                    if target.redirect || !safe_path(&cwd, &target.text) {
                        return None;
                    }
                    touched.entry(cwd.clone()).or_default();
                }
            } else {
                argv.push(word.text.as_str());
            }
        }
        if argv.len() == 1 && argv[0].contains('=') {
            if piped || segment.separator != ';' {
                return None;
            }
            continue;
        }
        if let ["cd", path] = argv.as_slice() {
            // A failing cd followed by ';' would leave an unknown cwd. Only
            // existing exact registered roots are accepted, never subdirectories.
            if piped {
                return None;
            }
            cwd = workspace_at(&roots, &cwd, path)?;
            if removed.contains(&cwd) {
                return None;
            }
            continue;
        }
        // M207: non-Git commands use the same shell safety decision as a
        // standalone trusted call. Do not impose a second path/runner whitelist.
        if argv.first() != Some(&"git") && argv.first() != Some(&"rm") {
            let standalone = segment.raw.as_str();
            if matches!(
                standalone_shell_policy::classify_command(standalone),
                standalone_shell_policy::ShellCommandPolicy::Allow { .. }
            ) && !codefactory_agent_loop::policy::is_delivery_command(standalone)
                && !sensitive(standalone)
            {
                continue;
            }
            return None;
        }
        let (root, args) = match argv.as_slice() {
            ["git", "-C", path, rest @ ..] => (workspace_at(&roots, &cwd, path)?, rest),
            ["git", rest @ ..] => (cwd.clone(), rest),
            ["rm", "-rf", paths @ ..] if !piped && !paths.is_empty() => {
                if paths.iter().any(|p| {
                    p.starts_with('-')
                        || !inside(&cwd, p)
                        || cwd.join(p).canonicalize().ok().as_ref() == Some(&cwd)
                }) {
                    return None;
                }
                touched
                    .entry(cwd.clone())
                    .or_default()
                    .extend(paths.iter().map(|p| p.to_string()));
                continue;
            }
            _ => return None,
        };
        if removed.contains(&root) {
            return None;
        }
        has_git = true;
        // Queries do not need path confinement or a snapshot. Reject execution
        // and output overrides before using the shared Git query classifier.
        let readonly = (codefactory_agent_core::command_segment_is_read_only_probe(&segment.raw)
            || matches!(
                args,
                ["stash", "list", ..] | ["stash", "show", ..] | ["worktree", "list", ..]
            )
            || matches!(
                args,
                ["config", "core.hooksPath"]
                    | ["config", "--get", "core.hooksPath"]
                    | ["config", "--local", "--get", "core.hooksPath"]
            ))
            && args.first() != Some(&"fetch")
            && !args.iter().any(|word| {
                let option = word.split('=').next().unwrap_or(word);
                [
                    "--ext-diff",
                    "--textconv",
                    "--output",
                    "--output-directory",
                    "--upload-pack",
                    "--receive-pack",
                ]
                .contains(&option)
            });
        if readonly {
            continue;
        }
        if args.first() == Some(&"reset") && args.iter().any(|word| *word == "--hard") {
            let revisions: Vec<_> = args[1..]
                .iter()
                .filter(|word| !word.starts_with('-'))
                .copied()
                .collect();
            if revisions.len() > 1 {
                return None;
            }
            let revision = revisions.first().copied().unwrap_or("HEAD");
            if !legacy_rewrite(&root, &format!("git reset --hard {revision}")) {
                return None;
            }
        }
        if args.first() == Some(&"worktree") {
            match args {
                ["worktree", "remove", flags @ ..] | ["worktree", "add", flags @ ..] => {
                    if piped {
                        return None;
                    }
                    let path_index = flags.iter().position(|w| !w.starts_with('-'))?;
                    let path = flags[path_index];
                    if !flags
                        .iter()
                        .enumerate()
                        .all(|(index, word)| index == path_index || bounded_args(&root, &[*word]))
                    {
                        return None;
                    }
                    if flags.iter().any(|word| {
                        word.starts_with("--orphan")
                            || word.starts_with("--detach=")
                            || word.starts_with("--reason")
                    }) {
                        return None;
                    }
                    let target = workspace_at(&roots, &root, path)?;
                    if target == root || target == cwd {
                        return None;
                    }
                    let common = |r: &Path| {
                        git(
                            r,
                            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
                        )
                        .and_then(|p| Path::new(&p).canonicalize().ok())
                    };
                    if common(&root).is_none() || common(&root) != common(&target) {
                        return None;
                    }
                    if args[1] == "remove" {
                        if !removed.insert(target.clone()) {
                            return None;
                        }
                        touched.entry(target).or_default().push(".".into());
                    } else {
                        touched.entry(target).or_default();
                    }
                    touched.entry(root).or_default();
                    continue;
                }
                _ => return None,
            }
        }
        if let ["fetch", rest @ ..] = args {
            if input_pipe || !fetch_args(rest) {
                return None;
            }
            touched.entry(root).or_default();
            continue;
        }
        if !local_denylist(&root, args) {
            return None;
        }
        if readonly {
            continue;
        }
        if input_pipe {
            return None;
        }
        // M182: option ordering is not authority. Confined non-interactive
        // local verbs use the shared denylist and the same durable snapshot.
        if args.first().is_some_and(|verb| {
            [
                "fetch",
                "stash",
                "merge",
                "rebase",
                "switch",
                "checkout",
                "restore",
                "clean",
                "add",
                "commit",
                "reset",
                "cherry-pick",
                "revert",
                "am",
                "apply",
                "config",
            ]
            .contains(verb)
        }) {
            touched.entry(root).or_default();
            continue;
        }
        match args {
            ["fetch", "--prune", "origin", "main"]
            | ["fetch", "origin", "main"]
            | ["merge", "origin/main", "--no-edit"]
            | ["stash"]
            | ["stash", "push"]
            | ["stash", "pop"] => (),
            ["stash", "push", "-m", message] if !message.is_empty() => (),
            ["checkout", branch] if argv.get(1) == Some(&"-C") && !branch.starts_with('-') => {
                // Only the current session workspace branch, not arbitrary refs.
                if git(&root, &["branch", "--show-current"]).as_deref() != Some(branch) {
                    return None;
                }
            }
            ["worktree", "remove", "--force", path] => {
                let target = workspace_at(&roots, &root, path)?;
                if target == root || target == cwd {
                    return None;
                }
                let common = |r: &Path| {
                    git(
                        r,
                        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
                    )
                    .and_then(|p| Path::new(&p).canonicalize().ok())
                };
                if common(&root).is_none() || common(&root) != common(&target) {
                    return None;
                }
                if !removed.insert(target.clone()) {
                    return None;
                }
                touched.entry(target).or_default().push(".".into());
            }
            _ => {
                let simple = std::iter::once("git")
                    .chain(args.iter().copied())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !legacy_rewrite(&root, &simple) {
                    return None;
                }
            }
        }
        touched.entry(root).or_default();
    }
    has_git.then(|| touched.into_iter().collect())
}

pub(super) fn local_rewrite(root: &Path, command: &str) -> bool {
    prove(&[root.into()], root, command).is_some()
}

/// M248: registration proves confinement, not destructive intent. Ordinary
/// synchronization is kernel-confined; all other sensitive actions keep M159.
pub(super) fn destructive_intent(command: &str) -> bool {
    let Some(segments) = executable_segments(command) else {
        return true;
    };
    segments.iter().any(|words| {
        if !segment_is_sensitive(words) {
            return false;
        }
        let args: Vec<_> = words.iter().map(String::as_str).collect();
        match args.as_slice() {
            ["git", "fetch", rest @ ..] => !fetch_args(rest),
            ["git", "merge", rest @ ..] => !merge_args(rest),
            // Staging and a new commit preserve existing history. Amend and
            // unsupported flags remain under the snapshot/approval path.
            ["git", "add", ..] => false,
            ["git", "commit", rest @ ..] => !commit_args(rest) || rest.contains(&"--amend"),
            ["git", "config", "core.hooksPath", ".githooks"]
            | ["git", "config", "--local", "core.hooksPath", ".githooks"] => false,
            _ => true,
        }
    })
}

pub(super) fn registered_rewrite(roots: &[std::path::PathBuf], cwd: &Path, command: &str) -> bool {
    prove(roots, cwd, command).is_some()
}

pub(super) fn backup_registered(
    roots: &[std::path::PathBuf],
    cwd: &Path,
    command: &str,
) -> Option<String> {
    let targets = prove(roots, cwd, command)?;
    let mut refs = Vec::new();
    for (root, paths) in targets {
        refs.push(snapshot(&root, &paths)?);
    }
    // An empty target set means the command contains only proved queries.
    // Do not turn an unrelated conflicted index into a read-only approval.
    Some(if refs.is_empty() {
        "read-only".into()
    } else {
        refs.join(",")
    })
}

/// Snapshot via a private index; never stash away or rewrite the live index.
/// Parent 1 retains all unpublished commits; parent 2 retains staged content.
pub(super) fn backup(root: &Path, command: &str) -> Option<String> {
    backup_registered(&[root.into()], root, command)
}
fn snapshot(root: &Path, paths: &[String]) -> Option<String> {
    let head = git(root, &["rev-parse", "HEAD"])?;
    let index_tree = git(root, &["write-tree"])?;
    let nonce = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos()
    );
    let dir = std::env::temp_dir().join(format!("cf-git-backup-{nonce}"));
    std::fs::create_dir(&dir).ok()?;
    let index = dir.join("index");
    let result = (|| {
        let run = |args: &[&str], input: Option<&str>| -> Option<String> {
            let mut child = crate::util::host_git::command()
                .no_window()
                .current_dir(root)
                .env("GIT_INDEX_FILE", &index)
                .env("GIT_AUTHOR_NAME", "CodeFactory backup")
                .env("GIT_AUTHOR_EMAIL", "backup@codefactory.invalid")
                .env("GIT_COMMITTER_NAME", "CodeFactory backup")
                .env("GIT_COMMITTER_EMAIL", "backup@codefactory.invalid")
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .ok()?;
            if let Some(input) = input {
                child.stdin.take()?.write_all(input.as_bytes()).ok()?;
            }
            let out = child.wait_with_output().ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).trim().into())
        };
        run(&["read-tree", &head], None)?;
        run(&["add", "-A", "--", "."], None)?;
        // Include ignored files explicitly targeted by rm (clean -x is not allowed).
        for path in paths {
            if root.join(path).exists() {
                run(&["add", "-f", "--", path], None)?;
            }
        }
        let tree = run(&["write-tree"], None)?;
        let staged = run(
            &["commit-tree", &index_tree, "-p", &head],
            Some("M159 staged backup\n"),
        )?;
        let commit = run(
            &["commit-tree", &tree, "-p", &head, "-p", &staged],
            Some("M159 workspace backup\n"),
        )?;
        let reference = format!("refs/codefactory/backup/{nonce}");
        run(
            &[
                "update-ref",
                &reference,
                &commit,
                "0000000000000000000000000000000000000000",
            ],
            None,
        )?;
        (git(root, &["rev-parse", &reference])? == commit).then_some(reference)
    })();
    let _ = std::fs::remove_dir_all(dir);
    result
}

#[cfg(test)]
mod tests {
    fn words(command: &str) -> Option<Vec<Vec<String>>> {
        super::segments(command).map(|segments| {
            segments
                .into_iter()
                .map(|s| s.words.into_iter().map(|w| w.text).collect())
                .collect()
        })
    }

    #[test]
    fn m166_quoted_backslash_paths_stay_literal_words() {
        // Windows worktrees are quoted with backslash separators. Treating a
        // backslash inside quotes as an escape silently demotes the whole
        // composition to an approval prompt.
        let command = r#"cd "C:\Users\runneradmin\AppData\Local\Temp\ws"; git stash push -m prefill; git status --porcelain | head"#;
        let parsed = words(command).expect("quoted Windows path must parse");
        assert_eq!(
            parsed[0],
            vec!["cd", r"C:\Users\runneradmin\AppData\Local\Temp\ws"]
        );
        assert_eq!(parsed[1], vec!["git", "stash", "push", "-m", "prefill"]);
        assert_eq!(parsed[2], vec!["git", "status", "--porcelain"]);
        assert_eq!(parsed[3], vec!["head"]);
        let assigned = words(r#"S="C:\ws\two"; git -C "$S" worktree remove --force "C:\ws\one""#)
            .expect("quoted assignment must parse");
        assert_eq!(assigned[0], vec![r"S=C:\ws\two"]);
        assert_eq!(
            assigned[1],
            vec![
                "git",
                "-C",
                r"C:\ws\two",
                "worktree",
                "remove",
                "--force",
                r"C:\ws\one"
            ]
        );
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

/// M208 regression tests: the session's own remote writes are judged segment by
/// segment, and anything the prover cannot place falls back to the gate.
#[cfg(test)]
mod m208_own_remote_actions {
    use super::{own_remote_actions, OwnRemoteAction};

    /// M257: label calls left the owned set — the merge queue's own switch stays
    /// behind approval — so the owned description writes are title and body,
    /// inline and from a workspace file.
    const OWN_PR_BODY: &str = "gh pr edit 929 --title 'fix: own PR' --body 'plain'";
    const OWN_PR_EVIDENCE: &str = "gh pr view 933 --json state,mergeable,statusCheckRollup; \
gh pr edit 933 --body-file .git/PR_BODY.md --title 'CI 绿'; \
gh pr comment 933 --body 'CI 全绿，待合并'";

    fn metadata(number: i64, body_files: &[&str]) -> OwnRemoteAction {
        OwnRemoteAction::PrMetadata {
            number,
            body_files: body_files.iter().map(|file| file.to_string()).collect(),
        }
    }

    fn actions(command: &str) -> Vec<OwnRemoteAction> {
        own_remote_actions(command).unwrap_or_else(|| panic!("must be owned: {command}"))
    }

    #[test]
    fn m208_single_own_pr_metadata_mutation_is_owned() {
        assert_eq!(actions(OWN_PR_BODY), vec![metadata(929, &[])]);
        assert_eq!(actions("gh pr ready 929"), vec![metadata(929, &[])]);
        assert_eq!(
            actions("gh pr edit 929 --title 'fix: x' --body 'plain'"),
            vec![metadata(929, &[])]
        );
        // M257: the body may come from a workspace file, and the file it names is
        // carried along so the caller keeps its containment.
        assert_eq!(
            actions("gh pr edit 929 --body-file .git/PR_BODY.md --title 'x'"),
            vec![metadata(929, &[".git/PR_BODY.md"])]
        );
        assert_eq!(
            actions("gh pr edit 929 --body-file=docs/PR_BODY.md"),
            vec![metadata(929, &["docs/PR_BODY.md"])]
        );
    }

    #[test]
    fn m208_compound_segments_are_judged_one_by_one() {
        // The read-only `gh pr view` segment needs no allowance and contributes
        // none; the two writes on the same own PR do.
        assert_eq!(
            actions(OWN_PR_EVIDENCE),
            vec![metadata(933, &[".git/PR_BODY.md"]), metadata(933, &[])]
        );
        // A read-only segment beside an owned one is judged on its own and stays
        // owned; it neither launders nor is laundered by the write beside it.
        assert_eq!(
            actions("git status --short && gh pr edit 929 --title 'fix: x'"),
            vec![metadata(929, &[])]
        );
    }

    /// M257 R3: the flags that decide the merge or retarget the write stay
    /// outside the owned set, and a `--body-file` path may not leave the
    /// workspace.
    #[test]
    fn m257_pr_edit_refuses_merge_and_retargeting_flags() {
        for command in [
            "gh pr edit 929 --add-label 'merge-queue: armed'",
            "gh pr edit 929 --remove-label 'merge-queue: armed'",
            "gh pr edit 929 --base main",
            "gh pr edit 929 --head other-branch",
            "gh pr edit 929 --repo someone/else --title 'x'",
            "gh pr edit 929 --title 'x' --add-label 'merge-queue: armed'",
            "gh pr edit 929 --body-file /etc/hosts",
            "gh pr edit 929 --body-file ../outside.md",
            "gh pr edit 929 --body-file ~/.ssh/id_rsa",
            "gh pr edit 929 --body-file -",
            "gh pr edit --body-file PR_BODY.md",
        ] {
            assert_eq!(own_remote_actions(command), None, "{command}");
        }
    }

    #[test]
    fn m208_own_branch_push_shapes_are_owned() {
        assert_eq!(
            actions("git push origin HEAD"),
            vec![OwnRemoteAction::PushBranch("HEAD".into())]
        );
        assert_eq!(
            actions("git push -u origin codefactory/objective-m208"),
            vec![OwnRemoteAction::PushBranch(
                "codefactory/objective-m208".into()
            )]
        );
        assert_eq!(
            actions("git push origin --delete codefactory/objective-m208"),
            vec![OwnRemoteAction::DeleteBranch(
                "codefactory/objective-m208".into()
            )]
        );
    }

    #[test]
    fn m208_unowned_or_unproved_segments_fall_back_to_the_gate() {
        for command in [
            "gh pr edit 929 --add-label 'merge-queue: armed'; gh auth token",
            "gh pr edit 929 --add-label 'merge-queue: armed'; rm -rf ../other",
            "gh pr edit 929 --add-label 'merge-queue: armed'; git reset --hard",
            "gh pr edit 929 --add-label 'merge-queue: armed'; gh pr merge 929 --squash",
            "gh pr merge 929 --squash",
            "gh pr close 929",
            "gh pr edit 929 --body-file /etc/hosts",
            "gh pr edit 929 --base main",
            "gh release delete v1.0.0",
            "git push --force origin HEAD",
            "git push origin main",
            "git push origin master",
            "git push origin HEAD:refs/heads/other",
            "git push --mirror origin",
        ] {
            assert!(own_remote_actions(command).is_none(), "{command}");
        }
        // Read-only traffic alone is not an owned remote write: it needs no
        // allowance, and must not create one for a later segment.
        assert!(own_remote_actions("gh pr view 933 --json state").is_none());
    }
}

/// M215 R1 (Windows): a canonicalised Windows path arrives verbatim
/// (`\\?\C:\...`). The proof must read it as the same directory it names, and
/// `\` inside such a word is a separator, not a shell escape.
#[cfg(test)]
mod m215_windows_path_forms {
    use super::{plain_windows_text, shell_text};

    #[test]
    fn verbatim_prefixes_reduce_to_the_plain_form() {
        assert_eq!(
            plain_windows_text(r"\\?\C:\Users\a\state"),
            r"C:\Users\a\state"
        );
        assert_eq!(
            plain_windows_text(r"\\?\UNC\server\share\ws"),
            r"\\server\share\ws"
        );
        assert_eq!(plain_windows_text("/Users/leo/ws"), "/Users/leo/ws");
        assert_eq!(plain_windows_text(r"C:\Users\a\state"), r"C:\Users\a\state");
    }

    #[test]
    fn windows_paths_keep_their_backslashes() {
        assert_eq!(
            shell_text(r"\\?\C:\Users\a\state").as_deref(),
            Some(r"\\?\C:\Users\a\state")
        );
        assert_eq!(
            shell_text(r"C:\Users\a\state").as_deref(),
            Some(r"C:\Users\a\state")
        );
        assert_eq!(
            shell_text(r#""\\?\C:\Users\a\state""#).as_deref(),
            Some(r"\\?\C:\Users\a\state")
        );
        // A plain POSIX word still undoes backslash escapes.
        assert_eq!(shell_text(r"my\ dir").as_deref(), Some("my dir"));
    }

    #[cfg(windows)]
    #[test]
    fn verbatim_root_contains_its_plain_child() {
        use super::within;
        use std::path::Path;
        assert!(within(Path::new(r"\\?\C:\ws"), Path::new(r"C:\ws\state")));
        assert!(within(Path::new(r"C:\ws"), Path::new(r"\\?\C:\ws\state")));
        assert!(!within(Path::new(r"\\?\C:\ws"), Path::new(r"C:\other")));
    }
}

#[cfg(test)]
mod m215_trusted_heredoc_and_read_only_git {
    use super::{python_heredoc_proof, sensitive};

    fn workspace() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src-tauri/src/agent")).unwrap();
        std::fs::create_dir_all(root.join("state")).unwrap();
        std::fs::write(
            root.join("src-tauri/src/agent/trusted_git.rs"),
            "// existing\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src-tauri/Cargo.toml"),
            "[package]\nname = \"synthetic\"\n",
        )
        .unwrap();
        std::fs::write(root.join("state/session.json"), "{\"state\":\"active\"}\n").unwrap();
        (temp, root)
    }

    /// R4: the five shapes the field log recorded as false positives.
    #[test]
    fn field_shapes_are_proved() {
        let (_temp, root) = workspace();
        let shapes = [
            "cd src-tauri && cat >> src/agent/trusted_git.rs <<'EOF'\n// appended by the agent\nfn synthetic() {}\nEOF".to_string(),
            "cd src-tauri && python3 - <<'PY'\nimport json\np = 'Cargo.toml'\nprint(json.load(open(p)))\nPY".to_string(),
            format!("cd {}/state && python3 - <<'PY'\nimport json\nprint(json.load(open('{}/state/session.json')))\nPY", root.display(), root.display()),
            "cd \"src-tauri\" && python3 - <<'PY'\nimport re\nprint(re.sub('a', 'b', 'aaa'))\nPY".to_string(),
            "cd src-tauri/.. && cat >> src-tauri/src/agent/trusted_git.rs <<'EOF'\n// parent-relative cd\nEOF".to_string(),
        ];
        for shape in shapes {
            assert!(
                python_heredoc_proof(Some(&root), &shape).is_ok(),
                "{shape}\n{:?}",
                python_heredoc_proof(Some(&root), &shape)
            );
        }
    }

    /// R4: the boundary did not move — outside destinations, outside paths in a
    /// body, protected files and unproved `cd` still ask for approval.
    #[test]
    fn boundaries_still_ask() {
        let (_temp, root) = workspace();
        for shape in [
            "cd /etc && cat > hosts <<'EOF'\ndata\nEOF".to_string(),
            "cd .. && python3 - <<'PY'\nprint('escape')\nPY".to_string(),
            format!(
                "cat >> {}/state/new.txt <<'EOF'\nsee /Users/leo/.ssh/id_rsa\nEOF",
                root.display()
            ),
            "python3 - <<'PY'\nopen('/Users/leo/.ssh/id_rsa').read()\nPY".to_string(),
            "python3 - <<'PY'\nimport subprocess\nsubprocess.run(['git', 'push'])\nPY".to_string(),
            "python3 - <<'PY'\nopen('../outside.txt', 'w').write('bad')\nPY".to_string(),
        ] {
            assert!(
                python_heredoc_proof(Some(&root), &shape).is_err(),
                "{shape}"
            );
        }
    }

    /// R1/R2: read-only commands may run before the heredoc; the proof judges
    /// the prefix with the same "read-only only" rule and then the body.
    #[test]
    fn read_only_prefix_before_heredoc_is_proved() {
        let (_temp, root) = workspace();
        let shapes = [
            format!(
                "cd {} && grep -rl trusted . | head; echo ===; python3 - <<'PY'\nimport json\nprint(json.load(open('state/session.json')))\nPY",
                root.display()
            ),
            "grep -rl trusted . | head; echo ===; python3 - <<'PY'\nprint('ok')\nPY".to_string(),
            "echo start; wc -l src-tauri/Cargo.toml; cat src-tauri/Cargo.toml | head -1; cat >> src-tauri/src/agent/trusted_git.rs <<'EOF'\n// appended after read-only commands\nEOF".to_string(),
            "ls -1 | head -3 && git status --short; python3 - <<'PY'\nprint('ok')\nPY".to_string(),
            "git log --oneline 2>&1 | head -3; python3 - <<'PY'\nprint('ok')\nPY".to_string(),
        ];
        for shape in shapes {
            assert!(
                python_heredoc_proof(Some(&root), &shape).is_ok(),
                "{shape}\n{:?}",
                python_heredoc_proof(Some(&root), &shape)
            );
        }
        for shape in [
            "rm -rf src; python3 - <<'PY'\nprint('ok')\nPY".to_string(),
            "git reset --hard HEAD~1; python3 - <<'PY'\nprint('ok')\nPY".to_string(),
            "cd /etc && grep -rl root . | head; python3 - <<'PY'\nprint('ok')\nPY".to_string(),
        ] {
            assert!(
                python_heredoc_proof(Some(&root), &shape).is_err(),
                "{shape}"
            );
        }
    }

    /// R3: read-only Git is allowed anywhere; any write verb in the same
    /// compound command still follows the original rule.
    #[test]
    fn read_only_git_needs_no_approval_but_writes_still_do() {
        for command in [
            "cd /tmp && git branch -r --contains origin/main",
            "git show origin/main:src-tauri/Cargo.toml | grep -c 'version'",
            "git status --short",
            "git log --oneline -3 && git diff --stat",
            "git rev-parse HEAD && git ls-files | wc -l",
        ] {
            assert!(!sensitive(command), "{command}");
        }
        for command in [
            "git status --short && git reset --hard HEAD~1",
            "git log --oneline && git clean -fd",
            "git branch -D feature",
            "git config core.hooksPath .githooks",
            "git show --ext-diff HEAD",
        ] {
            assert!(sensitive(command), "{command}");
        }
        let mixed = "git status --short; git push origin main";
        assert!(
            sensitive(mixed) || codefactory_agent_loop::policy::is_delivery_command(mixed),
            "{mixed}"
        );
    }
}

/// M221: `git commit -F -` / `git tag -F -` read the heredoc body as message
/// data. The body is never executed, so the interpreter proof does not apply;
/// the command is judged segment by segment instead (local Git writes inside
/// this workspace yes, remote actions and outside repositories still ask).
#[cfg(test)]
mod m221_git_data_heredoc {
    use super::{python_heredoc_proof, sensitive};

    fn workspace() -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src-tauri/src/agent")).unwrap();
        std::fs::write(root.join("tracked.txt"), "base\n").unwrap();
        (temp, root)
    }

    /// R1/R2: the commit message heredoc is data, in both shapes the field log
    /// recorded (`git commit -F - <<'MSG'` and `cat <<'MSG' | git commit -F -`).
    #[test]
    fn local_commit_message_heredoc_is_proved() {
        let (_temp, root) = workspace();
        let shapes = [
            "git add -A && git commit -q -F - <<'MSG'\nM221: local commit message\nMSG".to_string(),
            "git add -A\ngit commit -q -F - <<'MSG'\nM221: message after a newline separator\nMSG"
                .to_string(),
            "git add -A && git commit -q -F - <<'MSG'\nM221: message\nMSG\ngit log --oneline -1"
                .to_string(),
            "cat <<'MSG' | git commit -F -\nM221: piped message\nMSG".to_string(),
            "git commit --file=- <<'MSG'\nM221: long option\nMSG".to_string(),
            "git add -A && git tag -a v9 -F - <<'MSG'\nM221: tag message\nMSG".to_string(),
            format!(
                "cd {} && git add -A && git commit -F - <<'MSG'\nM221: absolute workspace cd\nMSG",
                root.display()
            ),
            // A message naming an outside path is still just text: it is never
            // resolved as a path and never executed.
            "git commit -F - <<'MSG'\nsee /Users/leo/.ssh/id_rsa for the incident\nMSG".to_string(),
        ];
        for shape in shapes {
            assert!(
                python_heredoc_proof(Some(&root), &shape).is_ok(),
                "{shape}\n{:?}",
                python_heredoc_proof(Some(&root), &shape)
            );
        }
        // R3: the original command no longer needs a local-rewrite snapshot.
        // The proof itself stays fail-closed without a workspace premise: the
        // gateway passes the registered root, a bare predicate does not.
        assert!(python_heredoc_proof(
            None,
            "git add -A && git commit -F - <<'MSG'\nM221: message\nMSG"
        )
        .is_err());
        assert!(sensitive(
            "git add -A && git commit -q -F - <<'MSG'\nM221: message\nMSG"
        ));
    }

    /// R3: the boundary did not move. Another repository, a rewriting prefix,
    /// a script body, an unquoted delimiter and trailing shell content still ask.
    #[test]
    fn git_data_heredoc_boundaries_still_ask() {
        let (_temp, root) = workspace();
        let shapes = [
            "git -C /tmp/other-repo commit -F - <<'MSG'\nmessage\nMSG".to_string(),
            "git --git-dir=/tmp/other/.git commit -F - <<'MSG'\nmessage\nMSG".to_string(),
            "git reset --hard HEAD~1 && git commit -F - <<'MSG'\nmessage\nMSG".to_string(),
            "git clean -fd && git add -A && git commit -F - <<'MSG'\nmessage\nMSG".to_string(),
            "git add -A && git commit -F - <<'MSG'\nmessage\nMSG\nrm -rf src".to_string(),
            "git add -A && git commit -F - <<MSG\nmessage\nMSG".to_string(),
            "git add -A && python3 - <<'PY'\nprint('script, not a message')\nPY".to_string(),
            "git add -A && git commit -F - <<'MSG'\nmessage\nMSG\npython3 - <<'PY'\nprint('extra')\nPY".to_string(),
            "cat <<'MSG' | git commit -F - | tee log.txt\nmessage\nMSG".to_string(),
            "cat <<'MSG' | sh\nmessage\nMSG".to_string(),
        ];
        for shape in shapes {
            assert!(
                python_heredoc_proof(Some(&root), &shape).is_err(),
                "{shape}"
            );
        }
    }
}

/// M221b: a `<<` inside shell quotes is an argument, not a heredoc. The field
/// sample was a read-only probe stopped for `heredoc 缺少换行` because the grep
/// pattern itself contained `<<`.
#[cfg(test)]
mod m221b_quoted_double_angle {
    use super::{has_unquoted_heredoc, python_heredoc_proof, sensitive};

    #[test]
    fn quoted_double_angle_is_a_pattern_not_a_heredoc() {
        let patterns = [
            "grep -n \"heredoc\\|<<\" src-tauri/src/tools/shell_policy.rs",
            "grep -rn 'a << b' src",
            "rg --fixed-strings \"<<\" .",
            "echo \"use <<EOF for heredocs\"",
            "sed -n 2680,2770p src-tauri/src/agent/tool_backend.rs",
        ];
        for command in patterns {
            assert!(!has_unquoted_heredoc(command), "{command}");
            assert!(!sensitive(command), "{command}");
        }
        assert!(has_unquoted_heredoc("cat <<'EOF'\nbody\nEOF"));
        assert!(has_unquoted_heredoc(
            "git commit -F - <<'MSG'\nmessage\nMSG"
        ));
        assert!(has_unquoted_heredoc(
            "grep \"quoted\" ; cat <<'EOF'\nbody\nEOF"
        ));
        assert!(!has_unquoted_heredoc("echo 'unterminated"));
    }

    #[test]
    fn the_read_only_probe_matches_the_field_command() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        std::fs::write(root.join("probe.rs"), "a\nb\nc\nd\ne\nf\n").unwrap();
        let command = format!(
            "cd \"{}\" && sed -n 1,5p probe.rs && grep -n \"heredoc\\|<<\" probe.rs | head -30",
            root.display()
        );
        assert!(!has_unquoted_heredoc(&command), "{command}");
        assert!(!sensitive(&command), "{command}");
        assert!(
            python_heredoc_proof(Some(&root), &command).is_err(),
            "not a heredoc, so the interpreter proof has nothing to prove: {command}"
        );
    }
}

mod trusted_shell_structure {
    //!
    //! `for`, `while`, `if`, subshells and `$(...)` used to hide the simple
    //! commands inside them from the per-segment judges, so a read-only loop was
    //! refused as "unproved git". This module unfolds those structures into the
    //! flat sequence of simple commands they would run. It is structure-only: no
    //! command text is whitelisted here, and everything that cannot be enumerated
    //! statically returns `None` so the caller keeps its fail-closed behaviour.

    /// Placeholder for the output of a `$(...)` we did enumerate. It is no longer a
    /// live expansion, so the dynamic-word rule must not fire on it.
    const SUBSTITUTION_MARK: char = '\u{1}';

    /// Deepest structure nesting we are willing to unfold.
    const MAX_STRUCTURE_DEPTH: usize = 16;

    /// Most literal `for` iterations we unroll.
    const MAX_UNROLLED_ITERATIONS: usize = 64;

    #[derive(Clone, Debug)]
    enum ShellToken {
        Word {
            text: String,
            substitutions: Vec<Vec<ShellToken>>,
        },
        Sep,
        OpenParen,
        CloseParen,
    }

    enum Dollar {
        Text(String),
        Substitution(Vec<ShellToken>),
    }

    #[derive(Clone, Debug)]
    enum ShellBinding {
        /// Known literal iteration value (`for c in a b`).
        Literal(String),
        /// Values we could not enumerate, but a local structure produced them
        /// (`for c in $(git rev-list ...)`).
        Bounded,
    }

    type ShellVars = std::collections::BTreeMap<String, ShellBinding>;

    /// One entry of a `for ... in ...` list.
    enum ShellValue {
        Literal(String),
        Bounded,
        Unknown,
    }

    /// M260 entry point: `None` means "cannot be enumerated, stay sensitive".
    pub(super) fn executable_segments(command: &str) -> Option<Vec<Vec<String>>> {
        let tokens = shell_tokens(command)?;
        let vars = ShellVars::new();
        let mut segments = Vec::new();
        let (stop, used) = parse_shell_block(&tokens, &vars, &mut segments, &[], 0)?;
        if stop.is_some() || used != tokens.len() {
            return None;
        }
        Some(segments)
    }

    fn shell_tokens(command: &str) -> Option<Vec<ShellToken>> {
        shell_tokens_at_depth(command, 0)
    }

    fn shell_tokens_at_depth(command: &str, depth: usize) -> Option<Vec<ShellToken>> {
        if depth > MAX_STRUCTURE_DEPTH {
            return None;
        }
        let chars: Vec<char> = command.chars().collect();
        let mut tokens: Vec<ShellToken> = Vec::new();
        let mut word = String::new();
        let mut substitutions: Vec<Vec<ShellToken>> = Vec::new();
        let mut in_word = false;
        let mut index = 0usize;
        while index < chars.len() {
            let ch = chars[index];
            match ch {
                '\'' => {
                    in_word = true;
                    index += 1;
                    loop {
                        match chars.get(index) {
                            None => return None,
                            Some('\'') => {
                                index += 1;
                                break;
                            }
                            Some(c) => {
                                word.push(*c);
                                index += 1;
                            }
                        }
                    }
                }
                '"' => {
                    in_word = true;
                    index += 1;
                    loop {
                        match chars.get(index) {
                            None => return None,
                            Some('"') => {
                                index += 1;
                                break;
                            }
                            Some('`') => return None,
                            Some('\\') if index + 1 < chars.len() => {
                                word.push(chars[index]);
                                word.push(chars[index + 1]);
                                index += 2;
                            }
                            Some('$') => {
                                let (consumed, dollar) = lex_dollar(&chars, index, depth)?;
                                index += consumed;
                                match dollar {
                                    Dollar::Text(text) => word.push_str(&text),
                                    Dollar::Substitution(inner) => {
                                        word.push(SUBSTITUTION_MARK);
                                        substitutions.push(inner);
                                    }
                                }
                            }
                            Some(c) => {
                                word.push(*c);
                                index += 1;
                            }
                        }
                    }
                }
                '\\' if index + 1 < chars.len() => {
                    word.push(ch);
                    word.push(chars[index + 1]);
                    in_word = true;
                    index += 2;
                }
                ';' | '|' | '&' | '>' | '<' | '\n' | '\r' => {
                    close_shell_word(&mut tokens, &mut word, &mut substitutions, &mut in_word);
                    tokens.push(ShellToken::Sep);
                    index += 1;
                }
                '(' => {
                    if in_word {
                        return None;
                    }
                    tokens.push(ShellToken::OpenParen);
                    index += 1;
                }
                ')' => {
                    // A closing parenthesis always ends the current word: `(git add -A)`
                    // has no space before `)`, and the word is still a plain word.
                    close_shell_word(&mut tokens, &mut word, &mut substitutions, &mut in_word);
                    tokens.push(ShellToken::CloseParen);
                    index += 1;
                }
                '`' => return None,
                '$' => {
                    let (consumed, dollar) = lex_dollar(&chars, index, depth)?;
                    index += consumed;
                    in_word = true;
                    match dollar {
                        Dollar::Text(text) => word.push_str(&text),
                        Dollar::Substitution(inner) => {
                            word.push(SUBSTITUTION_MARK);
                            substitutions.push(inner);
                        }
                    }
                }
                c if c.is_whitespace() => {
                    close_shell_word(&mut tokens, &mut word, &mut substitutions, &mut in_word);
                    index += 1;
                }
                c => {
                    word.push(c);
                    in_word = true;
                    index += 1;
                }
            }
        }
        close_shell_word(&mut tokens, &mut word, &mut substitutions, &mut in_word);
        Some(tokens)
    }

    fn close_shell_word(
        tokens: &mut Vec<ShellToken>,
        word: &mut String,
        substitutions: &mut Vec<Vec<ShellToken>>,
        in_word: &mut bool,
    ) {
        if !*in_word && word.is_empty() && substitutions.is_empty() {
            return;
        }
        let text = std::mem::take(word);
        let substitution_tokens = std::mem::take(substitutions);
        *in_word = false;
        // Grouping braces only work as standalone words; treat them as boundaries.
        if substitution_tokens.is_empty() && (text == "{" || text == "}") {
            tokens.push(ShellToken::Sep);
            return;
        }
        tokens.push(ShellToken::Word {
            text,
            substitutions: substitution_tokens,
        });
    }

    fn lex_dollar(chars: &[char], start: usize, depth: usize) -> Option<(usize, Dollar)> {
        let Some(next) = chars.get(start + 1).copied() else {
            return Some((1, Dollar::Text("$".into())));
        };
        match next {
            '(' => {
                // `$(( ))` is arithmetic, not a command substitution.
                //
                // M278 R2: an arithmetic body made only of literals, variables,
                // `+ - * / %` and parentheses computes a number and runs nothing,
                // so it stays a literal word the way `$VAR` does. Anything else
                // in there — a nested `$(...)`, a backtick, a quote, an
                // assignment — can still run a program, so it keeps the old
                // unproved answer.
                if chars.get(start + 2) == Some(&'(') {
                    let close = matching_paren(chars, start + 1)?;
                    if close <= start + 3 {
                        return None;
                    }
                    let body: String = chars[start + 3..close - 1].iter().collect();
                    if !pure_arithmetic_body(&body) {
                        return None;
                    }
                    let text: String = chars[start..=close].iter().collect();
                    return Some((close - start + 1, Dollar::Text(text)));
                }
                let close = matching_paren(chars, start + 1)?;
                let inner: String = chars[start + 2..close].iter().collect();
                let tokens = shell_tokens_at_depth(&inner, depth + 1)?;
                Some((close - start + 1, Dollar::Substitution(tokens)))
            }
            '{' => {
                let close = chars[start + 2..].iter().position(|c| *c == '}')? + start + 2;
                let text: String = chars[start..=close].iter().collect();
                Some((close - start + 1, Dollar::Text(text)))
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let mut end = start + 1;
                while matches!(chars.get(end), Some(c) if c.is_ascii_alphanumeric() || *c == '_') {
                    end += 1;
                }
                Some((
                    end - start,
                    Dollar::Text(chars[start..end].iter().collect()),
                ))
            }
            c if c.is_ascii_digit() => {
                let mut end = start + 1;
                while matches!(chars.get(end), Some(c) if c.is_ascii_digit()) {
                    end += 1;
                }
                Some((
                    end - start,
                    Dollar::Text(chars[start..end].iter().collect()),
                ))
            }
            _ => Some((1, Dollar::Text("$".into()))),
        }
    }

    /// M278/R2：`$((...))` 里到底能不能算出数而不执行任何东西。
    /// 只允许十进制字面量、变量名、`+ - * / %`、括号和空白——这些在 bash 算术里只
    /// 计算数值；出现 `$`、反引号、引号、`=`（会在算术里赋值）、`[`（数组下标可以嵌
    /// 命令替换）或分隔符，一律不认。
    fn pure_arithmetic_body(body: &str) -> bool {
        if body.trim().is_empty() {
            return false;
        }
        body.chars().all(|ch| match ch {
            '0'..='9' | 'a'..='z' | 'A'..='Z' | '_' | '+' | '-' | '*' | '/' | '%' | '('
            | ')' | ' ' | '\t' => true,
            _ => false,
        })
    }

    fn matching_paren(chars: &[char], start: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut quote: Option<char> = None;
        let mut index = start;
        while index < chars.len() {
            let ch = chars[index];
            if let Some(opening) = quote {
                if ch == '\\' && opening == '"' {
                    index += 2;
                    continue;
                }
                if ch == opening {
                    quote = None;
                }
                index += 1;
                continue;
            }
            match ch {
                '\'' | '"' => {
                    quote = Some(ch);
                    index += 1;
                }
                '`' => return None,
                '(' => {
                    depth += 1;
                    index += 1;
                }
                ')' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(index);
                    }
                    index += 1;
                }
                _ => index += 1,
            }
        }
        None
    }

    fn matching_token(tokens: &[ShellToken], start: usize) -> Option<usize> {
        let mut depth = 0usize;
        for (offset, token) in tokens[start..].iter().enumerate() {
            match token {
                ShellToken::OpenParen => depth += 1,
                ShellToken::CloseParen => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(start + offset);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn flush_shell_segment(current: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
        if !current.is_empty() {
            out.push(std::mem::take(current));
        }
    }

    fn parse_shell_block(
        tokens: &[ShellToken],
        vars: &ShellVars,
        out: &mut Vec<Vec<String>>,
        stop: &[&str],
        depth: usize,
    ) -> Option<(Option<String>, usize)> {
        if depth > MAX_STRUCTURE_DEPTH {
            return None;
        }
        let mut index = 0usize;
        let mut current: Vec<String> = Vec::new();
        while index < tokens.len() {
            match &tokens[index] {
                ShellToken::Sep => {
                    flush_shell_segment(&mut current, out);
                    index += 1;
                }
                ShellToken::CloseParen => return None,
                ShellToken::OpenParen => {
                    if !current.is_empty() {
                        return None;
                    }
                    let close = matching_token(tokens, index)?;
                    let (inner_stop, inner_used) =
                        parse_shell_block(&tokens[index + 1..close], vars, out, &[], depth + 1)?;
                    if inner_stop.is_some() || inner_used != close - index - 1 {
                        return None;
                    }
                    index = close + 1;
                }
                ShellToken::Word {
                    text,
                    substitutions,
                } => {
                    for inner in substitutions {
                        let (inner_stop, inner_used) =
                            parse_shell_block(inner, vars, out, &[], depth + 1)?;
                        if inner_stop.is_some() || inner_used != inner.len() {
                            return None;
                        }
                    }
                    if current.is_empty() && substitutions.is_empty() {
                        let lowered = text.to_ascii_lowercase();
                        if stop.iter().any(|word| *word == lowered) {
                            return Some((Some(lowered), index));
                        }
                        if lowered == "!" {
                            index += 1;
                            continue;
                        }
                        match lowered.as_str() {
                            "for" => {
                                index = parse_for_loop(tokens, index, vars, out, depth)?;
                                continue;
                            }
                            "while" | "until" => {
                                index = parse_while_loop(tokens, index, vars, out, depth)?;
                                continue;
                            }
                            "if" => {
                                index = parse_if_block(tokens, index, vars, out, depth)?;
                                continue;
                            }
                            // Structures we deliberately do not model, and keywords
                            // that only make sense inside one: fail closed.
                            "case" | "select" | "function" | "coproc" | "eval" | "source"
                            | "trap" | "esac" | "then" | "else" | "elif" | "fi" | "do" | "done"
                            | "in" => {
                                return None;
                            }
                            _ => {}
                        }
                    }
                    let resolved = resolve_shell_word(text, vars);
                    // M266 R3: `BASE=$(git rev-parse …) cmd` is an *assignment* whose
                    // value is a substitution — the command word is the next word, so
                    // the segment can still be named. Only a substitution standing in
                    // the command position itself is unnameable.
                    if current.is_empty()
                        && resolved.contains(SUBSTITUTION_MARK)
                        && !is_shell_assignment(&resolved)
                    {
                        return None;
                    }
                    current.push(resolved);
                    index += 1;
                }
            }
        }
        flush_shell_segment(&mut current, out);
        Some((None, index))
    }

    fn parse_for_loop(
        tokens: &[ShellToken],
        index: usize,
        vars: &ShellVars,
        out: &mut Vec<Vec<String>>,
        depth: usize,
    ) -> Option<usize> {
        let mut cursor = index + 1;
        let name = match tokens.get(cursor) {
            Some(ShellToken::Word {
                text,
                substitutions,
            }) if substitutions.is_empty() && is_shell_name(text) => text.clone(),
            _ => return None,
        };
        cursor += 1;
        let mut values: Vec<ShellValue> = Vec::new();
        match tokens.get(cursor) {
            Some(ShellToken::Word {
                text,
                substitutions,
            }) if substitutions.is_empty() && text == "in" => {
                cursor += 1;
                loop {
                    match tokens.get(cursor) {
                        None => return None,
                        Some(ShellToken::Sep) => {
                            cursor += 1;
                            break;
                        }
                        Some(ShellToken::Word {
                            text,
                            substitutions,
                        }) => {
                            if substitutions.is_empty() {
                                if is_shell_literal(text) {
                                    values.push(ShellValue::Literal(text.clone()));
                                } else {
                                    values.push(ShellValue::Unknown);
                                }
                            } else {
                                let only_marks = text.chars().all(|c| c == SUBSTITUTION_MARK)
                                    && text.chars().count() == substitutions.len();
                                if !only_marks {
                                    return None;
                                }
                                for inner in substitutions {
                                    let (inner_stop, inner_used) =
                                        parse_shell_block(inner, vars, out, &[], depth + 1)?;
                                    if inner_stop.is_some() || inner_used != inner.len() {
                                        return None;
                                    }
                                }
                                values.push(ShellValue::Bounded);
                            }
                            cursor += 1;
                        }
                        Some(ShellToken::OpenParen) | Some(ShellToken::CloseParen) => return None,
                    }
                }
            }
            Some(ShellToken::Sep) => {
                cursor += 1;
                values.push(ShellValue::Unknown);
            }
            _ => return None,
        }
        match tokens.get(cursor) {
            Some(ShellToken::Word {
                text,
                substitutions,
            }) if substitutions.is_empty() && text == "do" => {
                cursor += 1;
            }
            _ => return None,
        }
        let body = &tokens[cursor..];
        let unrollable = !values.is_empty()
            && values.len() <= MAX_UNROLLED_ITERATIONS
            && values
                .iter()
                .all(|value| matches!(value, ShellValue::Literal(_)));
        if unrollable {
            let mut done_at: Option<usize> = None;
            for value in &values {
                let ShellValue::Literal(value) = value else {
                    return None;
                };
                let mut scoped = vars.clone();
                scoped.insert(name.clone(), ShellBinding::Literal(value.clone()));
                let mut iteration = Vec::new();
                let (stop, used) =
                    parse_shell_block(body, &scoped, &mut iteration, &["done"], depth + 1)?;
                if stop.as_deref() != Some("done") {
                    return None;
                }
                match done_at {
                    None => done_at = Some(used),
                    Some(expected) if expected == used => {}
                    Some(_) => return None,
                }
                out.extend(iteration);
            }
            return done_at.map(|used| cursor + used + 1);
        }
        let bounded = values
            .iter()
            .any(|value| matches!(value, ShellValue::Bounded));
        let unknown = values
            .iter()
            .any(|value| matches!(value, ShellValue::Unknown));
        let mut scoped = vars.clone();
        if bounded && !unknown {
            scoped.insert(name, ShellBinding::Bounded);
        }
        let (stop, used) = parse_shell_block(body, &scoped, out, &["done"], depth + 1)?;
        if stop.as_deref() != Some("done") {
            return None;
        }
        Some(cursor + used + 1)
    }

    fn parse_while_loop(
        tokens: &[ShellToken],
        index: usize,
        vars: &ShellVars,
        out: &mut Vec<Vec<String>>,
        depth: usize,
    ) -> Option<usize> {
        let cursor = index + 1;
        let (stop, used) = parse_shell_block(&tokens[cursor..], vars, out, &["do"], depth + 1)?;
        if stop.as_deref() != Some("do") {
            return None;
        }
        let body_start = cursor + used + 1;
        let (stop, used) =
            parse_shell_block(&tokens[body_start..], vars, out, &["done"], depth + 1)?;
        if stop.as_deref() != Some("done") {
            return None;
        }
        Some(body_start + used + 1)
    }

    fn parse_if_block(
        tokens: &[ShellToken],
        index: usize,
        vars: &ShellVars,
        out: &mut Vec<Vec<String>>,
        depth: usize,
    ) -> Option<usize> {
        let mut cursor = index + 1;
        loop {
            let (stop, used) =
                parse_shell_block(&tokens[cursor..], vars, out, &["then"], depth + 1)?;
            if stop.as_deref() != Some("then") {
                return None;
            }
            cursor += used + 1;
            let (stop, used) = parse_shell_block(
                &tokens[cursor..],
                vars,
                out,
                &["elif", "else", "fi"],
                depth + 1,
            )?;
            match stop.as_deref() {
                Some("fi") => return Some(cursor + used + 1),
                Some("elif") => {
                    cursor += used + 1;
                }
                Some("else") => {
                    cursor += used + 1;
                    let (stop, used) =
                        parse_shell_block(&tokens[cursor..], vars, out, &["fi"], depth + 1)?;
                    if stop.as_deref() != Some("fi") {
                        return None;
                    }
                    return Some(cursor + used + 1);
                }
                _ => return None,
            }
        }
    }

    fn resolve_shell_word(text: &str, vars: &ShellVars) -> String {
        if !text.contains('$') {
            return text.to_string();
        }
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::new();
        let mut index = 0usize;
        while index < chars.len() {
            if chars[index] != '$' {
                out.push(chars[index]);
                index += 1;
                continue;
            }
            match chars.get(index + 1) {
                Some('{') => {
                    let Some(close) = chars[index + 2..].iter().position(|c| *c == '}') else {
                        out.push('$');
                        index += 1;
                        continue;
                    };
                    let close = index + 2 + close;
                    let name: String = chars[index + 2..close].iter().collect();
                    match vars.get(&name) {
                        Some(ShellBinding::Literal(value)) => out.push_str(value),
                        Some(ShellBinding::Bounded) => out.push(SUBSTITUTION_MARK),
                        None => out.extend(&chars[index..=close]),
                    }
                    index = close + 1;
                }
                Some(c) if c.is_ascii_alphabetic() || *c == '_' => {
                    let mut end = index + 1;
                    while matches!(chars.get(end), Some(c) if c.is_ascii_alphanumeric() || *c == '_')
                    {
                        end += 1;
                    }
                    let name: String = chars[index + 1..end].iter().collect();
                    match vars.get(&name) {
                        Some(ShellBinding::Literal(value)) => out.push_str(value),
                        Some(ShellBinding::Bounded) => out.push(SUBSTITUTION_MARK),
                        None => out.extend(&chars[index..end]),
                    }
                    index = end;
                }
                _ => {
                    out.push('$');
                    index += 1;
                }
            }
        }
        out
    }

    /// M266 R3: `NAME=value` shapes that bind a shell variable rather than name a
    /// command. Such a prefix may carry a substitution and still leave a command
    /// word behind it.
    fn is_shell_assignment(word: &str) -> bool {
        word.split_once('=')
            .is_some_and(|(name, _)| is_shell_name(name))
    }

    fn is_shell_name(text: &str) -> bool {
        let mut chars = text.chars();
        match chars.next() {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
            _ => return false,
        }
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    fn is_shell_literal(text: &str) -> bool {
        !text.is_empty()
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._/:@%+=,-".contains(c))
    }

    /// M260：循环 / 子 shell / `$(...)` / `if` 里的纯本地命令，不再因为「结构解析不了」
    /// 整条进审批；含远端写、破坏性改写、解释器 heredoc 的仍走既有判定。
    ///
    /// 沙箱闸门是 `covers_local_command(enforced, destructive, remote, outside_scope)`，
    /// 因此这里断言的是它的四个输入：`sensitive`（是否进入 M159 判定）、
    /// `destructive_intent`、`is_delivery_command`、`outside_write_sandbox_scope`。
    #[cfg(test)]
    mod m260_structures {
        use super::super::{destructive_intent, outside_write_sandbox_scope, sensitive};
        use codefactory_agent_loop::policy::is_delivery_command;

        /// R2 样本：现场原样（会话 00b00066 被拦下的那条）。
        const FIELD_SAMPLE: &str = "for c in c9eb5089c a00d7c16f; do echo \"--- $c ---\"; git log -1 --format='%B' $c | tail -8; done && grep -rn \"Release-Urgency\" .github/workflows/*.yml | head -10";

        /// R2：结构里只有本地 git 读写和普通命令。
        const LOCAL_STRUCTURES: [&str; 6] = [
            "for f in docs/a.md docs/b.md; do git add $f; done",
            "(git add -A && git commit -m \"synthetic\")",
            "while git status --porcelain | grep -q .; do git log -1 --oneline; done",
            "if git diff --quiet; then echo clean; else git log -1 --oneline; fi",
            "cd \"$(git rev-parse --show-toplevel)\" && git status --short",
            "for c in $(git rev-list -n 2 HEAD); do git show --stat $c; done",
        ];

        fn local_exemption_applies(command: &str) -> bool {
            !destructive_intent(command)
                && !is_delivery_command(command)
                && !outside_write_sandbox_scope(command)
        }

        #[test]
        fn m260_field_sample_produces_no_approval() {
            assert!(
                !sensitive(FIELD_SAMPLE),
                "field sample must not enter the unproved-git prompt: {FIELD_SAMPLE}"
            );
            assert!(
                !destructive_intent(FIELD_SAMPLE),
                "destructive: {FIELD_SAMPLE}"
            );
            assert!(!is_delivery_command(FIELD_SAMPLE), "remote: {FIELD_SAMPLE}");
            assert!(
                !outside_write_sandbox_scope(FIELD_SAMPLE),
                "outside_scope: {FIELD_SAMPLE}"
            );
            assert!(local_exemption_applies(FIELD_SAMPLE));
        }

        #[test]
        fn m260_local_structures_need_no_approval() {
            for command in LOCAL_STRUCTURES {
                assert!(
                    local_exemption_applies(command),
                    "local structure must stay inside the sandbox exemption: {command}"
                );
            }
        }

        #[test]
        fn m260_loop_with_remote_write_still_asks() {
            for command in [
                "for c in main dev; do git push origin $c; done",
                "while read b; do gh pr merge $b; done",
            ] {
                assert!(
                    is_delivery_command(command),
                    "remote write must stay remote: {command}"
                );
                assert!(
                    !local_exemption_applies(command),
                    "remote write must not be exempted by the sandbox: {command}"
                );
            }
        }

        #[test]
        fn m260_subshell_destructive_rewrite_still_asks() {
            for command in [
                "(git reset --hard origin/main)",
                "if git status --porcelain | grep -q .; then (git reset --hard origin/main); fi",
                "for c in origin/main; do git reset --hard $c; done",
            ] {
                assert!(
                    destructive_intent(command),
                    "destructive rewrite must stay destructive: {command}"
                );
                assert!(
                    !local_exemption_applies(command),
                    "destructive rewrite must not be exempted by the sandbox: {command}"
                );
            }
        }

        #[test]
        fn m260_loop_with_python_heredoc_still_asks() {
            let commands = [
                "for f in a.txt b.txt; do python3 - <<'PY'\nopen(f).read()\nPY\ndone",
                "while read f; do python3 - <<'PY'\nopen(f).read()\nPY\ndone",
            ];
            for command in commands {
                assert!(
                    sensitive(command),
                    "interpreter heredoc must stay unproved: {command}"
                );
                assert!(
                    !local_exemption_applies(command),
                    "interpreter heredoc must not be exempted: {command}"
                );
            }
        }

        #[test]
        fn m260_unparseable_structure_fails_closed() {
            // 原先这里还有一条 `for c in a b; do echo $((1+2)); done`：循环本身是有模型的
            // （见 LOCAL_STRUCTURES，按内容逐段判定），它当年能失败关闭靠的是 `$((...))`
            // 未证明。M278/R2 把纯算术证明成字面量后，这条只剩模型内的正常内容，已不是
            // 「未建模结构」的例子，因此移除；纯算术免审批的情况由 M278 回归表直接断言。
            for command in [
                "for c in a b; do echo `git rev-parse HEAD`; done",
                "case $x in a) git log -1;; esac",
                "for c in a b; do $(echo git) log -1; done",
            ] {
                assert!(
                    sensitive(command) || destructive_intent(command),
                    "unmodelled structure must fail closed: {command}"
                );
            }
        }

        #[test]
        fn m260_plain_git_judgements_unchanged() {
            assert!(!sensitive("git log -1 --format='%B'"));
            assert!(!destructive_intent("git log -1 --format='%B'"));
            assert!(sensitive("git reset --hard origin/main"));
            assert!(destructive_intent("git reset --hard origin/main"));
            assert!(is_delivery_command("git push origin main"));
        }
    }
}

/// M266 regression table: "is this a git operation?" is decided by the parsed
/// executable positions, never by a substring anywhere in the command text.
#[cfg(test)]
mod m266_tests {
    // This file is included verbatim by the integration tests under
    // `src-tauri/tests/` (`#[path = "../src/agent/trusted_git.rs"]`), where
    // neither `crate::agent` nor `crate::tools` exists and a nested `#[path]`
    // resolves against the including file's directory. So `super` is the git
    // module in both builds, and the shell half of the approval decision is
    // asserted in `src/tools/shell_policy.rs` instead of here.
    use super as trusted_git;

    /// Field samples that were refused with `trusted.git.unproved` although no git
    /// operation runs in an executable position. `sensitive` must be false, which
    /// is what keeps them out of the Git proof + approval path.
    const NOT_GIT_OPERATIONS: &[(&str, &str)] = &[
        (
            "R2/quoted-search-pattern",
            r#"grep -o "path:src-tauri/tests/[^\"]*" docs/testing/scenario-registry.json | sort -u | head; echo; grep -c "rust:" docs/testing/scenario-registry.json"#,
        ),
        (
            "R2/for-loop-with-command-substitution",
            r#"for i in 1 2 3 4 5 6; do out=$(CARGO_TARGET_DIR=$PWD/.codefactory-cache/cargo-target-m266 cargo test --manifest-path src-tauri/Cargo.toml 2>&1); echo "run $i: done"; done"#,
        ),
        (
            "R3/read-only-git-in-substitution",
            r#"BASE=$(git rev-parse origin/main) && SCENARIO_TEST_BASE_SHA=$BASE python3 tools/governance/validate_scenario_test_governance.py --ci --base-ref origin/main"#,
        ),
        ("baseline/read-only-git", "git log --oneline -5 | head"),
        (
            "baseline/git-word-inside-a-search-pattern",
            r#"grep -rn "git push origin main" src-tauri/src | head"#,
        ),
        (
            "baseline/git-word-inside-a-path",
            "ls .codefactory-cache/git-target-m266 && wc -l src-tauri/src/agent/trusted_git.rs",
        ),
    ];

    /// Real git operations. These must stay behind approval (R5): a stricter parser
    /// may not turn a write, a force-push or an unprovable command into an allow.
    /// Plain `git push` is owned by the delivery-authorization gate rather than the
    /// Git proof, so the predicate below covers both owners.
    const ASKS_FOR_APPROVAL: &[(&str, &str)] = &[
        ("R5/push", "git push origin main"),
        ("R5/force-push", "git push --force origin main"),
        ("R5/reset-hard", "git reset --hard HEAD"),
        ("R5/clean", "git clean -fdx"),
        (
            "R5/push-inside-shell-payload",
            r#"bash -c "git reset --hard""#,
        ),
        (
            "R5/unenumerable-command",
            "git push origin main; echo \"unterminated",
        ),
        // A backtick substitution runs at run time, so its content cannot be
        // unfolded into simple commands. It stays behind approval even though the
        // word it sits in is only an argument.
        ("R5/unenumerable-backticks", "echo `git status`"),
    ];

    /// The Git half of the approval decision. The shell-safety half is asserted
    /// where it lives (`tools::shell_policy`), because this file is compiled inside
    /// the integration tests and cannot reach that module.
    fn asks_for_approval(command: &str) -> bool {
        trusted_git::sensitive(command)
            || codefactory_agent_loop::policy::is_delivery_command(command)
    }

    #[test]
    fn m266_not_git_operations_are_not_sensitive() {
        let mut failures = Vec::new();
        for (name, command) in NOT_GIT_OPERATIONS {
            if asks_for_approval(command) {
                failures.push(format!(
                    "{name}: still needs approval ({:?}, sensitive={}) -> {command}",
                    trusted_git::git_entry(command),
                    trusted_git::sensitive(command),
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "M266/R2 false positives remain:\n{}",
            failures.join("\n")
        );
    }

    #[test]
    fn m266_real_git_operations_stay_sensitive() {
        let mut failures = Vec::new();
        for (name, command) in ASKS_FOR_APPROVAL {
            if !asks_for_approval(command) {
                failures.push(format!("{name}: no longer asks -> {command}"));
            }
        }
        assert!(
            failures.is_empty(),
            "M266/R5 regressions:\n{}",
            failures.join("\n")
        );
    }

    /// M278/R2：`$(( ))` 是算术，不是命令替换。纯算术体照 `$VAR` 一样当字面量，
    /// 里面还能藏命令的写法一律继续要求审批。
    #[test]
    fn m278_pure_arithmetic_is_literal_but_nested_commands_still_ask() {
        let literal = [
            ("literal", "echo $((1+1))"),
            ("quoted", "echo \"$(( (1+2)*3 ))\""),
            ("variable", "echo $((count % 7))"),
            ("prefix-assignment", "y=$((a*2)) echo ok"),
            ("subtraction", "echo $((n-1))"),
        ];
        let mut failures = Vec::new();
        for (name, command) in literal {
            if asks_for_approval(command) {
                failures.push(format!("{name}: still asks -> {command}"));
            }
        }
        assert!(
            failures.is_empty(),
            "M278/R2 arithmetic false positives:\n{}",
            failures.join("\n")
        );

        let commands = [
            ("nested substitution", "echo $(( $(rm -rf /tmp/m278) ))"),
            ("backtick", "echo $((`id`))"),
            ("arithmetic assignment", "echo $((x=1))"),
            ("array subscript", "echo $((arr[$(id)]))"),
        ];
        let mut failures = Vec::new();
        for (name, command) in commands {
            if !asks_for_approval(command) {
                failures.push(format!("{name}: no longer asks -> {command}"));
            }
        }
        assert!(
            failures.is_empty(),
            "M278/R2 arithmetic regressions:\n{}",
            failures.join("\n")
        );
    }

    /// R4: the message heredoc keeps its proof when the same line sends `2>&1`
    /// through a read-only output filter (`... <<'EOF' 2>&1 | tail -25`).
    #[test]
    fn m266_message_heredoc_survives_output_pipeline() {
        let command = "git commit -q -F - <<'EOF' 2>&1 | tail -25\nfeat(m266): entry review\nEOF";
        assert!(
            trusted_git::python_heredoc_proved(Some(&std::env::temp_dir()), command),
            "M266/R4: message heredoc behind a read-only output pipeline should be proved as data"
        );
    }

    /// R4 negative control: interpreter input is not message data.
    #[test]
    fn m266_interpreter_heredoc_is_still_unproved() {
        let command = "python3 - <<'EOF'\nimport os\nos.system('git reset --hard')\nEOF";
        assert!(
            !trusted_git::python_heredoc_proved(Some(&std::env::temp_dir()), command),
            "M266/R4: an interpreter heredoc must stay unproved"
        );
    }
}

/// M281 R2 (red first): a read-only `for` loop over a bounded list is still
/// refused with `trusted.shell.unproved`. The structure prover knows how to
/// unroll literal lists, but a list produced by a substitution (`$(git
/// ls-files)`, `$(find …)`) or a glob is `ShellValue::Bounded`/`Unknown`, so
/// the loop body loses its binding and the whole line collapses into "cannot
/// be decomposed into provable simple commands".
#[cfg(test)]
mod m281_readonly_for_loops {
    use super::sensitive;
    use super::standalone_shell_policy;

    /// R2 现场形态：只读的 `for` 循环，列表来自 `$()`，循环体只读文件与 Git 状态。
    const READ_ONLY_LOOPS: [&str; 5] = [
        "for f in $(git ls-files '*.rs'); do grep -n TODO \"$f\"; done",
        "for b in $(git branch --format='%(refname:short)'); do echo \"$b\"; done",
        "for f in $(find . -name '*.md' -not -path './target/*'); do wc -l \"$f\"; done",
        "for f in src/*.rs; do sed -n 1,5p \"$f\"; done",
        "for i in 1 2 3; do git status --porcelain; done",
    ];

    #[test]
    fn read_only_for_loops_are_proved_local() {
        let mut failures = Vec::new();
        for command in READ_ONLY_LOOPS {
            if sensitive(command) {
                failures.push(command);
            }
        }
        assert!(
            failures.is_empty(),
            "M281 R2: these read-only for loops must not require approval: {}",
            failures.join("\n")
        );
    }

    /// Negative control: the same structure carrying a destructive command is
    /// not silently proved — the shell-safety table still asks.
    #[test]
    fn for_loop_with_a_destructive_body_still_asks() {
        for command in [
            "for f in a b; do rm -rf \"$f\"; done",
            "for f in a b; do git reset --hard \"$f\"; done",
        ] {
            assert!(
                !matches!(
                    standalone_shell_policy::classify_command(command),
                    standalone_shell_policy::ShellCommandPolicy::Allow { .. }
                ),
                "M281 R2 negative control: {command} must not be auto-allowed"
            );
        }
    }
}

