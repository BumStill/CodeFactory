// SPDX-License-Identifier: Apache-2.0
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
pub(crate) fn executable_segments(command: &str) -> Option<Vec<Vec<String>>> {
    if command.len() > 32 * 1024 {
        return None;
    }
    let tokens = shell_tokens(command)?;
    let vars = ShellVars::new();
    let mut segments = Vec::new();
    let mut fuel = 8192usize;
    let (stop, used) = parse_shell_block(&tokens, &vars, &mut segments, &[], 0, &mut fuel)?;
    if stop.is_some() || used != tokens.len() {
        return None;
    }
    Some(segments)
}

/// Invocation inputs can replace the runtime before its literal program starts.
/// Prove absence of shell environment mutation from executable positions; never
/// search raw payload/data text for '=' or a list of loader variable names.
pub(crate) fn has_environment_mutation(command: &str) -> Option<bool> {
    if command.len() > 32 * 1024 {
        return None;
    }
    let tokens = shell_tokens(command)?;
    environment_mutation_tokens(&tokens, &mut 8192usize)
}

fn environment_mutation_tokens(tokens: &[ShellToken], fuel: &mut usize) -> Option<bool> {
    let mut command_start = true;
    for token in tokens {
        *fuel = fuel.checked_sub(1)?;
        match token {
            ShellToken::Sep | ShellToken::OpenParen | ShellToken::CloseParen => {
                command_start = true
            }
            ShellToken::Word {
                text,
                substitutions,
            } => {
                for inner in substitutions {
                    if environment_mutation_tokens(inner, fuel)? {
                        return Some(true);
                    }
                }
                if !command_start {
                    continue;
                }
                // In prefix position even an unsupported assignment spelling
                // (+=, array syntax) cannot establish an unchanged invocation.
                if text.contains('=') {
                    return Some(true);
                }
                let lower = text
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or(text)
                    .to_ascii_lowercase();
                let word = lower.strip_suffix(".exe").unwrap_or(&lower);
                if matches!(
                    word,
                    "export"
                        | "declare"
                        | "typeset"
                        | "local"
                        | "read"
                        | "unset"
                        | "set"
                        | "source"
                        | "."
                        | "eval"
                        | "alias"
                        | "function"
                        | "for"
                        | "select"
                ) {
                    return Some(true);
                }
                if matches!(
                    word,
                    "env"
                        | "command"
                        | "exec"
                        | "sudo"
                        | "nohup"
                        | "time"
                        | "builtin"
                        | "if"
                        | "then"
                        | "elif"
                        | "else"
                        | "while"
                        | "until"
                        | "do"
                        | "!"
                ) {
                    continue;
                }
                command_start = false;
            }
        }
    }
    Some(false)
}

fn shell_tokens(command: &str) -> Option<Vec<ShellToken>> {
    shell_tokens_at_depth(command, 0)
}

fn shell_tokens_at_depth(command: &str, depth: usize) -> Option<Vec<ShellToken>> {
    if depth > MAX_STRUCTURE_DEPTH {
        return None;
    }
    if command.len() > 32 * 1024 {
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
            if chars.get(start + 2) == Some(&'(') {
                return None;
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
    fuel: &mut usize,
) -> Option<(Option<String>, usize)> {
    *fuel = fuel.checked_sub(1)?;
    if depth > MAX_STRUCTURE_DEPTH {
        return None;
    }
    let mut index = 0usize;
    let mut current: Vec<String> = Vec::new();
    while index < tokens.len() {
        *fuel = fuel.checked_sub(1)?;
        if out.len() >= 2048 {
            return None;
        }
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
                    parse_shell_block(&tokens[index + 1..close], vars, out, &[], depth + 1, fuel)?;
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
                        parse_shell_block(inner, vars, out, &[], depth + 1, fuel)?;
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
                            index = parse_for_loop(tokens, index, vars, out, depth, fuel)?;
                            continue;
                        }
                        "while" | "until" => {
                            index = parse_while_loop(tokens, index, vars, out, depth, fuel)?;
                            continue;
                        }
                        "if" => {
                            index = parse_if_block(tokens, index, vars, out, depth, fuel)?;
                            continue;
                        }
                        // Structures we deliberately do not model, and keywords
                        // that only make sense inside one: fail closed.
                        "case" | "select" | "function" | "coproc" | "eval" | "source" | "trap"
                        | "esac" | "then" | "else" | "elif" | "fi" | "do" | "done" | "in" => {
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
    fuel: &mut usize,
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
                                    parse_shell_block(inner, vars, out, &[], depth + 1, fuel)?;
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
                parse_shell_block(body, &scoped, &mut iteration, &["done"], depth + 1, fuel)?;
            if stop.as_deref() != Some("done") {
                return None;
            }
            match done_at {
                None => done_at = Some(used),
                Some(expected) if expected == used => {}
                Some(_) => return None,
            }
            if out.len().saturating_add(iteration.len()) > 2048 {
                return None;
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
    let (stop, used) = parse_shell_block(body, &scoped, out, &["done"], depth + 1, fuel)?;
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
    fuel: &mut usize,
) -> Option<usize> {
    let cursor = index + 1;
    let (stop, used) = parse_shell_block(&tokens[cursor..], vars, out, &["do"], depth + 1, fuel)?;
    if stop.as_deref() != Some("do") {
        return None;
    }
    let body_start = cursor + used + 1;
    let (stop, used) =
        parse_shell_block(&tokens[body_start..], vars, out, &["done"], depth + 1, fuel)?;
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
    fuel: &mut usize,
) -> Option<usize> {
    let mut cursor = index + 1;
    loop {
        let (stop, used) =
            parse_shell_block(&tokens[cursor..], vars, out, &["then"], depth + 1, fuel)?;
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
            fuel,
        )?;
        match stop.as_deref() {
            Some("fi") => return Some(cursor + used + 1),
            Some("elif") => {
                cursor += used + 1;
            }
            Some("else") => {
                cursor += used + 1;
                let (stop, used) =
                    parse_shell_block(&tokens[cursor..], vars, out, &["fi"], depth + 1, fuel)?;
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
                while matches!(chars.get(end), Some(c) if c.is_ascii_alphanumeric() || *c == '_') {
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
