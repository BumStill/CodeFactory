// SPDX-License-Identifier: Apache-2.0
//! M147b: a PR body rewrite must not erase the session's release-urgency trailer.
//!
//! Delivery writes `Release-Urgency:` into the PR body, but the body is the only
//! carrier and any later turn can overwrite it with `gh pr edit --body`. #818 was
//! exactly that: `sessions.release_urgency=immediate`, merged body without the
//! trailer, no release cut. The commit message keeps a durable copy (see
//! `delivery_thin::run`); this module keeps the body honest at the tool layer.
use std::path::{Path, PathBuf};

use crate::agent::delivery::{self, ReleaseUrgency};
use super::ExecCtx;

/// A rewritten command, plus the audit line and the temp copies it owns.
pub(super) struct Guarded {
    pub command: String,
    pub note: String,
    // Held so the temp `--body-file` copies live until the command has run.
    _temps: Vec<TempCopy>,
}

struct TempCopy(PathBuf);

impl Drop for TempCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The session's persisted release policy, when this run has a session row.
pub(super) async fn session_urgency(ctx: &ExecCtx) -> Option<ReleaseUrgency> {
    let (Some(db), Some(session)) = (&ctx.db, &ctx.session_id) else {
        return None;
    };
    let urgency: String = sqlx::query_scalar("SELECT release_urgency FROM sessions WHERE id=?")
        .bind(session)
        .fetch_one(db)
        .await
        .ok()?;
    match urgency.as_str() {
        "immediate" => Some(ReleaseUrgency::Immediate),
        "hold" => Some(ReleaseUrgency::Hold),
        _ => None,
    }
}

/// One shell word: its byte span in the original command and its unquoted text.
struct Token {
    start: usize,
    end: usize,
    text: String,
}

/// Split a command line into words, honouring single quotes, double quotes and
/// backslash escapes. Spans are kept so a rewrite can splice the source instead
/// of re-serialising the whole command (which would mangle unrelated quoting).
fn tokenize(command: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut iter = command.char_indices().peekable();
    while let Some(&(start, ch)) = iter.peek() {
        if ch.is_whitespace() {
            iter.next();
            continue;
        }
        let mut text = String::new();
        let mut end = start;
        let mut quote: Option<char> = None;
        while let Some(&(index, current)) = iter.peek() {
            if let Some(active) = quote {
                if current == active {
                    quote = None;
                    iter.next();
                    end = index + current.len_utf8();
                    continue;
                }
                if active == '"' && current == '\\' {
                    // POSIX: inside double quotes only `$ ` " \` and newline are
                    // escaped; a `\n` in a PR body must stay a literal backslash-n,
                    // otherwise the body text we read back differs from what `gh`
                    // receives.
                    iter.next();
                    match iter.peek().copied() {
                        Some((escape_index, escaped)) if matches!(escaped, '"' | '\\' | '$' | '`') => {
                            text.push(escaped);
                            iter.next();
                            end = escape_index + escaped.len_utf8();
                        }
                        Some((escape_index, '\n')) => {
                            iter.next();
                            end = escape_index + 1;
                        }
                        _ => {
                            text.push('\\');
                            end = index + 1;
                        }
                    }
                    continue;
                }
                text.push(current);
                iter.next();
                end = index + current.len_utf8();
                continue;
            }
            if current.is_whitespace() {
                break;
            }
            if current == '\'' || current == '"' {
                quote = Some(current);
                iter.next();
                end = index + current.len_utf8();
                continue;
            }
            if current == '\\' {
                iter.next();
                if let Some(&(_, escaped)) = iter.peek() {
                    text.push(escaped);
                }
                iter.next();
                end = index + current.len_utf8();
                continue;
            }
            text.push(current);
            iter.next();
            end = index + current.len_utf8();
        }
        tokens.push(Token { start, end, text });
    }
    tokens
}

fn is_program(text: &str, name: &str) -> bool {
    Path::new(text)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.eq_ignore_ascii_case(name))
}

/// Quote one word for the shell the command will run in.
fn shell_quote(value: &str) -> String {
    if cfg!(windows) {
        // PowerShell: a single-quoted string is literal; `''` is an apostrophe.
        format!("'{}'", value.replace('\'', "''"))
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn temp_body_copy(body: &str) -> std::io::Result<PathBuf> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "codefactory-pr-body-{}-{seq}.md",
        std::process::id()
    ));
    std::fs::write(&path, body)?;
    Ok(path)
}

