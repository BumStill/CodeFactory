// SPDX-License-Identifier: Apache-2.0
//! Heuristic test-file detection.
//!
//! Used by write_file / edit_file to surface a visible breadcrumb in the
//! tool output whenever a test file is touched. Combined with the system
//! prompt's "test-modification discipline" rules, this gives the model a
//! mandatory awareness signal: it sees "TEST_FILE_MODIFIED" in its own
//! tool result and is required by the prompt to acknowledge whether the
//! change was justified by a real test bug or by laziness.
//!
//! We don't HARD-BLOCK test edits — the user explicitly wants the model
//! free to correct genuinely-wrong tests. The pattern is "discipline by
//! reflection," not by lock.

use std::path::Path;

/// Returns true if `path` looks like a test file under one of the
/// well-known conventions across mainstream language ecosystems.
pub fn is_test_path(path: &Path) -> bool {
    // Normalise to forward slashes so the same matcher works on Windows.
    let s = path.to_string_lossy().replace('\\', "/").to_lowercase();

    // Whole-segment directories that scream "tests live here."
    for seg in ["/tests/", "/test/", "/__tests__/", "/spec/", "/specs/"] {
        if s.contains(seg) {
            return true;
        }
    }
    // Path starting with the directory (no leading slash).
    for prefix in ["tests/", "test/", "__tests__/", "spec/", "specs/"] {
        if s.starts_with(prefix) {
            return true;
        }
    }

    // Filename conventions.
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        let n = name.to_lowercase();
        // JS/TS: foo.test.ts, foo.spec.tsx, foo.test.jsx
        if n.ends_with(".test.ts")
            || n.ends_with(".test.tsx")
            || n.ends_with(".test.js")
            || n.ends_with(".test.jsx")
            || n.ends_with(".test.mjs")
            || n.ends_with(".spec.ts")
            || n.ends_with(".spec.tsx")
            || n.ends_with(".spec.js")
            || n.ends_with(".spec.jsx")
        {
            return true;
        }
        // Python: test_*.py, *_test.py
        if n.starts_with("test_") && n.ends_with(".py") {
            return true;
        }
        if n.ends_with("_test.py") {
            return true;
        }
        // Go: *_test.go
        if n.ends_with("_test.go") {
            return true;
        }
        // Rust integration tests: tests/<name>.rs is caught by the dir match
        // above; rust unit tests live in #[cfg(test)] mod inside source
        // files — those we don't try to detect (would be noisy).
        // Ruby: *_spec.rb
        if n.ends_with("_spec.rb") {
            return true;
        }
        // Java/Kotlin: *Test.java, *Tests.kt
        if n.ends_with("test.java") || n.ends_with("tests.java") {
            return true;
        }
    }

    false
}

/// Prefix the LLM sees in its tool result when it touched a test file.
/// Phrased to nudge — not threaten — the model into the discipline check.
pub const TEST_MODIFIED_BANNER: &str =
    "⚠ TEST_FILE_MODIFIED — Per the test-modification discipline in your \
     system prompt, your next message MUST either (a) explain why this test \
     was genuinely wrong (cite the spec or expected behaviour), or (b) revert \
     this change and fix the implementation instead. Editing tests purely to \
     make them pass is forbidden.\n";

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn detects_canonical_test_dirs() {
        assert!(is_test_path(&p("src/tests/foo.rs")));
        assert!(is_test_path(&p("tests/integration.rs")));
        assert!(is_test_path(&p("src/__tests__/Widget.test.tsx")));
        assert!(is_test_path(&p("packages/core/test/utils.test.js")));
        assert!(is_test_path(&p("spec/models/user_spec.rb")));
    }

    #[test]
    fn detects_filename_conventions() {
        assert!(is_test_path(&p("src/util.test.ts")));
        assert!(is_test_path(&p("src/util.spec.tsx")));
        assert!(is_test_path(&p("backend/test_models.py")));
        assert!(is_test_path(&p("backend/models_test.py")));
        assert!(is_test_path(&p("cmd/server/main_test.go")));
        assert!(is_test_path(&p("models/user_spec.rb")));
    }

    #[test]
    fn rejects_non_test_paths() {
        assert!(!is_test_path(&p("src/util.ts")));
        assert!(!is_test_path(&p("README.md")));
        assert!(!is_test_path(&p("src/components/Widget.tsx")));
        assert!(!is_test_path(&p("backend/models.py")));
        assert!(!is_test_path(&p("cmd/server/main.go")));
        // "testing" in the name shouldn't trigger — too loose
        assert!(!is_test_path(&p("src/testing-utils.ts")));
    }

    #[test]
    fn handles_windows_backslash_paths() {
        assert!(is_test_path(&p(r"src\tests\foo.rs")));
        assert!(is_test_path(&p(r"src\__tests__\Widget.test.tsx")));
    }
}
