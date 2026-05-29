// SPDX-License-Identifier: Apache-2.0
//! `write_pptx` tool — synthesize a real .pptx PowerPoint file from a
//! structured slide list.
//!
//! Why this exists: users frequently want CodeFactory to ingest their
//! folder of reference docs + decks (already handled by knowledge_base
//! which extracts text from .docx / .pptx / .pdf) and produce a fresh
//! deck. The "produce" half had no tool — AI could output markdown but
//! the user still had to convert by hand.
//!
//! ## What it generates
//!
//! A minimum-viable OOXML PowerPoint package: zip with `[Content_Types].xml`,
//! `_rels/.rels`, one slide master + one slide layout (title+content),
//! and one slide XML per input entry. Opens cleanly in PowerPoint,
//! Keynote, LibreOffice Impress, and Google Slides.
//!
//! ## What it doesn't do (yet)
//!
//! - Theme customization (uses bare Office defaults)
//! - Images / shapes / charts
//! - Speaker notes
//! - Animation / transitions
//!
//! These can be layered later by extending the per-slide XML template
//! without changing the public API.

use serde::Deserialize;
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipWriter};

use super::{path_sanity, workspace_path, ExecCtx, ToolOutput};
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};

#[derive(Deserialize, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SlideLayout {
    /// Standard layout: title at top, bullets below. Default.
    #[default]
    Content,
    /// Section-divider: large centered title only, body ignored. Use
    /// between major sections of a deck. Acts as a chapter cover.
    Section,
}

#[derive(Deserialize, Debug, Default)]
pub struct PptxSlide {
    /// Slide heading. Required, shown at the top.
    pub title: String,
    /// Body content. Each entry becomes one bullet. Empty list is OK
    /// (title-only slide).
    #[serde(default)]
    pub bullets: Vec<String>,
    /// Visual layout. Defaults to `content` (title + bullets). Use
    /// `section` for chapter divider slides — big centered title only.
    #[serde(default)]
    pub layout: SlideLayout,
    /// Speaker notes. Rendered in PowerPoint's presenter view and
    /// notes pages, hidden in slide-show mode. Empty = no notes.
    #[serde(default)]
    pub notes: String,
}

#[derive(Deserialize)]
struct Args {
    /// Workspace-relative or absolute path ending in `.pptx`. Created
    /// (parent dirs `mkdir -p`-ed); overwrites if it exists.
    path: String,
    /// Slides in display order. Must have at least one.
    slides: Vec<PptxSlide>,
}

pub fn definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "write_pptx".into(),
            description: "Generate a real .pptx PowerPoint file from a list of slides. \
Each slide has a title and an optional list of bullets. Opens in PowerPoint, \
Keynote, Google Slides, LibreOffice Impress. Use this when the user wants a \
deck synthesized from references — pair with kb_search to pull source content \
first, then call this with the synthesized outline.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Workspace-relative or absolute path ending in .pptx. Parent dirs auto-created; overwrites existing.",
                    },
                    "slides": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "title":   { "type": "string", "description": "Slide heading (required)." },
                                "bullets": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Body bullets (one entry per bullet). Empty = title-only slide.",
                                },
                                "layout": {
                                    "type": "string",
                                    "enum": ["content", "section"],
                                    "description": "Visual layout. 'content' (default) = title + bullets. 'section' = chapter divider, big centered title only.",
                                },
                                "notes": {
                                    "type": "string",
                                    "description": "Speaker notes. Shown in presenter view / notes pages, hidden in slide show. Empty = no notes.",
                                },
                            },
                            "required": ["title"],
                        },
                    },
                },
                "required": ["path", "slides"],
            }),
        },
    }
}

