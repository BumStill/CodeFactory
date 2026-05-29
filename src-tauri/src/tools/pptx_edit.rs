// SPDX-License-Identifier: Apache-2.0
//! `read_pptx` + `edit_pptx` — structured, **style-preserving** editing of an
//! existing PowerPoint deck.
//!
//! Why this exists: `write_pptx` builds a fresh deck from scratch using bare
//! Office defaults. When a user uploads their own .pptx and asks us to
//! "enrich it into a new version", regenerating from scratch would throw away
//! their theme, colors, images, fonts and layout. These two tools instead
//! edit the original package *in place*:
//!
//!   1. `read_pptx` returns a structured outline. Every text paragraph is
//!      addressed by an `sN.F.P` locator (slide N, text-frame F, paragraph P).
//!   2. `edit_pptx` takes those locators + new text and rewrites only the
//!      `<a:t>` runs, keeping each paragraph's `<a:pPr>` / first-run `<a:rPr>`.
//!      All other zip parts (theme, media, slideMasters, layouts) are copied
//!      over byte-for-byte via `raw_copy_file`, so the deck's design survives.
//!
//! ## Parsing approach (no XML DOM dependency)
//!
//! The relevant OOXML elements — `p:txBody`, `a:p`, `a:r`, `a:t`, `a:pPr`,
//! `a:rPr` — never nest inside another element of the *same* name. That makes
//! a flat "find the next `<tag …>` then its matching `</tag>`" scan correct,
//! and lets us avoid pulling in a full XML parser just for this feature. We
//! only ever read text and splice strings at element boundaries; the bytes we
//! don't touch are preserved exactly.

use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;

use super::{workspace_path, ExecCtx, ToolOutput};
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};

// ─────────────────────────────────────────────────────────────────────────────
// read_pptx
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ReadArgs {
    path: String,
    /// When true, prefix each paragraph with its current formatting
    /// (`[font=… sz=… b color=… algn=…]`) so inconsistencies are visible before
    /// calling format_pptx.
    #[serde(default)]
    with_format: bool,
}

pub fn read_definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "read_pptx".into(),
            description: "Read an existing .pptx as a structured, editable outline. Every text \
paragraph is addressed by a locator `sN.F.P` = slide N, text-frame F, paragraph P (all the \
indices edit_pptx needs). Pair with edit_pptx to rewrite or expand the text while preserving \
the deck's theme, colors, images, fonts and layout. Use this — NOT write_pptx — when the user \
uploaded a deck and wants an enriched new version of it."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative or absolute path to an existing .pptx." },
                    "with_format": { "type": "boolean", "description": "Also report each paragraph's current font/size/color/bold/alignment — use before format_pptx to spot inconsistencies." }
                },
                "required": ["path"]
            }),
        },
    }
}

pub async fn execute_read(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let Ok(a) = serde_json::from_value::<ReadArgs>(args) else {
        return Ok(ToolOutput::err("Invalid arguments for read_pptx"));
    };
    let path = match workspace_path::resolve_existing(&ctx.cwd, &a.path) {
        Ok(p) => p,
        Err(e) => return Ok(ToolOutput::err(e.message())),
    };
    if !path.to_string_lossy().to_lowercase().ends_with(".pptx") {
        return Ok(ToolOutput::err("read_pptx path must end with .pptx"));
    }

    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) => return Ok(ToolOutput::err(format!("Cannot open {}: {e}", path.display()))),
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => return Ok(ToolOutput::err(format!("Invalid .pptx (not a zip): {e}"))),
    };

    let mut slide_nos: Vec<usize> = archive.file_names().filter_map(slide_no_from_name).collect();
    slide_nos.sort_unstable();
    if slide_nos.is_empty() {
        return Ok(ToolOutput::err("No slides found in deck (ppt/slides/slideN.xml missing)"));
    }

    let mut out = String::new();
    out.push_str(&format!(
        "Deck: {} ({} slides)\nLocator legend: sN.F.P = slide N, text-frame F, paragraph P (use these as edit_pptx slide/frame/para).\n",
        rel_for_display(&path, ctx),
        slide_nos.len()
    ));
    for n in slide_nos {
        let name = format!("ppt/slides/slide{n}.xml");
        let mut xml = String::new();
        if archive
            .by_name(&name)
            .and_then(|mut f| f.read_to_string(&mut xml).map_err(zip::result::ZipError::from))
            .is_err()
        {
            continue;
        }
        outline_for_slide(&xml, n, a.with_format, &mut out);
    }
    Ok(ToolOutput::ok(out))
}

fn outline_for_slide(xml: &str, slide_no: usize, with_format: bool, out: &mut String) {
    let frames = find_simple_elements(xml, "p:txBody", (0, xml.len()));
    out.push_str(&format!("\n# Slide {slide_no}\n"));
    if frames.is_empty() {
        out.push_str("(no text frames)\n");
        return;
    }
    for (fi, frame) in frames.iter().enumerate() {
        let hint = placeholder_hint(xml, frame.outer.0);
        match hint {
            Some(h) => out.push_str(&format!("frame {fi} ({h}):\n")),
            None => out.push_str(&format!("frame {fi}:\n")),
        }
        if let Some(inner) = frame.inner {
            let paras = find_simple_elements(xml, "a:p", inner);
            for (pi, para) in paras.iter().enumerate() {
                let text = paragraph_text(xml, para);
                let shown = if text.is_empty() { "(empty)" } else { &text };
                let fmt = if with_format { format_summary(xml, para) } else { String::new() };
                out.push_str(&format!("  s{slide_no}.{fi}.{pi}: {fmt}{shown}\n"));
            }
        }
    }
}

/// A compact `[font=… sz=… b color=… algn=…]` summary of a paragraph's current
/// formatting (paragraph `algn` + the first run's `rPr`). Empty when nothing of
/// note is set. Drives the `with_format` audit before format_pptx.
fn format_summary(xml: &str, para: &ElemSpan) -> String {
    let Some(inner) = para.inner else { return String::new() };
    let mut parts: Vec<String> = Vec::new();

    if let Some(p) = find_simple_elements(xml, "a:pPr", inner).into_iter().next() {
        let open_end = p.inner.map(|i| i.0).unwrap_or(p.outer.1);
        let open = &xml[p.outer.0..open_end];
        if let Some(a) = attr_value(open, "algn") {
            parts.push(format!("algn={a}"));
        }
    }

    if let Some(r) = find_simple_elements(xml, "a:r", inner).into_iter().next() {
        if let Some(rin) = r.inner {
            if let Some(rp) = find_simple_elements(xml, "a:rPr", rin).into_iter().next() {
                let open_end = rp.inner.map(|i| i.0).unwrap_or(rp.outer.1);
                let open = &xml[rp.outer.0..open_end];
                if let Some(sz) = attr_value(open, "sz") {
                    match sz.parse::<i64>() {
                        Ok(v) => parts.push(format!("sz={}", v / 100)),
                        Err(_) => parts.push(format!("sz={sz}")),
                    }
                }
                if attr_value(open, "b").as_deref() == Some("1") {
                    parts.push("b".into());
                }
                if attr_value(open, "i").as_deref() == Some("1") {
                    parts.push("i".into());
                }
                if let Some(rpi) = rp.inner {
                    if let Some(l) = find_simple_elements(xml, "a:latin", rpi).into_iter().next() {
                        if let Some(tf) = attr_value(&xml[l.outer.0..l.outer.1], "typeface") {
                            parts.push(format!("font={tf}"));
                        }
                    }
                    if let Some(sf) = find_simple_elements(xml, "a:solidFill", rpi).into_iter().next() {
                        if let Some(si) = sf.inner {
                            if let Some(c) = find_simple_elements(xml, "a:srgbClr", si).into_iter().next() {
                                if let Some(v) = attr_value(&xml[c.outer.0..c.outer.1], "val") {
                                    parts.push(format!("color={v}"));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!("[{}] ", parts.join(" "))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// edit_pptx
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize, Debug, Default, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum EditOp {
    /// Rewrite the target paragraph's text, keeping its formatting.
    #[default]
    Replace,
    /// Insert one or more new paragraphs *after* the target, cloning the
    /// target's bullet + run formatting so they blend in.
    InsertAfter,
}

#[derive(Deserialize, Debug)]
struct Edit {
    #[serde(default)]
    op: EditOp,
    /// 1-based slide number (the N in sN.F.P).
    slide: usize,
    /// 0-based text-frame index (the F).
    frame: usize,
    /// 0-based paragraph index (the P).
    para: usize,
    /// New text for a `replace`, or the single new paragraph for an
    /// `insert_after` when `texts` is absent.
    #[serde(default)]
    text: Option<String>,
    /// New paragraphs for an `insert_after` (one entry per new paragraph).
    #[serde(default)]
    texts: Option<Vec<String>>,
}

impl Edit {
    fn insert_texts(&self) -> Vec<String> {
        if let Some(v) = &self.texts {
            return v.iter().filter(|s| !s.trim().is_empty()).cloned().collect();
        }
        match &self.text {
            Some(t) if !t.trim().is_empty() => vec![t.clone()],
            _ => Vec::new(),
        }
    }
}

#[derive(Deserialize)]
struct EditArgs {
    path: String,
    #[serde(default)]
    out_path: Option<String>,
    edits: Vec<Edit>,
}

pub fn edit_definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "edit_pptx".into(),
            description: "Edit text in an existing .pptx IN PLACE, preserving the deck's theme, \
colors, images, fonts and layout. Address paragraphs with the sN.F.P locators from read_pptx. \
op 'replace' rewrites a paragraph's text (keeps its formatting); op 'insert_after' adds new \
paragraphs after one, cloning its bullet/run style. Writes to out_path (defaults to overwriting \
path). This is the way to produce an enriched new version of an uploaded deck — call read_pptx \
first to get locators, kb_search to pull supporting content, then edit_pptx."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Existing source .pptx (workspace-relative or absolute)." },
                    "out_path": { "type": "string", "description": "Where to write the result (.pptx). Omit to overwrite the source." },
                    "edits": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "op": { "type": "string", "enum": ["replace", "insert_after"], "description": "'replace' (default) rewrites the paragraph; 'insert_after' adds new paragraphs after it." },
                                "slide": { "type": "integer", "description": "1-based slide number (N in sN.F.P)." },
                                "frame": { "type": "integer", "description": "0-based text-frame index (F)." },
                                "para":  { "type": "integer", "description": "0-based paragraph index (P)." },
                                "text":  { "type": "string", "description": "New text for replace, or the single paragraph for insert_after." },
                                "texts": { "type": "array", "items": { "type": "string" }, "description": "For insert_after: one entry per new paragraph." }
                            },
                            "required": ["slide", "frame", "para"]
                        }
                    }
                },
                "required": ["path", "edits"]
            }),
        },
    }
}

