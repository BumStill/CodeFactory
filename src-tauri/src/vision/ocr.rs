// SPDX-License-Identifier: Apache-2.0
//! 本地 OCR 兜底：不把图片发送给任何模型，只从图片里取文字和文字位置。
//!
//! 后端按平台选择：
//! - macOS：`Vision.framework`，通过一次性编译的 Swift 小工具调用（系统自带，
//!   不需要用户安装任何东西）；
//! - Windows：`Windows.Media.Ocr`，通过 PowerShell 调用系统 OCR 引擎；
//! - 任意平台：`tesseract`（若已安装）。
//!
//! 返回的坐标统一归一化到 0-1，原点在左上角，这样"左上角写了什么"这类
//! 问题可以直接用坐标回答。

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::process::Command;

use crate::util::no_window::NoWindow;

/// 一个识别出的文字块及其归一化位置。
#[derive(Debug, Clone, PartialEq)]
pub struct OcrWord {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// OCR 结果。`backend` 会原样出现在工具输出里，便于判断证据强度。
#[derive(Debug, Clone)]
pub struct OcrResult {
    pub backend: String,
    pub text: String,
    pub words: Vec<OcrWord>,
}

/// 当前平台是否有任何可用 OCR 后端。
pub async fn available() -> bool {
    candidates().await.into_iter().next().is_some()
}

async fn candidates() -> Vec<&'static str> {
    let mut out = Vec::new();
    if cfg!(target_os = "macos") && command_exists("swiftc").await {
        out.push("macos-vision");
    }
    if cfg!(target_os = "windows") {
        out.push("windows-ocr");
    }
    if command_exists("tesseract").await {
        out.push("tesseract");
    }
    out
}

async fn command_exists(name: &str) -> bool {
    let probe = if cfg!(target_os = "windows") {
        "where"
    } else {
        "which"
    };
    Command::new(probe)
        .no_window()
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .map(|status| status.success())
        .unwrap_or(false)
}

/// 按平台优先级识别图片。全后端失败时返回最后一个错误。
pub async fn recognize(image_path: &Path) -> Result<OcrResult, String> {
    let ordered = candidates().await;
    if ordered.is_empty() {
        return Err("NO_OCR_BACKEND: 没有可用的本地 OCR 后端".to_string());
    }
    let mut last_error = String::new();
    for backend in ordered {
        let attempt = match backend {
            "macos-vision" => macos::recognize(image_path).await,
            "windows-ocr" => windows::recognize(image_path).await,
            "tesseract" => tesseract::recognize(image_path).await,
            other => Err(format!("UNKNOWN_OCR_BACKEND: {other}")),
        };
        match attempt {
            Ok(result) => return Ok(result),
            Err(error) => last_error = format!("{backend}: {error}"),
        }
    }
    Err(last_error)
}

/// 从 PNG 头读取像素尺寸（IHDR 固定在第 16..24 字节）。
pub fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 24 || &bytes[1..4] != b"PNG" {
        return None;
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    (width > 0 && height > 0).then_some((width, height))
}

/// 把像素坐标归一化到 0-1（原点左上）。
pub fn normalize(
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    image: (f32, f32),
) -> (f32, f32, f32, f32) {
    let (image_w, image_h) = image;
    if image_w <= 0.0 || image_h <= 0.0 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    (
        (left / image_w).clamp(0.0, 1.0),
        (top / image_h).clamp(0.0, 1.0),
        (width / image_w).clamp(0.0, 1.0),
        (height / image_h).clamp(0.0, 1.0),
    )
}

