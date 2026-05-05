//! Unified error types for the `specify` CLI and its domain crates.
//!
//! Every public function in a `specify-*` crate returns `Result<T, Error>`.
//! The variants are structured (rather than string-only) so `main.rs` can
//! pattern-match them to assign exit codes and pick an output format.

/// Validation outcome for a single rule check.
///
/// Kept in `specify-error` (rather than `specify-validate`) so the
/// `Error::Validation` payload stays dependency-free. See `DECISIONS.md`
/// ("Change A") for the rationale.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ValidationStatus {
    /// Rule passed.
    Pass,
    /// Rule failed.
    Fail,
    /// CLI defers judgment to the agent.
    Deferred,
}

impl std::fmt::Display for ValidationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pass => f.write_str("pass"),
            Self::Fail => f.write_str("fail"),
            Self::Deferred => f.write_str("deferred"),
        }
    }
}

/// Compact summary of a validation result, embedded in `Error::Validation`.
///
/// The rich `ValidationResult` type lives in `specify-validate`; converting
/// to this summary is a lossy projection but keeps `specify-error`
/// dependency-free from the rest of the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationSummary {
    /// Outcome of this validation check.
    pub status: ValidationStatus,
    /// Stable rule identifier (e.g. `proposal.why-has-content`).
    pub rule_id: String,
    /// Human-readable rule description.
    pub rule: String,
    /// Populated for `fail` (failure detail) and `deferred` (reason);
    /// `None` for `pass`.
    pub detail: Option<String>,
}

