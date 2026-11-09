// SPDX-License-Identifier: Apache-2.0
//! Bounded, non-executing proof for inline interpreter inputs. Unknown syntax
//! is an approval boundary, not evidence that a destructive command was found.
#[path = "shell_structure.rs"]
pub(crate) mod shell_structure;

const MAX_SOURCE: usize = 32 * 1024;
const MAX_DEPTH: usize = 8;

pub(crate) fn interpreter_name(binary: &str) -> Option<&'static str> {
    let lower = binary.rsplit(['/', '\\']).next()?.to_ascii_lowercase();
    let name = lower.strip_suffix(".exe").unwrap_or(&lower);
    if name == "python"
        || name.strip_prefix("python").is_some_and(|v| {
            !v.is_empty()
                && v.bytes().all(|b| b.is_ascii_digit() || b == b'.')
                && v.as_bytes()[0].is_ascii_digit()
        })
    {
        return Some("python");
    }
    match name {
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "ash" => Some("shell"),
        "node" | "nodejs" | "deno" | "bun" => Some("javascript"),
        "ruby" | "perl" | "pwsh" | "powershell" | "cmd" => Some("other"),
        _ => None,
    }
}

/// Script-file invocations retain their existing policy; inline, module and
/// stdin programs must be proved here (heredocs have a separate stronger gate).
pub(crate) fn unproved(command: &str) -> bool {
    unproved_at(command, 0, false)
}

/// An inline proof also needs an unchanged invocation environment. Companions
/// outside this small positive set may be shell builtins/functions that assign
/// variables; knowing no particular loader variable name is not evidence.
/// Interpreter and Git processes retain their own independent effect gates.
fn environment_neutral_segment(words: &[String]) -> bool {
    let mut i = 0;
    while let Some(word) = words.get(i) {
        if word.contains('=') {
            return false;
        }
        let lower = word
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(word)
            .to_ascii_lowercase();
        let name = lower.strip_suffix(".exe").unwrap_or(&lower);
        if matches!(
            name,
            "env" | "command" | "exec" | "sudo" | "nohup" | "time" | "builtin"
        ) {
            i += 1;
            continue;
        }
        if interpreter_name(word).is_some() {
            return true;
        }
        if name == "printf" {
            return !words[i + 1..].iter().any(|arg| {
                arg.starts_with("-v") || arg.contains(['$', '`', '\\', '*', '?', '[', ']', '\u{1}'])
            });
        }
        return matches!(
            name,
            "grep"
                | "rg"
                | "cat"
                | "head"
                | "tail"
                | "pwd"
                | "ls"
                | "echo"
                | "true"
                | "false"
                | "git"
        );
    }
    false
}

