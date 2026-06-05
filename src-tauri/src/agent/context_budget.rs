// SPDX-License-Identifier: Apache-2.0
//! Unified context budget for the agent system prompt.
//!
//! The system prompt is assembled from a fixed base persona plus several
//! "knowledge" blocks — project memory, README, project config, enabled
//! skills, and the user's preferences/learnings. Historically each block had
//! its own char cap, but there was no *total* ceiling and skills were entirely
//! unbounded, so a project with a big README plus several large skills could
//! quietly eat a huge slice of the context window before the conversation even
//! started.
//!
//! [`assemble`] gives them one shared budget. The base persona is always kept
//! in full (it's the contract). Budget is then allocated to the knowledge
//! blocks by priority — most important first — but blocks render in the order
//! they were supplied, so the prompt reads the same as before. Only when the
//! total is exceeded do the least-important blocks get truncated, or dropped if
//! what's left is too little to be useful.

/// Below this many chars a partial block is dropped rather than rendered as a
/// near-useless fragment.
const MIN_USEFUL_CHARS: usize = 200;

/// One knowledge block contributed to the system prompt. `content` is already
/// fully rendered (its own heading + body); the assembler only concatenates and
/// budgets — it never reformats a block.
pub struct Block {
    pub content: String,
    /// Eviction priority — lower is more important (allocated budget first).
    pub priority: u8,
    /// Per-block ceiling, independent of the shared total.
    pub max_chars: usize,
}

impl Block {
    pub fn new(content: impl Into<String>, priority: u8, max_chars: usize) -> Self {
        Self {
            content: content.into(),
            priority,
            max_chars,
        }
    }
}

/// Truncate `s` to at most `max` chars on a char boundary, marking the cut.
fn cap_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max).collect();
        format!("{}\n[… truncated to fit context budget]", kept.trim_end())
    }
}

/// Assemble `base` + the knowledge `blocks` within `total_budget` chars.
///
/// `base` is always included in full and does not count against the budget.
/// Each block is capped at its own `max_chars` and at whatever of the shared
/// budget remains after higher-priority blocks have taken their share. A block
/// that can't fit fully is truncated only if at least [`MIN_USEFUL_CHARS`]
/// remain; otherwise it's dropped. Blocks render in the order supplied.
pub fn assemble(base: String, blocks: Vec<Block>, total_budget: usize) -> String {
    // Pass 1 — allocate the shared budget by priority. Stable sort so blocks of
    // equal priority keep their supplied order.
    let mut order: Vec<usize> = (0..blocks.len()).collect();
    order.sort_by_key(|&i| blocks[i].priority);
    let mut alloc = vec![0usize; blocks.len()];
    let mut remaining = total_budget;
    for &i in &order {
        let want = blocks[i].content.chars().count().min(blocks[i].max_chars);
        let give = if want <= remaining {
            want // fits in full
        } else if remaining >= MIN_USEFUL_CHARS {
            remaining // partial, but still worth including
        } else {
            0 // too little left to bother
        };
        alloc[i] = give;
        remaining -= give;
    }

    // Pass 2 — render in the original (supplied) order, each block capped at its
    // allocation.
    let mut out = base;
    for (i, b) in blocks.iter().enumerate() {
        if alloc[i] == 0 {
            continue;
        }
        let rendered = cap_chars(&b.content, alloc[i]);
        if rendered.trim().is_empty() {
            continue;
        }
        out.push_str("\n\n");
        out.push_str(&rendered);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(content: &str, priority: u8, max_chars: usize) -> Block {
        Block::new(content, priority, max_chars)
    }

    #[test]
    fn base_only_when_no_blocks() {
        assert_eq!(assemble("BASE".into(), vec![], 1000), "BASE");
    }

    #[test]
    fn under_budget_keeps_everything_in_order() {
        let out = assemble(
            "BASE".into(),
            vec![
                block("AAA", 1, 100),
                block("BBB", 0, 100),
                block("CCC", 2, 100),
            ],
            1000,
        );
        // Rendered in SUPPLIED order (A, B, C) despite priorities, none truncated.
        assert_eq!(out, "BASE\n\nAAA\n\nBBB\n\nCCC");
    }

    #[test]
    fn per_block_cap_truncates() {
        let out = assemble("BASE".into(), vec![block("abcdefghij", 0, 4)], 1000);
        assert!(out.contains("abcd"));
        assert!(out.contains("truncated"));
        assert!(!out.contains("efgh"));
    }

    #[test]
    fn total_budget_evicts_lowest_priority_first() {
        // Two 100-char blocks but only ~120 of total budget. The higher-priority
        // (lower number) block is kept full; the other is dropped (remaining <
        // MIN_USEFUL_CHARS after the first takes 100).
        let keep = "K".repeat(100);
        let drop = "D".repeat(100);
        let out = assemble(
            "BASE".into(),
            vec![
                block(&drop, 5, 1000), // low priority, supplied first
                block(&keep, 0, 1000), // high priority
            ],
            120,
        );
        assert!(out.contains(&keep), "high-priority block must survive");
        assert!(!out.contains("DDDDD"), "low-priority block must be evicted");
    }

    #[test]
    fn partial_block_kept_when_enough_remains() {
        let big = "x".repeat(1000);
        // Budget 500, one block — gets truncated to ~500 (>= MIN_USEFUL_CHARS).
        let out = assemble("BASE".into(), vec![block(&big, 0, 1000)], 500);
        assert!(out.contains("truncated"));
        let body_len = out.len() - "BASE\n\n".len();
        assert!(body_len > MIN_USEFUL_CHARS && body_len < 1000);
    }

    #[test]
    fn empty_blocks_skipped() {
        let out = assemble("BASE".into(), vec![block("   ", 0, 100), block("X", 1, 100)], 1000);
        assert_eq!(out, "BASE\n\nX");
    }
}
