// SPDX-License-Identifier: Apache-2.0
//! Auditable M285 incident provenance captured from read-only session records.

pub(crate) const INCIDENT_PROVENANCE: &str = "bd4ba210 used thin delivery; sessions.delivery_authorized=1 with no session ceiling; thin_delivery_refs persisted PR #1075 as ThroughRelease; continuation dispatches retained boolean delivery authorization but did not persist draft/armed revocation";

#[cfg(test)]
mod tests {
    use super::INCIDENT_PROVENANCE;

    #[test]
    fn provenance_names_the_observed_authorization_sources() {
        assert!(INCIDENT_PROVENANCE.contains("thin delivery"));
        assert!(INCIDENT_PROVENANCE.contains("no session ceiling"));
        assert!(INCIDENT_PROVENANCE.contains("ThroughRelease"));
        assert!(INCIDENT_PROVENANCE.contains("draft/armed revocation"));
    }
}
