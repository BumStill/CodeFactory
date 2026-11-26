// SPDX-License-Identifier: Apache-2.0
//! Native clipboard fallback for WebViews that omit DataTransfer files.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path};

pub const MAX_ATTACHMENT_BYTES: usize = 50 * 1024 * 1024;
pub const MAX_MODEL_IMAGE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ATTACHMENT_MEBIBYTES: usize = MAX_ATTACHMENT_BYTES / 1024 / 1024;
pub const MAX_MODEL_IMAGE_MEBIBYTES: usize = MAX_MODEL_IMAGE_BYTES / 1024 / 1024;

#[derive(Serialize)]
pub struct AttachmentLimits {
    max_bytes: usize,
    max_mebibytes: usize,
    max_model_image_bytes: usize,
    max_model_image_mebibytes: usize,
}

#[tauri::command]
pub fn attachment_limits() -> AttachmentLimits {
    AttachmentLimits {
        max_bytes: MAX_ATTACHMENT_BYTES,
        max_mebibytes: MAX_ATTACHMENT_MEBIBYTES,
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
    } else if is_model_image(name) && size > MAX_MODEL_IMAGE_BYTES {
        Err(format!(
            "{name} 是图片，超过模型可接收的 {MAX_MODEL_IMAGE_MEBIBYTES}MiB 上限，已跳过；请压缩或缩小图片后重试"
        ))
    } else { Ok(()) }
}

fn is_model_image(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp"))
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
            if (png.length > __MAX_ATTACHMENT_BYTES__) throw new Error('粘贴图片超过附件上限，已跳过');
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
    if ($stream.Length -gt __MAX_ATTACHMENT_BYTES__) { throw '粘贴图片超过附件上限，已跳过' }
    $result += @{ name = ('粘贴图片-' + [DateTimeOffset]::Now.ToUnixTimeMilliseconds() + '.png'); data = [Convert]::ToBase64String($stream.ToArray()) }
  } finally { $stream.Dispose(); $image.Dispose() }
}
ConvertTo-Json -InputObject @($result) -Compress
"#;

fn read_native() -> Result<Vec<ClipboardItem>, String> {
    use crate::util::no_window::NoWindow;
    let script = SCRIPT_TEMPLATE.replace("__MAX_ATTACHMENT_BYTES__", &MAX_ATTACHMENT_BYTES.to_string());
    #[cfg(target_os = "macos")]
    let command = { let mut c = std::process::Command::new("/usr/bin/osascript"); c.args(["-l", "JavaScript", "-e", &script]); c };
    #[cfg(target_os = "windows")]
    let mut command = { let mut c = std::process::Command::new("powershell.exe"); c.args(["-NoProfile", "-NonInteractive", "-STA", "-Command", &script]); c };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Err("当前平台不支持原生剪贴板附件，请使用拖放或选择文件".into());
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
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
        if data.len() > (MAX_ATTACHMENT_BYTES + 2) / 3 * 4 { return Err(format!("{} 超过 {}MiB 上限，已跳过", item.name, MAX_ATTACHMENT_MEBIBYTES)); }
        let bytes = STANDARD.decode(data).map_err(|e| e.to_string())?;
        check_size(&item.name, bytes.len())?;
        Ok(bytes)
    }
}

/// Public IPC entry also usable by the local dispatch channel without GUI focus.
#[tauri::command]
pub async fn paste_session_attachments(app: tauri::AppHandle, cwd: Option<String>, session_id: Option<String>) -> Result<Vec<PasteResult>, String> {
    let items = tauri::async_runtime::spawn_blocking(read_native).await.map_err(|e| e.to_string())??;
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
pub struct AttachmentMetadata { size_bytes: u64 }
#[tauri::command]
pub fn attachment_metadata(path: String) -> Result<AttachmentMetadata, String> {
    let metadata = std::fs::metadata(Path::new(&path)).map_err(|e| format!("文件不存在、已被删除或无法访问：{e}"))?;
    if !metadata.is_file() { return Err("不是可预览的文件".into()); }
    Ok(AttachmentMetadata { size_bytes: metadata.len() })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn m190_clipboard_limits_and_file_list() {
        assert!(check_size("edge.bin", 50 * 1024 * 1024).is_ok());
        assert!(check_size("large.bin", 50 * 1024 * 1024 + 1).unwrap_err().contains("50MiB"));
        assert!(check_size("large.png", 8 * 1024 * 1024 + 1).unwrap_err().contains("模型可接收的 8MiB"));
        let limits = attachment_limits();
        assert_eq!(limits.max_bytes, 50 * 1024 * 1024);
        assert_eq!(limits.max_model_image_bytes, 8 * 1024 * 1024);
        assert!(SCRIPT_TEMPLATE.contains("__MAX_ATTACHMENT_BYTES__"));
        assert!(!SCRIPT_TEMPLATE.contains("26214400"));
        assert!(!SCRIPT_TEMPLATE.contains("25MB"));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("synthetic.txt");
        std::fs::write(&path, b"clipboard synthetic bytes").unwrap();
        let items: Vec<ClipboardItem> = serde_json::from_value(serde_json::json!([
            {"name":"synthetic.txt", "path": path}, {"name":"image.png", "data":"aW1hZ2U="}
        ])).unwrap();
        assert_eq!(item_bytes(&items[0]).unwrap(), b"clipboard synthetic bytes");
        assert_eq!(item_bytes(&items[1]).unwrap(), b"image");
        assert_eq!(attachment_metadata(path.to_string_lossy().into()).unwrap().size_bytes, 25);
        std::fs::remove_file(path).unwrap();
        assert!(item_bytes(&items[0]).is_err());
    }
}
