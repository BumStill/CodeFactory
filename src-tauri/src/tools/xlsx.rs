// SPDX-License-Identifier: Apache-2.0
//! read_xlsx / edit_xlsx — let the agent read a spreadsheet as a table and write
//! values into specific cells (e.g. "summarize column B into column C").
//!
//! read_xlsx uses calamine. edit_xlsx reads the whole workbook, applies the
//! requested cell edits, and rewrites it with rust_xlsxwriter.
//!
//! Caveat: edit_xlsx rebuilds each sheet from its *values* — original cell
//! styling and live formulas (kept as their last computed value) are not
//! preserved. Fine for data tables (the common case); not for styled reports.

use calamine::{open_workbook_auto, Data, DataType, Reader};
use rust_xlsxwriter::Workbook;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{workspace_path, ExecCtx, ToolOutput};
use crate::errors::Result;
use crate::openrouter::types::{FunctionDefinition, ToolDefinition};

// ── A1 helpers ────────────────────────────────────────────────────────────────

/// Parse an A1 reference ("C2", "AB14") into 0-based (row, col). None if invalid.
fn parse_a1(s: &str) -> Option<(u32, u32)> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_ascii_digit())?;
    let (col_part, row_part) = s.split_at(split);
    if col_part.is_empty() {
        return None;
    }
    let mut col: u32 = 0;
    for c in col_part.chars() {
        if !c.is_ascii_alphabetic() {
            return None;
        }
        col = col * 26 + (c.to_ascii_uppercase() as u32 - 'A' as u32 + 1);
    }
    let row: u32 = row_part.parse().ok()?;
    if col == 0 || row == 0 {
        return None;
    }
    Some((row - 1, col - 1))
}

/// 0-based column index → letters (0 → "A", 26 → "AA").
fn col_letter(col0: u32) -> String {
    let mut s = String::new();
    let mut n = col0 + 1;
    while n > 0 {
        let rem = (n - 1) % 26;
        s.insert(0, (b'A' + rem as u8) as char);
        n = (n - 1) / 26;
    }
    s
}

// ── read_xlsx ─────────────────────────────────────────────────────────────────

pub fn read_definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "read_xlsx".into(),
            description: "Read an .xlsx spreadsheet as a table: each row is listed with its \
                1-based row number and lettered columns (A, B, C…) and cell values — so you can \
                act per-cell (e.g. summarize column B into column C, then write with edit_xlsx). \
                Reads the first sheet unless `sheet` is given."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the .xlsx file" },
                    "sheet": { "type": "string", "description": "Sheet name (optional; default = first sheet)" }
                },
                "required": ["path"]
            }),
        },
    }
}

