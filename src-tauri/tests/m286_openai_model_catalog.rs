// SPDX-License-Identifier: Apache-2.0
//! M286 specification-presence smoke test. Behavioral tests live beside the Rust modules.

#[test]
fn m286_spec_tracks_all_requirements() {
    let spec = include_str!("../../docs/specs/feature-specs/m286-openai-api-model-catalog.md");
    for requirement in 1..=9 {
        assert!(
            spec.contains(&format!("R{requirement}：")),
            "M286 spec must retain R{requirement}"
        );
    }
    assert!(spec.contains("Release-Urgency: default"));
}