fn unproved_at(command: &str, depth: usize, shell_payload: bool) -> bool {
    if command.len() > MAX_SOURCE || depth > MAX_DEPTH {
        return true;
    }
    let Some(segments) = shell_structure::executable_segments(command) else {
        return true;
    };
    let environment_mutation = shell_structure::has_environment_mutation(command).unwrap_or(true)
        || segments
            .iter()
            .any(|words| !environment_neutral_segment(words));
    for words in segments {
        let mut i = 0;
        while let Some(word) = words.get(i) {
            let wrapper_name = word
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(word)
                .to_ascii_lowercase();
            let wrapper_name = wrapper_name.strip_suffix(".exe").unwrap_or(&wrapper_name);
            if word.contains('=')
                || matches!(
                    wrapper_name,
                    "env" | "command" | "exec" | "sudo" | "nohup" | "time" | "builtin"
                )
            {
                i += 1;
            } else {
                break;
            }
        }
        let Some(binary) = words.get(i) else {
            continue;
        };
        if binary.starts_with('-') {
            // `command -v` / `-V` is executable discovery, not execution.
            if i > 0 && words[i - 1] == "command" && matches!(binary.as_str(), "-v" | "-V") {
                continue;
            }
            return true;
        }
        let executable = binary
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(binary)
            .to_ascii_lowercase();
        let executable = executable.strip_suffix(".exe").unwrap_or(&executable);
        if matches!(
            executable,
            "timeout"
                | "xargs"
                | "parallel"
                | "nice"
                | "setsid"
                | "busybox"
                | "chrt"
                | "taskset"
                | "stdbuf"
                | "ionice"
                | "su"
                | "doas"
                | "runuser"
                | "chroot"
                | "alias"
                | "eval"
                | "trap"
        ) {
            return true;
        }
        if binary.contains(['$', '`', '\\', '*', '?', '[', ']', '\u{1}']) {
            return true;
        }
        let Some(kind) = interpreter_name(binary) else {
            if shell_payload
                && !matches!(
                    binary.as_str(),
                    "git"
                        | "echo"
                        | "printf"
                        | "pwd"
                        | "ls"
                        | "cat"
                        | "head"
                        | "tail"
                        | "wc"
                        | "grep"
                        | "rg"
                        | "true"
                        | "false"
                        | "test"
                        | "["
                )
            {
                return true;
            }
            continue;
        };
        let args = &words[i + 1..];
        let base_name = binary
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(binary)
            .to_ascii_lowercase();
        let base = base_name.strip_suffix(".exe").unwrap_or(&base_name);
        if base == "deno" && args.first().is_some_and(|a| a == "eval") {
            if environment_mutation {
                return true;
            }
            if args.len() != 2
                || args.get(1).is_none_or(|body| {
                    body.contains(['$', '`', '\u{1}'])
                        || !literal_output_program(body, "javascript")
                })
            {
                return true;
            }
            continue;
        }
        if matches!(base, "bun" | "node" | "nodejs")
            && args
                .first()
                .is_some_and(|a| matches!(a.as_str(), "exec" | "inspect"))
        {
            return true;
        }
        if args.is_empty() {
            return true;
        }
        let mut a = 0;
        let mut payload = None;
        while a < args.len() {
            let arg = args[a].as_str();
            if arg.contains(['$', '`', '\\', '*', '?', '[', ']', '\u{1}']) {
                return true;
            }
            let code_flag = match kind {
                "python" => arg == "-c",
                "shell" => matches!(arg, "-c" | "-lc" | "-ec" | "-ic"),
                "javascript" => matches!(arg, "-e" | "-p" | "--eval" | "--print"),
                _ => matches!(
                    arg.to_ascii_lowercase().as_str(),
                    "-e" | "-c" | "/c" | "-command"
                ),
            };
            if code_flag {
                let Some(body) = args.get(a + 1) else {
                    return true;
                };
                if kind != "python" && args.len() != a + 2 {
                    return true;
                }
                payload = Some(body.as_str());
                break;
            }
            if kind == "python" && arg.starts_with("-c") && arg.len() > 2 {
                payload = Some(&arg[2..]);
                break;
            }
            // Only flags with no user-code side effects may precede a proof.
            if (kind == "python" && matches!(arg, "-I" | "-S" | "-E" | "-s" | "-u" | "-B"))
                || (kind == "shell" && matches!(arg, "--noprofile" | "--norc"))
            {
                a += 1;
                continue;
            }
            if arg == "--version" || arg == "-V" {
                if a + 1 == args.len() {
                    break;
                } else {
                    return true;
                }
            }
            if arg.starts_with('-') || arg.starts_with('/') && kind == "other" {
                return true;
            }
            if matches!(base, "powershell" | "pwsh" | "cmd") {
                return true;
            }
            // A filename is not inline code. Its execution remains governed by
            // the pre-existing script-file/tool policy, not this literal proof.
            break;
        }
        if a == args.len() {
            return true;
        }
        if let Some(body) = payload {
            if environment_mutation {
                return true;
            }
            // The structure parser intentionally loses quote provenance. A
            // retained expansion might have originated inside double quotes;
            // never prove the pre-expansion literal as the executed program.
            if body.contains(['$', '`', '\u{1}']) {
                return true;
            }
            if kind == "shell" {
                if unproved_at(body, depth + 1, true) {
                    return true;
                }
            } else if !literal_output_program(body, kind) {
                return true;
            }
        }
    }
    false
}

