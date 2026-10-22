// SPDX-License-Identifier: Apache-2.0
//! `view_image`：让用户在会话里说"看看这张图"时，任何模型都能拿到图片内容。
//!
//! 执行顺序是固定的，且每一步都在输出里留痕：
//! 1. 图片文件 → （文档则先渲染指定页）→ 原始字节；
//! 2. 有支持视觉的端点 → 把图片发过去，返回描述（缓存命中则不重复调用）；
//! 3. 没有视觉端点 → 本地 OCR，输出里写明"未使用视觉模型"；
//! 4. 两条路径都不可用 → 可读的说明文本，**不让回合失败**。

use std::path::PathBuf;

use serde_json::{json, Value};

use super::{ExecCtx, ToolOutput};
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};
use crate::vision::{self, ocr, render, ImageInput, VisionAnswer};

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp"];

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".to_string(),
        function: FunctionDefinition {
            name: "view_image".to_string(),
            description: "Look at an image file (or one page of a PPTX/PDF) and answer a question about it. Use this instead of guessing whenever the user refers to a picture, screenshot, slide, chart, or asks what is in an image. Returns a textual description; the first line states whether the image was sent to a vision-capable endpoint or read by local OCR.".to_string(),
            parameters: json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Image file path (png/jpg/jpeg/webp/gif/bmp), or a .pptx/.pdf document path combined with `page`."
                },
                "question": {
                    "type": "string",
                    "description": "What to find out from the image. Be specific: 'what does the error dialog say', 'where is the logo placed', 'summarize this slide'."
                },
                "page": {
                    "type": "integer",
                    "description": "1-based page/slide number for .pptx or .pdf input. Defaults to 1."
                }
            },
            "required": ["path", "question"]
        }),
        },
    }
}

fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn resolve_path(ctx: &ExecCtx, raw: &str) -> PathBuf {
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path
    } else {
        ctx.cwd.join(path)
    }
}

fn mime_for_extension(extension: &str) -> &'static str {
    match extension {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        _ => "image/png",
    }
}

pub async fn execute(args: Value, ctx: &ExecCtx) -> ToolOutput {
    let Some(raw_path) = arg_str(&args, "path") else {
        return ToolOutput::err("view_image 需要一个非空的 `path`");
    };
    let Some(question) = arg_str(&args, "question") else {
        return ToolOutput::err("view_image 需要一个非空的 `question`（想问图片什么）");
    };
    let page = args
        .get("page")
        .and_then(Value::as_u64)
        .map(|value| value as u32)
        .unwrap_or(1);

    let path = resolve_path(ctx, &raw_path);
    if !path.exists() {
        return ToolOutput::err(format!("IMAGE_NOT_FOUND: {}", path.display()));
    }
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_lowercase();

    // 文档：先渲染成 PNG，渲染器与局限写进元数据。
    let mut renderer: Option<String> = None;
    let mut render_note: Option<String> = None;
    let mut label = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("image")
        .to_string();

    let (bytes, mime) = if IMAGE_EXTENSIONS.contains(&extension.as_str()) {
        match std::fs::read(&path) {
            Ok(bytes) => (bytes, mime_for_extension(&extension)),
            Err(error) => return ToolOutput::err(format!("IMAGE_READ_FAILED: {error}")),
        }
    } else if matches!(extension.as_str(), "pptx" | "ppt" | "pdf") {
        let out_dir = std::env::temp_dir().join(format!(
            "codefactory-vision-{}",
            std::process::id()
        ));
        match render::render_page(&path, page, &out_dir).await {
            Ok(rendered) => {
                renderer = Some(rendered.renderer);
                render_note = rendered.note;
                label = format!("{label} 第 {page} 页");
                match std::fs::read(&rendered.png_path) {
                    Ok(bytes) => (bytes, "image/png"),
                    Err(error) => return ToolOutput::err(format!("RENDER_READ_FAILED: {error}")),
                }
            }
            Err(error) => {
                return ToolOutput::ok(format!(
                    "无法渲染该文档的第 {page} 页，因此没有读到任何画面内容。\n原因：{error}\n\n\
                     可用渲染器：{:?}。PPTX/PDF 按页渲染需要系统自带 QuickLook（仅第 1 页）、\
                     PowerPoint，或 LibreOffice + poppler(pdftoppm)。",
                    render::available_renderers()
                ))
            }
        }
    } else {
        return ToolOutput::err(format!(
            "UNSUPPORTED_IMAGE_TYPE: .{extension}（支持 png/jpg/jpeg/webp/gif/bmp 与 pptx/pdf）"
        ));
    };

    if bytes.len() > vision::MAX_IMAGE_BYTES {
        return ToolOutput::ok(format!(
            "图片太大，没有发送给任何端点：{} 字节超过上限 {} 字节。请先压缩或裁剪后再看。",
            bytes.len(),
            vision::MAX_IMAGE_BYTES
        ));
    }

    let image = ImageInput::new(bytes, mime, label);
    let session_id = ctx.session_id.clone().unwrap_or_else(|| "no-session".to_string());
    let db = ctx.db.as_ref();
    let routes = ctx
        .settings
        .as_ref()
        .map(vision::vision_routes)
        .unwrap_or_default();

    let mut attempts: Vec<String> = Vec::new();

    for route in routes {
        let route_label = format!("{}/{}", route.endpoint_name, route.model_id);
        let outcome = vision::describe_cached(db, &session_id, &image, &question, || {
            vision::ask_vision_route(route.clone(), &session_id, &image, &question)
        })
        .await;
        match outcome {
            Ok((answer, hit)) => {
                let cache_hit = hit;
                return finish(answer, &image, &question, renderer, render_note, cache_hit, attempts);
            }
            Err(error) => attempts.push(format!("{route_label}: {error}")),
        }
    }

    // 没有视觉端点（或全部失败）：退到本地 OCR。
    let ocr_error = match local_ocr(&path, &extension, page).await {
        Ok(result) => {
            let answer = vision::local_ocr_answer(result, &question);
            return finish(answer, &image, &question, renderer, render_note, false, attempts);
        }
        Err(error) => Some(error),
    };
    let answer = vision::no_backend_answer(&image, &question, ocr_error);
    finish(answer, &image, &question, renderer, render_note, false, attempts)
}

