// SPDX-License-Identifier: Apache-2.0
//! Native clipboard fallback for WebViews that omit DataTransfer files.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};
use tauri::Manager;

pub const MAX_ATTACHMENT_BYTES: usize = 200 * 1024 * 1024;
pub const MAX_CLIPBOARD_BYTES: usize = 20 * 1024 * 1024;
pub const MAX_MODEL_IMAGE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ATTACHMENT_MEBIBYTES: usize = MAX_ATTACHMENT_BYTES / 1024 / 1024;
pub const MAX_CLIPBOARD_MEBIBYTES: usize = MAX_CLIPBOARD_BYTES / 1024 / 1024;
pub const MAX_MODEL_IMAGE_MEBIBYTES: usize = MAX_MODEL_IMAGE_BYTES / 1024 / 1024;

#[derive(Serialize)]
pub struct AttachmentLimits {
    max_bytes: usize,
    max_mebibytes: usize,
    max_clipboard_bytes: usize,
    max_clipboard_mebibytes: usize,
    max_model_image_bytes: usize,
    max_model_image_mebibytes: usize,
}

#[tauri::command]
pub fn attachment_limits() -> AttachmentLimits {
    AttachmentLimits {
        max_bytes: MAX_ATTACHMENT_BYTES,
        max_mebibytes: MAX_ATTACHMENT_MEBIBYTES,
        max_clipboard_bytes: MAX_CLIPBOARD_BYTES,
        max_clipboard_mebibytes: MAX_CLIPBOARD_MEBIBYTES,
        max_model_image_bytes: MAX_MODEL_IMAGE_BYTES,
        max_model_image_mebibytes: MAX_MODEL_IMAGE_MEBIBYTES,
    }
}

#[derive(Deserialize)]
struct ClipboardItem {
    name: String,
    path: Option<String>,
    data: Option<String>,
}
#[derive(Serialize)]
pub struct PasteResult {
    path: String,
    name: String,
    size_bytes: usize,
    error: Option<String>,
}

pub fn check_size(name: &str, size: usize) -> Result<(), String> {
    if size > MAX_ATTACHMENT_BYTES {
        Err(format!("{name} 超过 {MAX_ATTACHMENT_MEBIBYTES}MiB 上限，已跳过"))
    } else { Ok(()) }
}

pub fn check_clipboard_size(name: &str, size: usize) -> Result<(), String> {
    if size > MAX_CLIPBOARD_BYTES {
        Err(format!("{name} 超过 {MAX_CLIPBOARD_MEBIBYTES}MiB 剪贴板上限，请用添加附件或拖入"))
    } else {
        Ok(())
    }
}

// All filenames/paths are clipboard data, never interpolated into commands.
#[cfg(target_os = "macos")]
const SCRIPT_TEMPLATE: &str = r#"
ObjC.import('AppKit');
ObjC.import('Foundation');
function run() {
    const board = $.NSPasteboard.generalPasteboard;
    const result = [];
    const items = board.pasteboardItems;
    if (items) for (let i = 0; i < items.count; i++) {
        const value = items.objectAtIndex(i).stringForType('public.file-url');
        if (value && !value.isNil()) {
            const url = $.NSURL.URLWithString(value);
            if (url.isFileURL) result.push({name: ObjC.unwrap(url.lastPathComponent), path: ObjC.unwrap(url.path)});
        }
    }
    if (result.length === 0) {
        const image = $.NSImage.alloc.initWithPasteboard(board);
        if (image && !image.isNil()) {
            const bitmap = $.NSBitmapImageRep.alloc.initWithData(image.TIFFRepresentation);
            const png = bitmap.representationUsingTypeProperties($.NSPNGFileType, $({}));
            if (png.length > __MAX_CLIPBOARD_BYTES__) throw new Error('粘贴图片超过附件上限，已跳过');
            result.push({name: '粘贴图片-' + Date.now() + '.png', data: ObjC.unwrap(png.base64EncodedStringWithOptions(0))});
        }
    }
    return JSON.stringify(result);
}
"#;
#[cfg(target_os = "windows")]
const SCRIPT_TEMPLATE: &str = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
$result = @()
if ([System.Windows.Forms.Clipboard]::ContainsFileDropList()) {
  foreach ($path in [System.Windows.Forms.Clipboard]::GetFileDropList()) {
    $result += @{ name = [IO.Path]::GetFileName($path); path = $path }
  }
} elseif ([System.Windows.Forms.Clipboard]::ContainsImage()) {
  $image = [System.Windows.Forms.Clipboard]::GetImage()
  $stream = New-Object IO.MemoryStream
  try {
    $image.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
    if ($stream.Length -gt __MAX_CLIPBOARD_BYTES__) { throw '粘贴图片超过附件上限，已跳过' }
    $result += @{ name = ('粘贴图片-' + [DateTimeOffset]::Now.ToUnixTimeMilliseconds() + '.png'); data = [Convert]::ToBase64String($stream.ToArray()) }
  } finally { $stream.Dispose(); $image.Dispose() }
}
ConvertTo-Json -InputObject @($result) -Compress
"#;