pub async fn execute(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let a: Args = match serde_json::from_value(args.clone()) {
        Ok(v) => v,
        Err(e) => {
            return Ok(ToolOutput::err(format!(
                "Invalid arguments for write_pptx: {e}. Received: {}",
                serde_json::to_string(&args).unwrap_or_default()
            )));
        }
    };

    if a.slides.is_empty() {
        return Ok(ToolOutput::err("write_pptx requires at least one slide".to_string()));
    }
    if a.slides.len() > 200 {
        return Ok(ToolOutput::err(
            "write_pptx cap is 200 slides per call (got {N}). Split into multiple calls if needed."
                .replace("{N}", &a.slides.len().to_string()),
        ));
    }

    let abs_path = match workspace_path::resolve_writable(&ctx.cwd, &a.path) {
        Ok(p) => p,
        Err(err) => return Ok(ToolOutput::err(err.message())),
    };
    if !abs_path.to_string_lossy().to_lowercase().ends_with(".pptx") {
        return Ok(ToolOutput::err("path must end with .pptx".to_string()));
    }
    // Hallucinated-path guard — same pattern as write_file.
    if let Some(s) = path_sanity::check(&abs_path) {
        return Ok(ToolOutput::err(path_sanity::format_error(&s, &abs_path, "write_pptx")));
    }
    if let Some(parent) = abs_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| crate::errors::AppError::Other(format!("mkdir failed: {e}")))?;
    }

    let bytes = build_pptx_bytes(&a.slides).map_err(|e| {
        crate::errors::AppError::Other(format!("pptx build failed: {e}"))
    })?;
    std::fs::write(&abs_path, &bytes)
        .map_err(|e| crate::errors::AppError::Other(format!("write failed: {e}")))?;

    Ok(ToolOutput::ok(format!(
        "Wrote {} ({} slides, {} bytes). Open with PowerPoint / Keynote / Google Slides.",
        rel_for_display(&abs_path, ctx),
        a.slides.len(),
        bytes.len()
    )))
}

fn rel_for_display(p: &Path, ctx: &ExecCtx) -> String {
    p.strip_prefix(&ctx.cwd)
        .map(|r| r.to_string_lossy().to_string())
        .unwrap_or_else(|_| p.to_string_lossy().to_string())
}

// ─────────────────────────────────────────────────────────────────────────────
// PPTX bytes builder.
//
// Output is a zip with this layout (the absolute minimum for Office /
// Keynote / Google Slides to accept and render):
//
//   [Content_Types].xml             — MIME type declarations
//   _rels/.rels                     — top-level relationships
//   docProps/app.xml                — app metadata (optional but Office complains without)
//   docProps/core.xml               — core metadata (creator, modified, ...)
//   ppt/presentation.xml            — slide list + slide size
//   ppt/_rels/presentation.xml.rels — links to slides + masters + theme
//   ppt/theme/theme1.xml            — color scheme
//   ppt/slideMasters/slideMaster1.xml
//   ppt/slideMasters/_rels/slideMaster1.xml.rels
//   ppt/slideLayouts/slideLayout1.xml
//   ppt/slideLayouts/_rels/slideLayout1.xml.rels
//   ppt/slides/slide{N}.xml             — one per input slide
//   ppt/slides/_rels/slide{N}.xml.rels  — one per input slide
//
// Every XML file is a hand-rolled string template. Tested by opening
// generated files in PowerPoint 2024 (mac), Keynote, and Google Slides.
// ─────────────────────────────────────────────────────────────────────────────

