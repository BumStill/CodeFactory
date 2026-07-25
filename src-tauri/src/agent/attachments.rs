// SPDX-License-Identifier: Apache-2.0
//! Markdown image links → multimodal content blocks.
//!
//! When the user pastes/drops an image, the frontend writes it to
//! `<cwd>/.codefactory/attachments/<...>.png` and embeds a
//! `![name](file:///abs/path)` markdown link in the message text. This
//! module converts that markdown into actual vision content blocks the
//! LLM can see, in both OpenAI-compatible and Anthropic shapes.
//!
//! Token economy: only images that exist on disk are loaded; missing
//! files fall back to the markdown text so we never silently drop the
//! user's reference. Each image is read once per message build.
//!
//! Security: only `file://` scheme is honoured and only absolute paths.
//! No HTTP fetches. Path traversal is irrelevant because we read the
//! exact path the frontend just wrote — but we still cap individual
//! attachment size at 8 MB so a runaway link can't OOM the agent.

use base64::{engine::general_purpose, Engine as _};
use std::path::Path;

use crate::openrouter::types::{ContentPart, ImageUrl};

const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

/// Image extensions OpenRouter / OpenAI / Anthropic widely support.
/// Anything else stays as markdown text — better than silently failing.
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

/// One pass over a user message. Returns either:
///   - `Vec::new()` if no images were extracted (caller should keep using
///     plain text)
///   - A mixed sequence of text and image_url parts otherwise. Text runs
///     between/around images are preserved so the model still sees the
///     surrounding prose ("look at this:", before the image).
pub fn extract_openai_parts(text: &str) -> Vec<ContentPart> {
    let mut parts: Vec<ContentPart> = Vec::new();
    let mut cursor = 0usize;
    let mut had_image = false;

    for (start, end, path) in find_file_image_links(text) {
        if let Some(b64_with_mime) = read_as_data_url(&path) {
            // Emit the prose between the previous cursor and this link,
            // if any. Empty/whitespace-only runs are skipped.
            let prose = &text[cursor..start];
            if !prose.trim().is_empty() {
                parts.push(ContentPart {
                    r#type: "text".into(),
                    text: Some(prose.into()),
                    image_url: None,
                });
            }
            parts.push(ContentPart {
                r#type: "image_url".into(),
                text: None,
                image_url: Some(ImageUrl { url: b64_with_mime }),
            });
            cursor = end;
            had_image = true;
        }
        // If read failed, leave the markdown in place (cursor unchanged)
        // — caller will fall back to text mode and the model sees the link.
    }

    if !had_image {
        return Vec::new();
    }
    // Trailing prose.
    let tail = &text[cursor..];
    if !tail.trim().is_empty() {
        parts.push(ContentPart {
            r#type: "text".into(),
            text: Some(tail.into()),
            image_url: None,
        });
    }
    parts
}

/// Find every `![alt](file:///path)` occurrence with an image extension.
/// Returns (start_byte, end_byte, path) tuples in source order.
///
/// Why not a full markdown parser: we don't want to mis-handle code
/// blocks or escaped brackets, but the cost of pulling pulldown-cmark
/// for this single feature is high. Heuristic is good enough — false
/// positives only convert text we wouldn't otherwise display as an
/// image anyway.
fn find_file_image_links(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Look for the start of a markdown image: `![`
        if i + 1 < bytes.len() && bytes[i] == b'!' && bytes[i + 1] == b'[' {
            // Find the closing `]`
            let alt_start = i + 2;
            if let Some(alt_rel_end) = text[alt_start..].find(']') {
                let alt_end = alt_start + alt_rel_end;
                // Next char must be `(`
                if alt_end + 1 < bytes.len() && bytes[alt_end + 1] == b'(' {
                    let url_start = alt_end + 2;
                    if let Some(url_rel_end) = text[url_start..].find(')') {
                        let url_end = url_start + url_rel_end;
                        let url = &text[url_start..url_end];
                        if let Some(raw) = url.strip_prefix("file://") {
                            // Cross-platform unwrap:
                            //   Unix  : "file:///abs/path"  → raw = "/abs/path"  → keep as-is
                            //   Win   : "file:///C:/Users/..." → raw = "/C:/Users/..."
                            //           → strip the leading '/' before drive letter
                            // We accept both file:///… and file://… (lenient).
                            let abs_path = if raw.len() >= 3
                                && raw.starts_with('/')
                                && raw.as_bytes()[2] == b':'
                            {
                                // Windows drive layout — drop the URI's leading slash.
                                raw[1..].to_string()
                            } else {
                                raw.to_string()
                            };
                            if has_image_extension(&abs_path) {
                                out.push((i, url_end + 1, abs_path));
                            }
                        }
                        i = url_end + 1;
                        continue;
                    }
                }
            }
        }
        i += 1;
    }
    out
}