fn read_native() -> Result<Vec<ClipboardItem>, String> {
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        return Err("当前平台不支持原生剪贴板附件，请使用拖放或选择文件".into());
    }
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        use crate::util::no_window::NoWindow;
        let script = SCRIPT_TEMPLATE.replace("__MAX_CLIPBOARD_BYTES__", &MAX_CLIPBOARD_BYTES.to_string());
        #[cfg(target_os = "macos")]
        let command = { let mut c = std::process::Command::new("/usr/bin/osascript"); c.args(["-l", "JavaScript", "-e", &script]); c };
        #[cfg(target_os = "windows")]
        let mut command = { let mut c = std::process::Command::new("powershell.exe"); c.args(["-NoProfile", "-NonInteractive", "-STA", "-Command", &script]); c };
        let output = command.no_window().output().map_err(|e| e.to_string())?;
        if !output.status.success() { return Err(String::from_utf8_lossy(&output.stderr).to_string()); }
        serde_json::from_slice(&output.stdout).map_err(|e| format!("剪贴板数据无法读取：{e}"))
    }
}

fn item_bytes(item: &ClipboardItem) -> Result<Vec<u8>, String> {
    if let Some(path) = &item.path {
        let file = std::fs::File::open(path).map_err(|e| format!("{} 读取失败：{e}", item.name))?;
        check_size(&item.name, file.metadata().map_err(|e| e.to_string())?.len() as usize)?;
        let mut bytes = Vec::new();
        file.take((MAX_ATTACHMENT_BYTES + 1) as u64).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        check_size(&item.name, bytes.len())?;
        Ok(bytes)
    } else {
        let data = item.data.as_deref().ok_or("剪贴板图片数据为空")?;
        if data.len() > (MAX_CLIPBOARD_BYTES + 2) / 3 * 4 { return Err(format!("{} 超过 {}MiB 剪贴板上限，请用添加附件或拖入", item.name, MAX_CLIPBOARD_MEBIBYTES)); }
        let bytes = STANDARD.decode(data).map_err(|e| e.to_string())?;
        check_clipboard_size(&item.name, bytes.len())?;
        Ok(bytes)
    }
}

static CANCELLED_COPIES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn cancelled_copies() -> &'static Mutex<HashSet<String>> {
    CANCELLED_COPIES.get_or_init(|| Mutex::new(HashSet::new()))
}

#[derive(Serialize, Clone)]
pub struct CopyProgress {
    copy_id: String,
    name: String,
    bytes_copied: u64,
    total_bytes: u64,
}

#[tauri::command]
pub fn cancel_attachment_copy(copy_id: String) -> Result<(), String> {
    cancelled_copies().lock().map_err(|_| "附件复制取消状态不可用".to_string())?.insert(copy_id);
    Ok(())
}

fn copy_destination(data: &Path, cwd: Option<&str>, session_id: Option<&str>, source: &Path) -> Result<PathBuf, String> {
    let dir = crate::commands::chat::chat_links::attachment_dir(data, cwd, session_id)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("无法创建附件目录：{e}"))?;
    let ext = source.extension().and_then(|s| s.to_str()).map(|s| s.to_lowercase()).filter(|s| s.chars().all(|c| c.is_ascii_alphanumeric())).unwrap_or_else(|| "bin".into());
    Ok(dir.join(format!("{}-{}.{}", chrono::Utc::now().timestamp_millis(), &uuid::Uuid::new_v4().simple().to_string()[..8], ext)))
}

