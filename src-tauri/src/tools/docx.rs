// SPDX-License-Identifier: Apache-2.0
//! `write_docx` tool — synthesize a real .docx Word file from structured blocks.
//!
//! Companion to `write_pptx`: pptx covers slide decks, docx covers
//! reports / memos / RFCs / specs. Both share the OOXML zip+XML pattern;
//! docx is actually simpler (one document.xml carries all body content
//! vs pptx's per-slide files + masters + layouts).
//!
//! ## What it generates
//!
//! A minimum-viable OOXML WordprocessingML package: zip with
//! `[Content_Types].xml`, top-level rels, `docProps/{app,core}.xml`,
//! `word/document.xml` (the content), and `word/styles.xml` (heading +
//! body styles). Opens cleanly in Word 2024, Pages, LibreOffice Writer,
//! Google Docs.
//!
//! ## Block model
//!
//! Each input block is one of:
//!   - `{ "kind": "heading", "level": 1..6, "text": "..." }`
//!   - `{ "kind": "paragraph", "text": "..." }`
//!   - `{ "kind": "bullet", "text": "..." }` (level 0 list item)
//!   - `{ "kind": "numbered", "text": "..." }` (level 0 numbered list)
//!
//! Future kinds (tables, images, code blocks) can extend without
//! changing the public surface — kind discriminator + new XML emitter.

use serde::Deserialize;
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

use super::{path_sanity, workspace_path, ExecCtx, ToolOutput};
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};

#[derive(Deserialize, Debug)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum DocxBlock {
    Heading {
        #[serde(default = "default_heading_level")]
        level: u8,
        text: String,
    },
    Paragraph { text: String },
    Bullet    { text: String },
    Numbered  { text: String },
}

fn default_heading_level() -> u8 { 1 }

#[derive(Deserialize)]
struct Args {
    /// Workspace-relative or absolute path ending in `.docx`.
    path: String,
    /// Document body in order. Must have at least one block.
    blocks: Vec<DocxBlock>,
}

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "write_docx".into(),
            description: "Generate a real .docx Word file from a list of structured blocks. \
Block kinds: heading (level 1-6), paragraph, bullet, numbered. Opens in Word, Pages, \
LibreOffice Writer, Google Docs. Use this when the user wants a report / memo / \
spec / RFC synthesized — pair with kb_search to pull source content first.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative or absolute path ending in .docx.",
                    },
                    "blocks": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "oneOf": [
                                { "type": "object", "properties": {
                                    "kind":  { "const": "heading" },
                                    "level": { "type": "integer", "minimum": 1, "maximum": 6 },
                                    "text":  { "type": "string" }
                                }, "required": ["kind", "text"] },
                                { "type": "object", "properties": {
                                    "kind": { "const": "paragraph" },
                                    "text": { "type": "string" }
                                }, "required": ["kind", "text"] },
                                { "type": "object", "properties": {
                                    "kind": { "const": "bullet" },
                                    "text": { "type": "string" }
                                }, "required": ["kind", "text"] },
                                { "type": "object", "properties": {
                                    "kind": { "const": "numbered" },
                                    "text": { "type": "string" }
                                }, "required": ["kind", "text"] }
                            ]
                        },
                    },
                },
                "required": ["path", "blocks"],
            }),
        },
    }
}

pub async fn execute(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let a: Args = match serde_json::from_value(args.clone()) {
        Ok(v) => v,
        Err(e) => {
            return Ok(ToolOutput::err(format!(
                "Invalid arguments for write_docx: {e}. Received: {}",
                serde_json::to_string(&args).unwrap_or_default()
            )));
        }
    };

    if a.blocks.is_empty() {
        return Ok(ToolOutput::err("write_docx requires at least one block".to_string()));
    }
    if a.blocks.len() > 1000 {
        return Ok(ToolOutput::err(format!(
            "write_docx cap is 1000 blocks per call (got {}). Split into multiple files if needed.",
            a.blocks.len()
        )));
    }

    let abs_path = match workspace_path::resolve_writable(&ctx.cwd, &a.path) {
        Ok(p) => p,
        Err(err) => return Ok(ToolOutput::err(err.message())),
    };
    if !abs_path.to_string_lossy().to_lowercase().ends_with(".docx") {
        return Ok(ToolOutput::err("path must end with .docx".to_string()));
    }
    if let Some(s) = path_sanity::check(&abs_path) {
        return Ok(ToolOutput::err(path_sanity::format_error(&s, &abs_path, "write_docx")));
    }
    if let Some(parent) = abs_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| crate::errors::AppError::Other(format!("mkdir failed: {e}")))?;
    }

    let bytes = build_docx_bytes(&a.blocks).map_err(|e| {
        crate::errors::AppError::Other(format!("docx build failed: {e}"))
    })?;
    super::file_lock::atomic_write(&abs_path, &bytes).await
        .map_err(|e| crate::errors::AppError::Other(format!("write failed: {e}")))?;

    Ok(ToolOutput::ok(format!(
        "Wrote {} ({} blocks, {} bytes). Open with Word / Pages / Google Docs.",
        rel_for_display(&abs_path, ctx),
        a.blocks.len(),
        bytes.len()
    )))
}