pub async fn execute_edit(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let a: EditArgs = match serde_json::from_value(args.clone()) {
        Ok(v) => v,
        Err(e) => {
            return Ok(ToolOutput::err(format!(
                "Invalid arguments for edit_pptx: {e}. Received: {}",
                serde_json::to_string(&args).unwrap_or_default()
            )));
        }
    };
    if a.edits.is_empty() {
        return Ok(ToolOutput::err("edit_pptx requires at least one edit"));
    }

    let src = match workspace_path::resolve_existing(&ctx.cwd, &a.path) {
        Ok(p) => p,
        Err(e) => return Ok(ToolOutput::err(e.message())),
    };
    if !src.to_string_lossy().to_lowercase().ends_with(".pptx") {
        return Ok(ToolOutput::err("edit_pptx path must end with .pptx"));
    }

    let out_request = a.out_path.clone().unwrap_or_else(|| a.path.clone());
    let out_path = match workspace_path::resolve_writable(&ctx.cwd, &out_request) {
        Ok(p) => p,
        Err(e) => return Ok(ToolOutput::err(e.message())),
    };
    if !out_path.to_string_lossy().to_lowercase().ends_with(".pptx") {
        return Ok(ToolOutput::err("edit_pptx out_path must end with .pptx"));
    }

    // Group edits by slide so we parse each targeted slide once.
    let mut by_slide: HashMap<usize, Vec<&Edit>> = HashMap::new();
    for e in &a.edits {
        by_slide.entry(e.slide).or_default().push(e);
    }

    // Read just the targeted slides' XML.
    let file = match std::fs::File::open(&src) {
        Ok(f) => f,
        Err(e) => return Ok(ToolOutput::err(format!("Cannot open {}: {e}", src.display()))),
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => return Ok(ToolOutput::err(format!("Invalid .pptx (not a zip): {e}"))),
    };

    let mut modified: HashMap<String, Vec<u8>> = HashMap::new();
    let mut applied = 0usize;
    for (slide_no, edits) in &by_slide {
        let name = format!("ppt/slides/slide{slide_no}.xml");
        let mut xml = String::new();
        if archive
            .by_name(&name)
            .and_then(|mut f| f.read_to_string(&mut xml).map_err(zip::result::ZipError::from))
            .is_err()
        {
            return Ok(ToolOutput::err(format!(
                "slide {slide_no} not found in deck ({name})"
            )));
        }
        match apply_edits_to_slide(&xml, edits) {
            Ok(new_xml) => {
                modified.insert(name, new_xml.into_bytes());
                applied += edits.len();
            }
            Err(msg) => return Ok(ToolOutput::err(msg)),
        }
    }
    drop(archive);

    let bytes = match repackage(&src, &modified) {
        Ok(b) => b,
        Err(e) => return Ok(ToolOutput::err(format!("repackage failed: {e}"))),
    };

    if let Some(parent) = out_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Ok(ToolOutput::err(format!("mkdir failed: {e}")));
        }
    }
    if let Err(e) = std::fs::write(&out_path, &bytes) {
        return Ok(ToolOutput::err(format!("write failed: {e}")));
    }

    Ok(ToolOutput::ok(format!(
        "Applied {applied} edit(s) across {} slide(s) → {} ({} bytes). Theme, images and layout preserved.",
        by_slide.len(),
        rel_for_display(&out_path, ctx),
        bytes.len()
    )))
}

/// Apply every edit targeting one slide. Edits are resolved against the
/// ORIGINAL parse, then spliced right-to-left so byte offsets stay valid.
fn apply_edits_to_slide(xml: &str, edits: &[&Edit]) -> std::result::Result<String, String> {
    let frames = find_simple_elements(xml, "p:txBody", (0, xml.len()));
    let mut splices: Vec<(usize, usize, String)> = Vec::new();

    for e in edits {
        let frame = frames.get(e.frame).ok_or_else(|| {
            format!(
                "slide {} has no frame index {} (found {} frame(s)) — re-check read_pptx",
                e.slide,
                e.frame,
                frames.len()
            )
        })?;
        let frame_inner = frame
            .inner
            .ok_or_else(|| format!("slide {} frame {} has no body", e.slide, e.frame))?;
        let paras = find_simple_elements(xml, "a:p", frame_inner);
        let para = paras.get(e.para).ok_or_else(|| {
            format!(
                "slide {} frame {} has no paragraph index {} (found {})",
                e.slide,
                e.frame,
                e.para,
                paras.len()
            )
        })?;

        match e.op {
            EditOp::Replace => {
                let text = e.text.as_deref().ok_or_else(|| {
                    format!("replace edit for s{}.{}.{} requires `text`", e.slide, e.frame, e.para)
                })?;
                let rebuilt = rebuild_paragraph(xml, para, text);
                splices.push((para.outer.0, para.outer.1, rebuilt));
            }
            EditOp::InsertAfter => {
                let texts = e.insert_texts();
                if texts.is_empty() {
                    return Err(format!(
                        "insert_after edit for s{}.{}.{} requires non-empty `texts` (or `text`)",
                        e.slide, e.frame, e.para
                    ));
                }
                let mut ins = String::new();
                for t in &texts {
                    ins.push_str(&rebuild_paragraph(xml, para, t));
                }
                // Zero-width splice at the end of the target paragraph.
                splices.push((para.outer.1, para.outer.1, ins));
            }
        }
    }

    // Right-to-left, and reject overlapping targets (same paragraph twice).
    splices.sort_by_key(|s| std::cmp::Reverse(s.0));
    for w in splices.windows(2) {
        // w[0] has the larger start (applied first). It must not overlap w[1].
        if w[1].1 > w[0].0 {
            return Err("overlapping edits target the same paragraph; issue one edit per paragraph".into());
        }
    }

    let mut out = xml.to_string();
    for (s, e, rep) in splices {
        out.replace_range(s..e, &rep);
    }
    Ok(out)
}

