// SPDX-License-Identifier: Apache-2.0
//! 文档页面渲染：把 PPTX / PDF 的某一页变成 PNG，交给视觉桥。
//!
//! 渲染器按平台优先级探测，全部失败时返回明确错误（调用方据此降级为
//! "只算文字，不算版面"并说明原因）：
//! - Windows：PowerPoint COM 导出幻灯片；
//! - macOS：QuickLook（仅第 1 页）+ LibreOffice + poppler 组合；
//! - 全平台：LibreOffice (`soffice`) 转 PDF，再由 `pdftoppm` / `sips` 取页。

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

/// 一页渲染结果。
#[derive(Debug, Clone)]
pub struct RenderedPage {
    pub png_path: PathBuf,
    pub renderer: String,
    /// 例如"只渲染了第 1 页"这类必须让调用方知道的能力边界。
    pub note: Option<String>,
}

fn command_exists(name: &str) -> bool {
    let probe = if cfg!(target_os = "windows") { "where" } else { "which" };
    std::process::Command::new(probe)
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// 当前环境能用的渲染器名称，用于诊断输出。
pub fn available_renderers() -> Vec<&'static str> {
    let mut out = Vec::new();
    if cfg!(target_os = "windows") {
        out.push("powerpoint-com");
    }
    if cfg!(target_os = "macos") && command_exists("qlmanage") {
        out.push("quicklook");
    }
    if command_exists("soffice") {
        out.push("libreoffice");
    }
    out
}

/// 渲染 `document` 的第 `page` 页（1 起）到 `out_dir`。
pub async fn render_page(document: &Path, page: u32, out_dir: &Path) -> Result<RenderedPage, String> {
    if page == 0 {
        return Err("PAGE_OUT_OF_RANGE: 页码从 1 开始".to_string());
    }
    if !document.exists() {
        return Err(format!("DOCUMENT_NOT_FOUND: {}", document.display()));
    }
    std::fs::create_dir_all(out_dir).map_err(|error| format!("RENDER_DIR: {error}"))?;

    let extension = document
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_lowercase();
    let is_pptx = matches!(extension.as_str(), "pptx" | "ppt");
    let is_pdf = extension == "pdf";
    if !is_pptx && !is_pdf {
        return Err(format!("UNSUPPORTED_DOCUMENT: .{extension} 不能按页渲染"));
    }

    let mut failures: Vec<String> = Vec::new();

    #[cfg(target_os = "windows")]
    if is_pptx {
        match powerpoint::export_slide(document, page, out_dir).await {
            Ok(png_path) => {
                return Ok(RenderedPage {
                    png_path,
                    renderer: "powerpoint-com".to_string(),
                    note: None,
                })
            }
            Err(error) => failures.push(format!("powerpoint-com: {error}")),
        }
    }

    // PDF 第 1 页：QuickLook 直接出图（macOS）。
    if is_pdf && page == 1 && cfg!(target_os = "macos") {
        match quicklook_thumbnail(document, out_dir, 1600).await {
            Ok(png_path) => {
                return Ok(RenderedPage {
                    png_path,
                    renderer: "quicklook".to_string(),
                    note: Some("QuickLook 只能渲染第 1 页".to_string()),
                })
            }
            Err(error) => failures.push(format!("quicklook: {error}")),
        }
    }

    match libreoffice::render(document, page, out_dir, is_pdf).await {
        Ok(png_path) => {
            return Ok(RenderedPage {
                png_path,
                renderer: "libreoffice".to_string(),
                note: None,
            })
        }
        Err(error) => failures.push(format!("libreoffice: {error}")),
    }

    // 兜底：PPTX 第 1 页用 QuickLook 取缩略图。缩略图分辨率有限，
    // 因此 note 里必须写明，避免把小图当版面证据。
    if is_pptx && page == 1 && cfg!(target_os = "macos") {
        if let Ok(png_path) = quicklook_thumbnail(document, out_dir, 1600).await {
            return Ok(RenderedPage {
                png_path,
                renderer: "quicklook".to_string(),
                note: Some("QuickLook 缩略图，分辨率有限，只能看大致布局".to_string()),
            });
        }
    }

    Err(format!(
        "NO_RENDERER: 没有可用的页面渲染器。可用：{:?}；失败：{}",
        available_renderers(),
        failures.join(" | ")
    ))
}

