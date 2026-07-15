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

/// Build the full bundle (preferences + learnings + memory.md) as a single
/// human-readable block. Empty string if nothing to inject. Used by the spec
/// workbench, which has no other channel for memory.md.
pub async fn build(pool: &SqlitePool, cwd: &str) -> String {
    let mut sections = collect_prefs_and_learnings(pool, cwd).await;
    if let Some(memory) = build_memory_section(cwd) {
        sections.push(memory);
    }
    wrap_sections(sections)
}

/// Preferences + accepted learnings only — for the main chat/agent loop, which
/// already injects `.codefactory/memory.md` via `build_system_prompt_for`. We
/// deliberately skip the memory section here so it isn't duplicated. Empty
/// string if there's nothing to add.
pub async fn build_prefs_and_learnings(pool: &SqlitePool, cwd: &str) -> String {
    wrap_sections(collect_prefs_and_learnings(pool, cwd).await)
}

async fn collect_prefs_and_learnings(pool: &SqlitePool, cwd: &str) -> Vec<String> {
    let mut sections: Vec<String> = Vec::new();
    if let Some(prefs) = build_preferences_section(pool, cwd).await {
        sections.push(prefs);
    }
    if let Some(learnings) = build_learnings_section(pool, cwd).await {
        sections.push(learnings);
    }
    sections
}

fn wrap_sections(sections: Vec<String>) -> String {
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
    use crate::commands::preferences::GLOBAL_CWD;
    use std::collections::HashMap;

    // Two-tier resolution: global defaults + per-project overrides.
    // Project wins on key conflicts so users can override globals locally
    // (e.g. "I want TDD everywhere, but in this experimental repo I don't").
    let global: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM user_preferences WHERE cwd = ? ORDER BY key")
            .bind(GLOBAL_CWD)
            .fetch_all(pool)
            .await
            .ok()?;

    let project: Vec<(String, String)> =
        sqlx::query_as("SELECT key, value FROM user_preferences WHERE cwd = ? ORDER BY key")
            .bind(cwd)
            .fetch_all(pool)
            .await
            .ok()?;

    // Merge: start with global, override with project entries.
    let mut merged: HashMap<String, String> = HashMap::new();
    for (k, v) in global.into_iter().chain(project.into_iter()) {
        merged.insert(k, v);
    }

    let mut sorted: Vec<(String, String)> = merged
        .into_iter()
        .filter(|(_, v)| !v.trim().is_empty())
        .collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    if sorted.is_empty() {
        None
    } else {
        let lines: Vec<String> = sorted
            .iter()
            .map(|(k, v)| format!("- {}: {}", k, v))
            .collect();
        Some(format!("### Preferences\n{}", lines.join("\n")))
    }
}