/// Rebuild a single `<a:p>…</a:p>` with `new_text`, preserving the original
/// paragraph's open tag, its `<a:pPr>`, the first run's `<a:rPr>`, and any
/// `<a:endParaRPr>`. Subsequent runs are dropped (their text is folded into
/// the single rebuilt run). Returns a full `<a:p>…</a:p>` string.
fn rebuild_paragraph(xml: &str, para: &ElemSpan, new_text: &str) -> String {
    let Some((inner_s, inner_e)) = para.inner else {
        // Self-closing/empty paragraph: synthesize a minimal run.
        return format!(
            "<a:p><a:r><a:rPr lang=\"en-US\" dirty=\"0\"/><a:t>{}</a:t></a:r></a:p>",
            xml_escape(new_text)
        );
    };
    let open_tag = &xml[para.outer.0..inner_s]; // includes the trailing '>'

    let ppr = find_simple_elements(xml, "a:pPr", (inner_s, inner_e))
        .into_iter()
        .next()
        .map(|e| &xml[e.outer.0..e.outer.1])
        .unwrap_or("");

    let rpr = find_simple_elements(xml, "a:r", (inner_s, inner_e))
        .into_iter()
        .next()
        .and_then(|r| r.inner)
        .and_then(|(rs, re)| find_simple_elements(xml, "a:rPr", (rs, re)).into_iter().next())
        .map(|e| xml[e.outer.0..e.outer.1].to_string())
        .unwrap_or_default();

    let end_pr = find_simple_elements(xml, "a:endParaRPr", (inner_s, inner_e))
        .into_iter()
        .next()
        .map(|e| xml[e.outer.0..e.outer.1].to_string())
        .unwrap_or_default();

    format!(
        "{open_tag}{ppr}<a:r>{rpr}<a:t>{}</a:t></a:r>{end_pr}</a:p>",
        xml_escape(new_text)
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Zip repackaging — modified parts replaced, everything else copied verbatim.
// ─────────────────────────────────────────────────────────────────────────────

fn repackage(
    src: &Path,
    modified: &HashMap<String, Vec<u8>>,
) -> std::result::Result<Vec<u8>, String> {
    let file = std::fs::File::open(src).map_err(|e| format!("open {}: {e}", src.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("read zip: {e}"))?;
    let buf = std::io::Cursor::new(Vec::<u8>::new());
    let mut zip = zip::ZipWriter::new(buf);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    for i in 0..archive.len() {
        let name = archive
            .by_index(i)
            .map_err(|e| format!("index {i}: {e}"))?
            .name()
            .to_string();
        if let Some(bytes) = modified.get(&name) {
            zip.start_file::<_, ()>(&name, opts)
                .map_err(|e| format!("start_file {name}: {e}"))?;
            zip.write_all(bytes)
                .map_err(|e| format!("write {name}: {e}"))?;
        } else {
            let raw = archive
                .by_index_raw(i)
                .map_err(|e| format!("raw index {i}: {e}"))?;
            zip.raw_copy_file(raw)
                .map_err(|e| format!("copy {name}: {e}"))?;
        }
    }
    let cursor = zip.finish().map_err(|e| format!("finish: {e}"))?;
    Ok(cursor.into_inner())
}

// ─────────────────────────────────────────────────────────────────────────────
// format_pptx — deep typographic beautification (font / size / color / weight /
// alignment / spacing), applied in place so theme, images and layout survive.
//
// Scope: we only rewrite *text* properties (`<a:rPr>` on every run + the
// paragraph's `<a:pPr>`). We deliberately do NOT move or resize shapes — with no
// render engine we can't see overflow, and nudging geometry blind is how decks
// get broken. So "deep beautification" here means consistent typography and
// rhythm, not re-layout.
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize, Debug, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Scope {
    /// Every paragraph in every text frame.
    #[default]
    All,
    /// Title / subtitle placeholders only.
    Title,
    /// Everything that isn't a title (body placeholders + plain text boxes).
    Body,
}

/// One formatting rule. Any field left unset is not touched, so rules compose:
/// a deck-wide `scope: "all"` font rule plus a `scope: "title"` size rule both
/// apply, with later rules winning on overlapping fields.
#[derive(Deserialize, Debug, Default, Clone)]
struct Rule {
    #[serde(default)]
    scope: Scope,
    /// Restrict this rule to one 1-based slide.
    #[serde(default)]
    slide: Option<usize>,
    /// Restrict this rule to one 0-based text-frame (requires `slide`).
    #[serde(default)]
    frame: Option<usize>,
    font: Option<String>,
    /// Font size in points.
    size: Option<f64>,
    bold: Option<bool>,
    italic: Option<bool>,
    /// Hex color, `RRGGBB` or `#RRGGBB`.
    color: Option<String>,
    /// `left` | `center` | `right` | `justify`.
    align: Option<String>,
    /// Line spacing as a percentage (100 = single, 150 = 1.5×).
    line_spacing: Option<f64>,
    /// Space before the paragraph, in points.
    space_before: Option<f64>,
    /// Space after the paragraph, in points.
    space_after: Option<f64>,
}

impl Rule {
    fn matches(&self, slide: usize, frame: usize, is_title: bool) -> bool {
        if self.slide.is_some_and(|s| s != slide) {
            return false;
        }
        if self.frame.is_some_and(|f| f != frame) {
            return false;
        }
        match self.scope {
            Scope::All => true,
            Scope::Title => is_title,
            Scope::Body => !is_title,
        }
    }
}

/// The merged, validated formatting to apply to one paragraph.
#[derive(Default, Clone)]
struct Fmt {
    font: Option<String>,
    size: Option<f64>,
    bold: Option<bool>,
    italic: Option<bool>,
    color: Option<String>,
    align: Option<String>,
    line_spacing: Option<f64>,
    space_before: Option<f64>,
    space_after: Option<f64>,
}

impl Fmt {
    fn merge(&mut self, r: &Rule) {
        if r.font.is_some() {
            self.font = r.font.clone();
        }
        if r.size.is_some() {
            self.size = r.size;
        }
        if r.bold.is_some() {
            self.bold = r.bold;
        }
        if r.italic.is_some() {
            self.italic = r.italic;
        }
        if r.color.is_some() {
            self.color = r.color.clone();
        }
        if r.align.is_some() {
            self.align = r.align.clone();
        }
        if r.line_spacing.is_some() {
            self.line_spacing = r.line_spacing;
        }
        if r.space_before.is_some() {
            self.space_before = r.space_before;
        }
        if r.space_after.is_some() {
            self.space_after = r.space_after;
        }
    }
    /// Run-level properties (live on `<a:rPr>`).
    fn has_run(&self) -> bool {
        self.font.is_some()
            || self.size.is_some()
            || self.bold.is_some()
            || self.italic.is_some()
            || self.color.is_some()
    }
    /// Paragraph-level properties (live on `<a:pPr>`).
    fn has_para(&self) -> bool {
        self.align.is_some()
            || self.line_spacing.is_some()
            || self.space_before.is_some()
            || self.space_after.is_some()
    }
}

#[derive(Deserialize)]
struct FormatArgs {
    path: String,
    #[serde(default)]
    out_path: Option<String>,
    rules: Vec<Rule>,
}

pub fn format_definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "format_pptx".into(),
            description: "Beautify/normalize the typography of an existing .pptx IN PLACE — unify \
fonts, sizes, colors, weight, alignment and paragraph spacing across the deck while keeping its \
theme, images and layout. Apply a list of `rules`; each targets paragraphs by `scope` ('all' | \
'title' | 'body', optionally narrowed to one `slide`/`frame`) and sets any of: font, size (pt), \
bold, italic, color (hex), align, line_spacing (%), space_before/space_after (pt). Unset fields \
are left untouched, so rules compose (e.g. one 'all' font rule + one 'title' size rule). Tip: call \
read_pptx with with_format:true first to see current inconsistencies. NOTE: this only rewrites \
text formatting — it never moves or resizes shapes, so it can't fix layout overflow."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Existing source .pptx (workspace-relative or absolute)." },
                    "out_path": { "type": "string", "description": "Where to write the result (.pptx). Omit to overwrite the source." },
                    "rules": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "scope": { "type": "string", "enum": ["all", "title", "body"], "description": "Which paragraphs this rule hits. Default 'all'." },
                                "slide": { "type": "integer", "description": "Optional: restrict to one 1-based slide." },
                                "frame": { "type": "integer", "description": "Optional: restrict to one 0-based text-frame (needs slide)." },
                                "font":  { "type": "string", "description": "Typeface, e.g. 'Calibri' or 'Microsoft YaHei' (set on latin/ea/cs)." },
                                "size":  { "type": "number", "description": "Font size in points." },
                                "bold":  { "type": "boolean" },
                                "italic": { "type": "boolean" },
                                "color": { "type": "string", "description": "Hex color RRGGBB or #RRGGBB." },
                                "align": { "type": "string", "enum": ["left", "center", "right", "justify"], "description": "Paragraph alignment." },
                                "line_spacing": { "type": "number", "description": "Line spacing percentage (100 = single, 150 = 1.5x)." },
                                "space_before": { "type": "number", "description": "Space before paragraph, in points." },
                                "space_after": { "type": "number", "description": "Space after paragraph, in points." }
                            }
                        }
                    }
                },
                "required": ["path", "rules"]
            }),
        },
    }
}