/// 解析 `tesseract ... tsv` 输出第 5 级（词）行。
pub fn parse_tesseract_tsv(tsv: &str) -> Vec<OcrWord> {
    let mut words = Vec::new();
    let mut extent = (0.0_f32, 0.0_f32);
    for line in tsv.lines().skip(1) {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 12 {
            continue;
        }
        if cols[0].trim() != "5" {
            continue;
        }
        let text = cols[11].trim();
        if text.is_empty() {
            continue;
        }
        let parse = |index: usize| cols[index].trim().parse::<f32>().unwrap_or(0.0);
        let (left, top, width, height) = (parse(6), parse(7), parse(8), parse(9));
        extent.0 = extent.0.max(left + width);
        extent.1 = extent.1.max(top + height);
        words.push(OcrWord {
            text: text.to_string(),
            x: left,
            y: top,
            width,
            height,
        });
    }
    // tesseract 的 tsv 不带画布尺寸；用最大外接矩形近似，避免把坐标丢掉。
    if words
        .iter()
        .any(|word| word.width > 1.0 || word.height > 1.0)
        && extent.0 > 0.0
        && extent.1 > 0.0
    {
        for word in &mut words {
            let (x, y, w, h) = normalize(word.x, word.y, word.width, word.height, extent);
            word.x = x;
            word.y = y;
            word.width = w;
            word.height = h;
        }
    }
    words
}

fn join_lines(words: &[OcrWord]) -> String {
    // 按 y 分桶成行，行内按 x 排序，贴近人读的顺序。
    let mut sorted: Vec<&OcrWord> = words.iter().collect();
    sorted.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal));
    let mut lines: Vec<Vec<&OcrWord>> = Vec::new();
    for word in sorted {
        match lines.last_mut() {
            Some(line) if (word.y - line[0].y).abs() < 0.02 => line.push(word),
            _ => lines.push(vec![word]),
        }
    }
    let mut out = String::new();
    for mut line in lines {
        line.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(std::cmp::Ordering::Equal));
        let text = line
            .iter()
            .map(|word| word.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        if !text.is_empty() {
            out.push_str(&text);
            out.push('\n');
        }
    }
    out.trim_end().to_string()
}

mod macos {
    use super::*;

    const SHIM: &str = r#"
import Foundation
import AppKit
import Vision

let args = CommandLine.arguments
guard args.count > 1 else { exit(2) }
let url = URL(fileURLWithPath: args[1])
guard let image = NSImage(contentsOf: url),
      let tiff = image.tiffRepresentation,
      let bitmap = NSBitmapImageRep(data: tiff),
      let cg = bitmap.cgImage else { exit(3) }

let request = VNRecognizeTextRequest()
request.recognitionLevel = .accurate
request.usesLanguageCorrection = true
let handler = VNImageRequestHandler(cgImage: cg, options: [:])
try handler.perform([request])

var lines: [String] = []
for case let observation as VNRecognizedTextObservation in request.results ?? [] {
    guard let top = observation.topCandidates(1).first else { continue }
    let box = observation.boundingBox
    let x = box.origin.x
    let y = 1.0 - (box.origin.y + box.size.height)
    let joined = [top.string, "\(x)", "\(y)", "\(box.size.width)", "\(box.size.height)"] as [String]
    lines.append(joined.joined(separator: "\u{001F}"))
}
print(lines.joined(separator: "\n"))
"#;

    fn cache_dir() -> PathBuf {
        std::env::temp_dir().join("codefactory-vision-ocr")
    }