fn stream_copy_path(
    data: &Path,
    cwd: Option<&str>,
    session_id: Option<&str>,
    source: &Path,
    copy_id: &str,
    progress: &tauri::ipc::Channel<CopyProgress>,
) -> Result<PasteResult, String> {
    let name = source.file_name().and_then(|value| value.to_str()).unwrap_or("文件").to_string();
    let mut input = std::fs::File::open(source).map_err(|e| format!("{name} 无法读取（文件可能被占用、未下载或无权限）：{e}"))?;
    let total = input.metadata().map_err(|e| format!("{name} 无法读取大小：{e}"))?.len();
    check_size(&name, usize::try_from(total).unwrap_or(usize::MAX))?;
    let destination = copy_destination(data, cwd, session_id, source)?;
    let partial = destination.with_extension("partial");
    let mut output = std::fs::File::create(&partial).map_err(|e| format!("{name} 无法创建目标文件（请检查磁盘空间与权限）：{e}"))?;
    let mut buffer = [0u8; 256 * 1024];
    let mut copied = 0u64;
    loop {
        if cancelled_copies().lock().map_err(|_| "附件复制取消状态不可用".to_string())?.remove(copy_id) {
            drop(output);
            let _ = std::fs::remove_file(&partial);
            return Err(format!("{name} 复制已取消"));
        }
        let read = input.read(&mut buffer).map_err(|e| format!("{name} 读取失败（文件可能被 Office 占用或 OneDrive 尚未下载）：{e}"))?;
        if read == 0 { break; }
        output.write_all(&buffer[..read]).map_err(|e| format!("{name} 写入失败（请检查磁盘空间与权限）：{e}"))?;
        copied += read as u64;
        let _ = progress.send(CopyProgress { copy_id: copy_id.to_string(), name: name.clone(), bytes_copied: copied, total_bytes: total });
    }
    output.sync_all().map_err(|e| format!("{name} 落盘失败：{e}"))?;
    std::fs::rename(&partial, &destination).map_err(|e| format!("{name} 完成附件保存失败：{e}"))?;
    Ok(PasteResult { path: destination.to_string_lossy().to_string(), name, size_bytes: copied as usize, error: None })
}

#[tauri::command]
pub async fn copy_session_attachments(
    app: tauri::AppHandle,
    cwd: Option<String>,
    session_id: Option<String>,
    paths: Vec<String>,
    copy_id: String,
    progress: tauri::ipc::Channel<CopyProgress>,
) -> Result<Vec<PasteResult>, String> {
    let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || paths.into_iter().map(|path| {
        let source = PathBuf::from(path);
        match stream_copy_path(&data, cwd.as_deref(), session_id.as_deref(), &source, &copy_id, &progress) {
            Ok(saved) => saved,
            Err(error) => PasteResult { path: String::new(), name: source.file_name().and_then(|v| v.to_str()).unwrap_or("文件").to_string(), size_bytes: 0, error: Some(error) },
        }
    }).collect()).await.map_err(|e| e.to_string())
}

/// Public IPC entry also usable by the local dispatch channel without GUI focus.
#[tauri::command]
pub async fn paste_session_attachments(app: tauri::AppHandle, cwd: Option<String>, session_id: Option<String>, paths: Option<Vec<String>>) -> Result<Vec<PasteResult>, String> {
    let items = match paths {
        Some(paths) => {
            let data = app.path().app_data_dir().map_err(|e| e.to_string())?;
            let progress = tauri::ipc::Channel::new(|_| Ok(()));
            return tauri::async_runtime::spawn_blocking(move || paths.into_iter().map(|path| {
                let source = PathBuf::from(path);
                match stream_copy_path(&data, cwd.as_deref(), session_id.as_deref(), &source, "clipboard-files", &progress) {
                    Ok(saved) => saved,
                    Err(error) => PasteResult { path: String::new(), name: source.file_name().and_then(|v| v.to_str()).unwrap_or("文件").to_string(), size_bytes: 0, error: Some(error) },
                }
            }).collect()).await.map_err(|e| e.to_string());
        }
        None => tauri::async_runtime::spawn_blocking(read_native).await.map_err(|e| e.to_string())??,
    };
    let mut result = Vec::new();
    for item in items {
        let name = item.name.clone();
        let bytes = tauri::async_runtime::spawn_blocking(move || item_bytes(&item)).await.map_err(|e| e.to_string())?;
        let saved = match bytes {
            Ok(bytes) => crate::commands::chat::chat_links::save_session_attachment(app.clone(), cwd.clone(), session_id.clone(), name.clone(), STANDARD.encode(bytes)).await,
            Err(e) => Err(e),
        };
        result.push(match saved {
            Ok(saved) => PasteResult { path: saved.path, name, size_bytes: saved.size_bytes, error: None },
            Err(error) => PasteResult { path: String::new(), name, size_bytes: 0, error: Some(error) },
        });
    }
    Ok(result)
}

