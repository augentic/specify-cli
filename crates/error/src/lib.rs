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

    /// Illegal plan entry status transition. Mirrors the `Lifecycle`
    /// variant in carrying stringified `PlanStatus` values to keep
    /// `specify-error` at the root of the dependency graph. The caller
    /// (in `specify-change::plan`) formats the strings via
    /// `format!("{:?}", status)`.
    #[error("illegal plan transition: cannot go from {from} to {to}")]
    PlanTransition { from: String, to: String },

    /// `Plan::archive` refused to archive a plan that still contains
    /// non-terminal entries, and the caller did not pass `force`.
    /// `entries` lists the offending entry names.
    #[error("plan has outstanding non-terminal work: {entries:?}")]
    PlanHasOutstandingWork { entries: Vec<String> },

    /// A `planChange.scope` entry violated the `manifest` XOR
    /// `(include|exclude)` invariant enforced by RFC-3a §*The `scope`
    /// field*. Surfaced both by programmatic constructors
    /// (`Scope::try_new`) and by the `serde` `try_from` deserialization
    /// hook (in which case the message gets wrapped into
    /// `serde_yaml::Error` and re-surfaced as [`Error::Yaml`] at the
    /// outer call site). The payload is a human-readable explanation
    /// naming the offending field combination.
    #[error("invalid plan scope: {0}")]
    InvalidPlanScope(String),

    /// A `planChange.scope` map carried a key that does not appear in
    /// the entry's declared `sources` list. Surfaced pre-write by the
    /// CLI's `specify initiative create`/`amend` paths (via the
    /// post-mutation `Plan::validate` sweep in
    /// `specify-change::plan`) so a plan with an orphan scope key
    /// never hits disk, and by `specify-validate` (RFC-3a §*How
    /// `scope` travels through the pipeline*) at validation time.
    /// The stable diagnostic ID on the wire is
    /// `scope-key-not-in-sources`; see `emit_json_error` in
    /// `main.rs`.
    #[error("scope key '{key}' on change '{change}' is not declared in sources")]
    InvalidPlanScopeKey { change: String, key: String },

    /// `PlanLockGuard::acquire` found another live `/spec:execute`
    /// driver holding `.specify/plan.lock`. `pid` is the contents of
    /// the lockfile (confirmed alive via the host-level PID check).
    /// Stale locks (dead PID / malformed content) are reclaimed
    /// silently and do not surface this variant.
    #[error("another /spec:execute driver is running (pid {pid}); refusing to proceed")]
    DriverBusy { pid: u32 },

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
    fn plan_transition_display() {
        let err = Error::PlanTransition {
            from: "Done".to_string(),
            to: "InProgress".to_string(),
        };
        assert_eq!(err.to_string(), "illegal plan transition: cannot go from Done to InProgress");
    }

    #[test]
    fn plan_has_outstanding_work_display() {
        let err = Error::PlanHasOutstandingWork {
            entries: vec!["checkout-api".to_string(), "checkout-ui".to_string()],
        };
        assert_eq!(
            err.to_string(),
            "plan has outstanding non-terminal work: [\"checkout-api\", \"checkout-ui\"]"
        );
    }

    #[test]
    fn invalid_plan_scope_key_display() {
        let err = Error::InvalidPlanScopeKey {
            change: "ingest-pipeline".to_string(),
            key: "orders".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "scope key 'orders' on change 'ingest-pipeline' is not declared in sources"
        );
    }

    #[test]
    fn driver_busy_display() {
        let err = Error::DriverBusy { pid: 4242 };
        assert_eq!(
            err.to_string(),
            "another /spec:execute driver is running (pid 4242); refusing to proceed"
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