pub async fn execute_format(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    let mut a: FormatArgs = match serde_json::from_value(args.clone()) {
        Ok(v) => v,
        Err(e) => {
            return Ok(ToolOutput::err(format!(
                "Invalid arguments for format_pptx: {e}. Received: {}",
                serde_json::to_string(&args).unwrap_or_default()
            )));
        }
    };
    if a.rules.is_empty() {
        return Ok(ToolOutput::err("format_pptx requires at least one rule"));
    }
    if let Err(msg) = normalize_rules(&mut a.rules) {
        return Ok(ToolOutput::err(msg));
    }

    let src = match workspace_path::resolve_existing(&ctx.cwd, &a.path) {
        Ok(p) => p,
        Err(e) => return Ok(ToolOutput::err(e.message())),
    };
    if !src.to_string_lossy().to_lowercase().ends_with(".pptx") {
        return Ok(ToolOutput::err("format_pptx path must end with .pptx"));
    }
    let out_request = a.out_path.clone().unwrap_or_else(|| a.path.clone());
    let out_path = match workspace_path::resolve_writable(&ctx.cwd, &out_request) {
        Ok(p) => p,
        Err(e) => return Ok(ToolOutput::err(e.message())),
    };
    if !out_path.to_string_lossy().to_lowercase().ends_with(".pptx") {
        return Ok(ToolOutput::err("format_pptx out_path must end with .pptx"));
    }

    let file = match std::fs::File::open(&src) {
        Ok(f) => f,
        Err(e) => return Ok(ToolOutput::err(format!("Cannot open {}: {e}", src.display()))),
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => return Ok(ToolOutput::err(format!("Invalid .pptx (not a zip): {e}"))),
    };

    let mut slide_nos: Vec<usize> = archive.file_names().filter_map(slide_no_from_name).collect();
    slide_nos.sort_unstable();
    if slide_nos.is_empty() {
        return Ok(ToolOutput::err("No slides found in deck (ppt/slides/slideN.xml missing)"));
    }

    let mut modified: HashMap<String, Vec<u8>> = HashMap::new();
    let mut total_paras = 0usize;
    for n in &slide_nos {
        let name = format!("ppt/slides/slide{n}.xml");
        let mut xml = String::new();
        if archive
            .by_name(&name)
            .and_then(|mut f| f.read_to_string(&mut xml).map_err(zip::result::ZipError::from))
            .is_err()
        {
            continue;
        }
        match format_slide(&xml, *n, &a.rules) {
            Ok((new_xml, changed)) if changed > 0 => {
                modified.insert(name, new_xml.into_bytes());
                total_paras += changed;
            }
            Ok(_) => {}
            Err(msg) => return Ok(ToolOutput::err(msg)),
        }
    }
    drop(archive);

    if modified.is_empty() {
        return Ok(ToolOutput::ok(
            "No paragraphs matched the rules — nothing changed. Re-check scope/slide/frame against read_pptx.",
        ));
    }

    let bytes = match repackage(&src, &modified) {
        Ok(b) => b,
        Err(e) => return Ok(ToolOutput::err(format!("repackage failed: {e}"))),
    };
    if let Some(parent) = out_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return Ok(ToolOutput::err(format!("mkdir failed: {e}")));
        }
    }
    if let Err(e) = std::fs::write(&out_path, &bytes) {
        return Ok(ToolOutput::err(format!("write failed: {e}")));
    }

    Ok(ToolOutput::ok(format!(
        "Formatted {total_paras} paragraph(s) across {} slide(s) → {} ({} bytes). Theme, images and layout preserved.",
        modified.len(),
        rel_for_display(&out_path, ctx),
        bytes.len()
    )))
}

/// Validate + canonicalize rule values up front so a bad color/align fails fast
/// with a clear message instead of producing a broken deck.
fn normalize_rules(rules: &mut [Rule]) -> std::result::Result<(), String> {
    for (i, r) in rules.iter_mut().enumerate() {
        if let Some(c) = &r.color {
            r.color = Some(norm_color(c).ok_or_else(|| {
                format!("rule {i}: invalid color {c:?} — use hex RRGGBB (e.g. \"1F2937\")")
            })?);
        }
        if let Some(al) = &r.align {
            r.align = Some(norm_align(al).ok_or_else(|| {
                format!("rule {i}: invalid align {al:?} — use left/center/right/justify")
            })?);
        }
        if r.size.is_some_and(|s| !(1.0..=400.0).contains(&s)) {
            return Err(format!("rule {i}: size must be between 1 and 400 points"));
        }
        if r.line_spacing.is_some_and(|s| s <= 0.0) {
            return Err(format!("rule {i}: line_spacing must be a positive percentage"));
        }
        if r.space_before.is_some_and(|s| s < 0.0) || r.space_after.is_some_and(|s| s < 0.0) {
            return Err(format!("rule {i}: space_before/space_after must be >= 0"));
        }
        if r.frame.is_some() && r.slide.is_none() {
            return Err(format!("rule {i}: `frame` requires `slide`"));
        }
    }
    Ok(())
}

fn norm_align(s: &str) -> Option<String> {
    match s.trim().to_lowercase().as_str() {
        "l" | "left" => Some("l".into()),
        "ctr" | "center" | "centre" | "middle" => Some("ctr".into()),
        "r" | "right" => Some("r".into()),
        "just" | "justify" | "justified" => Some("just".into()),
        _ => None,
    }
}

fn norm_color(s: &str) -> Option<String> {
    let t = s.trim().trim_start_matches('#');
    (t.len() == 6 && t.chars().all(|c| c.is_ascii_hexdigit())).then(|| t.to_uppercase())
}

/// Apply the matching rules to every paragraph on one slide. Returns the new
/// slide XML and the count of paragraphs touched. Splices are gathered against
/// the original parse and applied right-to-left so byte offsets stay valid.
fn format_slide(
    xml: &str,
    slide_no: usize,
    rules: &[Rule],
) -> std::result::Result<(String, usize), String> {
    let frames = find_simple_elements(xml, "p:txBody", (0, xml.len()));
    let mut splices: Vec<(usize, usize, String)> = Vec::new();
    let mut changed = 0usize;

    for (fi, frame) in frames.iter().enumerate() {
        let Some(finner) = frame.inner else { continue };
        let hint = placeholder_hint(xml, frame.outer.0);
        let is_title = hint
            .as_deref()
            .is_some_and(|h| matches!(h, "title" | "ctrTitle" | "subTitle"));

        for para in find_simple_elements(xml, "a:p", finner) {
            let mut fmt = Fmt::default();
            for r in rules {
                if r.matches(slide_no, fi, is_title) {
                    fmt.merge(r);
                }
            }
            let Some(pinner) = para.inner else { continue };
            let (run_fmt, para_fmt) = (fmt.has_run(), fmt.has_para());
            if !run_fmt && !para_fmt {
                continue;
            }

            if para_fmt {
                match find_simple_elements(xml, "a:pPr", pinner).into_iter().next() {
                    Some(p) => splices.push((p.outer.0, p.outer.1, rewrite_props(xml, p, ElemKind::PPr, &fmt))),
                    None => splices.push((pinner.0, pinner.0, new_props("a:pPr", ElemKind::PPr, &fmt))),
                }
            }
            if run_fmt {
                for run in find_simple_elements(xml, "a:r", pinner) {
                    let Some(rinner) = run.inner else { continue };
                    match find_simple_elements(xml, "a:rPr", rinner).into_iter().next() {
                        Some(rp) => splices.push((rp.outer.0, rp.outer.1, rewrite_props(xml, rp, ElemKind::RPr, &fmt))),
                        None => splices.push((rinner.0, rinner.0, new_props("a:rPr", ElemKind::RPr, &fmt))),
                    }
                }
                // The paragraph mark's run properties, so empty/trailing lines match too.
                if let Some(ep) = find_simple_elements(xml, "a:endParaRPr", pinner).into_iter().next() {
                    splices.push((ep.outer.0, ep.outer.1, rewrite_props(xml, ep, ElemKind::RPr, &fmt)));
                }
            }
            changed += 1;
        }
    }

    splices.sort_by_key(|s| std::cmp::Reverse(s.0));
    for w in splices.windows(2) {
        if w[1].1 > w[0].0 {
            return Err(format!("internal: overlapping format splices on slide {slide_no}"));
        }
    }
    let mut out = xml.to_string();
    for (s, e, rep) in splices {
        out.replace_range(s..e, &rep);
    }
    Ok((out, changed))
}