pub async fn execute_read(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    #[derive(Deserialize)]
    struct A {
        path: String,
        sheet: Option<String>,
    }
    let a: A = match serde_json::from_value(args) {
        Ok(v) => v,
        Err(e) => return Ok(ToolOutput::err(format!("read_xlsx 参数错误: {e}"))),
    };
    let path = match workspace_path::resolve_existing(&ctx.cwd, &a.path) {
        Ok(p) => p,
        Err(e) => return Ok(ToolOutput::err(e.message())),
    };
    if !path.to_string_lossy().to_lowercase().ends_with(".xlsx") {
        return Ok(ToolOutput::err("read_xlsx 的 path 必须以 .xlsx 结尾"));
    }

    let mut wb = match open_workbook_auto(&path) {
        Ok(w) => w,
        Err(e) => return Ok(ToolOutput::err(format!("打不开 {}: {e}", path.display()))),
    };
    let names = wb.sheet_names().to_vec();
    if names.is_empty() {
        return Ok(ToolOutput::err("工作簿里没有工作表"));
    }
    let sheet_name = match &a.sheet {
        Some(s) => s.clone(),
        None => names[0].clone(),
    };
    let range = match wb.worksheet_range(&sheet_name) {
        Ok(r) => r,
        Err(e) => {
            return Ok(ToolOutput::err(format!(
                "读不到工作表「{sheet_name}」: {e}。可用工作表: {}",
                names.join(", ")
            )))
        }
    };

    let (start_row, start_col) = range.start().unwrap_or((0, 0));
    let height = range.height();
    let width = range.width();
    if height == 0 || width == 0 {
        return Ok(ToolOutput::ok(format!(
            "工作表「{sheet_name}」为空。工作簿里的工作表: {}",
            names.join(", ")
        )));
    }

    let mut out = String::new();
    out.push_str(&format!(
        "工作表「{sheet_name}」共 {height} 行 × {width} 列（数据起始 {}{}）。工作簿工作表: {}\n\
         每行格式: 行号: 列字母=值 …（用 edit_xlsx 写回,如 cell=\"C2\"）\n\n",
        col_letter(start_col),
        start_row + 1,
        names.join(", ")
    ));

    // Soft cap so a huge sheet doesn't blow the context; note any truncation.
    const MAX_ROWS: usize = 400;
    let mut shown = 0usize;
    for (ri, row) in range.rows().enumerate() {
        if shown >= MAX_ROWS {
            out.push_str(&format!(
                "\n…(已省略剩余 {} 行;如需可分批用 sheet/范围继续)\n",
                height - shown
            ));
            break;
        }
        let abs_row = start_row as usize + ri + 1; // 1-based row number
        let mut line = format!("行{abs_row}:");
        for (ci, cell) in row.iter().enumerate() {
            let letter = col_letter(start_col + ci as u32);
            let v = cell.to_string();
            if !v.is_empty() {
                line.push_str(&format!(" {letter}={v} |"));
            }
        }
        out.push_str(&line);
        out.push('\n');
        shown += 1;
    }
    Ok(ToolOutput::ok(out))
}

// ── edit_xlsx ─────────────────────────────────────────────────────────────────

pub fn edit_definition() -> ToolDefinition {
    ToolDefinition {
        r#type: "function".into(),
        function: FunctionDefinition {
            name: "edit_xlsx".into(),
            description: "Write values into specific cells of an .xlsx file and save it. Pass a \
                list of { cell, value } (A1 refs like \"C2\"). All other cells are preserved (by \
                value). Use this to write summaries/results back into a column. NOTE: rebuilds the \
                sheet from values — original styling and formulas are not kept."
                .into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the .xlsx file" },
                    "sheet": { "type": "string", "description": "Sheet to edit (optional; default = first sheet)" },
                    "edits": {
                        "type": "array",
                        "description": "Cells to set",
                        "items": {
                            "type": "object",
                            "properties": {
                                "cell": { "type": "string", "description": "A1 reference, e.g. C2" },
                                "value": { "type": "string", "description": "Text to write into the cell" }
                            },
                            "required": ["cell", "value"]
                        }
                    }
                },
                "required": ["path", "edits"]
            }),
        },
    }
}

