// SPDX-License-Identifier: Apache-2.0
use std::path::{Path, PathBuf};
use tauri::Manager;

pub fn attachment_dir(data: &Path, cwd: Option<&str>, session: Option<&str>) -> Result<PathBuf, String> {
    if let Some(cwd) = cwd.filter(|v| !v.trim().is_empty()) {
        return Ok(PathBuf::from(cwd).join(".codefactory").join("attachments"));
    }
    let session = session.unwrap_or("unscoped");
    if session.is_empty() || !session.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("invalid attachment session id".into());
    }
    Ok(data.join("attachments").join(session))
}

fn normalize_local(target: &str) -> Result<String, String> {
    let mut path = if target.to_ascii_lowercase().starts_with("file://") {
        let url = url::Url::parse(target).map_err(|e| e.to_string())?;
        if !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
            return Err("invalid file URL".into());
        }
        let decoded = percent_decode(url.path())?;
        match url.host_str() {
            Some(host) if host != "localhost" => format!("//{host}{decoded}"),
            _ => decoded,
        }
    } else { target.to_string() };
    if path.starts_with("\\\\?\\UNC\\") { path = format!("\\\\{}", &path[8..]); }
    else if path.starts_with("\\\\?\\") { path = path[4..].to_string(); }
    if path.as_bytes().first() == Some(&b'/') && path.as_bytes().get(2) == Some(&b':') {
        path = path[1..].to_string();
    }
    let drive = path.as_bytes().get(1) == Some(&b':') && path.as_bytes()[0].is_ascii_alphabetic();
    if path.is_empty() || path.contains('\0') || (!drive && path.split(['/', '\\']).next().unwrap_or("").contains(':')) {
        return Err("only local paths and http(s) links are allowed".into());
    }
    if drive && !matches!(path.as_bytes().get(2), Some(b'/') | Some(b'\\')) {
        return Err("drive-relative paths are not supported".into());
    }
    Ok(path)
}

fn percent_decode(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = value.get(i + 1..i + 3).ok_or("invalid file URL escape")?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| "invalid file URL escape")?);
            i += 3;
        } else { out.push(bytes[i]); i += 1; }
    }
    String::from_utf8(out).map_err(|e| e.to_string())
}

// Allow known passive document/media formats only. Unknown files, scripts,
// executable permissions and runnable ancestors (.app etc.) are revealed.
fn must_reveal(path: &Path, metadata: &std::fs::Metadata) -> bool {
    if metadata.is_dir() { return true; }
    let extension = path.extension().and_then(|v| v.to_str()).unwrap_or("").to_ascii_lowercase();
    let passive = matches!(extension.as_str(), "txt" | "md" | "markdown" | "pdf" | "docx" | "pptx" | "xlsx" | "csv" | "json" | "yaml" | "yml" | "toml" | "png" | "jpg" | "jpeg" | "gif" | "webp" | "mp3" | "mp4" | "wav");
    if !passive { return true; }
    if path.ancestors().any(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("app"))) { return true; }
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 != 0 { return true; }
    }
    false
}

#[tauri::command]
pub async fn open_chat_link(app: tauri::AppHandle, target: String, cwd: Option<String>, session_id: Option<String>) -> Result<(), String> {
    use crate::util::no_window::NoWindow;
    if target.starts_with("https://") || target.starts_with("http://") {
        let url = url::Url::parse(&target).map_err(|e| e.to_string())?;
        if url.host_str().is_none() { return Err("invalid http(s) URL".into()); }
        use tauri_plugin_shell::ShellExt;
        #[allow(deprecated)]
        return app.shell().open(url.as_str(), None).map_err(|e| e.to_string());
    }
    let local = normalize_local(&target)?;
    let mut path = PathBuf::from(local);
    if !path.is_absolute() {
        let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
        let base = if let Some(cwd) = cwd.as_deref().filter(|v| !v.trim().is_empty()) {
            PathBuf::from(cwd)
        } else { attachment_dir(&data, None, session_id.as_deref())? };
        path = base.join(path);
    }
    let path = std::fs::canonicalize(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let metadata = std::fs::metadata(&path).map_err(|e| e.to_string())?;
    if metadata.is_file() { std::fs::File::open(&path).map_err(|e| e.to_string())?; }
    else { std::fs::read_dir(&path).map_err(|e| e.to_string())?; }
    let reveal = must_reveal(&path, &metadata);
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = std::process::Command::new("/usr/bin/open").no_window();
        if reveal && (!metadata.is_dir() || path.extension().is_some_and(|e| e.eq_ignore_ascii_case("app"))) { c.arg("-R"); }
        c.arg(&path); c
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        // ShellExecute through the plugin for passive documents; Explorer is
        // exclusively used for reveal, never cmd/start or a shell expression.
        if !reveal {
            use tauri_plugin_shell::ShellExt;
            #[allow(deprecated)]
            return app.shell().open(path.to_string_lossy().as_ref(), None).map_err(|e| e.to_string());
        }
        let mut c = std::process::Command::new("explorer.exe").no_window();
        if !metadata.is_dir() { c.arg(format!("/select,{}", path.display())); }
        else { c.arg(&path); }
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(if reveal && metadata.is_file() { path.parent().unwrap_or(&path) } else { &path }); c
    };
    let status = command.no_window().status().map_err(|e| e.to_string())?;
    #[cfg(target_os = "windows")]
    { let _ = status; Ok(()) }
    #[cfg(not(target_os = "windows"))]
    if status.success() { Ok(()) } else { Err(format!("system opener failed: {status}")) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn m135_attachment_destinations() {
        assert_eq!(attachment_dir(Path::new("/data"), None, Some("session-1")).unwrap(), PathBuf::from("/data/attachments/session-1"));
        assert_eq!(attachment_dir(Path::new("/data"), Some("/project"), None).unwrap(), PathBuf::from("/project/.codefactory/attachments"));
        assert!(attachment_dir(Path::new("/data"), None, Some("../escape")).is_err());
    }
    #[test]
    fn m135_paths_and_schemes() {
        for (raw, expected) in [("file:///tmp/A%20B.pdf", "/tmp/A B.pdf"), ("file:///C:/Users/A%20B/a.pdf", "C:/Users/A B/a.pdf"), (r"\\?\C:\Users\a.pdf", r"C:\Users\a.pdf"), (r"\\?\UNC\server\share\a.pdf", r"\\server\share\a.pdf"), ("file://server/share/a.pdf", "//server/share/a.pdf")] {
            assert_eq!(normalize_local(raw).unwrap(), expected);
        }
        for unsafe_link in ["javascript:alert(1)", "mailto:a@b.com", "data:text/plain,x", "shell:AppsFolder", "C:relative.exe", "file:///tmp/%00.pdf"] {
            assert!(normalize_local(unsafe_link).is_err(), "{unsafe_link}");
        }
    }
    #[test]
    fn m135_scripts_unknown_and_executable_files_are_revealed() {
        let dir = tempfile::tempdir().unwrap();
        for (name, expected) in [("report.pdf", false), ("run.sh", true), ("run.exe", true), ("run.cmd", true), ("unknown", true)] {
            let path = dir.path().join(name);
            std::fs::write(&path, "synthetic").unwrap();
            assert_eq!(must_reveal(&path, &std::fs::metadata(&path).unwrap()), expected);
        }
        assert!(must_reveal(dir.path(), &std::fs::metadata(dir.path()).unwrap()));
    }
}