fn build_pptx_bytes(slides: &[PptxSlide]) -> std::result::Result<Vec<u8>, String> {
    let buf = std::io::Cursor::new(Vec::<u8>::new());
    let mut zip = ZipWriter::new(buf);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    // Helper closures consume `&mut zip` via reborrow each call.
    macro_rules! write_part {
        ($path:expr, $content:expr) => {{
            zip.start_file::<_, ()>($path, opts)
                .map_err(|e| format!("start_file {}: {e}", $path))?;
            zip.write_all($content.as_bytes())
                .map_err(|e| format!("write {}: {e}", $path))?;
        }};
    }

    // Indexes of slides that carry speaker notes — used to emit notesSlide
    // parts + presentation.xml content-type overrides only for those that
    // need them (notesless slides skip the extra files entirely).
    let notes_indexes: Vec<usize> = slides
        .iter()
        .enumerate()
        .filter_map(|(i, s)| if !s.notes.trim().is_empty() { Some(i) } else { None })
        .collect();
    let has_notes = !notes_indexes.is_empty();

    // ── Static infrastructure ─────────────────────────────────────────
    write_part!("[Content_Types].xml", content_types_xml(slides.len(), &notes_indexes));
    write_part!("_rels/.rels", REL_TOP);
    write_part!("docProps/app.xml", APP_XML);
    write_part!("docProps/core.xml", CORE_XML);
    write_part!("ppt/presentation.xml", presentation_xml(slides.len()));
    write_part!("ppt/_rels/presentation.xml.rels", presentation_rels(slides.len(), has_notes));
    write_part!("ppt/theme/theme1.xml", THEME_XML);
    write_part!("ppt/slideMasters/slideMaster1.xml", SLIDE_MASTER_XML);
    write_part!("ppt/slideMasters/_rels/slideMaster1.xml.rels", SLIDE_MASTER_RELS);
    write_part!("ppt/slideLayouts/slideLayout1.xml", SLIDE_LAYOUT_XML);
    write_part!("ppt/slideLayouts/_rels/slideLayout1.xml.rels", SLIDE_LAYOUT_RELS);

    if has_notes {
        write_part!("ppt/notesMasters/notesMaster1.xml", NOTES_MASTER_XML);
        write_part!("ppt/notesMasters/_rels/notesMaster1.xml.rels", NOTES_MASTER_RELS);
    }

    // ── Per-slide files ───────────────────────────────────────────────
    for (i, slide) in slides.iter().enumerate() {
        let n = i + 1;
        let slide_path = format!("ppt/slides/slide{n}.xml");
        let rels_path  = format!("ppt/slides/_rels/slide{n}.xml.rels");
        write_part!(&slide_path, slide_xml(slide));
        // Only this slide's rels reference its notesSlide when notes exist.
        let needs_notes = !slide.notes.trim().is_empty();
        write_part!(&rels_path, slide_rels(needs_notes, n));
        if needs_notes {
            let notes_path = format!("ppt/notesSlides/notesSlide{n}.xml");
            let notes_rels_path = format!("ppt/notesSlides/_rels/notesSlide{n}.xml.rels");
            write_part!(&notes_path, notes_slide_xml(&slide.notes));
            write_part!(&notes_rels_path, notes_slide_rels(n));
        }
    }

    // `finish` consumes the writer and returns the underlying Cursor.
    let cursor = zip.finish().map_err(|e| format!("zip finish: {e}"))?;
    Ok(cursor.into_inner())
}

// ── XML escape ───────────────────────────────────────────────────────────────

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

// ── Dynamic XML templates ────────────────────────────────────────────────────

fn content_types_xml(n: usize, notes_indexes: &[usize]) -> String {
    let mut overrides = String::new();
    for i in 1..=n {
        overrides.push_str(&format!(
            "<Override PartName=\"/ppt/slides/slide{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>"
        ));
    }
    // notesSlide overrides + a single notesMaster override (shared by all)
    if !notes_indexes.is_empty() {
        overrides.push_str("<Override PartName=\"/ppt/notesMasters/notesMaster1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.notesMaster+xml\"/>");
        for idx in notes_indexes {
            let n = idx + 1;
            overrides.push_str(&format!(
                "<Override PartName=\"/ppt/notesSlides/notesSlide{n}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml\"/>"
            ));
        }
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
<Override PartName="/ppt/slideMasters/slideMaster1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml"/>
<Override PartName="/ppt/slideLayouts/slideLayout1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml"/>
<Override PartName="/ppt/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>
<Override PartName="/docProps/app.xml" ContentType="application/vnd.openxmlformats-officedocument.extended-properties+xml"/>
<Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>
{overrides}
</Types>"#
    )
}

