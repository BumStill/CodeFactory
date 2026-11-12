// SPDX-License-Identifier: Apache-2.0
//! Exact, bounded POSIX file intents. Never used to grant shell permission.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Component, Path},
};
const MAX_FILE: usize = 2 * 1024 * 1024;
const MAX_TOTAL: usize = 8 * 1024 * 1024;
const MAX_PATHS: usize = 64;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    pub path: String,
    pub before: String,
    pub after: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub entries: Vec<Entry>,
}
fn digest(bytes: Option<&[u8]>) -> String {
    bytes
        .map(super::bytes_digest)
        .unwrap_or_else(super::absent_file_digest)
}
pub(crate) fn is_locator(locator: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(locator)
        .ok()
        .is_some_and(|value| value.get("shell_intent_v1").is_some())
}
fn safe(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', '\0', '\n', '\r', ':'])
        && Path::new(path)
            .components()
            .all(|c| matches!(c,Component::Normal(x) if x!=".git"))
}
fn open_regular(path: &Path, expected: &std::fs::Metadata) -> Option<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // A path replaced by a FIFO must not block before fstat can reject it.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x0020_0000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let file = options.open(path).ok()?;
    let opened = file.metadata().ok()?;
    if !opened.is_file() || opened.len() > MAX_FILE as u64 {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.dev() != expected.dev()
            || opened.ino() != expected.ino()
            || opened.nlink() != 1
            || opened.mode() & 0o111 != 0
        {
            return None;
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if opened.file_attributes() & 0x400 != 0
            || opened.creation_time() != expected.creation_time()
        {
            return None;
        }
    }
    Some(file)
}
/// Returns None on unsupported metadata, never follows a link, and bounds reads.
fn read(cwd: &Path, path: &str) -> Option<Option<Vec<u8>>> {
    if !safe(path) {
        return None;
    }
    let mut current = cwd.to_path_buf();
    let components: Vec<_> = Path::new(path).components().collect();
    for (i, component) in components.iter().enumerate() {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return None;
                }
                if i + 1 < components.len() {
                    if !meta.is_dir() {
                        return None;
                    }
                    continue;
                }
                if !meta.is_file() || meta.len() > MAX_FILE as u64 {
                    return None;
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if meta.nlink() != 1 || meta.mode() & 0o111 != 0 {
                        return None;
                    }
                }
                let file = open_regular(&current, &meta)?;
                let mut bytes = Vec::new();
                file.take(MAX_FILE as u64 + 1)
                    .read_to_end(&mut bytes)
                    .ok()?;
                if bytes.len() > MAX_FILE {
                    return None;
                }
                return Some(Some(bytes));
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound && i + 1 == components.len() =>
            {
                return Some(None)
            }
            Err(_) => return None,
        }
    }
    None
}
impl Manifest {
    pub(crate) fn valid(&self) -> bool {
        !self.entries.is_empty()
            && self.entries.len() <= MAX_PATHS
            && self.entries.windows(2).all(|w| w[0].path < w[1].path)
            && self.entries.iter().all(|e| {
                safe(&e.path)
                    && [&e.before, &e.after].iter().all(|s| {
                        s.len() == 71
                            && s.starts_with("sha256:")
                            && s[7..].bytes().all(|b| b.is_ascii_hexdigit())
                    })
            })
    }
    pub(crate) fn digest(&self, after: bool) -> String {
        let values: Vec<_> = self
            .entries
            .iter()
            .map(|e| (&e.path, if after { &e.after } else { &e.before }))
            .collect();
        super::bytes_digest(&serde_json::to_vec(&values).expect("string digest tuple serializes"))
    }
    pub(crate) fn locator(&self) -> String {
        serde_json::json!({"shell_intent_v1":self}).to_string()
    }
    pub(crate) fn observe(&self, cwd: &Path) -> Option<String> {
        if !self.valid() {
            return None;
        }
        let mut values = Vec::new();
        let mut total = 0usize;
        for entry in &self.entries {
            let bytes = read(cwd, &entry.path)?;
            total = total.checked_add(bytes.as_ref().map_or(0, Vec::len))?;
            if total > MAX_TOTAL {
                return None;
            }
            values.push((&entry.path, digest(bytes.as_deref())));
        }
        Some(super::bytes_digest(&serde_json::to_vec(&values).ok()?))
    }
}
#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    Out(bool),
    And,
    Sequence,
}
/// A complete lexer for the deliberately literal grammar, not a general shell parser.
fn lex(command: &str) -> Option<Vec<Token>> {
    if command.len() > 65536 || command.contains('\0') {
        return None;
    }
    let chars: Vec<char> = command.trim().chars().collect();
    let mut i = 0;
    let mut tokens = Vec::new();
    while i < chars.len() {
        if matches!(chars[i], ' ' | '\t') {
            i += 1;
            continue;
        }
        if chars[i] == '>' {
            i += 1;
            let append = chars.get(i) == Some(&'>');
            if append {
                i += 1;
            }
            tokens.push(Token::Out(append));
            continue;
        }
        if chars[i] == '&' && chars.get(i + 1) == Some(&'&') {
            i += 2;
            tokens.push(Token::And);
            continue;
        }
        if chars[i] == ';' {
            i += 1;
            tokens.push(Token::Sequence);
            continue;
        }
        let mut word = String::new();
        let mut started = false;
        while i < chars.len() && !matches!(chars[i], ' ' | '\t' | '>' | '&' | ';') {
            started = true;
            match chars[i] {
                '\'' => {
                    i += 1;
                    while i < chars.len() && chars[i] != '\'' {
                        word.push(chars[i]);
                        i += 1;
                    }
                    if chars.get(i) != Some(&'\'') {
                        return None;
                    }
                    i += 1;
                }
                '"' => {
                    i += 1;
                    while i < chars.len() && chars[i] != '"' {
                        match chars[i] {
                            '$' | '`' | '\n' | '\r' => return None,
                            '\\' => {
                                i += 1;
                                let next = *chars.get(i)?;
                                if matches!(next, '\n' | '\r') {
                                    return None;
                                }
                                if matches!(next, '"' | '\\' | '$' | '`') {
                                    word.push(next);
                                } else {
                                    word.push('\\');
                                    word.push(next);
                                }
                                i += 1;
                            }
                            c => {
                                word.push(c);
                                i += 1;
                            }
                        }
                    }
                    if chars.get(i) != Some(&'"') {
                        return None;
                    }
                    i += 1;
                }
                '\\' => {
                    i += 1;
                    let c = *chars.get(i)?;
                    if matches!(c, '\n' | '\r') {
                        return None;
                    }
                    word.push(c);
                    i += 1;
                }
                '$' | '`' | '*' | '?' | '[' | ']' | '{' | '}' | '(' | ')' | '|' | '<' | '~'
                | '!' | '#' | '\n' | '\r' | '\0' => return None,
                c => {
                    word.push(c);
                    i += 1;
                }
            }
        }
        if !started {
            return None;
        }
        // Never reinterpret an IO_NUMBER redirection as stdout.
        if chars.get(i) == Some(&'>')
            && !word.is_empty()
            && word.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        tokens.push(Token::Word(word));
        if tokens.len() > 4096 {
            return None;
        }
    }
    Some(tokens)
}
fn printf(args: &[String]) -> Option<Vec<u8>> {
    let separator = args.first().is_some_and(|x| x == "--");
    let args = if separator { &args[1..] } else { args };
    let format = args.first()?;
    if format.starts_with('-') && !separator {
        return None;
    }
    let mut pieces: Vec<Option<Vec<u8>>> = Vec::new();
    let mut text = Vec::new();
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        match c {
            '%' => match chars.next()? {
                '%' => text.push(b'%'),
                's' => {
                    pieces.push(Some(std::mem::take(&mut text)));
                    pieces.push(None);
                }
                _ => return None,
            },
            '\\' => text.push(match chars.next()? {
                'n' => b'\n',
                'r' => b'\r',
                't' => b'\t',
                'a' => 7,
                'b' => 8,
                'f' => 12,
                'v' => 11,
                '\\' => b'\\',
                _ => return None,
            }),
            _ => {
                let mut buf = [0; 4];
                text.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    pieces.push(Some(text));
    let slots = pieces.iter().filter(|p| p.is_none()).count();
    if slots == 0 && args.len() > 1 {
        return None;
    }
    let mut out = Vec::new();
    let mut cursor = 1usize;
    loop {
        for piece in &pieces {
            let bytes = match piece {
                Some(bytes) => bytes.as_slice(),
                None => {
                    let value = args.get(cursor).map(String::as_bytes).unwrap_or_default();
                    cursor += 1;
                    value
                }
            };
            if out.len().checked_add(bytes.len())? > MAX_FILE {
                return None;
            }
            out.extend_from_slice(bytes);
        }
        if slots == 0 || cursor >= args.len() {
            break;
        }
    }
    Some(out)
}
#[derive(Clone)]
struct State {
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
}
struct Simulation<'a> {
    cwd: &'a Path,
    files: BTreeMap<String, State>,
    read_bytes: usize,
}
impl Simulation<'_> {
    fn get(&mut self, path: &str) -> Option<Option<Vec<u8>>> {
        if !self.files.contains_key(path) {
            if self.files.len() >= MAX_PATHS {
                return None;
            }
            let value = read(self.cwd, path)?;
            self.read_bytes = self
                .read_bytes
                .checked_add(value.as_ref().map_or(0, Vec::len))?;
            if self.read_bytes > MAX_TOTAL {
                return None;
            }
            self.files.insert(
                path.into(),
                State {
                    before: value.clone(),
                    after: value,
                },
            );
        }
        Some(self.files.get(path)?.after.clone())
    }
    fn set(&mut self, path: &str, value: Option<Vec<u8>>) -> Option<()> {
        self.get(path)?;
        if value.as_ref().is_some_and(|v| v.len() > MAX_FILE) {
            return None;
        }
        self.files.get_mut(path)?.after = value;
        if self
            .files
            .values()
            .map(|s| s.after.as_ref().map_or(0, Vec::len))
            .sum::<usize>()
            > MAX_TOTAL
        {
            return None;
        }
        Some(())
    }
    fn statement(&mut self, tokens: &[Token]) -> Option<()> {
        let mut words = Vec::new();
        let mut output = None;
        let mut i = 0;
        while i < tokens.len() {
            match &tokens[i] {
                Token::Word(word) if output.is_none() => words.push(word.clone()),
                Token::Out(append) if output.is_none() => {
                    let Token::Word(path) = tokens.get(i + 1)? else {
                        return None;
                    };
                    if i + 2 != tokens.len() {
                        return None;
                    }
                    output = Some((path.clone(), *append));
                    i += 1;
                }
                _ => return None,
            }
            i += 1;
        }
        let command = words.first()?.as_str();
        let command = match command {
            "printf" | "echo" | "cat" | "cp" | "mv" | "rm" => command,
            "/bin/printf" | "/usr/bin/printf" => "printf",
            "/bin/echo" | "/usr/bin/echo" => "echo",
            "/bin/cat" | "/usr/bin/cat" => "cat",
            "/bin/cp" | "/usr/bin/cp" => "cp",
            "/bin/mv" | "/usr/bin/mv" => "mv",
            "/bin/rm" | "/usr/bin/rm" => "rm",
            _ => return None,
        };
        let args = &words[1..];
        if matches!(command, "printf" | "echo" | "cat") {
            let (path, append) = output?;
            let bytes = match command {
                "printf" => printf(args)?,
                "echo" => {
                    if args.first().is_some_and(|s| s.starts_with('-'))
                        || args.iter().any(|s| s.contains('\\'))
                    {
                        return None;
                    }
                    let mut b = args.join(" ").into_bytes();
                    b.push(b'\n');
                    b
                }
                _ => {
                    let args = if args.first().is_some_and(|s| s == "--") {
                        &args[1..]
                    } else {
                        args
                    };
                    if args.is_empty() {
                        return None;
                    }
                    let mut b = Vec::new();
                    for source in args {
                        if source.starts_with('-') || source == &path {
                            return None;
                        }
                        let data = self.get(source)??;
                        if b.len().checked_add(data.len())? > MAX_FILE {
                            return None;
                        }
                        b.extend(data);
                    }
                    b
                }
            };
            let mut result = if append {
                self.get(&path)?.unwrap_or_default()
            } else {
                self.get(&path)?;
                Vec::new()
            };
            if result.len().checked_add(bytes.len())? > MAX_FILE {
                return None;
            }
            result.extend(bytes);
            self.set(&path, Some(result))
        } else {
            if output.is_some() {
                return None;
            }
            let mut args = args;
            let force = command == "rm" && args.first().is_some_and(|s| s == "-f");
            if force {
                args = &args[1..];
            }
            if args.first().is_some_and(|s| s == "--") {
                args = &args[1..];
            }
            if args.is_empty() || args.iter().any(|s| s.starts_with('-')) {
                return None;
            }
            if command == "rm" {
                for path in args {
                    if self.get(path)?.is_none() && !force {
                        return None;
                    }
                    self.set(path, None)?;
                }
                Some(())
            } else {
                if args.len() != 2 || args[0] == args[1] {
                    return None;
                }
                let bytes = self.get(&args[0])??;
                self.set(&args[1], Some(bytes))?;
                if command == "mv" {
                    self.set(&args[0], None)?;
                }
                Some(())
            }
        }
    }
}
pub(crate) fn plan(cwd: &Path, command: &str) -> Option<Manifest> {
    if !cfg!(unix) {
        return None;
    }
    let tokens = lex(command)?;
    if tokens.is_empty() {
        return None;
    }
    let mut sim = Simulation {
        cwd,
        files: BTreeMap::new(),
        read_bytes: 0,
    };
    let mut start = 0;
    let mut count = 0;
    for i in 0..=tokens.len() {
        if i == tokens.len() || matches!(tokens[i], Token::And | Token::Sequence) {
            if start == i {
                if i == tokens.len() && matches!(tokens.last(), Some(Token::Sequence)) {
                    break;
                }
                return None;
            }
            count += 1;
            if count > 32 {
                return None;
            }
            sim.statement(&tokens[start..i])?;
            start = i + 1;
        }
    }
    let entries = sim
        .files
        .into_iter()
        .map(|(path, state)| Entry {
            path,
            before: digest(state.before.as_deref()),
            after: digest(state.after.as_deref()),
        })
        .collect();
    let result = Manifest { entries };
    result.valid().then_some(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(unix))]
    #[test]
    fn r2_shell_intent_does_not_claim_posix_proof_for_other_shells() {
        let dir = tempfile::tempdir().unwrap();
        assert!(plan(dir.path(), "printf '%s' text > out").is_none());
    }
    #[cfg(unix)]
    #[test]
    fn r2_shell_intent_predicts_literal_creation_append_copy_move_delete() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("old.txt"), b"old\n").unwrap();
        let p=plan(dir.path(),"printf '%s\\n' 'new text' > new.txt; printf '%s' tail >> new.txt && cp new.txt copy.txt && mv copy.txt moved.txt && rm old.txt").expect("complete deterministic file plan");
        let entry = |name: &str| p.entries.iter().find(|e| e.path == name).unwrap();
        assert_eq!(
            entry("new.txt").after,
            super::super::bytes_digest(b"new text\ntail")
        );
        assert_eq!(entry("moved.txt").after, entry("new.txt").after);
        assert_eq!(entry("copy.txt").after, super::super::absent_file_digest());
        assert_eq!(entry("old.txt").after, super::super::absent_file_digest());
        assert!(
            !dir.path().join("new.txt").exists(),
            "planner must be read-only"
        );
    }
    #[cfg(unix)]
    #[test]
    fn r2_shell_intent_retains_unchanged_read_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("source.txt"), b"input").unwrap();
        let p = plan(dir.path(), "cat source.txt > destination.txt").unwrap();
        let source = p.entries.iter().find(|e| e.path == "source.txt").unwrap();
        assert_eq!(source.before, source.after);
        assert_eq!(p.entries.len(), 2);
    }
    #[cfg(unix)]
    #[test]
    fn r2_shell_intent_matches_real_posix_shell_bytes() {
        for command in [
            "printf '%s=%s\\n' 'a b' '' one two three > out",
            "printf '%s%%\\t%s\\n' '百分' 'x' > out",
            "printf 'literal\\n' > out; printf '%s' tail >> out;",
            "printf '%s' 'single'\"double\" > out",
            "printf '' > out",
            "printf -- '-literal' > out",
            "echo 'literal $HOME' 'x;y' > out",
            "cat source > out && cp out copy && mv copy moved && rm source",
        ] {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("source"), b"source\n").unwrap();
            let manifest =
                plan(dir.path(), command).unwrap_or_else(|| panic!("not planned: {command}"));
            let output = std::process::Command::new("/bin/sh")
                .args(["-c", command])
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(output.status.success(), "{command}: {:?}", output);
            assert_eq!(
                manifest.observe(dir.path()),
                Some(manifest.digest(true)),
                "{command}"
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn r2_shell_intent_open_rejects_fifo_and_link_replacement_without_blocking() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("victim");
        std::fs::write(&path, b"regular").unwrap();
        let original = std::fs::symlink_metadata(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let (sender, receiver) = std::sync::mpsc::channel();
        let moved = path.clone();
        std::thread::spawn(move || {
            let _ = sender.send(open_regular(&moved, &original).is_none());
        });
        assert!(receiver
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("regular-to-FIFO replacement must not block open"));
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"regular").unwrap();
        let original = std::fs::symlink_metadata(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(dir.path().join("other"), b"foreign").unwrap();
        std::os::unix::fs::symlink("other", &path).unwrap();
        assert!(open_regular(&path, &original).is_none());
    }
    #[cfg(unix)]
    #[test]
    fn r2_shell_intent_rejects_links_modes_and_resource_excess() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("source"), b"input").unwrap();
        symlink("source", dir.path().join("link")).unwrap();
        assert!(plan(dir.path(), "cp link out").is_none());
        std::fs::hard_link(dir.path().join("source"), dir.path().join("hard")).unwrap();
        assert!(plan(dir.path(), "cp source out").is_none());
        std::fs::remove_file(dir.path().join("hard")).unwrap();
        std::fs::set_permissions(
            dir.path().join("source"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert!(plan(dir.path(), "cp source out").is_none());
        std::fs::write(dir.path().join("large"), vec![b'x'; MAX_FILE + 1]).unwrap();
        assert!(plan(dir.path(), "cat large > out").is_none());
        assert!(plan(dir.path(), &"printf x > out;".repeat(33)).is_none());
    }
    #[test]
    fn r2_shell_intent_never_partially_proves_dynamic_or_unknown_tail() {
        let dir = tempfile::tempdir().unwrap();
        for command in [
            "printf x > x; python -c 'print(1)'",
            "printf '%s' \"$HOME\" > x",
            "printf x > x | cat",
            "printf x > x &",
            "cp *.rs x",
            "FOO=bar printf x > x",
            "printf '%b' '\\x41' > x",
            "printf '%s' x > ../x",
            "echo -n x > out",
            "printf '%s' x 2>out",
            "printf 'nul\0' > x",
            "printf \"line\\\ncontinued\" > x",
        ] {
            assert!(plan(dir.path(), command).is_none(), "{command}");
        }
    }
}