/// Structured error type for all `specify-*` crates.
///
/// Variants carry enough context for the CLI to assign exit codes and
/// choose an output format without string-parsing.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The `.specify/project.yaml` file is missing.
    #[error("not initialized: .specify/project.yaml not found")]
    NotInitialized,

    /// Schema resolution failed with the given reason.
    #[error("schema resolution failed: {0}")]
    SchemaResolution(String),

    /// A configuration or input error.
    #[error("config error: {0}")]
    Config(String),

    /// Validation failed with one or more findings.
    #[error("validation failed: {count} errors")]
    Validation {
        /// Number of error-level findings.
        count: usize,
        /// Individual validation results.
        results: Vec<ValidationSummary>,
    },

    /// Spec merge failed.
    #[error("merge failed: {0}")]
    Merge(String),

    /// An illegal lifecycle transition was attempted.
    #[error("lifecycle error: expected {expected}, found {found}")]
    Lifecycle {
        /// Description of the expected state or transition.
        expected: String,
        /// The actual state encountered.
        found: String,
    },

    /// The installed CLI version is older than the project floor.
    #[error("specify version {found} is older than the project floor {required}; upgrade the CLI")]
    SpecifyVersionTooOld {
        /// Minimum version the project requires.
        required: String,
        /// Version currently installed.
        found: String,
    },

    /// Illegal plan entry status transition. Mirrors the `Lifecycle`
    /// variant in carrying stringified `PlanStatus` values to keep
    /// `specify-error` at the root of the dependency graph. The caller
    /// (in `specify-change::plan`) formats the strings via
    /// `format!("{:?}", status)`.
    #[error("illegal plan transition: cannot go from {from} to {to}")]
    PlanTransition {
        /// Source status of the attempted transition.
        from: String,
        /// Target status of the attempted transition.
        to: String,
    },

    /// `Plan::archive` refused to archive a plan that still contains
    /// non-terminal entries, and the caller did not pass `force`.
    /// `entries` lists the offending entry names.
    #[error("plan has outstanding non-terminal work: {entries:?}")]
    PlanHasOutstandingWork {
        /// Names of plan entries not yet in a terminal state.
        entries: Vec<String>,
    },

    /// `PlanLockGuard::acquire` found another live `/spec:execute`
    /// driver holding `.specify/plan.lock`. `pid` is the contents of
    /// the lockfile (confirmed alive via the host-level PID check).
    /// Stale locks (dead PID / malformed content) are reclaimed
    /// silently and do not surface this variant.
    #[error("another /spec:execute driver is running (pid {pid}); refusing to proceed")]
    DriverBusy {
        /// PID of the process that holds the lock.
        pid: u32,
    },

    /// A required artifact was not found at the expected path.
    #[error("{kind} not found at {}", path.display())]
    ArtifactNotFound {
        /// Kind of artifact (e.g. `.metadata.yaml`).
        kind: &'static str,
        /// Path where the artifact was expected.
        path: std::path::PathBuf,
    },

    /// A change directory was expected but not found.
    #[error("change '{name}' not found")]
    ChangeNotFound {
        /// Kebab-case change name.
        name: String,
    },

    /// `registry.yaml` was expected but is absent.
    #[error("no registry declared at registry.yaml")]
    RegistryMissing,

    /// One or more legacy v1-layout artifacts were found under
    /// `.specify/`. The CLI moved these to the repo root in the v2
    /// layout; the operator must run `specify migrate v2-layout` to
    /// move them in place. `paths` enumerates every offending entry
    /// the detector saw, in deterministic order.
    #[error("legacy v1 layout detected; run `specify migrate v2-layout` to upgrade ({paths:?})")]
    LegacyLayout {
        /// Repo-relative paths of the legacy artifacts the detector
        /// found (e.g. `.specify/registry.yaml`).
        paths: Vec<String>,
    },

    /// A name failed kebab-case validation.
    #[error("invalid name: {0}")]
    InvalidName(String),

    /// An I/O error propagated from the standard library.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A YAML deserialization error.
    #[error(transparent)]
    Yaml(#[from] serde_saphyr::Error),

    /// A YAML serialization error.
    #[error(transparent)]
    YamlSer(#[from] serde_saphyr::ser::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(status: ValidationStatus) -> ValidationSummary {
        ValidationSummary {
            status,
            rule_id: "rule.example".to_string(),
            rule: "Example rule".to_string(),
            detail: if status == ValidationStatus::Pass {
                None
            } else {
                Some("detail".to_string())
            },
        }
    }

    #[test]
    fn not_init_display() {
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
    fn validation_display_payload() {
        let err = Error::Validation {
            count: 2,
            results: vec![summary(ValidationStatus::Fail), summary(ValidationStatus::Deferred)],
        };
        assert_eq!(err.to_string(), "validation failed: 2 errors");
        if let Error::Validation { count, results } = err {
            assert_eq!(count, 2);
            assert_eq!(results.len(), 2);
            assert_eq!(results[0].status, ValidationStatus::Fail);
            assert_eq!(results[1].status, ValidationStatus::Deferred);
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
    fn version_too_old_display() {
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
    fn plan_outstanding_display() {
        let err = Error::PlanHasOutstandingWork {
            entries: vec!["checkout-api".to_string(), "checkout-ui".to_string()],
        };
        assert_eq!(
            err.to_string(),
            "plan has outstanding non-terminal work: [\"checkout-api\", \"checkout-ui\"]"
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
    fn artifact_not_found_display() {
        let err = Error::ArtifactNotFound {
            kind: ".metadata.yaml",
            path: std::path::PathBuf::from("/tmp/x"),
        };
        assert_eq!(err.to_string(), ".metadata.yaml not found at /tmp/x");
    }

    #[test]
    fn invalid_name_display() {
        let err = Error::InvalidName("bad--name".to_string());
        assert_eq!(err.to_string(), "invalid name: bad--name");
    }

    #[test]
    fn legacy_layout_display() {
        let err = Error::LegacyLayout {
            paths: vec![".specify/registry.yaml".to_string(), ".specify/plan.yaml".to_string()],
        };
        let s = err.to_string();
        assert!(s.contains("legacy v1 layout detected"), "expected legacy-layout banner, got: {s}");
        assert!(
            s.contains("specify migrate v2-layout"),
            "expected migration command in message, got: {s}"
        );
        assert!(
            s.contains(".specify/registry.yaml") && s.contains(".specify/plan.yaml"),
            "expected offending paths to surface, got: {s}"
        );
    }

    #[test]
    fn io_from() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err: Error = io.into();
        assert!(matches!(err, Error::Io(_)));
        assert_eq!(err.to_string(), "missing");
    }

    #[test]
    fn yaml_from() {
        let parse_err: serde_saphyr::Error = serde_saphyr::from_str::<String>(":\n\t- bad")
            .expect_err("expected a YAML parse error");
        let display = parse_err.to_string();
        let err: Error = parse_err.into();
        assert!(matches!(err, Error::Yaml(_)));
        assert_eq!(err.to_string(), display);
    }
}