/// Deliberately small positive grammar: comments/pass and calls to print or
/// console.log with scalar literals only. No imports, variables, interpolation,
/// attribute lookup, operators, user calls or runtime evaluation is admitted.
pub(crate) fn literal_output_program(source: &str, kind: &str) -> bool {
    if source.len() > MAX_SOURCE {
        return false;
    }
    let mut p = source.trim();
    while !p.is_empty() {
        if kind == "python" && p.starts_with('#') {
            p = p
                .split_once(['\n', '\r'])
                .map_or("", |(_, rest)| rest)
                .trim_start();
            continue;
        }
        if kind == "python"
            && (p == "pass"
                || p.starts_with("pass;")
                || p.starts_with("pass\n")
                || p.starts_with("pass ")
                || p.starts_with("pass\t"))
        {
            p = &p[4..];
        } else {
            let call = match kind {
                "python" => "print",
                "javascript" => "console.log",
                _ => return false,
            };
            let Some(rest) = p.strip_prefix(call) else {
                return false;
            };
            let Some(rest) = rest.trim_start().strip_prefix('(') else {
                return false;
            };
            p = rest.trim_start();
            if !p.starts_with(')') {
                loop {
                    let Some(rest) = literal(p, kind) else {
                        return false;
                    };
                    p = rest.trim_start();
                    if let Some(rest) = p.strip_prefix(',') {
                        p = rest.trim_start();
                        if p.starts_with(')') {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
            let Some(rest) = p.strip_prefix(')') else {
                return false;
            };
            p = rest;
        }
        p = p.trim_start_matches([' ', '\t', '\r']);
        if p.is_empty() {
            return true;
        }
        if kind == "python" && p.starts_with('#') {
            continue;
        }
        if !p.starts_with([';', '\n']) {
            return false;
        }
        p = p[1..].trim_start();
    }
    true
}

fn literal<'a>(s: &'a str, kind: &str) -> Option<&'a str> {
    let first = *s.as_bytes().first()?;
    if first == b'\'' || first == b'"' {
        let bytes = s.as_bytes();
        let mut i = 1;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => {
                    i += 2;
                }
                b'\n' | b'\r' => return None,
                b if b == first => return Some(&s[i + 1..]),
                _ => i += 1,
            }
        }
        return None;
    }
    for keyword in if kind == "python" {
        &["True", "False", "None"][..]
    } else {
        &["true", "false", "null"][..]
    } {
        if let Some(rest) = s.strip_prefix(keyword) {
            return Some(rest);
        }
    }
    let mut end = usize::from(first == b'-');
    let start = end;
    while s.as_bytes().get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    (end > start).then(|| &s[end..])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn positive_literal_boundary() {
        for s in [
            "python3 -c 'print(1)'",
            "python3.11 -I -c 'print(\"git reset --hard\")'",
            "/usr/bin/python3 -c 'pass # no operation'",
            "node -e 'console.log(\"git reset --hard\")'",
            "grep 'python -c evil' file",
            "bash -c 'git status --short'",
            "echo 'git reset --hard'",
            "python3 tools/check.py",
            "python3 -c 'print(1)' --ignored",
            "python3 -c'print(1)'",
        ] {
            assert!(!unproved(s), "{s}");
        }
        for s in [
            "python3 -c 'eval(\"print(1)\")'",
            "python3 -c 'print(f\"{evil()}\")'",
            "python3 -c 'print(1+f())'",
            "python3 -c 'print(1); evil()'",
            "python3 -",
            "python3 -m module",
            "python3 -c",
            "python3 -X foo -c 'print(1)'",
        ] {
            assert!(unproved(s), "{s}");
        }
    }
    #[test]
    fn resource_and_parse_failures_are_unproved() {
        assert!(unproved(&" ".repeat(MAX_SOURCE + 1)));
        assert!(unproved("python3 -c 'unterminated"));
        let source = format!(
            "for a in {}; do for b in {}; do echo ok; done; done",
            (0..64).map(|i| i.to_string()).collect::<Vec<_>>().join(" "),
            (0..64).map(|i| i.to_string()).collect::<Vec<_>>().join(" ")
        );
        assert!(unproved(&source));
        // A global traversal budget also bounds loops that emit no commands.
        let values = (0..64).map(|i| i.to_string()).collect::<Vec<_>>().join(" ");
        let mut empty = ";".to_string();
        for _ in 0..5 {
            empty = format!("for x in {values}; do {empty} done;");
        }
        assert!(unproved(&empty));
    }
}
