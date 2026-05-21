// SPDX-License-Identifier: Apache-2.0
//! Catch hallucinated paths *before* they cause a wrong-file write.
//!
//! The user reported that the LLM sometimes invents paths that look plausible
//! but don't exist — `app/__iniy/`, `src/infrastru/`, `code/infracture/`.
//! These are usually typos or truncations of nearby real paths. If we just
//! pass them through to `write_file` the new file lands in the wrong place
//! and is hard to clean up later.
//!
//! This module looks at the target path before any IO:
//!   1. Walk up until we find the deepest existing ancestor.
//!   2. Identify the first segment that doesn't exist.
//!   3. Compare it (Levenshtein) against the real siblings of that ancestor.
//!   4. If the closest sibling is within `MAX_TYPO_DISTANCE` edits, return
//!      a suggestion — the tool aborts and tells the model the right path.
//!
//! Conservative thresholds keep false-positives low: distance ≤ 2 and the
//! candidate name must be ≥ 3 chars. Genuinely-new directory creation
//! (where the new name has no neighbour within 2 edits) sails through.

use std::path::{Path, PathBuf};

const MAX_TYPO_DISTANCE: usize = 2;
const MIN_NAME_LEN: usize = 3;

/// Result of a path-sanity check.
#[derive(Debug, Clone)]
pub struct TypoSuggestion {
    /// The first non-existent segment in the user-provided path.
    pub bad_segment: String,
    /// Closest existing sibling within edit-distance threshold.
    pub suggested_segment: String,
    /// Full corrected path (target with bad segment swapped for suggested).
    pub corrected_path: PathBuf,
    pub edit_distance: usize,
}

/// Return `Some(TypoSuggestion)` if the target's parent chain has a missing
/// segment that closely resembles an existing sibling. Otherwise `None` —
/// the path is either valid or genuinely new.
pub fn check(target: &Path) -> Option<TypoSuggestion> {
    let parent = target.parent()?;
    if parent.exists() {
        return None; // legit — creating a new file in an existing dir
    }

    // Walk up to the deepest existing ancestor.
    let mut ancestor = parent;
    loop {
        match ancestor.parent() {
            Some(p) => {
                if p.exists() {
                    break;
                }
                ancestor = p;
            }
            None => return None, // walked off the root, give up
        }
    }
    let real_ancestor = ancestor.parent()?; // parent of the first non-existent

    // The first missing segment relative to real_ancestor — that's what
    // we suspect is the typo.
    let rel = parent.strip_prefix(real_ancestor).ok()?;
    let bad_segment = rel.components().next()?.as_os_str().to_string_lossy().into_owned();

    if bad_segment.len() < MIN_NAME_LEN {
        return None;
    }

    // Find best match among real_ancestor's children.
    let entries = std::fs::read_dir(real_ancestor).ok()?;
    let mut best: Option<(String, usize)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.len() < MIN_NAME_LEN {
            continue;
        }
        let dist = levenshtein(&bad_segment, &name);
        if dist == 0 || dist > MAX_TYPO_DISTANCE {
            continue;
        }
        if best.as_ref().map_or(true, |(_, d)| dist < *d) {
            best = Some((name, dist));
        }
    }

    let (suggested_segment, edit_distance) = best?;

    // Reconstruct the corrected path: real_ancestor / suggested / rest…
    let mut corrected = real_ancestor.join(&suggested_segment);
    for c in rel.components().skip(1) {
        corrected.push(c.as_os_str());
    }
    if let Some(fname) = target.file_name() {
        corrected.push(fname);
    }

    Some(TypoSuggestion {
        bad_segment,
        suggested_segment,
        corrected_path: corrected,
        edit_distance,
    })
}

/// Format a suggestion as a user-actionable error message for the model.
pub fn format_error(s: &TypoSuggestion, original: &Path, tool_name: &str) -> String {
    format!(
        "{tool_name}: path '{}' looks like a typo — '{}' doesn't exist but '{}' does (edit distance {}). \
         Did you mean '{}'? Use list_files or read_file first to verify the real path, then retry.",
        original.display(),
        s.bad_segment,
        s.suggested_segment,
        s.edit_distance,
        s.corrected_path.display(),
    )
}

/// Classic dynamic-programming Levenshtein. Two short strings, allocation-free
/// would be possible but this is plenty fast for the ~10-30 char names we
/// compare against.
fn levenshtein(a: &str, b: &str) -> usize {
    let av: Vec<char> = a.chars().collect();
    let bv: Vec<char> = b.chars().collect();
    let m = av.len();
    let n = bv.len();
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];
    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if av[i - 1] == bv[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min((curr[j - 1] + 1).min(prev[j - 1] + cost));
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein("__init__", "__iniy"), 3);  // del, sub, del? Let's count carefully
        assert_eq!(levenshtein("infrastructure", "infrastru"), 5);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("abc", "abc"), 0);
    }
}