fn presentation_xml(n: usize) -> String {
    let mut slide_ids = String::new();
    // Slide id pool starts at 256 by Office convention.
    for i in 0..n {
        let sid = 256 + i;
        let r_id = i + 2; // rId1 = slideMaster, rId2+ = slides
        slide_ids.push_str(&format!("<p:sldId id=\"{sid}\" r:id=\"rId{r_id}\"/>"));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" saveSubsetFonts="1">
<p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst>
<p:sldIdLst>{slide_ids}</p:sldIdLst>
<p:sldSz cx="12192000" cy="6858000" type="screen16x9"/>
<p:notesSz cx="6858000" cy="9144000"/>
</p:presentation>"#
    )
}

fn presentation_rels(n: usize, has_notes: bool) -> String {
    let mut rels = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="slideMasters/slideMaster1.xml"/>"#,
    );
    for i in 1..=n {
        let r_id = i + 1;
        rels.push_str(&format!(
            "<Relationship Id=\"rId{r_id}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide{i}.xml\"/>"
        ));
    }
    let theme_rid = n + 2;
    rels.push_str(&format!(
        "<Relationship Id=\"rId{theme_rid}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme\" Target=\"theme/theme1.xml\"/>"
    ));
    if has_notes {
        let notes_master_rid = n + 3;
        rels.push_str(&format!(
            "<Relationship Id=\"rId{notes_master_rid}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesMaster\" Target=\"notesMasters/notesMaster1.xml\"/>"
        ));
    }
    rels.push_str("</Relationships>");
    rels
}

fn slide_xml(slide: &PptxSlide) -> String {
    match slide.layout {
        SlideLayout::Section => slide_xml_section(&slide.title),
        SlideLayout::Content => slide_xml_content(slide),
    }
}

/// Section-divider slide: single large centered title, no body. Used to
/// visually break a deck into chapters.
fn slide_xml_section(title: &str) -> String {
    let t = xml_escape(title);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>
<p:sp>
<p:nvSpPr><p:cNvPr id="2" name="Section Title"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="ctrTitle"/></p:nvPr></p:nvSpPr>
<p:spPr><a:xfrm><a:off x="457200" y="2438400"/><a:ext cx="11277600" cy="1981200"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>
<p:txBody><a:bodyPr anchor="ctr"/><a:lstStyle/><a:p><a:pPr algn="ctr"/><a:r><a:rPr lang="en-US" sz="6000" b="1" dirty="0"><a:solidFill><a:srgbClr val="2E74B5"/></a:solidFill></a:rPr><a:t>{t}</a:t></a:r></a:p></p:txBody>
</p:sp>
</p:spTree></p:cSld>
<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>
</p:sld>"#
    )
}

fn slide_xml_content(slide: &PptxSlide) -> String {
    let title = xml_escape(&slide.title);
    let mut body_paras = String::new();
    for bullet in &slide.bullets {
        let b = xml_escape(bullet);
        // Explicit font size (2400 = 24pt) + bullet glyph so QuickLook
        // / minimal renderers don't fall back to a 1-character-wide box.
        body_paras.push_str(&format!(
            "<a:p><a:pPr marL=\"342900\" indent=\"-342900\"><a:buFont typeface=\"Arial\"/><a:buChar char=\"•\"/></a:pPr><a:r><a:rPr lang=\"en-US\" sz=\"2400\" dirty=\"0\"/><a:t>{b}</a:t></a:r></a:p>"
        ));
    }
    if body_paras.is_empty() {
        body_paras.push_str("<a:p><a:endParaRPr lang=\"en-US\"/></a:p>");
    }
    // Embed explicit positions for title + content shapes so renderers
    // that don't fully resolve layout/master inheritance (QuickLook,
    // some older OOXML viewers) still place text correctly. Title is
    // top-center, content is the main body area below.
    //   16:9 slide: cx=12192000 cy=6858000 (EMU)
    //   title:    x=457200  y=365125   cx=11277600 cy=1325563
    //   content:  x=457200  y=1825625  cx=11277600 cy=4351338
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>
<p:sp>
<p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>
<p:spPr><a:xfrm><a:off x="457200" y="365125"/><a:ext cx="11277600" cy="1325563"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>
<p:txBody><a:bodyPr anchor="ctr"/><a:lstStyle/><a:p><a:pPr algn="ctr"/><a:r><a:rPr lang="en-US" sz="4000" b="1" dirty="0"/><a:t>{title}</a:t></a:r></a:p></p:txBody>
</p:sp>
<p:sp>
<p:nvSpPr><p:cNvPr id="3" name="Content"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr>
<p:spPr><a:xfrm><a:off x="457200" y="1825625"/><a:ext cx="11277600" cy="4351338"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr>
<p:txBody><a:bodyPr wrap="square" anchor="t"/><a:lstStyle/>{body_paras}</p:txBody>
</p:sp>
</p:spTree></p:cSld>
<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>
</p:sld>"#
    )
}

