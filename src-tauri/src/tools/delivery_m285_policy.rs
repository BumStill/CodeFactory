// SPDX-License-Identifier: Apache-2.0
use crate::config::settings::DeliveryCeiling;

pub(crate) fn effective_delivery_ceiling(
    authorized: DeliveryCeiling,
    requested: Option<DeliveryCeiling>,
) -> DeliveryCeiling {
    requested
        .map(|value| authorized.clamp_request(value))
        .unwrap_or(authorized)
}

pub(crate) fn ceiling_was_merge_authorized(label: &str) -> bool {
    matches!(
        label,
        "ThroughMerge" | "ThroughRelease" | "through_merge" | "through_release"
    )
}

pub(crate) fn ceiling_records_revocation(label: &str) -> bool {
    label.starts_with("revoked:")
}

pub(crate) fn existing_pr_merge_refusal(
    is_draft: bool,
    was_merge_authorized: bool,
    is_armed: bool,
    ceiling: DeliveryCeiling,
) -> Option<String> {
    if ceiling.rank() < DeliveryCeiling::ThroughMerge.rank() {
        return None;
    }
    if is_draft {
        return Some(format!(
            "PR 当前是草稿；这表示用户已撤回合并授权。当前上限为 {}，未转为 ready、未打 merge-queue: armed。需要用户新的明确合并授权。",
            ceiling.label()
        ));
    }
    if was_merge_authorized && !is_armed {
        return Some(format!(
            "PR 的 merge-queue: armed 已被用户或外部操作者摘掉；合并授权已撤回并跨重启保持。当前上限为 {}，需要用户新的明确授权，系统不会反复重试打标。",
            ceiling.label()
        ));
    }
    None
}

pub(crate) fn preserve_existing_pr_content<'a>(
    remote_title: &'a str,
    remote_body: &'a str,
) -> (&'a str, &'a str) {
    (remote_title, remote_body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_ceiling_is_the_lower_value() {
        assert_eq!(effective_delivery_ceiling(DeliveryCeiling::ThroughRelease, Some(DeliveryCeiling::PrOnly)), DeliveryCeiling::PrOnly);
        assert_eq!(effective_delivery_ceiling(DeliveryCeiling::PrOnly, Some(DeliveryCeiling::ThroughRelease)), DeliveryCeiling::PrOnly);
    }

    #[test]
    fn draft_and_removed_arm_are_readable_revocations() {
        let draft = existing_pr_merge_refusal(true, true, false, DeliveryCeiling::ThroughMerge).unwrap();
        assert!(draft.contains("草稿") && draft.contains("撤回"), "{draft}");
        let removed = existing_pr_merge_refusal(false, true, false, DeliveryCeiling::ThroughMerge).unwrap();
        assert!(removed.contains("摘掉") && removed.contains("明确授权"), "{removed}");
        assert!(existing_pr_merge_refusal(false, false, false, DeliveryCeiling::PrOnly).is_none());
    }

    #[test]
    fn revocation_marker_is_distinct_from_prior_authorization() {
        assert!(ceiling_was_merge_authorized("ThroughRelease"));
        assert!(!ceiling_was_merge_authorized("revoked:ThroughRelease"));
        assert!(ceiling_records_revocation("revoked:ThroughRelease"));
    }

    #[test]
    fn recovery_keeps_requirement_table() {
        let body = "Summary\n\nR | 是否完成 | 证据\nR1 | 是 | synthetic";
        let (title, kept) = preserve_existing_pr_content("fix: maintainer title", body);
        assert_eq!(title, "fix: maintainer title");
        assert_eq!(kept, body);
    }
}