pub async fn execute_edit(args: Value, ctx: &ExecCtx) -> Result<ToolOutput> {
    #[derive(Deserialize)]
    struct Edit {
        cell: String,
        value: String,
    }
    #[derive(Deserialize)]
    struct A {
        path: String,
        sheet: Option<String>,
        edits: Vec<Edit>,
    }
    let a: A = match serde_json::from_value(args) {
        Ok(v) => v,
        Err(e) => return Ok(ToolOutput::err(format!("edit_xlsx 参数错误: {e}"))),
    };
    if a.edits.is_empty() {
        return Ok(ToolOutput::err("edits 为空,没有要写的单元格"));
    }
    let path = match workspace_path::resolve_existing(&ctx.cwd, &a.path) {
        Ok(p) => p,
        Err(e) => return Ok(ToolOutput::err(e.message())),
    };
    if !path.to_string_lossy().to_lowercase().ends_with(".xlsx") {
        return Ok(ToolOutput::err("edit_xlsx 的 path 必须以 .xlsx 结尾"));
    }

    // Parse the requested edits up front.
    let mut parsed: Vec<((u32, u32), String)> = Vec::with_capacity(a.edits.len());
    for e in &a.edits {
        match parse_a1(&e.cell) {
            Some(rc) => parsed.push((rc, e.value.clone())),
            None => return Ok(ToolOutput::err(format!("无效单元格引用「{}」", e.cell))),
        }
    }

    let mut src = match open_workbook_auto(&path) {
        Ok(w) => w,
        Err(e) => return Ok(ToolOutput::err(format!("打不开 {}: {e}", path.display()))),
    };
    let names = src.sheet_names().to_vec();
    if names.is_empty() {
        return Ok(ToolOutput::err("工作簿里没有工作表"));
    }
    let target = match &a.sheet {
        Some(s) => s.clone(),
        None => names[0].clone(),
    };
    if !names.iter().any(|n| n == &target) {
        return Ok(ToolOutput::err(format!(
            "没有工作表「{target}」。可用: {}",
            names.join(", ")
        )));
    }

    let mut book = Workbook::new();
    for name in &names {
        let range = match src.worksheet_range(name) {
            Ok(r) => r,
            Err(e) => return Ok(ToolOutput::err(format!("读不到工作表「{name}」: {e}"))),
        };
        let ws = book.add_worksheet();
        if ws.set_name(name).is_err() {
            // Sheet name too long / invalid for the writer — fall back to default.
        }
        let (sr, sc) = range.start().unwrap_or((0, 0));
        for (ri, row) in range.rows().enumerate() {
            for (ci, cell) in row.iter().enumerate() {
                if matches!(cell, Data::Empty) {
                    continue;
                }
                let r = sr + ri as u32;
                let c = (sc + ci as u32) as u16;
                let _ = match cell.as_f64() {
                    Some(f) => ws.write_number(r, c, f),
                    None => ws.write_string(r, c, cell.to_string()),
                };
            }
        }
        if name == &target {
            for ((r, c), v) in &parsed {
                let _ = ws.write_string(*r, *c as u16, v);
            }
        }
    }

    let bytes = match book.save_to_buffer() {
        Ok(bytes) => bytes,
        Err(e) => {
            return Ok(ToolOutput::err(format!(
                "序列化 {} 失败: {e}",
                path.display()
            )))
        }
    };
    if let Err(e) = super::file_lock::atomic_write(&path, &bytes).await {
        return Ok(ToolOutput::err(format!("保存 {} 失败: {e}", path.display())));
    }
    let cells = parsed
        .iter()
        .map(|((r, c), _)| format!("{}{}", col_letter(*c), r + 1))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(ToolOutput::ok(format!(
        "已写入工作表「{target}」的 {} 个单元格({cells})并保存。",
        parsed.len()
    )))
}

#[cfg(test)]
mod tests {
    use super::{col_letter, parse_a1};

    #[test]
    fn a1_parses() {
        assert_eq!(parse_a1("A1"), Some((0, 0)));
        assert_eq!(parse_a1("C2"), Some((1, 2)));
        assert_eq!(parse_a1("AA10"), Some((9, 26)));
        assert_eq!(parse_a1("c3"), Some((2, 2))); // lowercase column ok
        assert_eq!(parse_a1(" B4 "), Some((3, 1))); // trimmed
    }

    #[test]
    fn a1_rejects_garbage() {
        assert_eq!(parse_a1(""), None);
        assert_eq!(parse_a1("3"), None); // no column letters
        assert_eq!(parse_a1("C"), None); // no row number
        assert_eq!(parse_a1("C0"), None); // rows are 1-based
        assert_eq!(parse_a1("C-1"), None);
    }

    #[test]
    fn columns_to_letters() {
        assert_eq!(col_letter(0), "A");
        assert_eq!(col_letter(25), "Z");
        assert_eq!(col_letter(26), "AA");
        assert_eq!(col_letter(701), "ZZ");
        assert_eq!(col_letter(702), "AAA");
    }
}