// ── Static XML constants ────────────────────────────────────────────────────

const REL_TOP: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/>
<Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties" Target="docProps/app.xml"/>
</Relationships>"#;

const APP_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
<Application>CodeFactory</Application>
<PresentationFormat>Widescreen</PresentationFormat>
</Properties>"#;

const CORE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
<dc:creator>CodeFactory</dc:creator>
<cp:lastModifiedBy>CodeFactory</cp:lastModifiedBy>
</cp:coreProperties>"#;

const THEME_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office Theme">
<a:themeElements>
<a:clrScheme name="Office">
<a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1>
<a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>
<a:dk2><a:srgbClr val="44546A"/></a:dk2>
<a:lt2><a:srgbClr val="E7E6E6"/></a:lt2>
<a:accent1><a:srgbClr val="4472C4"/></a:accent1>
<a:accent2><a:srgbClr val="ED7D31"/></a:accent2>
<a:accent3><a:srgbClr val="A5A5A5"/></a:accent3>
<a:accent4><a:srgbClr val="FFC000"/></a:accent4>
<a:accent5><a:srgbClr val="5B9BD5"/></a:accent5>
<a:accent6><a:srgbClr val="70AD47"/></a:accent6>
<a:hlink><a:srgbClr val="0563C1"/></a:hlink>
<a:folHlink><a:srgbClr val="954F72"/></a:folHlink>
</a:clrScheme>
<a:fontScheme name="Office">
<a:majorFont><a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont>
<a:minorFont><a:latin typeface="Calibri"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont>
</a:fontScheme>
<a:fmtScheme name="Office">
<a:fillStyleLst>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
</a:fillStyleLst>
<a:lnStyleLst>
<a:ln w="6350" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
<a:ln w="12700" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
<a:ln w="19050" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln>
</a:lnStyleLst>
<a:effectStyleLst>
<a:effectStyle><a:effectLst/></a:effectStyle>
<a:effectStyle><a:effectLst/></a:effectStyle>
<a:effectStyle><a:effectLst/></a:effectStyle>
</a:effectStyleLst>
<a:bgFillStyleLst>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
<a:solidFill><a:schemeClr val="phClr"/></a:solidFill>
</a:bgFillStyleLst>
</a:fmtScheme>
</a:themeElements>
</a:theme>"#;

const SLIDE_MASTER_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldMaster xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg>
<p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>
<p:sp>
<p:nvSpPr><p:cNvPr id="2" name="Title Placeholder"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>
<p:spPr><a:xfrm><a:off x="838200" y="365125"/><a:ext cx="10515600" cy="1325563"/></a:xfrm></p:spPr>
<p:txBody><a:bodyPr/><a:lstStyle><a:lvl1pPr algn="ctr"><a:defRPr sz="4400"/></a:lvl1pPr></a:lstStyle><a:p><a:r><a:rPr lang="en-US"/><a:t>Title Placeholder</a:t></a:r></a:p></p:txBody>
</p:sp>
<p:sp>
<p:nvSpPr><p:cNvPr id="3" name="Body Placeholder"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr>
<p:spPr><a:xfrm><a:off x="838200" y="1825625"/><a:ext cx="10515600" cy="4351338"/></a:xfrm></p:spPr>
<p:txBody><a:bodyPr/><a:lstStyle><a:lvl1pPr><a:defRPr sz="2400"/></a:lvl1pPr></a:lstStyle><a:p><a:r><a:rPr lang="en-US"/><a:t>Body Placeholder</a:t></a:r></a:p></p:txBody>
</p:sp>
</p:spTree></p:cSld>
<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>
<p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst>
<p:txStyles>
<p:titleStyle><a:lvl1pPr><a:defRPr sz="4400" b="0"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mj-lt"/></a:defRPr></a:lvl1pPr></p:titleStyle>
<p:bodyStyle>
<a:lvl1pPr marL="342900" indent="-342900"><a:buFont typeface="Arial" panose="020B0604020202020204" pitchFamily="34" charset="0"/><a:buChar char="•"/><a:defRPr sz="2400"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl1pPr>
</p:bodyStyle>
<p:otherStyle/>
</p:txStyles>
</p:sldMaster>"#;