    /// 编译一次即可复用：源码与产物都在临时目录，不进仓库也不进用户配置。
    async fn shim_binary() -> Result<PathBuf, String> {
        let dir = cache_dir();
        std::fs::create_dir_all(&dir).map_err(|error| format!("SHIM_DIR: {error}"))?;
        let source = dir.join("vision-ocr.swift");
        let binary = dir.join("vision-ocr");
        let needs_build = match (std::fs::metadata(&binary), std::fs::metadata(&source)) {
            (Ok(bin), Ok(src)) => match (bin.modified(), src.modified()) {
                (Ok(bin_time), Ok(src_time)) => bin_time < src_time,
                _ => !binary.exists(),
            },
            _ => true,
        };
        if needs_build {
            std::fs::write(&source, SHIM).map_err(|error| format!("SHIM_WRITE: {error}"))?;
            let output = Command::new("swiftc")
                .no_window()
                .arg("-O")
                .arg("-o")
                .arg(&binary)
                .arg(&source)
                .output()
                .await
                .map_err(|error| format!("SHIM_COMPILE_SPAWN: {error}"))?;
            if !output.status.success() {
                return Err(format!(
                    "SHIM_COMPILE: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
        }
        Ok(binary)
    }

    pub async fn recognize(image_path: &Path) -> Result<OcrResult, String> {
        let binary = shim_binary().await?;
        let output = Command::new(&binary)
            .no_window()
            .arg(image_path)
            .output()
            .await
            .map_err(|error| format!("VISION_SPAWN: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "VISION_EXIT {}: {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let words = parse_shim_output(&stdout);
        Ok(OcrResult {
            backend: "macos-vision".to_string(),
            text: join_lines(&words),
            words,
        })
    }

    pub fn parse_shim_output(stdout: &str) -> Vec<OcrWord> {
        let mut words = Vec::new();
        for line in stdout.lines() {
            let cols: Vec<&str> = line.split('\u{001F}').collect();
            if cols.len() < 5 {
                continue;
            }
            let text = cols[0].trim();
            if text.is_empty() {
                continue;
            }
            let parse = |index: usize| cols[index].trim().parse::<f32>().unwrap_or(0.0);
            words.push(OcrWord {
                text: text.to_string(),
                x: parse(1).clamp(0.0, 1.0),
                y: parse(2).clamp(0.0, 1.0),
                width: parse(3).clamp(0.0, 1.0),
                height: parse(4).clamp(0.0, 1.0),
            });
        }
        words
    }
}

mod windows {
    use super::*;

    const SCRIPT: &str = r#"
param([string]$Path)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$null = [Windows.Media.Ocr.OcrEngine, Windows.Foundation, ContentType = WindowsRuntime]
$null = [Windows.Graphics.Imaging.BitmapDecoder, Windows.Foundation, ContentType = WindowsRuntime]
$null = [Windows.Storage.StorageFile, Windows.Foundation, ContentType = WindowsRuntime]

$asTask = ([System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and
    $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
})[0]

function Await($op, $type) {
    $task = $asTask.MakeGenericMethod($type).Invoke($null, @($op))
    $task.Wait(30000) | Out-Null
    $task.Result
}

$file = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync($Path)) ([Windows.Storage.StorageFile])
$stream = Await ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
$decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
$bitmap = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])

$engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
if ($null -eq $engine) { $engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromLanguage('en-US') }
if ($null -eq $engine) { throw 'NO_OCR_ENGINE' }
$result = Await ($engine.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult])

$width = [double]$bitmap.PixelWidth
$height = [double]$bitmap.PixelHeight
$out = @()
foreach ($line in $result.Lines) {
    foreach ($word in $line.Words) {
        $rect = $word.BoundingRect
        $out += [string]::Join([char]0x1F, @(
            $word.Text,
            [string]($rect.X / $width),
            [string]($rect.Y / $height),
            [string]($rect.Width / $width),
            [string]($rect.Height / $height)
        ))
    }
}
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$out -join "`n"
"#;

    pub async fn recognize(image_path: &Path) -> Result<OcrResult, String> {
        let dir = std::env::temp_dir().join("codefactory-vision-ocr");
        std::fs::create_dir_all(&dir).map_err(|error| format!("OCR_DIR: {error}"))?;
        let script = dir.join("windows-ocr.ps1");
        std::fs::write(&script, SCRIPT).map_err(|error| format!("OCR_WRITE: {error}"))?;
        let output = Command::new("powershell")
            .no_window()
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&script)
            .arg(image_path)
            .output()
            .await
            .map_err(|error| format!("OCR_SPAWN: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "OCR_EXIT {}: {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let words = super::macos::parse_shim_output(&stdout);
        Ok(OcrResult {
            backend: "windows-ocr".to_string(),
            text: join_lines(&words),
            words,
        })
    }
}

mod tesseract {
    use super::*;

    pub async fn recognize(image_path: &Path) -> Result<OcrResult, String> {
        let output = Command::new("tesseract")
            .no_window()
            .arg(image_path)
            .arg("stdout")
            .arg("tsv")
            .output()
            .await
            .map_err(|error| format!("TESSERACT_SPAWN: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "TESSERACT_EXIT {}: {}",
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let words = parse_tesseract_tsv(&String::from_utf8_lossy(&output.stdout));
        Ok(OcrResult {
            backend: "tesseract".to_string(),
            text: join_lines(&words),
            words,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_dimensions_reads_ihdr() {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&3_u32.to_be_bytes());
        png.extend_from_slice(&7_u32.to_be_bytes());
        png.extend_from_slice(&[0; 8]);
        assert_eq!(png_dimensions(&png), Some((3, 7)));
        assert_eq!(png_dimensions(b"not a png"), None);
    }

    #[test]
    fn tesseract_tsv_keeps_only_word_rows_and_normalizes() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
1\t1\t0\t0\t0\t0\t0\t0\t1000\t500\t-1\t\n\
5\t1\t1\t1\t1\t1\t100\t100\t200\t50\t95\tHello\n\
5\t1\t1\t1\t1\t2\t400\t100\t150\t50\t93\tworld\n\
5\t1\t1\t1\t2\t1\t100\t400\t200\t50\t90\tsecond\n";
        let words = parse_tesseract_tsv(tsv);
        assert_eq!(words.len(), 3);
        assert_eq!(words[0].text, "Hello");
        assert!(words[0].x > 0.0 && words[0].x < 1.0, "{:?}", words[0]);
        let text = join_lines(&words);
        assert_eq!(text, "Hello world\nsecond");
    }

    #[test]
    fn normalize_clamps_and_handles_zero_canvas() {
        assert_eq!(
            normalize(50.0, 25.0, 100.0, 50.0, (200.0, 100.0)),
            (0.25, 0.25, 0.5, 0.5)
        );
        assert_eq!(
            normalize(1.0, 1.0, 1.0, 1.0, (0.0, 0.0)),
            (0.0, 0.0, 0.0, 0.0)
        );
        assert_eq!(
            normalize(1000.0, 1000.0, 10.0, 10.0, (100.0, 100.0)),
            (1.0, 1.0, 0.1, 0.1)
        );
    }

    #[test]
    fn vision_shim_output_is_parsed_with_top_left_origin() {
        let stdout = "标题\u{1F}0.1\u{1F}0.2\u{1F}0.3\u{1F}0.05\n";
        let words = super::macos::parse_shim_output(stdout);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].text, "标题");
        assert!((words[0].y - 0.2).abs() < 1e-6);
    }

    #[tokio::test]
    async fn real_ocr_reads_text_from_a_rendered_image() {
        // 用 QuickLook 把一段纯文本渲染成 PNG，再让系统 OCR 读回来。
        // 这同时验证了渲染管线与 OCR 管线，且不依赖任何网络或视觉模型。
        let dir = std::env::temp_dir().join(format!("cf-vision-ocr-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let source = dir.join("sample.txt");
        std::fs::write(&source, "HELLO FACTORY 42").unwrap();
        let rendered = super::super::render::quicklook_thumbnail(&source, &dir, 1200).await;
        let Ok(png_path) = rendered else {
            // 无图形会话（headless CI）跳过，不算失败。
            eprintln!("skipping live OCR check: thumbnail render unavailable");
            return;
        };
        let result = recognize(&png_path)
            .await
            .expect("local OCR must read rendered text");
        let upper = result.text.to_uppercase();
        assert!(
            upper.contains("HELLO") && upper.contains("42"),
            "OCR text mismatch: {:?}",
            result.text
        );
        assert!(!result.words.is_empty(), "OCR must report word boxes");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