async fn build_learnings_section(pool: &SqlitePool, cwd: &str) -> Option<String> {
    // New Phase 4 memory is inert until an exact Eval-passed activation
    // receipt exists. Active versioned memory takes precedence within the
    // bounded prompt budget; rolled-back rows remain auditable but inert.
    let mut rows: Vec<(String,)> = sqlx::query_as(
        "SELECT content FROM evolution_active_memory
         WHERE cwd=? AND active=1 ORDER BY activated_at DESC LIMIT ?",
    )
    .bind(cwd)
    .bind(LEARNING_EVENTS_LIMIT)
    .fetch_all(pool)
    .await
    .ok()?;
    let remaining = LEARNING_EVENTS_LIMIT.saturating_sub(rows.len() as i64);
    if remaining > 0 {
        let legacy: Vec<(String,)> = sqlx::query_as(
            "SELECT suggestion FROM learning_events \
             WHERE cwd = ? AND status = 'accepted' \
             ORDER BY decided_at DESC LIMIT ?",
        )
        .bind(cwd)
        .bind(remaining)
        .fetch_all(pool)
        .await
        .ok()?;
        rows.extend(legacy);
    }
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
    Some(format!(
        "### Project memory (.codefactory/memory.md)\n{}",
        snippet
    ))
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
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE learning_events (
                id TEXT PRIMARY KEY, session_id TEXT, cwd TEXT,
                observation TEXT, suggestion TEXT, status TEXT,
                created_at TEXT, decided_at TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE evolution_active_memory (
                candidate_id TEXT PRIMARY KEY, cwd TEXT, revision INTEGER,
                activation_id TEXT, content TEXT, content_hash TEXT,
                active INTEGER, activated_at TEXT, rolled_back_at TEXT
            )",
        )
        .execute(&pool)
        .await
        .unwrap();
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
        assert!(
            s.contains("### Preferences"),
            "expected preferences section, got: {s}"
        );
        assert!(s.contains("- autonomy_level: high"));
    }

    #[tokio::test]
    async fn empty_preference_values_skipped() {
        let pool = fresh_pool().await;
        sqlx::query(
            "INSERT INTO user_preferences VALUES ('/proj','code_style','','default','2026-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let s = build(&pool, "/proj").await;
        assert_eq!(s, "", "empty values should produce no preferences section");
    }

    #[tokio::test]
    async fn project_preference_overrides_global() {
        let pool = fresh_pool().await;
        // Global says concise; project says verbose. Project must win.
        sqlx::query(
            "INSERT INTO user_preferences VALUES \
             ('_global_','communication_style','concise','user','2026-01-01'), \
             ('_global_','autonomy_level','low','user','2026-01-01'), \
             ('/proj','communication_style','verbose','user','2026-01-02')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let s = build(&pool, "/proj").await;
        assert!(
            s.contains("communication_style: verbose"),
            "project should override global, got: {s}"
        );
        assert!(
            !s.contains("communication_style: concise"),
            "global must not leak through, got: {s}"
        );
        // Non-conflicting global still appears
        assert!(
            s.contains("autonomy_level: low"),
            "non-conflicting global must inherit, got: {s}"
        );
    }

    #[tokio::test]
    async fn global_only_works_without_project_prefs() {
        let pool = fresh_pool().await;
        sqlx::query(
            "INSERT INTO user_preferences VALUES \
             ('_global_','testing_habit','tdd','user','2026-01-01')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let s = build(&pool, "/proj").await;
        assert!(s.contains("testing_habit: tdd"));
    }

    #[tokio::test]
    async fn only_accepted_learnings_included() {
        let pool = fresh_pool().await;
        for (id, status, sug) in [
            ("a", "accepted", "always add empty-array test"),
            ("b", "pending", "ignore me"),
            ("c", "rejected", "ignore me too"),
        ] {
            sqlx::query("INSERT INTO learning_events VALUES (?,?,?,?,?,?,?,?)")
                .bind(id)
                .bind("s1")
                .bind("/proj")
                .bind("obs")
                .bind(sug)
                .bind(status)
                .bind("2026-01-01")
                .bind("2026-01-02")
                .execute(&pool)
                .await
                .unwrap();
        }
        let s = build(&pool, "/proj").await;
        assert!(s.contains("always add empty-array test"));
        assert!(!s.contains("ignore me"));
    }

    #[tokio::test]
    async fn prefs_and_learnings_emits_prefs_without_memory_section() {
        let pool = fresh_pool().await;
        sqlx::query(
            "INSERT INTO user_preferences VALUES ('/proj','autonomy_level','high','user','2026-01-01')",
        )
        .execute(&pool).await.unwrap();
        let s = build_prefs_and_learnings(&pool, "/proj").await;
        assert!(
            s.contains("### Preferences"),
            "expected preferences, got: {s}"
        );
        assert!(s.contains("- autonomy_level: high"));
        // The chat variant must never carry the memory section (it's injected
        // separately by build_system_prompt_for and would otherwise duplicate).
        assert!(
            !s.contains("Project memory"),
            "memory must be excluded, got: {s}"
        );
    }

    #[tokio::test]
    async fn prefs_and_learnings_empty_when_nothing_set() {
        let pool = fresh_pool().await;
        let s = build_prefs_and_learnings(&pool, "/nonexistent/proj").await;
        assert_eq!(s, "");
    }

    #[tokio::test]
    async fn only_active_phase4_memory_is_injected_and_rollback_removes_it() {
        let pool = fresh_pool().await;
        sqlx::query(
            "INSERT INTO evolution_active_memory
             (candidate_id,cwd,revision,activation_id,content,content_hash,active,activated_at)
             VALUES ('c1','/proj',1,'a1','active evolution memory','hash',1,'2026-07-15'),
                    ('c2','/proj',1,'a2','rolled back memory','hash2',0,'2026-07-15')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let context = build_prefs_and_learnings(&pool, "/proj").await;
        assert!(context.contains("active evolution memory"));
        assert!(!context.contains("rolled back memory"));
        sqlx::query("UPDATE evolution_active_memory SET active=0 WHERE candidate_id='c1'")
            .execute(&pool)
            .await
            .unwrap();
        let after = build_prefs_and_learnings(&pool, "/proj").await;
        assert!(!after.contains("active evolution memory"));
    }

    #[tokio::test]
    async fn active_versioned_memory_is_not_starved_by_legacy_prompt_limit() {
        let pool = fresh_pool().await;
        for index in 0..LEARNING_EVENTS_LIMIT {
            sqlx::query(
                 "INSERT INTO learning_events
                 (id,session_id,cwd,observation,suggestion,status,created_at,decided_at)
                 VALUES (?, 's1', '/proj', 'obs', ?, 'accepted', ?, ?)",
            )
            .bind(format!("legacy-{index}"))
            .bind(format!("legacy memory {index}"))
            .bind(format!("2026-07-{:02}", index + 1))
            .bind(format!("2026-07-{:02}", index + 1))
            .execute(&pool)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO evolution_active_memory
             (candidate_id,cwd,revision,activation_id,content,content_hash,active,activated_at)
             VALUES ('active-priority','/proj',1,'a-priority','priority active memory','hash',1,'2026-08-01')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let context = build_prefs_and_learnings(&pool, "/proj").await;
        assert!(context.contains("priority active memory"));
        assert_eq!(context.matches("- legacy memory").count(), 19);
    }
}