/// QuickLook 缩略图。只对"第 1 页/整文件预览"成立。
pub async fn quicklook_thumbnail(source: &Path, out_dir: &Path, size: u32) -> Result<PathBuf, String> {
    if !cfg!(target_os = "macos") {
        return Err("QUICKLOOK_UNAVAILABLE: 仅 macOS 有 qlmanage".to_string());
    }
    std::fs::create_dir_all(out_dir).map_err(|error| format!("RENDER_DIR: {error}"))?;
    let before = newest_png(out_dir);
    let output = Command::new("qlmanage")
        .arg("-t")
        .arg("-s")
        .arg(size.to_string())
        .arg("-o")
        .arg(out_dir)
        .arg(source)
        .output()
        .await
        .map_err(|error| format!("QUICKLOOK_SPAWN: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "QUICKLOOK_EXIT {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    newest_png(out_dir)
        .filter(|path| Some(path) != before.as_ref())
        .ok_or_else(|| "QUICKLOOK_NO_OUTPUT: qlmanage 没有产出 PNG".to_string())
}

fn newest_png(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("png") {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        if best.as_ref().map(|(time, _)| modified >= *time).unwrap_or(true) {
            best = Some((modified, path));
        }
    }
    best.map(|(_, path)| path)
}

#[cfg(target_os = "windows")]
mod powerpoint {
    use super::*;

    const SCRIPT: &str = r#"
param([string]$Path, [int]$Page, [string]$OutDir)
$ErrorActionPreference = 'Stop'
$app = New-Object -ComObject PowerPoint.Application
try {
    $presentation = $app.Presentations.Open($Path, $true, $false, $false)
    try {
        if ($Page -gt $presentation.Slides.Count) { throw "PAGE_OUT_OF_RANGE: $Page > $($presentation.Slides.Count)" }
        $target = Join-Path $OutDir ("slide-" + $Page + ".png")
        $presentation.Slides.Item($Page).Export($target, "PNG", 1600, 1200)
        Write-Output $target
    } finally { $presentation.Close() }
} finally { $app.Quit() }
"#;

    pub async fn export_slide(document: &Path, page: u32, out_dir: &Path) -> Result<PathBuf, String> {
        let script = out_dir.join("export-slide.ps1");
        std::fs::write(&script, SCRIPT).map_err(|error| format!("PPT_SCRIPT: {error}"))?;
        let output = Command::new("powershell")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&script)
            .arg(document)
            .arg(page.to_string())
            .arg(out_dir)
            .output()
            .await
            .map_err(|error| format!("PPT_SPAWN: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "PPT_EXIT: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let produced = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let path = PathBuf::from(&produced);
        if path.exists() {
            return Ok(path);
        }
        newest_png(out_dir).ok_or_else(|| "PPT_NO_OUTPUT".to_string())
    }
}

mod libreoffice {
    use super::*;

    pub async fn render(document: &Path, page: u32, out_dir: &Path, is_pdf: bool) -> Result<PathBuf, String> {
        let pdf = if is_pdf {
            document.to_path_buf()
        } else {
            convert_to_pdf(document, out_dir).await?
        };

        // 优先 pdftoppm：任意页码、分辨率可控。
        if command_exists("pdftoppm") {
            let prefix = out_dir.join(format!("page-{page}"));
            let output = Command::new("pdftoppm")
                .arg("-f")
                .arg(page.to_string())
                .arg("-l")
                .arg(page.to_string())
                .arg("-r")
                .arg("150")
                .arg("-png")
                .arg("-singlefile")
                .arg(&pdf)
                .arg(&prefix)
                .output()
                .await
                .map_err(|error| format!("PDFTOPPM_SPAWN: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "PDFTOPPM_EXIT: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            let png = prefix.with_extension("png");
            if png.exists() {
                return Ok(png);
            }
            return Err("PDFTOPPM_NO_OUTPUT".to_string());
        }

        // 只在第 1 页时退到 sips（它不做分页）。
        if page == 1 && command_exists("sips") {
            let png = out_dir.join("page-1.png");
            let output = Command::new("sips")
                .arg("-s")
                .arg("format")
                .arg("png")
                .arg(&pdf)
                .arg("--out")
                .arg(&png)
                .output()
                .await
                .map_err(|error| format!("SIPS_SPAWN: {error}"))?;
            if output.status.success() && png.exists() {
                return Ok(png);
            }
            return Err("SIPS_FAILED".to_string());
        }

        Err("NO_PDF_RASTERIZER: 需要 pdftoppm（poppler）才能渲染指定页码".to_string())
    }

    async fn convert_to_pdf(document: &Path, out_dir: &Path) -> Result<PathBuf, String> {
        if !command_exists("soffice") {
            return Err("SOFFICE_MISSING: 未安装 LibreOffice".to_string());
        }
        let output = Command::new("soffice")
            .arg("--headless")
            .arg("--convert-to")
            .arg("pdf")
            .arg("--outdir")
            .arg(out_dir)
            .arg(document)
            .output()
            .await
            .map_err(|error| format!("SOFFICE_SPAWN: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "SOFFICE_EXIT: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let stem = document
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("document");
        let pdf = out_dir.join(format!("{stem}.pdf"));
        if pdf.exists() {
            Ok(pdf)
        } else {
            Err("SOFFICE_NO_PDF: 转换后没有找到 PDF".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_probe_reports_what_exists() {
        // 断言探针结果与环境自洽，而不是绑死某个平台。
        for name in available_renderers() {
            let probe = match name {
                // COM 依赖 PowerPoint 安装，无法用 which 判定。
                "powerpoint-com" => None,
                "quicklook" => Some("qlmanage"),
                "libreoffice" => Some("soffice"),
                other => Some(other),
            };
            if let Some(probe) = probe {
                assert!(command_exists(probe), "{name} reported but {probe} missing");
            }
        }
    }

    #[tokio::test]
    async fn unsupported_extension_is_rejected_with_a_clear_reason() {
        let dir = std::env::temp_dir().join(format!("cf-render-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("notes.txt");
        std::fs::write(&doc, "hi").unwrap();
        let error = render_page(&doc, 1, &dir).await.unwrap_err();
        assert!(error.starts_with("UNSUPPORTED_DOCUMENT"), "{error}");
        let error = render_page(&doc, 0, &dir).await.unwrap_err();
        assert!(error.starts_with("PAGE_OUT_OF_RANGE"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn quicklook_renders_a_real_png() {
        let dir = std::env::temp_dir().join(format!("cf-render-ql-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let doc = dir.join("sample.txt");
        std::fs::write(&doc, "CODEX RENDER CHECK").unwrap();
        match quicklook_thumbnail(&doc, &dir, 900).await {
            Ok(png) => {
                let bytes = std::fs::read(&png).unwrap();
                assert!(crate::vision::ocr::png_dimensions(&bytes).is_some(), "thumbnail is not a PNG");
            }
            Err(error) => {
                // 无窗口会话时不强制失败，但必须打印原因。
                eprintln!("quicklook unavailable: {error}");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
