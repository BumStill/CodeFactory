// SPDX-License-Identifier: Apache-2.0
//! M285 regression tests colocated with the private delivery modules.

use crate::config::settings::DeliveryCeiling;
use super::delivery_m285_policy::{
    effective_delivery_ceiling, existing_pr_merge_refusal, preserve_existing_pr_content,
};

#[test]
fn effective_ceiling_is_the_lower_authorized_and_requested_value() {
    assert_eq!(effective_delivery_ceiling(DeliveryCeiling::ThroughRelease, Some(DeliveryCeiling::PrOnly)), DeliveryCeiling::PrOnly);
    assert_eq!(effective_delivery_ceiling(DeliveryCeiling::PrOnly, Some(DeliveryCeiling::ThroughRelease)), DeliveryCeiling::PrOnly);
}

#[test]
fn draft_and_removed_arm_are_readable_revocations() {
    let draft = existing_pr_merge_refusal(true, true, false, DeliveryCeiling::ThroughMerge)
        .expect("draft PR must refuse merge authorization");
    assert!(draft.contains("草稿") && draft.contains("撤回"), "{draft}");
    let removed = existing_pr_merge_refusal(false, true, false, DeliveryCeiling::ThroughMerge)
        .expect("removed arm must refuse after restart");
    assert!(removed.contains("摘掉") && removed.contains("明确授权"), "{removed}");
    assert!(existing_pr_merge_refusal(false, false, false, DeliveryCeiling::PrOnly).is_none());
}

#[test]
fn recovery_preserves_remote_title_body_and_requirement_table() {
    let remote_title = "fix: maintainer title";
    let remote_body = "Summary\n\nR | 是否完成 | 证据\nR1 | 是 | synthetic";
    let (title, body) = preserve_existing_pr_content(remote_title, remote_body);
    assert_eq!(title, remote_title);
    assert_eq!(body, remote_body);
    assert!(body.contains("R | 是否完成 | 证据"));
}