#[derive(Serialize)]
pub struct AttachmentMetadata {
    size_bytes: Option<u64>,
    max_bytes: usize,
    max_mebibytes: usize,
    max_clipboard_bytes: usize,
    max_clipboard_mebibytes: usize,
    max_model_image_bytes: usize,
    max_model_image_mebibytes: usize,
}
#[tauri::command]
pub fn attachment_metadata(path: Option<String>) -> Result<AttachmentMetadata, String> {
    let size_bytes = match path {
        Some(path) => {
            let metadata = std::fs::metadata(Path::new(&path)).map_err(|e| format!("文件不存在、已被删除或无法访问：{e}"))?;
            if !metadata.is_file() { return Err("不是可预览的文件".into()); }
            Some(metadata.len())
        }
        None => None,
    };
    Ok(AttachmentMetadata {
        size_bytes,
        max_bytes: MAX_ATTACHMENT_BYTES,
        max_mebibytes: MAX_ATTACHMENT_MEBIBYTES,
        max_clipboard_bytes: MAX_CLIPBOARD_BYTES,
        max_clipboard_mebibytes: MAX_CLIPBOARD_MEBIBYTES,
        max_model_image_bytes: MAX_MODEL_IMAGE_BYTES,
        max_model_image_mebibytes: MAX_MODEL_IMAGE_MEBIBYTES,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn m190_clipboard_limits_and_file_list() {
        assert!(check_size("edge.bin", 200 * 1024 * 1024).is_ok());
        assert!(check_size("large.bin", 200 * 1024 * 1024 + 1).unwrap_err().contains("200MiB"));
        assert_eq!(check_clipboard_size("edge.png", 20 * 1024 * 1024), Ok(()));
        assert!(check_clipboard_size("large.png", 20 * 1024 * 1024 + 1).unwrap_err().contains("请用添加附件或拖入"));
        let limits = attachment_limits();
        assert_eq!(limits.max_bytes, 200 * 1024 * 1024);
        assert_eq!(limits.max_clipboard_bytes, 20 * 1024 * 1024);
        assert_eq!(limits.max_model_image_bytes, 8 * 1024 * 1024);
        let metadata_limits = attachment_metadata(None).unwrap();
        assert_eq!(metadata_limits.max_bytes, limits.max_bytes);
        assert_eq!(metadata_limits.max_model_image_bytes, limits.max_model_image_bytes);
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            assert!(SCRIPT_TEMPLATE.contains("__MAX_CLIPBOARD_BYTES__"));
            assert!(!SCRIPT_TEMPLATE.contains("26214400"));
            assert!(!SCRIPT_TEMPLATE.contains("25MB"));
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let err = super::read_native().unwrap_err();
            assert!(err.contains("拖放"));
            assert!(err.contains("选择文件"));
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.txt");
        std::fs::write(&path, b"clipboard synthetic bytes").unwrap();
        let items: Vec<ClipboardItem> = serde_json::from_value(serde_json::json!([
            {"name":"synthetic.txt", "path": path}, {"name":"image.png", "data":"aW1hZ2U="}
        ])).unwrap();
        assert_eq!(item_bytes(&items[0]).unwrap(), b"clipboard synthetic bytes");
        assert_eq!(item_bytes(&items[1]).unwrap(), b"image");
        assert_eq!(attachment_metadata(Some(path.to_string_lossy().into())).unwrap().size_bytes, Some(25));
        std::fs::remove_file(path).unwrap();
        assert!(item_bytes(&items[0]).is_err());
    }
}
