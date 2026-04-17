//! Unified error types for the `specify` CLI and its domain crates.
//!
//! Every public function in a `specify-*` crate returns `Result<T, Error>`.
//! The variants are structured (rather than string-only) so `main.rs` can
//! pattern-match them to assign exit codes and pick an output format.

/// Compact summary of a validation result, embedded in `Error::Validation`.
///
/// The rich `ValidationResult` type lives in `specify-validate`; converting
/// to this summary is a lossy projection (the enum variant collapses into
/// a `status` string) but keeps `specify-error` dependency-free from the
/// rest of the workspace. See `DECISIONS.md` ("Change A — Error::Validation
/// payload") for the rationale.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationResultSummary {
    /// One of `"pass"`, `"fail"`, or `"deferred"`.
    pub status: String,
    /// Stable rule identifier (e.g. `proposal.why-has-content`).
    pub rule_id: String,
    /// Human-readable rule description.
    pub rule: String,
    /// Populated for `fail` (failure detail) and `deferred` (reason);
    /// `None` for `pass`.
    pub detail: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not initialized: .specify/project.yaml not found")]
    NotInitialized,

    #[error("schema resolution failed: {0}")]
    SchemaResolution(String),

    #[error("config error: {0}")]
    Config(String),

    #[error("validation failed: {count} errors")]
    Validation { count: usize, results: Vec<ValidationResultSummary> },

    #[error("merge failed: {0}")]
    Merge(String),

    #[error("lifecycle error: expected {expected}, found {found}")]
    Lifecycle { expected: String, found: String },

    #[error("specify version {found} is older than the project floor {required}; upgrade the CLI")]
    SpecifyVersionTooOld { required: String, found: String },

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Yaml(#[from] serde_yaml::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(status: &str) -> ValidationResultSummary {
        ValidationResultSummary {
            status: status.to_string(),
            rule_id: "rule.example".to_string(),
            rule: "Example rule".to_string(),
            detail: if status == "pass" { None } else { Some("detail".to_string()) },
        }
    }

    #[test]
    fn not_initialized_display() {
        let err = Error::NotInitialized;
        assert_eq!(err.to_string(), "not initialized: .specify/project.yaml not found");
    }

    #[test]
    fn schema_resolution_display() {
        let err = Error::SchemaResolution("boom".to_string());
        assert_eq!(err.to_string(), "schema resolution failed: boom");
    }

    #[test]
    fn config_display() {
        let err = Error::Config("bad key".to_string());
        assert_eq!(err.to_string(), "config error: bad key");
    }

    #[test]
    fn validation_display_and_payload() {
        let err = Error::Validation {
            count: 2,
            results: vec![summary("fail"), summary("deferred")],
        };
        assert_eq!(err.to_string(), "validation failed: 2 errors");
        if let Error::Validation { count, results } = err {
            assert_eq!(count, 2);
            assert_eq!(results.len(), 2);
            assert_eq!(results[0].status, "fail");
            assert_eq!(results[1].status, "deferred");
        } else {
            panic!("expected Validation variant");
        }
    }

    #[test]
    fn merge_display() {
        let err = Error::Merge("conflict".to_string());
        assert_eq!(err.to_string(), "merge failed: conflict");
    }

    #[test]
    fn lifecycle_display() {
        let err = Error::Lifecycle {
            expected: "Defining".to_string(),
            found: "Merged".to_string(),
        };
        assert_eq!(err.to_string(), "lifecycle error: expected Defining, found Merged");
    }

    #[test]
    fn specify_version_too_old_display() {
        let err = Error::SpecifyVersionTooOld {
            required: "0.2.0".to_string(),
            found: "0.1.0".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "specify version 0.1.0 is older than the project floor 0.2.0; upgrade the CLI"
        );
    }

    #[test]
    fn io_from_conversion() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err: Error = io.into();
        assert!(matches!(err, Error::Io(_)));
        assert_eq!(err.to_string(), "missing");
    }

    #[test]
    fn yaml_from_conversion() {
        let parse_err: serde_yaml::Error = serde_yaml::from_str::<serde_yaml::Value>(":\n\t- bad")
            .expect_err("expected a YAML parse error");
        let display = parse_err.to_string();
        let err: Error = parse_err.into();
        assert!(matches!(err, Error::Yaml(_)));
        assert_eq!(err.to_string(), display);
    }
}
