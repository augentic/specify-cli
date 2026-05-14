//! Phase-outcome discriminant. The wire-format wrapper that pairs an
//! [`Kind`] with phase + timestamp metadata lives in
//! [`crate::slice::metadata`] as `Outcome`.

use serde::{Deserialize, Serialize};

/// Phase outcome reported to `/change:execute`. Unit variants serialise
/// as `outcome: success` etc.; [`Self::RegistryAmendmentRequired`] is
/// an externally-tagged map carrying its proposal payload.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, strum::Display)]
#[serde(rename_all = "kebab-case", rename_all_fields = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
#[non_exhaustive]
pub enum Kind {
    /// Phase completed successfully.
    Success,
    /// Phase failed.
    Failure,
    /// Phase deferred (needs human input).
    Deferred,
    /// Phase blocked pending a registry amendment. `/change:execute`
    /// treats this like `deferred` and surfaces the proposal payload to
    /// the operator.
    RegistryAmendmentRequired {
        /// Kebab-case project name proposed for the registry.
        proposed_name: String,
        /// Clone URL for the proposed project (git remote / ssh /
        /// http(s) / `git+...`). Same shape rules as
        /// `specify registry add --url`.
        proposed_url: String,
        /// Capability identifier (e.g. `omnia@v1`).
        proposed_capability: String,
        /// Optional human-readable description of the proposed project.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        proposed_description: Option<String>,
        /// Free-form rationale, surfaced verbatim to the operator.
        rationale: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_display_matches_serde() {
        assert_eq!(Kind::Success.to_string(), "success");
        assert_eq!(Kind::Failure.to_string(), "failure");
        assert_eq!(Kind::Deferred.to_string(), "deferred");
        let proposal = Kind::RegistryAmendmentRequired {
            proposed_name: "alpha-gateway".to_string(),
            proposed_url: "git@github.com:augentic/alpha-gateway.git".to_string(),
            proposed_capability: "omnia@v1".to_string(),
            proposed_description: None,
            rationale: "build discovered tangled code".to_string(),
        };
        assert_eq!(proposal.to_string(), "registry-amendment-required");
    }
}