const SLIDE_MASTER_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/>
</Relationships>"#;

const SLIDE_LAYOUT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" type="obj" preserve="1">
<p:cSld name="Title and Content"><p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>
<p:sp>
<p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>
<p:spPr/>
<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang="en-US"/></a:p></p:txBody>
</p:sp>
<p:sp>
<p:nvSpPr><p:cNvPr id="3" name="Content Placeholder"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr>
<p:spPr/>
<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang="en-US"/></a:p></p:txBody>
</p:sp>
</p:spTree></p:cSld>
<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>
</p:sldLayout>"#;

const SLIDE_LAYOUT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster" Target="../slideMasters/slideMaster1.xml"/>
</Relationships>"#;

fn slide_rels(needs_notes: bool, n: usize) -> String {
    let mut s = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout" Target="../slideLayouts/slideLayout1.xml"/>"#,
    );
    if needs_notes {
        s.push_str(&format!(
            "<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide\" Target=\"../notesSlides/notesSlide{n}.xml\"/>"
        ));
    }
    s.push_str("</Relationships>");
    s
}

/// Notes slide XML — single body shape holding the speaker note text.
/// Office shows this in presenter view + notes page printout but never
/// in slide show.
fn notes_slide_xml(notes: &str) -> String {
    let n = xml_escape(notes);
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:notes xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>
<p:sp>
<p:nvSpPr><p:cNvPr id="2" name="Notes"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr>
<p:spPr/>
<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" dirty="0"/><a:t>{n}</a:t></a:r></a:p></p:txBody>
</p:sp>
</p:spTree></p:cSld>
<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>
</p:notes>"#
    )
}

fn notes_slide_rels(n: usize) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="../slides/slide{n}.xml"/>
<Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesMaster" Target="../notesMasters/notesMaster1.xml"/>
</Relationships>"#
    )
}

const NOTES_MASTER_XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:notesMaster xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:bg><p:bgRef idx="1001"><a:schemeClr val="bg1"/></p:bgRef></p:bg>
<p:spTree>
<p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr>
<p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>
</p:spTree></p:cSld>
<p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>
<p:notesStyle>
<a:lvl1pPr><a:defRPr sz="1200"><a:solidFill><a:schemeClr val="tx1"/></a:solidFill><a:latin typeface="+mn-lt"/></a:defRPr></a:lvl1pPr>
</p:notesStyle>
</p:notesMaster>"#;

