// SPDX-License-Identifier: Apache-2.0
//! M285 named acceptance target. Behaviour assertions live beside the private
//! delivery modules; this target proves the production entry is wired to them.

#[test]
fn m285_delivery_entry_is_wired_to_ceiling_and_draft_guards() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let modules = std::fs::read_to_string(root.join("src/tools/mod.rs")).unwrap();
    let wrapper = std::fs::read_to_string(root.join("src/tools/delivery_thin_m285.rs")).unwrap();
    assert!(modules.contains("#[path = \"delivery_thin_m285.rs\"]\nmod delivery_thin;"));
    assert!(wrapper.contains("effective_delivery_ceiling(authorized_ceiling, requested)"));
    assert!(wrapper.contains("existing_pr_merge_refusal("));
    assert!(wrapper.contains("number,title,body,isDraft,labels"));
    assert!(wrapper.contains("preserve_existing_pr_content(title, body)"));
}
