// SPDX-License-Identifier: Apache-2.0
//! User context bundle — the single source of truth for "what the AI
//! knows about this user and project". Built fresh per AI call so
//! recent memory edits and learning acceptances reflect immediately.
//!
//! Composition:
//!   1. Structured preferences  (typed key-value, surfaced in Profile UI)
//!   2. Accepted learning events (suggestions the user approved)
//!   3. Free-form memory.md     (anything the user wrote / accepted)
//!   4. Live interjections      (transient mid-session redirections)
//!
//! Token economy: the whole bundle is capped at ~2KB by design. Memory.md
//! is the only unbounded source and is trimmed at 4000 chars; learning
//! events at 20 most recent; preferences at all of them (small).
//!
//! Callers should treat the returned string as a *single block* to
//! prepend or wrap into a system-prompt section — no parsing assumed.

use sqlx::SqlitePool;
use std::path::PathBuf;

const MEMORY_CHAR_CAP: usize = 4000;
const LEARNING_EVENTS_LIMIT: i64 = 20;

/// Build the bundle as a single human-readable block. Empty string if
/// nothing to inject (caller can skip the section entirely).
pub async fn build(pool: &SqlitePool, cwd: &str) -> String {
    let mut sections: Vec<String> = Vec::new();

    if let Some(prefs) = build_preferences_section(pool, cwd).await {
        sections.push(prefs);
    }
    if let Some(learnings) = build_learnings_section(pool, cwd).await {
        sections.push(learnings);
    }
    if let Some(memory) = build_memory_section(cwd) {
        sections.push(memory);
    }

    if sections.is_empty() {
        String::new()
    } else {
        format!(
            "## What we know about this user / project\n\n{}\n",
            sections.join("\n\n")
        )
    }
}

async fn build_preferences_section(pool: &SqlitePool, cwd: &str) -> Option<String> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT key, value FROM user_preferences WHERE cwd = ? ORDER BY key",
    )
    .bind(cwd)
    .fetch_all(pool)
    .await
    .ok()?;
    let lines: Vec<String> = rows
        .into_iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .map(|(k, v)| format!("- {}: {}", k, v))
        .collect();
    if lines.is_empty() {
        None
    } else {
        Some(format!("### Preferences\n{}", lines.join("\n")))
    }
}

async fn build_learnings_section(pool: &SqlitePool, cwd: &str) -> Option<String> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT suggestion FROM learning_events \
         WHERE cwd = ? AND status = 'accepted' \
         ORDER BY decided_at DESC LIMIT ?",
    )
    .bind(cwd)
    .bind(LEARNING_EVENTS_LIMIT)
    .fetch_all(pool)
    .await
    .ok()?;
    if rows.is_empty() {
        return None;
    }
    let lines: Vec<String> = rows
        .into_iter()
        .map(|(s,)| format!("- {}", s.trim()))
        .collect();
    Some(format!("### Accepted learnings\n{}", lines.join("\n")))
}

fn build_memory_section(cwd: &str) -> Option<String> {
    let path = PathBuf::from(cwd).join(".codefactory").join("memory.md");
    let content = std::fs::read_to_string(&path).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Memory.md is unbounded; cap at MEMORY_CHAR_CAP chars from the END
    // so the most-recently-appended facts always make it in.
    let snippet = if trimmed.len() > MEMORY_CHAR_CAP {
        let start = trimmed.len() - MEMORY_CHAR_CAP;
        format!("…(truncated)…\n{}", &trimmed[start..])
    } else {
        trimmed.to_string()
    };
    Some(format!("### Project memory (.codefactory/memory.md)\n{}", snippet))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn fresh_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(":memory:")
            .await
            .unwrap();
        sqlx::query(
            "CREATE TABLE user_preferences (
                cwd TEXT, key TEXT, value TEXT, source TEXT, updated_at TEXT,
                PRIMARY KEY (cwd, key)
            )",
        )
        .execute(&pool).await.unwrap();
        sqlx::query(
            "CREATE TABLE learning_events (
                id TEXT PRIMARY KEY, session_id TEXT, cwd TEXT,
                observation TEXT, suggestion TEXT, status TEXT,
                created_at TEXT, decided_at TEXT
            )",
        )
        .execute(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn empty_returns_empty_string() {
        let pool = fresh_pool().await;
        // Use a path that definitely has no memory.md
        let s = build(&pool, "/nonexistent/proj").await;
        assert_eq!(s, "");
    }

    #[tokio::test]
    async fn preferences_section_emitted_when_set() {
        let pool = fresh_pool().await;
        sqlx::query(
            "INSERT INTO user_preferences VALUES ('/proj','autonomy_level','high','user','2026-01-01')",
        )
        .execute(&pool).await.unwrap();
        let s = build(&pool, "/proj").await;
        assert!(s.contains("### Preferences"), "expected preferences section, got: {s}");
        assert!(s.contains("- autonomy_level: high"));
    }

    #[tokio::test]
    async fn empty_preference_values_skipped() {
        let pool = fresh_pool().await;
        sqlx::query(
            "INSERT INTO user_preferences VALUES ('/proj','code_style','','default','2026-01-01')",
        )
        .execute(&pool).await.unwrap();
        let s = build(&pool, "/proj").await;
        assert_eq!(s, "", "empty values should produce no preferences section");
    }

    #[tokio::test]
    async fn only_accepted_learnings_included() {
        let pool = fresh_pool().await;
        for (id, status, sug) in [
            ("a", "accepted", "always add empty-array test"),
            ("b", "pending",  "ignore me"),
            ("c", "rejected", "ignore me too"),
        ] {
            sqlx::query("INSERT INTO learning_events VALUES (?,?,?,?,?,?,?,?)")
                .bind(id).bind("s1").bind("/proj")
                .bind("obs").bind(sug).bind(status)
                .bind("2026-01-01").bind("2026-01-02")
                .execute(&pool).await.unwrap();
        }
        let s = build(&pool, "/proj").await;
        assert!(s.contains("always add empty-array test"));
        assert!(!s.contains("ignore me"));
    }
}