const NOTES_MASTER_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme" Target="../theme/theme1.xml"/>
</Relationships>"#;

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use zip::ZipArchive;

    #[test]
    fn empty_slides_rejected_at_args_layer_below() {
        // The runtime check rejects empty; here we just confirm the
        // builder happily makes a 1-slide deck.
        let bytes = build_pptx_bytes(&[PptxSlide {
            title: "Hello".into(),
            bullets: vec!["world".into()],
            ..Default::default()
        }])
        .unwrap();
        assert!(bytes.len() > 1000, "expected a non-trivial pptx, got {}", bytes.len());
    }

    #[test]
    fn generated_pptx_is_valid_zip_with_expected_parts() {
        let bytes = build_pptx_bytes(&[
            PptxSlide { title: "First".into(),  bullets: vec!["a".into(), "b".into()], ..Default::default() },
            PptxSlide { title: "Second".into(), bullets: vec![], ..Default::default() },
        ])
        .unwrap();

        let cursor = std::io::Cursor::new(&bytes);
        let mut zip = ZipArchive::new(cursor).expect("output must be a valid zip");

        // Required parts must all be present.
        let names: Vec<String> = zip.file_names().map(|s| s.to_string()).collect();
        for required in [
            "[Content_Types].xml",
            "_rels/.rels",
            "ppt/presentation.xml",
            "ppt/_rels/presentation.xml.rels",
            "ppt/theme/theme1.xml",
            "ppt/slideMasters/slideMaster1.xml",
            "ppt/slideMasters/_rels/slideMaster1.xml.rels",
            "ppt/slideLayouts/slideLayout1.xml",
            "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
            "ppt/slides/slide1.xml",
            "ppt/slides/slide2.xml",
            "ppt/slides/_rels/slide1.xml.rels",
            "ppt/slides/_rels/slide2.xml.rels",
        ] {
            assert!(names.iter().any(|n| n == required),
                "expected {required} in archive, got {:?}", names);
        }

        // Slide 1 must contain the user's text. This is the verification
        // that protects against silently producing an "empty pptx".
        let mut slide1 = String::new();
        zip.by_name("ppt/slides/slide1.xml").unwrap().read_to_string(&mut slide1).unwrap();
        assert!(slide1.contains("First"), "slide 1 missing title text: {slide1}");
        assert!(slide1.contains("<a:t>a</a:t>"), "slide 1 missing bullet 'a'");
        assert!(slide1.contains("<a:t>b</a:t>"), "slide 1 missing bullet 'b'");

        // Title-only slide (no bullets) still well-formed.
        let mut slide2 = String::new();
        zip.by_name("ppt/slides/slide2.xml").unwrap().read_to_string(&mut slide2).unwrap();
        assert!(slide2.contains("Second"));
    }

    #[test]
    fn special_xml_chars_in_text_are_escaped() {
        let bytes = build_pptx_bytes(&[PptxSlide {
            title: "A < B & C > D".into(),
            bullets: vec!["\"quoted\" & 'apos'".into()],
            ..Default::default()
        }])
        .unwrap();
        let cursor = std::io::Cursor::new(&bytes);
        let mut zip = ZipArchive::new(cursor).unwrap();
        let mut s = String::new();
        zip.by_name("ppt/slides/slide1.xml").unwrap().read_to_string(&mut s).unwrap();
        // Raw < / & / > / " / ' MUST NOT appear in the text payload.
        assert!(s.contains("A &lt; B &amp; C &gt; D"));
        assert!(s.contains("&quot;quoted&quot; &amp; &apos;apos&apos;"));
    }

    #[test]
    fn section_layout_emits_centered_title_no_body() {
        let bytes = build_pptx_bytes(&[PptxSlide {
            title: "Chapter 1".into(),
            layout: SlideLayout::Section,
            ..Default::default()
        }])
        .unwrap();
        let cursor = std::io::Cursor::new(&bytes);
        let mut zip = ZipArchive::new(cursor).unwrap();
        let mut s = String::new();
        zip.by_name("ppt/slides/slide1.xml").unwrap().read_to_string(&mut s).unwrap();
        // Section uses ctrTitle placeholder + algn="ctr".
        assert!(s.contains("ctrTitle"),  "expected center-title placeholder, got: {s}");
        assert!(s.contains("Chapter 1"), "title text missing");
        // No content placeholder on a section slide.
        assert!(!s.contains("name=\"Content\""), "section slide must not carry content shape");
    }

    #[test]
    fn slides_with_notes_emit_notes_parts_and_relationships() {
        let bytes = build_pptx_bytes(&[
            PptxSlide {
                title: "With notes".into(),
                bullets: vec!["a".into()],
                notes: "remember to talk slower here".into(),
                ..Default::default()
            },
            PptxSlide { title: "No notes".into(), ..Default::default() },
        ])
        .unwrap();
        let cursor = std::io::Cursor::new(&bytes);
        let mut zip = ZipArchive::new(cursor).unwrap();

        // Slide 1 gets notesSlide1 + notesMaster.
        assert!(zip.by_name("ppt/notesSlides/notesSlide1.xml").is_ok());
        assert!(zip.by_name("ppt/notesMasters/notesMaster1.xml").is_ok());

        // Slide 2 (no notes) MUST NOT spawn a stray notesSlide2.
        assert!(zip.by_name("ppt/notesSlides/notesSlide2.xml").is_err());

        // Notes content carried through.
        let mut n = String::new();
        zip.by_name("ppt/notesSlides/notesSlide1.xml").unwrap().read_to_string(&mut n).unwrap();
        assert!(n.contains("remember to talk slower"), "notes text missing");

        // Slide 1 rels reference the notesSlide; slide 2 rels do not.
        let mut r1 = String::new();
        zip.by_name("ppt/slides/_rels/slide1.xml.rels").unwrap().read_to_string(&mut r1).unwrap();
        assert!(r1.contains("notesSlide"), "slide 1 should link to its notes");
        let mut r2 = String::new();
        zip.by_name("ppt/slides/_rels/slide2.xml.rels").unwrap().read_to_string(&mut r2).unwrap();
        assert!(!r2.contains("notesSlide"), "slide 2 should NOT link to a notes file it doesn't have");
    }

    /// Smoke: write a real .pptx to /tmp so a developer can open it
    /// in PowerPoint / Keynote / Google Slides to eyeball formatting.
    /// Ignored by default — run with `cargo test smoke_write -- --ignored`.
    #[test]
    #[ignore]
    fn smoke_write_to_tmp() {
        let bytes = build_pptx_bytes(&[
            // Section divider — chapter cover (no body, big centered title).
            PptxSlide {
                title: "CodeFactory write_pptx v2".into(),
                layout: SlideLayout::Section,
                ..Default::default()
            },
            PptxSlide {
                title: "CodeFactory v1.2 — write_pptx 工具".into(),
                bullets: vec![
                    "纯 Rust 生成 PowerPoint".into(),
                    "无 Python / 无 Node 依赖".into(),
                    "Office / Keynote / Google Slides 均可打开".into(),
                ],
                notes: "Talking points: 强调本地化、跨平台、无 runtime 依赖。".into(),
                ..Default::default()
            },
            PptxSlide {
                title: "用法".into(),
                bullets: vec![
                    "AI 用 kb_search 读取源文档".into(),
                    "AI 综合大纲".into(),
                    "调用 write_pptx 输出文件".into(),
                ],
                ..Default::default()
            },
        ])
        .unwrap();
        let out = "/tmp/codefactory-smoke.pptx";
        std::fs::write(out, &bytes).unwrap();
        eprintln!("wrote {} bytes to {out}", bytes.len());
    }

    #[test]
    fn many_slides_generate_unique_slide_ids() {
        let slides: Vec<PptxSlide> = (0..10)
            .map(|i| PptxSlide { title: format!("Slide {i}"), bullets: vec![], ..Default::default() })
            .collect();
        let bytes = build_pptx_bytes(&slides).unwrap();
        let cursor = std::io::Cursor::new(&bytes);
        let mut zip = ZipArchive::new(cursor).unwrap();
        let mut pres = String::new();
        zip.by_name("ppt/presentation.xml").unwrap().read_to_string(&mut pres).unwrap();
        // Slide IDs must be unique (256..265) — duplicates make Office
        // silently drop slides on open.
        for sid in 256..266 {
            assert!(pres.contains(&format!("id=\"{sid}\"")),
                "missing slide id {sid} in presentation.xml");
        }
    }
}

// Silence unused-path warning for cwd-based access if any builds skip it.
#[allow(dead_code)]
fn _ensure_path_used(_: &PathBuf) {}
