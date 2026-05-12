//! Validation outcome enum + summary used by [`crate::Error::Validation`].

/// Validation outcome for a single rule check.
#[derive(Debug, Copy, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// Rule passed.
    Pass,
    /// Rule failed.
    Fail,
    /// CLI defers judgment to the agent.
    Deferred,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pass => f.write_str("pass"),
            Self::Fail => f.write_str("fail"),
            Self::Deferred => f.write_str("deferred"),
        }
    }
}

/// Compact summary of a validation result, embedded in `Error::Validation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// Outcome of this validation check.
    pub status: Status,
    /// Stable rule identifier (e.g. `proposal.why-has-content`).
    pub rule_id: String,
    /// Human-readable rule description.
    pub rule: String,
    /// Populated for `fail` (failure detail) and `deferred` (reason);
    /// `None` for `pass`.
    pub detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{Status, Summary};
    use crate::Error;

    fn summary(status: Status) -> Summary {
        Summary {
            status,
            rule_id: "rule.example".to_string(),
            rule: "Example rule".to_string(),
            detail: if status == Status::Pass { None } else { Some("detail".to_string()) },
        }
    }

    #[test]
    fn validation_payload_round_trips() {
        let err = Error::Validation {
            results: vec![summary(Status::Fail), summary(Status::Deferred)],
        };
        assert_eq!(err.to_string(), "validation failed: 2 errors");
        let Error::Validation { results } = err else {
            panic!("expected Validation variant");
        };
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].status, Status::Fail);
        assert_eq!(results[1].status, Status::Deferred);
    }
}
