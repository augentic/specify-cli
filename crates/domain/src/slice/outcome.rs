//! Phase-outcome discriminant. The wire-format wrapper that pairs an
//! [`Kind`] with phase + timestamp metadata lives in
//! [`crate::slice::metadata`] as `Outcome`.

use serde::{Deserialize, Serialize};

/// Phase outcome reported to `/change:execute`. Unit variants serialise
/// as `outcome: success` etc.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Kind {
    /// Phase completed successfully.
    Success,
    /// Phase failed.
    Failure,
    /// Phase deferred (needs human input).
    Deferred,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_display_matches_serde() {
        assert_eq!(Kind::Success.to_string(), "success");
        assert_eq!(Kind::Failure.to_string(), "failure");
        assert_eq!(Kind::Deferred.to_string(), "deferred");
    }
}