/// 文档走"渲染后的 PNG 再 OCR"，图片直接 OCR；这样两条路径看到的是同一张图。
async fn local_ocr(
    path: &std::path::Path,
    extension: &str,
    page: u32,
) -> Result<ocr::OcrResult, String> {
    if IMAGE_EXTENSIONS.contains(&extension) {
        return ocr::recognize(path).await;
    }
    let out_dir = std::env::temp_dir().join(format!("codefactory-vision-ocr-{}", std::process::id()));
    let rendered = render::render_page(path, page, &out_dir).await?;
    ocr::recognize(&rendered.png_path).await
}

#[allow(clippy::too_many_arguments)]
fn finish(
    answer: VisionAnswer,
    image: &ImageInput,
    question: &str,
    renderer: Option<String>,
    render_note: Option<String>,
    cache_hit: bool,
    attempts: Vec<String>,
) -> ToolOutput {
    let mut content = answer.render(image, question);
    if let Some(note) = render_note.as_deref() {
        content.push_str(&format!("\n\n渲染说明：{note}"));
    }
    if cache_hit {
        content.push_str("\n\n（本次命中缓存，没有再次调用模型）");
    }
    if !attempts.is_empty() {
        content.push_str(&format!("\n\n视觉端点尝试记录：{}", attempts.join(" | ")));
    }

    let metadata = json!({
        "surface": vision::SURFACE,
        "backend": answer.backend.as_str(),
        "endpoint": answer.endpoint,
        "model": answer.model,
        "ocr_backend": answer.ocr_backend,
        "input_tokens": answer.input_tokens,
        "output_tokens": answer.output_tokens,
        "cache_hit": cache_hit,
        "renderer": renderer,
        "degraded": answer.degraded_note.is_some(),
    });
    ToolOutput::ok(content).with_metadata(metadata)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_exposure_follows_model_vision_capability() {
        let names = |has_vision: bool| -> Vec<String> {
            super::super::definitions_for_capability(has_vision)
                .into_iter()
                .map(|definition| definition.function.name)
                .collect()
        };
        let text_only_model = names(false);
        let vision_model = names(true);
        assert!(
            text_only_model.iter().any(|name| name == "view_image"),
            "a text-only model must be offered view_image"
        );
        assert!(
            !vision_model.iter().any(|name| name == "view_image"),
            "a vision model must not be offered view_image"
        );
        // 除此之外清单一致，说明过滤只影响这一个工具。
        assert_eq!(
            text_only_model.len(),
            vision_model.len() + 1,
            "only view_image may be filtered"
        );
    }

    fn ctx() -> ExecCtx {
        ExecCtx::new(std::env::temp_dir(), None)
    }

    #[tokio::test]
    async fn missing_arguments_fail_with_actionable_text() {
        let out = execute(json!({}), &ctx()).await;
        assert!(out.is_error);
        assert!(out.content.contains("path"));
        let out = execute(json!({ "path": "x.png" }), &ctx()).await;
        assert!(out.is_error);
        assert!(out.content.contains("question"));
    }

    #[tokio::test]
    async fn missing_file_is_reported_not_panicked() {
        let out = execute(json!({ "path": "nope.png", "question": "what" }), &ctx()).await;
        assert!(out.is_error);
        assert!(out.content.starts_with("IMAGE_NOT_FOUND"));
    }

    #[tokio::test]
    async fn unsupported_extension_is_rejected_by_name() {
        let dir = std::env::temp_dir().join(format!("cf-view-image-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "hi").unwrap();
        let out = execute(
            json!({ "path": file.to_string_lossy(), "question": "what" }),
            &ExecCtx::new(dir.clone(), None),
        )
        .await;
        assert!(out.is_error);
        assert!(out.content.contains("UNSUPPORTED_IMAGE_TYPE"), "{}", out.content);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn no_vision_route_and_no_ocr_still_returns_readable_text() {
        // 一个真实的 1x1 PNG：没有视觉端点时应走 OCR；OCR 不可用时也必须给出说明。
        let dir = std::env::temp_dir().join(format!("cf-view-image-ok-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("tiny.png");
        std::fs::write(
            &png,
            [
                0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D',
                b'R', 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0,
                0x0A, b'I', b'D', b'A', b'T', 0x78, 0x9C, 0x63, 0, 0, 0, 2, 0, 1, 0, 0, 0, 0,
                0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82,
            ],
        )
        .unwrap();
        let out = execute(
            json!({ "path": png.to_string_lossy(), "question": "图里有什么" }),
            &ExecCtx::new(dir.clone(), None),
        )
        .await;
        assert!(!out.is_error, "the turn must not fail: {}", out.content);
        let metadata = out.metadata.expect("metadata records the attribution");
        assert_eq!(metadata["surface"], vision::SURFACE);
        assert!(
            out.content.contains("本地 OCR") || out.content.contains("没有任何可用的看图路径"),
            "{}",
            out.content
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