/// Rewrite `gh pr edit` so the body it writes still carries the session trailer.
pub(super) fn guard_command(command: &str, cwd: &Path, urgency: ReleaseUrgency) -> Option<Guarded> {
    let tokens = tokenize(command);
    let gh = tokens.iter().position(|token| is_program(&token.text, "gh"))?;
    // Tolerate `gh -R owner/repo pr edit`; anything else is not the shape we guard.
    let mut index = gh + 1;
    while let Some(token) = tokens.get(index) {
        if token.text == "-R" || token.text == "--repo" {
            index += 2;
        } else if token.text.starts_with('-') {
            index += 1;
        } else {
            break;
        }
    }
    if tokens.get(index)?.text != "pr" || tokens.get(index + 1)?.text != "edit" {
        return None;
    }

    // (span, replacement) edits, applied in reverse so earlier spans stay valid.
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut temps: Vec<TempCopy> = Vec::new();
    let mut carried_file = false;
    let mut position = index + 2;
    while position < tokens.len() {
        let token = &tokens[position];
        let (flag, inline) = match token.text.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_string())),
            _ => (token.text.as_str(), None),
        };
        let is_body = flag == "--body" || flag == "-b";
        let is_body_file = flag == "--body-file" || flag == "-F";
        if !is_body && !is_body_file {
            position += 1;
            continue;
        }
        let (value, span) = match inline.as_ref() {
            Some(value) => (value.clone(), (token.start, token.end)),
            None => match tokens.get(position + 1) {
                Some(next) => (next.text.clone(), (next.start, next.end)),
                None => break,
            },
        };
        position = if inline.is_some() { position + 1 } else { position + 2 };
        let updated = delivery::append_release_urgency(value.clone(), Some(urgency));
        if updated == value {
            continue;
        }
        let replacement = if is_body {
            shell_quote(&updated)
        } else {
            // Never edit the caller's file: hand `gh` a temp copy that carries the
            // trailer, so the workspace stays exactly as the model wrote it.
            let Ok(path) = cwd.join(&value).canonicalize().and_then(|path| {
                let source = std::fs::read_to_string(&path)?;
                temp_body_copy(&delivery::append_release_urgency(source, Some(urgency)))
            }) else {
                continue;
            };
            carried_file = true;
            let quoted = shell_quote(&path.display().to_string());
            temps.push(TempCopy(path));
            quoted
        };
        let replacement = match inline {
            Some(_) => format!("{flag}={replacement}"),
            None => replacement,
        };
        edits.push((span.0, span.1, replacement));
    }
    if edits.is_empty() {
        return None;
    }
    edits.sort_by_key(|(start, _, _)| *start);
    let mut guarded = command.to_string();
    for (start, end, replacement) in edits.into_iter().rev() {
        guarded.replace_range(start..end, &replacement);
    }
    Some(Guarded {
        command: guarded,
        note: format!(
            "M147b：已把会话的 Release-Urgency: {} 补回本次 gh pr edit 写入的 PR 正文{}，正文覆盖不会抹掉交付 trailer。",
            match urgency { ReleaseUrgency::Immediate => "immediate", ReleaseUrgency::Hold => "hold" },
            if carried_file { "（--body-file 用临时副本，原文件未改）" } else { "" }
        ),
        _temps: temps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn immediate() -> ReleaseUrgency {
        ReleaseUrgency::Immediate
    }

    #[test]
    fn m147b_gh_pr_edit_body_keeps_the_trailer() {
        let cwd = std::env::temp_dir();
        let guarded = guard_command(
            "gh pr edit 818 --body \"plain body\"",
            &cwd,
            immediate(),
        )
        .expect("a gh pr edit --body without the trailer must be rewritten");
        assert!(guarded.command.contains("Release-Urgency: immediate"), "{}", guarded.command);
        assert!(guarded.command.contains("plain body"), "{}", guarded.command);
        assert!(guarded.command.starts_with("gh pr edit 818 --body "), "{}", guarded.command);
        assert!(guarded.note.contains("Release-Urgency: immediate"), "{}", guarded.note);
    }

    #[test]
    fn m147b_body_that_already_carries_the_trailer_is_left_alone() {
        let cwd = std::env::temp_dir();
        // A real body arrives with real newlines inside the quoted word (a shell
        // passes them through), so the guard must see the trailer and stand down.
        let command = "gh pr edit 818 --body \"plain body\n\nRelease-Urgency: immediate\"";
        assert!(guard_command(command, &cwd, immediate()).is_none(), "{command}");
    }

    #[test]
    fn m147b_body_file_is_copied_not_edited() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("body.md");
        std::fs::write(&source, "plain body").unwrap();
        let guarded = guard_command(
            &format!("gh pr edit 818 --body-file {}", source.display()),
            dir.path(),
            immediate(),
        )
        .expect("a body file without the trailer must be rewritten");
        assert!(!guarded.command.contains(&source.display().to_string()), "{}", guarded.command);
        let copy = guarded
            .command
            .split_once("--body-file ")
            .and_then(|(_, rest)| rest.split('\'').nth(1))
            .map(PathBuf::from)
            .expect("the rewritten command names a temp copy");
        let written = std::fs::read_to_string(&copy).unwrap();
        assert!(written.contains("Release-Urgency: immediate"), "{written}");
        assert_eq!(std::fs::read_to_string(&source).unwrap(), "plain body");
    }

    #[test]
    fn m147b_unrelated_commands_are_never_rewritten() {
        let cwd = std::env::temp_dir();
        for command in [
            "gh pr create --title 'fix: x' --body 'plain'",
            "git commit -m 'fix: x'",
            "gh pr edit 818 --add-label 'merge-queue: armed'",
            "echo 'gh pr edit 818 --body plain'",
        ] {
            assert!(guard_command(command, &cwd, immediate()).is_none(), "{command}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn m147b_the_real_gh_receives_the_trailer_when_the_body_is_rewritten() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let args_file = dir.path().join("args.txt");
        let fake = dir.path().join("gh");
        std::fs::write(
            &fake,
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {}\n", args_file.display()),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let guarded = guard_command("gh pr edit 818 --body 'plain body'", dir.path(), immediate())
            .expect("the command must be rewritten");
        let status = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&guarded.command)
            .env("PATH", format!("{}:{}", dir.path().display(), std::env::var("PATH").unwrap_or_default()))
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        let received = std::fs::read_to_string(&args_file).unwrap();
        assert!(received.contains("plain body"), "{received}");
        assert!(received.contains("Release-Urgency: immediate"), "{received}");
    }
}