fn rel_for_display(p: &Path, ctx: &ExecCtx) -> String {
    p.strip_prefix(&ctx.cwd)
        .map(|r| r.to_string_lossy().to_string())
        .unwrap_or_else(|_| p.to_string_lossy().to_string())
}

// ─────────────────────────────────────────────────────────────────────────────
// DOCX bytes builder.
// ─────────────────────────────────────────────────────────────────────────────

fn build_docx_bytes(blocks: &[DocxBlock]) -> std::result::Result<Vec<u8>, String> {
    let buf = std::io::Cursor::new(Vec::<u8>::new());
    let mut zip = ZipWriter::new(buf);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    macro_rules! write_part {
        ($path:expr, $content:expr) => {{
            zip.start_file::<_, ()>($path, opts)
                .map_err(|e| format!("start_file {}: {e}", $path))?;
            zip.write_all($content.as_bytes())
                .map_err(|e| format!("write {}: {e}", $path))?;
        }};
    }

    write_part!("[Content_Types].xml", CONTENT_TYPES_XML);
    write_part!("_rels/.rels", REL_TOP);
    write_part!("docProps/app.xml", APP_XML);
    write_part!("docProps/core.xml", CORE_XML);
    write_part!("word/_rels/document.xml.rels", DOCUMENT_RELS);
    write_part!("word/styles.xml", STYLES_XML);
    write_part!("word/numbering.xml", NUMBERING_XML);
    write_part!("word/document.xml", document_xml(blocks));

    let cursor = zip.finish().map_err(|e| format!("zip finish: {e}"))?;
    Ok(cursor.into_inner())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn document_xml(blocks: &[DocxBlock]) -> String {
    let mut body = String::new();
    for b in blocks {
        body.push_str(&block_xml(b));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body>
{body}<w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="720" w:footer="720" w:gutter="0"/></w:sectPr>
</w:body>
</w:document>"#
    )
}

fn block_xml(b: &DocxBlock) -> String {
    match b {
        DocxBlock::Heading { level, text } => {
            let lvl = (*level).clamp(1, 6);
            let style = format!("Heading{lvl}");
            let t = xml_escape(text);
            format!(
                "<w:p><w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr><w:r><w:t xml:space=\"preserve\">{t}</w:t></w:r></w:p>\n"
            )
        }
        DocxBlock::Paragraph { text } => {
            let t = xml_escape(text);
            format!(
                "<w:p><w:r><w:t xml:space=\"preserve\">{t}</w:t></w:r></w:p>\n"
            )
        }
        DocxBlock::Bullet { text } => {
            let t = xml_escape(text);
            // numId="1" is defined in numbering.xml as bullets; ilvl=0 is the
            // outermost level. Word renders this as a • bullet.
            format!(
                "<w:p><w:pPr><w:pStyle w:val=\"ListParagraph\"/><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"1\"/></w:numPr></w:pPr><w:r><w:t xml:space=\"preserve\">{t}</w:t></w:r></w:p>\n"
            )
        }
        DocxBlock::Numbered { text } => {
            let t = xml_escape(text);
            // numId="2" is numbered list in numbering.xml.
            format!(
                "<w:p><w:pPr><w:pStyle w:val=\"ListParagraph\"/><w:numPr><w:ilvl w:val=\"0\"/><w:numId w:val=\"2\"/></w:numPr></w:pPr><w:r><w:t xml:space=\"preserve\">{t}</w:t></w:r></w:p>\n"
            )
        }
    }
}

// ── Static XML constants ────────────────────────────────────────────────────

const CONTENT_TYPES_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>
<Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/>
<Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>
<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>
</Types>"#;

const REL_TOP: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>
<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/>
</Relationships>"#;

const APP_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties">
<Application>CodeFactory</Application>
</Properties>"#;

const CORE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:creator>CodeFactory</dc:creator>
<cp:lastModifiedBy>CodeFactory</cp:lastModifiedBy>
</cp:coreProperties>"#;

const DOCUMENT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>
</Relationships>"#;

const STYLES_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="SimSun"/><w:sz w:val="22"/></w:rPr></w:rPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:spacing w:before="240" w:after="60"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:rFonts w:ascii="Calibri Light" w:hAnsi="Calibri Light" w:eastAsia="SimSun"/><w:b/><w:sz w:val="40"/><w:color w:val="2E74B5"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:spacing w:before="200" w:after="60"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:rFonts w:ascii="Calibri Light" w:hAnsi="Calibri Light" w:eastAsia="SimSun"/><w:b/><w:sz w:val="32"/><w:color w:val="2E74B5"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:spacing w:before="180" w:after="60"/><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:rFonts w:ascii="Calibri Light" w:hAnsi="Calibri Light" w:eastAsia="SimSun"/><w:b/><w:sz w:val="28"/><w:color w:val="1F4E79"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading4"><w:name w:val="heading 4"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:spacing w:before="160" w:after="60"/><w:outlineLvl w:val="3"/></w:pPr><w:rPr><w:b/><w:sz w:val="24"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading5"><w:name w:val="heading 5"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:spacing w:before="140" w:after="60"/><w:outlineLvl w:val="4"/></w:pPr><w:rPr><w:b/><w:sz w:val="22"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading6"><w:name w:val="heading 6"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:spacing w:before="120" w:after="60"/><w:outlineLvl w:val="5"/></w:pPr><w:rPr><w:b/><w:i/><w:sz w:val="22"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="ListParagraph"><w:name w:val="List Paragraph"/><w:basedOn w:val="Normal"/><w:pPr><w:ind w:left="720"/></w:pPr></w:style>
</w:styles>"#;

const NUMBERING_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="bullet"/><w:lvlText w:val="•"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr><w:rPr><w:rFonts w:ascii="Symbol" w:hAnsi="Symbol"/></w:rPr></w:lvl></w:abstractNum>
<w:abstractNum w:abstractNumId="1"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum>
<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
<w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>
</w:numbering>"#;

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use zip::ZipArchive;

    fn read_part(zip: &mut ZipArchive<std::io::Cursor<&Vec<u8>>>, name: &str) -> String {
        let mut s = String::new();
        zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
        s
    }

    #[test]
    fn generates_valid_zip_with_expected_parts() {
        let bytes = build_docx_bytes(&[
            DocxBlock::Heading { level: 1, text: "Title".into() },
            DocxBlock::Paragraph { text: "Body sentence.".into() },
            DocxBlock::Bullet { text: "first bullet".into() },
            DocxBlock::Numbered { text: "first number".into() },
        ]).unwrap();
        let cursor = std::io::Cursor::new(&bytes);
        let mut zip = ZipArchive::new(cursor).expect("must be valid zip");
        for required in [
            "[Content_Types].xml", "_rels/.rels",
            "docProps/app.xml", "docProps/core.xml",
            "word/document.xml", "word/styles.xml", "word/numbering.xml",
            "word/_rels/document.xml.rels",
        ] {
            assert!(zip.by_name(required).is_ok(), "missing {required}");
        }
    }

    #[test]
    fn content_round_trips_through_document_xml() {
        let bytes = build_docx_bytes(&[
            DocxBlock::Heading { level: 2, text: "Section".into() },
            DocxBlock::Paragraph { text: "Hello world".into() },
            DocxBlock::Bullet { text: "first bullet".into() },
        ]).unwrap();
        let cursor = std::io::Cursor::new(&bytes);
        let mut zip = ZipArchive::new(cursor).unwrap();
        let doc = read_part(&mut zip, "word/document.xml");
        assert!(doc.contains("Section"));
        assert!(doc.contains("Hello world"));
        assert!(doc.contains("first bullet"));
        assert!(doc.contains("Heading2"));
        assert!(doc.contains("numId w:val=\"1\""), "bullet should reference numId=1");
    }

    #[test]
    fn special_xml_chars_are_escaped() {
        let bytes = build_docx_bytes(&[
            DocxBlock::Paragraph { text: "A < B & C > D \"q\" 'a'".into() },
        ]).unwrap();
        let cursor = std::io::Cursor::new(&bytes);
        let mut zip = ZipArchive::new(cursor).unwrap();
        let doc = read_part(&mut zip, "word/document.xml");
        assert!(doc.contains("A &lt; B &amp; C &gt; D"));
        assert!(doc.contains("&quot;q&quot;"));
        assert!(doc.contains("&apos;a&apos;"));
    }

    #[test]
    fn heading_level_clamped_to_1_6() {
        let bytes = build_docx_bytes(&[
            DocxBlock::Heading { level: 9, text: "Over".into() },
            DocxBlock::Heading { level: 0, text: "Under".into() },
        ]).unwrap();
        let cursor = std::io::Cursor::new(&bytes);
        let mut zip = ZipArchive::new(cursor).unwrap();
        let doc = read_part(&mut zip, "word/document.xml");
        assert!(doc.contains("Heading6"), "level 9 should clamp to 6");
        assert!(doc.contains("Heading1"), "level 0 should clamp to 1");
    }

    #[test]
    #[ignore]
    fn smoke_write_to_tmp() {
        let bytes = build_docx_bytes(&[
            DocxBlock::Heading { level: 1, text: "CodeFactory write_docx 验证".into() },
            DocxBlock::Paragraph { text: "这是一段中文正文，验证 OOXML 在 macOS QuickLook 渲染。".into() },
            DocxBlock::Heading { level: 2, text: "要点".into() },
            DocxBlock::Bullet { text: "纯 Rust 实现，零外部依赖".into() },
            DocxBlock::Bullet { text: "Word / Pages / LibreOffice / Google Docs 都能打开".into() },
            DocxBlock::Heading { level: 2, text: "用法".into() },
            DocxBlock::Numbered { text: "AI 调用 kb_search 读取源文档".into() },
            DocxBlock::Numbered { text: "AI 综合大纲".into() },
            DocxBlock::Numbered { text: "AI 调用 write_docx 出文件".into() },
        ]).unwrap();
        std::fs::write("/tmp/codefactory-docx-smoke.docx", &bytes).unwrap();
        eprintln!("wrote {} bytes", bytes.len());
    }
}