/// Re-serialize an existing `<a:rPr>` / `<a:pPr>` / `<a:endParaRPr>` element with
/// `fmt` merged in, keeping unmanaged attributes and children.
fn rewrite_props(xml: &str, span: ElemSpan, kind: ElemKind, fmt: &Fmt) -> String {
    let s = &xml[span.outer.0..span.outer.1];
    let mut el = Element::parse(s).unwrap_or_else(|| Element {
        prefixed: if matches!(kind, ElemKind::PPr) { "a:pPr".into() } else { "a:rPr".into() },
        attrs: Vec::new(),
        children: Vec::new(),
    });
    apply_fmt(&mut el, kind, fmt);
    el.serialize(kind)
}

/// Build a fresh `<a:rPr>` / `<a:pPr>` element from `fmt` (used when the target
/// run/paragraph had no property element yet).
fn new_props(prefixed: &str, kind: ElemKind, fmt: &Fmt) -> String {
    let mut el = Element { prefixed: prefixed.into(), attrs: Vec::new(), children: Vec::new() };
    apply_fmt(&mut el, kind, fmt);
    el.serialize(kind)
}

fn apply_fmt(el: &mut Element, kind: ElemKind, f: &Fmt) {
    match kind {
        ElemKind::RPr => {
            if let Some(sz) = f.size {
                el.set_attr("sz", ((sz * 100.0).round() as i64).to_string());
            }
            if let Some(b) = f.bold {
                el.set_attr("b", if b { "1" } else { "0" }.into());
            }
            if let Some(it) = f.italic {
                el.set_attr("i", if it { "1" } else { "0" }.into());
            }
            if let Some(c) = &f.color {
                el.set_child("a:solidFill", format!("<a:solidFill><a:srgbClr val=\"{c}\"/></a:solidFill>"));
            }
            if let Some(font) = &f.font {
                let esc = xml_escape(font);
                el.set_child("a:latin", format!("<a:latin typeface=\"{esc}\"/>"));
                el.set_child("a:ea", format!("<a:ea typeface=\"{esc}\"/>"));
                el.set_child("a:cs", format!("<a:cs typeface=\"{esc}\"/>"));
            }
        }
        ElemKind::PPr => {
            if let Some(a) = &f.align {
                el.set_attr("algn", a.clone());
            }
            if let Some(ls) = f.line_spacing {
                el.set_child("a:lnSpc", format!("<a:lnSpc><a:spcPct val=\"{}\"/></a:lnSpc>", (ls * 1000.0).round() as i64));
            }
            if let Some(sb) = f.space_before {
                el.set_child("a:spcBef", format!("<a:spcBef><a:spcPts val=\"{}\"/></a:spcBef>", (sb * 100.0).round() as i64));
            }
            if let Some(sa) = f.space_after {
                el.set_child("a:spcAft", format!("<a:spcAft><a:spcPts val=\"{}\"/></a:spcAft>", (sa * 100.0).round() as i64));
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Minimal OOXML property-element model (a:rPr / a:pPr) with schema-correct child
// ordering. We only ever touch a handful of attributes/children; everything else
// on the element is parsed, preserved and re-emitted unchanged.
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum ElemKind {
    RPr,
    PPr,
}

struct Element {
    prefixed: String,
    attrs: Vec<(String, String)>,
    children: Vec<(String, String)>, // (prefixed name, full serialized text)
}

impl Element {
    fn parse(s: &str) -> Option<Element> {
        let b = s.as_bytes();
        if b.first() != Some(&b'<') {
            return None;
        }
        let mut j = 1;
        while j < b.len() && !matches!(b[j], b'>' | b'/' | b' ' | b'\t' | b'\r' | b'\n') {
            j += 1;
        }
        let prefixed = s[1..j].to_string();
        let gt = s[j..].find('>').map(|r| j + r)?;
        let self_closing = gt > 0 && b[gt - 1] == b'/';
        let attr_end = if self_closing { gt - 1 } else { gt };
        let attrs = parse_attrs(&s[j..attr_end]);
        let children = if self_closing {
            Vec::new()
        } else {
            let close = format!("</{prefixed}>");
            let inner_end = s.len().checked_sub(close.len())?;
            if inner_end < gt + 1 {
                Vec::new()
            } else {
                split_children(&s[gt + 1..inner_end])
            }
        };
        Some(Element { prefixed, attrs, children })
    }

    fn set_attr(&mut self, name: &str, val: String) {
        for a in &mut self.attrs {
            if a.0 == name {
                a.1 = val;
                return;
            }
        }
        self.attrs.push((name.to_string(), val));
    }

    fn set_child(&mut self, prefixed: &str, text: String) {
        self.children.retain(|c| c.0 != prefixed);
        self.children.push((prefixed.to_string(), text));
    }

    fn serialize(&self, kind: ElemKind) -> String {
        let mut s = String::new();
        s.push('<');
        s.push_str(&self.prefixed);
        for (k, v) in &self.attrs {
            s.push(' ');
            s.push_str(k);
            s.push_str("=\"");
            s.push_str(v);
            s.push('"');
        }
        if self.children.is_empty() {
            s.push_str("/>");
            return s;
        }
        s.push('>');
        let mut kids = self.children.clone();
        kids.sort_by_key(|c| child_ordinal(kind, local_name(&c.0)));
        for (_, t) in kids {
            s.push_str(&t);
        }
        s.push_str("</");
        s.push_str(&self.prefixed);
        s.push('>');
        s
    }
}

fn parse_attrs(region: &str) -> Vec<(String, String)> {
    let b = region.as_bytes();
    let n = b.len();
    let mut i = 0;
    let mut out = Vec::new();
    while i < n {
        while i < n && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        if i >= n {
            break;
        }
        let name_start = i;
        while i < n && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        let name = &region[name_start..i];
        while i < n && b[i] != b'=' {
            i += 1;
        }
        if i >= n {
            break;
        }
        i += 1; // past '='
        while i < n && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= n {
            break;
        }
        let quote = b[i];
        if quote != b'"' && quote != b'\'' {
            break;
        }
        i += 1;
        let val_start = i;
        while i < n && b[i] != quote {
            i += 1;
        }
        let val = &region[val_start..i.min(n)];
        i += 1; // past closing quote
        if !name.is_empty() {
            out.push((name.to_string(), val.to_string()));
        }
    }
    out
}

/// Split the inner content of a property element into its top-level child
/// elements. Correct for OOXML property children, which never self-nest.
fn split_children(inner: &str) -> Vec<(String, String)> {
    let b = inner.as_bytes();
    let n = b.len();
    let mut i = 0;
    let mut out = Vec::new();
    while i < n {
        while i < n && b[i] != b'<' {
            i += 1;
        }
        if i >= n {
            break;
        }
        let start = i;
        let mut j = i + 1;
        while j < n && !matches!(b[j], b'>' | b'/' | b' ' | b'\t' | b'\r' | b'\n') {
            j += 1;
        }
        let prefixed = inner[i + 1..j].to_string();
        let mut k = j;
        while k < n && b[k] != b'>' {
            k += 1;
        }
        if k >= n {
            break;
        }
        if b[k - 1] == b'/' {
            out.push((prefixed, inner[start..k + 1].to_string()));
            i = k + 1;
        } else {
            let close = format!("</{prefixed}>");
            match inner[k + 1..].find(&close) {
                Some(rel) => {
                    let end = k + 1 + rel + close.len();
                    out.push((prefixed, inner[start..end].to_string()));
                    i = end;
                }
                None => break,
            }
        }
    }
    out
}

fn local_name(prefixed: &str) -> &str {
    prefixed.rsplit(':').next().unwrap_or(prefixed)
}

/// Schema child ordering for CT_TextCharacterProperties (rPr) and
/// CT_TextParagraphProperties (pPr). Emitting children out of order makes
/// PowerPoint reject the part, so managed and preserved children alike are
/// sorted by these ranks. Unknown children sort late (but before extLst).
fn child_ordinal(kind: ElemKind, local: &str) -> u32 {
    match kind {
        ElemKind::RPr => match local {
            "ln" => 0,
            "noFill" | "solidFill" | "gradFill" | "blipFill" | "pattFill" | "grpFill" => 1,
            "effectLst" | "effectDag" => 2,
            "highlight" => 3,
            "uLnTx" | "uLn" => 4,
            "uFillTx" | "uFill" => 5,
            "latin" => 6,
            "ea" => 7,
            "cs" => 8,
            "sym" => 9,
            "hlinkClick" => 10,
            "hlinkMouseOver" => 11,
            "rtl" => 12,
            "extLst" => 100,
            _ => 99,
        },
        ElemKind::PPr => match local {
            "lnSpc" => 0,
            "spcBef" => 1,
            "spcAft" => 2,
            "buClrTx" | "buClr" => 3,
            "buSzTx" | "buSzPct" | "buSzPts" => 4,
            "buFontTx" | "buFont" => 5,
            "buNone" | "buAutoNum" | "buChar" => 6,
            "tabLst" => 7,
            "defRPr" => 8,
            "extLst" => 100,
            _ => 99,
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Flat OOXML element scanner (see module docs for why a DOM isn't needed).
// ─────────────────────────────────────────────────────────────────────────────

/// A located element. `outer` is the byte span of the whole element including
/// its tags; `inner` is the content between `>` and `</…>` (None when the
/// element is self-closing, e.g. `<a:pPr/>`).
#[derive(Debug, Clone, Copy)]
struct ElemSpan {
    outer: (usize, usize),
    inner: Option<(usize, usize)>,
}

/// Find every non-nesting element named `tag` (e.g. `"a:p"`) within
/// `xml[region.0..region.1]`, in document order. Correct only for element
/// types that never contain another element of the same name — true for all
/// the OOXML tags we touch.
fn find_simple_elements(xml: &str, tag: &str, region: (usize, usize)) -> Vec<ElemSpan> {
    let bytes = xml.as_bytes();
    let open_pat = format!("<{tag}");
    let close_pat = format!("</{tag}>");
    let mut out = Vec::new();
    let hi = region.1.min(xml.len());
    let mut i = region.0;

    while i < hi {
        let Some(rel) = xml[i..hi].find(&open_pat) else { break };
        let open_start = i + rel;
        let name_end = open_start + open_pat.len();
        // The char after the tag name must delimit it, else this is a longer
        // tag that merely shares the prefix (e.g. `<a:pPr` when tag is `a:p`).
        let is_boundary = matches!(
            bytes.get(name_end),
            Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n') | Some(b'/')
        );
        if !is_boundary {
            i = name_end;
            continue;
        }
        let Some(gt_rel) = xml[name_end..hi].find('>') else { break };
        let open_tag_end = name_end + gt_rel + 1; // just past '>'
        if open_tag_end >= 2 && bytes[open_tag_end - 2] == b'/' {
            out.push(ElemSpan { outer: (open_start, open_tag_end), inner: None });
            i = open_tag_end;
            continue;
        }
        let Some(close_rel) = xml[open_tag_end..hi].find(&close_pat) else { break };
        let inner_start = open_tag_end;
        let inner_end = open_tag_end + close_rel;
        let outer_end = inner_end + close_pat.len();
        out.push(ElemSpan {
            outer: (open_start, outer_end),
            inner: Some((inner_start, inner_end)),
        });
        i = outer_end;
    }
    out
}

/// Concatenated visible text of a paragraph (all its `<a:t>` runs, unescaped).
fn paragraph_text(xml: &str, para: &ElemSpan) -> String {
    let Some(inner) = para.inner else { return String::new() };
    let mut s = String::new();
    for t in find_simple_elements(xml, "a:t", inner) {
        if let Some((a, b)) = t.inner {
            s.push_str(&xml_unescape(&xml[a..b]));
        }
    }
    s.trim().to_string()
}

/// Best-effort placeholder hint for a text frame: the `type` (or `idx`) of the
/// nearest `<p:ph …>` preceding the frame. Helps the model tell a title from a
/// body. Returns None when the frame isn't a placeholder.
fn placeholder_hint(xml: &str, before: usize) -> Option<String> {
    let region = &xml[..before.min(xml.len())];
    let idx = region.rfind("<p:ph")?;
    let tag_end = region[idx..].find('>').map(|r| idx + r)?;
    let tag = &region[idx..tag_end];
    if let Some(t) = attr_value(tag, "type") {
        return Some(t);
    }
    attr_value(tag, "idx").map(|i| format!("body idx {i}"))
}

fn attr_value(tag: &str, attr: &str) -> Option<String> {
    let pat = format!("{attr}=\"");
    let s = tag.find(&pat)? + pat.len();
    let e = tag[s..].find('"').map(|r| s + r)?;
    Some(tag[s..e].to_string())
}

fn slide_no_from_name(name: &str) -> Option<usize> {
    name.strip_prefix("ppt/slides/slide")?
        .strip_suffix(".xml")?
        .parse()
        .ok()
}

fn rel_for_display(p: &Path, ctx: &ExecCtx) -> String {
    p.strip_prefix(&ctx.cwd)
        .map(|r| r.to_string_lossy().to_string())
        .unwrap_or_else(|_| p.to_string_lossy().to_string())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Reverse of `xml_escape`. `&amp;` is decoded last so that doubly-escaped
/// sequences like `&amp;lt;` round-trip to the literal `&lt;`.
fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use zip::write::SimpleFileOptions;
    use zip::{CompressionMethod, ZipArchive, ZipWriter};

    const SLIDE1: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://a" xmlns:r="http://r" xmlns:p="http://p">
<p:cSld><p:spTree>
<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>
<p:txBody><a:bodyPr/><a:p><a:pPr algn="ctr"/><a:r><a:rPr lang="en-US" sz="4000" b="1"/><a:t>Original Title</a:t></a:r></a:p></p:txBody></p:sp>
<p:sp><p:nvSpPr><p:cNvPr id="3" name="Content"/><p:cNvSpPr/><p:nvPr><p:ph idx="1"/></p:nvPr></p:nvSpPr>
<p:txBody><a:bodyPr/><a:p><a:pPr marL="342900" indent="-342900"><a:buChar char="•"/></a:pPr><a:r><a:rPr lang="en-US" sz="2400"/><a:t>First </a:t></a:r><a:r><a:rPr lang="en-US" sz="2400" b="1"/><a:t>bullet</a:t></a:r></a:p><a:p><a:pPr marL="342900" indent="-342900"><a:buChar char="•"/></a:pPr><a:r><a:rPr lang="en-US" sz="2400"/><a:t>Second bullet</a:t></a:r></a:p></p:txBody></p:sp>
</p:spTree></p:cSld></p:sld>"#;

    fn make_deck(slide1: &str) -> Vec<u8> {
        let buf = Cursor::new(Vec::<u8>::new());
        let mut zip = ZipWriter::new(buf);
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        // A media part we must NOT disturb.
        zip.start_file::<_, ()>("ppt/media/image1.png", opts).unwrap();
        zip.write_all(b"\x89PNG\r\n\x1a\nFAKEIMAGE").unwrap();
        zip.start_file::<_, ()>("ppt/theme/theme1.xml", opts).unwrap();
        zip.write_all(b"<theme>accent1=ABC123</theme>").unwrap();
        zip.start_file::<_, ()>("ppt/slides/slide1.xml", opts).unwrap();
        zip.write_all(slide1.as_bytes()).unwrap();
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn finds_frames_and_paragraphs() {
        let frames = find_simple_elements(SLIDE1, "p:txBody", (0, SLIDE1.len()));
        assert_eq!(frames.len(), 2, "expected title + content frames");
        let paras = find_simple_elements(SLIDE1, "a:p", frames[1].inner.unwrap());
        assert_eq!(paras.len(), 2, "content frame has two bullets");
    }

    #[test]
    fn paragraph_text_joins_runs() {
        let frames = find_simple_elements(SLIDE1, "p:txBody", (0, SLIDE1.len()));
        let paras = find_simple_elements(SLIDE1, "a:p", frames[1].inner.unwrap());
        // "First " + "bullet" across two runs.
        assert_eq!(paragraph_text(SLIDE1, &paras[0]), "First bullet");
    }

    #[test]
    fn scanner_does_not_confuse_ppr_with_p() {
        // `a:p` scan must skip `a:pPr`.
        let frames = find_simple_elements(SLIDE1, "p:txBody", (0, SLIDE1.len()));
        let paras = find_simple_elements(SLIDE1, "a:p", frames[0].inner.unwrap());
        assert_eq!(paras.len(), 1);
        assert_eq!(paragraph_text(SLIDE1, &paras[0]), "Original Title");
    }

    #[test]
    fn replace_preserves_first_run_formatting_and_drops_extra_runs() {
        let frames = find_simple_elements(SLIDE1, "p:txBody", (0, SLIDE1.len()));
        let paras = find_simple_elements(SLIDE1, "a:p", frames[1].inner.unwrap());
        let rebuilt = rebuild_paragraph(SLIDE1, &paras[0], "Much richer first bullet");
        // First run's rPr (sz="2400") preserved; pPr (bullet) preserved.
        assert!(rebuilt.contains("sz=\"2400\""), "first run rPr lost: {rebuilt}");
        assert!(rebuilt.contains("buChar"), "paragraph bullet props lost: {rebuilt}");
        assert!(rebuilt.contains("<a:t>Much richer first bullet</a:t>"));
        // The second run's bold text must be gone (folded into one run).
        assert!(!rebuilt.contains("bullet</a:t>") || rebuilt.matches("<a:t>").count() == 1,
            "expected a single run after replace: {rebuilt}");
        assert_eq!(rebuilt.matches("<a:t>").count(), 1, "expected exactly one text run");
    }

    #[test]
    fn replace_escapes_special_chars() {
        let frames = find_simple_elements(SLIDE1, "p:txBody", (0, SLIDE1.len()));
        let paras = find_simple_elements(SLIDE1, "a:p", frames[0].inner.unwrap());
        let rebuilt = rebuild_paragraph(SLIDE1, &paras[0], "A < B & \"C\"");
        assert!(rebuilt.contains("A &lt; B &amp; &quot;C&quot;"), "got: {rebuilt}");
    }

    #[test]
    fn apply_replace_and_insert_after_in_one_slide() {
        let edits = vec![
            Edit { op: EditOp::Replace, slide: 1, frame: 0, para: 0, text: Some("New Title".into()), texts: None },
            Edit { op: EditOp::InsertAfter, slide: 1, frame: 1, para: 1, text: None, texts: Some(vec!["Third bullet".into(), "Fourth bullet".into()]) },
        ];
        let refs: Vec<&Edit> = edits.iter().collect();
        let new_xml = apply_edits_to_slide(SLIDE1, &refs).unwrap();
        assert!(new_xml.contains("<a:t>New Title</a:t>"), "title not replaced");
        assert!(new_xml.contains("<a:t>Third bullet</a:t>"), "insert missing");
        assert!(new_xml.contains("<a:t>Fourth bullet</a:t>"), "second insert missing");
        // Inserted bullets cloned the bullet formatting.
        let content = find_simple_elements(&new_xml, "p:txBody", (0, new_xml.len()));
        let paras = find_simple_elements(&new_xml, "a:p", content[1].inner.unwrap());
        assert_eq!(paras.len(), 4, "content frame should now have four bullets");
    }

    #[test]
    fn missing_locator_errors_clearly() {
        let edits = vec![Edit { op: EditOp::Replace, slide: 1, frame: 9, para: 0, text: Some("x".into()), texts: None }];
        let refs: Vec<&Edit> = edits.iter().collect();
        let err = apply_edits_to_slide(SLIDE1, &refs).unwrap_err();
        assert!(err.contains("no frame index 9"), "got: {err}");
    }

    #[tokio::test]
    async fn end_to_end_read_then_edit_via_tool_entrypoints() {
        let deck = make_deck(SLIDE1);
        let cwd = std::env::temp_dir().join(format!("cf-pptx-e2e-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(cwd.join("deck.pptx"), &deck).unwrap();
        let ctx = ExecCtx::new(cwd.clone(), None);

        // read_pptx surfaces the locators + current text.
        let read = execute_read(json!({ "path": "deck.pptx" }), &ctx).await.unwrap();
        assert!(!read.is_error, "read_pptx errored: {}", read.content);
        assert!(read.content.contains("s1.0.0"), "missing title locator: {}", read.content);
        assert!(read.content.contains("Original Title"));
        assert!(read.content.contains("title"), "placeholder hint missing: {}", read.content);

        // edit_pptx rewrites the title and appends a KB-derived bullet.
        let edit = execute_edit(
            json!({
                "path": "deck.pptx",
                "out_path": "deck-enriched.pptx",
                "edits": [
                    { "op": "replace", "slide": 1, "frame": 0, "para": 0, "text": "Enriched Title" },
                    { "op": "insert_after", "slide": 1, "frame": 1, "para": 1, "texts": ["KB-derived bullet"] }
                ]
            }),
            &ctx,
        )
        .await
        .unwrap();
        assert!(!edit.is_error, "edit_pptx errored: {}", edit.content);

        // Output is a valid zip, enriched, with theme/media intact and the
        // source deck untouched.
        let out_bytes = std::fs::read(cwd.join("deck-enriched.pptx")).unwrap();
        let mut zip = ZipArchive::new(Cursor::new(out_bytes)).unwrap();
        let mut slide = String::new();
        zip.by_name("ppt/slides/slide1.xml").unwrap().read_to_string(&mut slide).unwrap();
        assert!(slide.contains("Enriched Title"), "title not enriched");
        assert!(slide.contains("KB-derived bullet"), "bullet not appended");
        let mut img = Vec::new();
        zip.by_name("ppt/media/image1.png").unwrap().read_to_end(&mut img).unwrap();
        assert_eq!(img, b"\x89PNG\r\n\x1a\nFAKEIMAGE", "media must survive the edit");
        assert!(std::fs::read(cwd.join("deck.pptx")).unwrap() == deck, "source deck must be untouched");

        std::fs::remove_dir_all(&cwd).ok();
    }

    // ── format_pptx ─────────────────────────────────────────────────────────

    fn fmt_all(rule: serde_json::Value) -> Vec<Rule> {
        let mut rules: Vec<Rule> = vec![serde_json::from_value(rule).unwrap()];
        normalize_rules(&mut rules).unwrap();
        rules
    }

    #[test]
    fn format_sets_font_size_color_on_existing_rpr() {
        let rules = fmt_all(json!({ "scope": "all", "font": "Calibri", "size": 18, "color": "#1F2937" }));
        let (out, changed) = format_slide(SLIDE1, 1, &rules).unwrap();
        assert!(changed >= 3, "expected title + two bullets touched, got {changed}");
        assert!(out.contains("<a:latin typeface=\"Calibri\"/>"), "font not set: {out}");
        assert!(out.contains("<a:ea typeface=\"Calibri\"/>"));
        assert!(out.contains("<a:cs typeface=\"Calibri\"/>"));
        assert!(out.contains("sz=\"1800\""), "size not converted to hundredths: {out}");
        assert!(out.contains("<a:srgbClr val=\"1F2937\"/>"), "color not set: {out}");
        // The original bold run keeps its b="1" (we didn't set bold).
        assert!(out.contains("b=\"1\""), "unrelated bold attribute dropped");
    }

    #[test]
    fn format_creates_rpr_and_ppr_when_absent() {
        // A run with no rPr and a paragraph with no pPr.
        let xml = r#"<p:sld xmlns:a="http://a"><p:cSld><p:spTree><p:sp><p:txBody><a:bodyPr/><a:p><a:r><a:t>bare</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#;
        let rules = fmt_all(json!({ "scope": "all", "size": 20, "align": "center" }));
        let (out, changed) = format_slide(xml, 1, &rules).unwrap();
        assert_eq!(changed, 1);
        assert!(out.contains("<a:rPr sz=\"2000\"/>"), "rPr not created: {out}");
        assert!(out.contains("<a:pPr algn=\"ctr\">") || out.contains("algn=\"ctr\""), "pPr not created: {out}");
        // rPr must sit before the text run content, pPr before the runs.
        assert!(out.find("<a:pPr").unwrap() < out.find("<a:r>").unwrap(), "pPr must precede runs");
        assert!(out.find("<a:rPr").unwrap() < out.find("<a:t>").unwrap(), "rPr must precede a:t");
    }

    #[test]
    fn format_emits_children_in_schema_order() {
        // Setting color + font on an rPr: solidFill (rank 1) must precede latin (6).
        let rules = fmt_all(json!({ "scope": "title", "font": "Arial", "color": "ABCDEF" }));
        let (out, _) = format_slide(SLIDE1, 1, &rules).unwrap();
        let fill = out.find("<a:solidFill>").expect("solidFill present");
        let latin = out.find("<a:latin").expect("latin present");
        assert!(fill < latin, "solidFill must precede latin in rPr: {out}");

        // pPr: lnSpc (0) before spcBef (1) before spcAft (2).
        let rules = fmt_all(json!({ "scope": "body", "line_spacing": 150, "space_before": 6, "space_after": 12 }));
        let (out, _) = format_slide(SLIDE1, 1, &rules).unwrap();
        let ln = out.find("<a:lnSpc>").expect("lnSpc");
        let bef = out.find("<a:spcBef>").expect("spcBef");
        let aft = out.find("<a:spcAft>").expect("spcAft");
        assert!(ln < bef && bef < aft, "pPr spacing children out of order: {out}");
        assert!(out.contains("<a:spcPct val=\"150000\"/>"), "line spacing units: {out}");
        assert!(out.contains("<a:spcBef><a:spcPts val=\"600\"/></a:spcBef>"), "space_before units: {out}");
        assert!(out.contains("<a:spcAft><a:spcPts val=\"1200\"/></a:spcAft>"), "space_after units: {out}");
        // Existing bullet props (buChar, rank 6) survive.
        assert!(out.contains("buChar"), "bullet props dropped: {out}");
    }

    #[test]
    fn format_title_scope_leaves_body_untouched() {
        let rules = fmt_all(json!({ "scope": "title", "size": 54 }));
        let (out, _) = format_slide(SLIDE1, 1, &rules).unwrap();
        // Title size becomes 5400; the body bullets keep their 2400.
        assert!(out.contains("sz=\"5400\""), "title not resized: {out}");
        assert!(out.contains("sz=\"2400\""), "body size should be untouched: {out}");
    }

    #[test]
    fn format_preserves_run_text() {
        let rules = fmt_all(json!({ "scope": "all", "font": "Calibri" }));
        let (out, _) = format_slide(SLIDE1, 1, &rules).unwrap();
        // Reformatting must not disturb the visible text in any run.
        assert!(out.contains("<a:t>Original Title</a:t>"));
        assert!(out.contains("<a:t>First </a:t>"));
        assert!(out.contains("<a:t>bullet</a:t>"));
        assert!(out.contains("<a:t>Second bullet</a:t>"));
    }

    #[test]
    fn invalid_color_rejected() {
        let mut rules: Vec<Rule> = vec![serde_json::from_value(json!({ "color": "not-a-color" })).unwrap()];
        let err = normalize_rules(&mut rules).unwrap_err();
        assert!(err.contains("invalid color"), "got: {err}");
    }

    #[test]
    fn frame_without_slide_rejected() {
        let mut rules: Vec<Rule> = vec![serde_json::from_value(json!({ "frame": 0, "size": 18 })).unwrap()];
        let err = normalize_rules(&mut rules).unwrap_err();
        assert!(err.contains("`frame` requires `slide`"), "got: {err}");
    }

    #[test]
    fn element_roundtrips_unmanaged_content() {
        let src = r#"<a:rPr lang="en-US" sz="2400" b="1"><a:hlinkClick r:id="rId3"/></a:rPr>"#;
        let mut el = Element::parse(src).unwrap();
        // Touch only the font; lang/b/hlinkClick must survive, hlinkClick stays last.
        el.set_child("a:latin", "<a:latin typeface=\"Calibri\"/>".into());
        let out = el.serialize(ElemKind::RPr);
        assert!(out.contains("lang=\"en-US\""));
        assert!(out.contains("b=\"1\""));
        assert!(out.contains("<a:hlinkClick r:id=\"rId3\"/>"));
        assert!(out.find("<a:latin").unwrap() < out.find("<a:hlinkClick").unwrap(), "latin(6) must precede hlinkClick(10): {out}");
    }

    #[tokio::test]
    async fn format_pptx_via_entrypoint_preserves_media() {
        let deck = make_deck(SLIDE1);
        let cwd = std::env::temp_dir().join(format!("cf-pptx-fmt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(cwd.join("deck.pptx"), &deck).unwrap();
        let ctx = ExecCtx::new(cwd.clone(), None);

        let res = execute_format(
            json!({
                "path": "deck.pptx",
                "out_path": "deck-pretty.pptx",
                "rules": [
                    { "scope": "all", "font": "Calibri", "color": "374151" },
                    { "scope": "title", "size": 40, "align": "left" },
                    { "scope": "body", "size": 18, "line_spacing": 120 }
                ]
            }),
            &ctx,
        )
        .await
        .unwrap();
        assert!(!res.is_error, "format_pptx errored: {}", res.content);

        let out_bytes = std::fs::read(cwd.join("deck-pretty.pptx")).unwrap();
        let mut zip = ZipArchive::new(Cursor::new(out_bytes)).unwrap();
        let mut slide = String::new();
        zip.by_name("ppt/slides/slide1.xml").unwrap().read_to_string(&mut slide).unwrap();
        assert!(slide.contains("<a:latin typeface=\"Calibri\"/>"), "font missing: {slide}");
        assert!(slide.contains("sz=\"4000\""), "title size missing");
        assert!(slide.contains("sz=\"1800\""), "body size missing");
        assert!(slide.contains("<a:spcPct val=\"120000\"/>"), "line spacing missing");
        assert!(slide.contains("<a:t>Original Title</a:t>"), "text mangled");
        // Media survives; source untouched.
        let mut img = Vec::new();
        zip.by_name("ppt/media/image1.png").unwrap().read_to_end(&mut img).unwrap();
        assert_eq!(img, b"\x89PNG\r\n\x1a\nFAKEIMAGE");
        assert!(std::fs::read(cwd.join("deck.pptx")).unwrap() == deck, "source deck must be untouched");

        std::fs::remove_dir_all(&cwd).ok();
    }

    #[tokio::test]
    async fn read_with_format_reports_props() {
        let deck = make_deck(SLIDE1);
        let cwd = std::env::temp_dir().join(format!("cf-pptx-rf-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(cwd.join("deck.pptx"), &deck).unwrap();
        let ctx = ExecCtx::new(cwd.clone(), None);

        let read = execute_read(json!({ "path": "deck.pptx", "with_format": true }), &ctx).await.unwrap();
        assert!(!read.is_error, "read errored: {}", read.content);
        // Title run is 40pt bold, centered.
        assert!(read.content.contains("sz=40"), "size summary missing: {}", read.content);
        assert!(read.content.contains("algn=ctr"), "align summary missing: {}", read.content);
        assert!(read.content.contains("b]") || read.content.contains("b "), "bold summary missing: {}", read.content);

        std::fs::remove_dir_all(&cwd).ok();
    }

    #[test]
    fn repackage_preserves_untouched_parts() {
        let deck = make_deck(SLIDE1);
        let dir = std::env::temp_dir().join(format!("cf-pptxedit-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("deck.pptx");
        std::fs::write(&src, &deck).unwrap();

        let mut modified = HashMap::new();
        let new_slide = SLIDE1.replace("Original Title", "Brand New Title");
        modified.insert("ppt/slides/slide1.xml".to_string(), new_slide.into_bytes());

        let bytes = repackage(&src, &modified).unwrap();
        let mut zip = ZipArchive::new(Cursor::new(bytes)).unwrap();

        // Media + theme bytes unchanged.
        let mut img = Vec::new();
        zip.by_name("ppt/media/image1.png").unwrap().read_to_end(&mut img).unwrap();
        assert_eq!(img, b"\x89PNG\r\n\x1a\nFAKEIMAGE");
        let mut theme = String::new();
        zip.by_name("ppt/theme/theme1.xml").unwrap().read_to_string(&mut theme).unwrap();
        assert_eq!(theme, "<theme>accent1=ABC123</theme>");
        // Slide updated.
        let mut s = String::new();
        zip.by_name("ppt/slides/slide1.xml").unwrap().read_to_string(&mut s).unwrap();
        assert!(s.contains("Brand New Title"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