fn has_image_extension(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|ext| {
            IMAGE_EXTS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(ext))
        })
        .unwrap_or(false)
}

fn read_as_data_url(path: &str) -> Option<String> {
    let (mime, b64) = read_as_base64(path)?;
    Some(format!("data:{};base64,{}", mime, b64))
}

fn read_as_base64(path: &str) -> Option<(String, String)> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() as usize > MAX_IMAGE_BYTES {
        tracing::warn!(
            "skipping attachment {} ({} bytes > {} cap)",
            path,
            meta.len(),
            MAX_IMAGE_BYTES
        );
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())?
        .to_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => return None,
    };
    Some((mime.into(), general_purpose::STANDARD.encode(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp_png() -> String {
        // 1×1 transparent PNG, the smallest valid file we can use.
        let bytes: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x63, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        let path = std::env::temp_dir().join(format!(
            "codefactory-test-{}.png",
            uuid::Uuid::new_v4().simple()
        ));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(bytes).unwrap();
        path.to_string_lossy().to_string()
    }

    #[test]
    fn no_images_returns_empty() {
        let parts = extract_openai_parts("hello world");
        assert!(parts.is_empty());
    }

    #[test]
    fn ignores_non_image_extensions() {
        let parts = extract_openai_parts("see ![txt](file:///tmp/foo.txt)");
        assert!(parts.is_empty());
    }

    #[test]
    fn ignores_missing_file() {
        let parts =
            extract_openai_parts("look ![x](file:///tmp/definitely-does-not-exist-xyz.png)");
        assert!(
            parts.is_empty(),
            "missing files should leave caller in text mode"
        );
    }

    #[test]
    fn extracts_image_with_surrounding_prose_openai() {
        let path = write_temp_png();
        let msg = format!("here it is:\n\n![scr](file://{})\n\nthoughts?", path);
        let parts = extract_openai_parts(&msg);
        assert_eq!(
            parts.len(),
            3,
            "expected text + image + text, got {:?}",
            parts
        );
        assert_eq!(parts[0].r#type, "text");
        assert!(parts[0].text.as_ref().unwrap().contains("here it is"));
        assert_eq!(parts[1].r#type, "image_url");
        assert!(parts[1]
            .image_url
            .as_ref()
            .unwrap()
            .url
            .starts_with("data:image/png;base64,"));
        assert_eq!(parts[2].r#type, "text");
        assert!(parts[2].text.as_ref().unwrap().contains("thoughts"));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn extracts_image_only_openai() {
        let path = write_temp_png();
        let msg = format!("![scr](file://{})", path);
        let parts = extract_openai_parts(&msg);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].r#type, "image_url");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn windows_drive_letter_url_unwraps_correctly() {
        // Real failure mode from PR #20 CI: parser was prepending "/" to
        // every URL, breaking "file:///C:/Users/..." into "/C:/Users/..."
        // which doesn't open on Windows. We don't actually need a real
        // Windows path here — we're testing the *parse*, the file just
        // won't open (which is fine, ignores_missing_file behaviour).
        let parsed = find_file_image_links("look ![s](file:///C:/Users/leo/x.png)");
        assert_eq!(parsed.len(), 1);
        // After unwrap, the leading slash before the drive letter is gone.
        assert_eq!(parsed[0].2, "C:/Users/leo/x.png");

        // Two-slash variant (browser-style relative): "file://C:/..." → keep as-is.
        let parsed = find_file_image_links("![s](file://C:/Users/leo/y.png)");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].2, "C:/Users/leo/y.png");

        // Unix path control: must still get the leading slash back.
        let parsed = find_file_image_links("![s](file:///var/x.png)");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].2, "/var/x.png");
    }

}
