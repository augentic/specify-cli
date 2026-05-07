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

    /// A slice directory was expected but not found.
    #[error("slice '{name}' not found")]
    SliceNotFound {
        /// Kebab-case slice name.
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

    /// `specify migrate slice-layout` (RFC-13 chunk 3.6) refused to
    /// run because at least one per-loop unit under
    /// `.specify/changes/<name>/` carries an unfinished phase
    /// (lifecycle status not in `Merged` or `Dropped`). The operator
    /// must finish the slice loop (`specify slice merge run <name>`
    /// once it reaches `complete`) or discard it
    /// (`specify slice drop <name>`) before re-running the migration.
    /// `in_progress` is `(slice_name, lifecycle_status)` pairs in
    /// deterministic (alphabetical) order so JSON output and error
    /// text stay stable across runs.
    #[error(
        "slice-migration-blocked-by-in-progress: {} slice(s) carry an unfinished phase. \
         Finish or drop them before migrating: {}. \
         Run `specify slice drop <name>` to discard each, or complete the slice loop \
         (`specify slice merge run <name>` once it reaches `complete`).",
        in_progress.len(),
        format_in_progress(in_progress)
    )]
    SliceMigrationBlockedByInProgress {
        /// `(slice_name, lifecycle_status)` for each non-terminal
        /// slice the detector found, sorted by slice name.
        in_progress: Vec<(String, String)>,
    },

    /// `specify migrate slice-layout` (RFC-13 chunk 3.6) refused to
    /// run because both `.specify/changes/` (the v1 source) and
    /// `.specify/slices/` (the post-migration destination) are
    /// present. A previous migration was interrupted or someone
    /// hand-edited the tree; the operator must inspect both
    /// directories and reconcile manually before re-running.
    #[error(
        "slice-migration-target-exists: both `{}` and `{}` exist; the migration cannot proceed \
         while the destination is non-empty. Inspect both directories, move any needed contents \
         out of `.specify/slices/`, then remove the empty `.specify/slices/` and re-run \
         `specify migrate slice-layout`.",
        changes.display(),
        slices.display()
    )]
    SliceMigrationTargetExists {
        /// Path to the v1 source directory (`.specify/changes/`).
        changes: std::path::PathBuf,
        /// Path to the post-migration destination (`.specify/slices/`).
        slices: std::path::PathBuf,
    },

    /// `specify migrate change-noun` (RFC-13 chunk 3.7) refused to
    /// run because both `initiative.md` (the pre-Phase-3.7 source) and
    /// `change.md` (the post-migration destination) are present at the
    /// repo root. A previous migration was interrupted or someone
    /// hand-edited the tree; the operator must inspect both files and
    /// reconcile manually before re-running.
    #[error(
        "change-noun-migration-target-exists: both `{}` and `{}` exist at the repo root; \
         the migration cannot proceed while the destination is already in place. Inspect \
         both files, move the canonical content into `change.md`, remove the legacy \
         `initiative.md`, and re-run `specify migrate change-noun`.",
        initiative.display(),
        change.display()
    )]
    ChangeNounMigrationTargetExists {
        /// Path to the pre-Phase-3.7 source file (`initiative.md`).
        initiative: std::path::PathBuf,
        /// Path to the post-migration destination file (`change.md`).
        change: std::path::PathBuf,
    },

    /// A `specify change *` verb encountered the pre-Phase-3.7
    /// operator brief filename (`initiative.md`) at the repo root
    /// without the post-RFC-13 `change.md` companion. RFC-13 chunk 3.7
    /// renamed the umbrella brief filename in place; the post-RFC CLI
    /// surface refuses to read the legacy filename and points the
    /// operator at `specify migrate change-noun`. `path` names the
    /// offending legacy file.
    ///
    /// See: <https://github.com/augentic/specify/blob/main/rfcs/rfc-13-extensibility.md#migration>
    #[error(
        "change-brief-became-change-md: found legacy `initiative.md` at {} but expected \
         `change.md`. RFC-13 renamed the umbrella brief filename: `initiative.md` is now \
         `change.md`. Run `specify migrate change-noun` to migrate. \
         See: https://github.com/augentic/specify/blob/main/rfcs/rfc-13-extensibility.md#migration",
        path.display()
    )]
    ChangeBriefBecameChangeMd {
        /// Path to the pre-Phase-3.7 file the detector found.
        path: std::path::PathBuf,
    },

    /// The capability resolver found a pre-RFC-13 `schema.yaml` (or a
    /// `project.yaml` carrying the v1 `schema:` field) where it expected
    /// a `capability.yaml` (or `capability:` field). RFC-13 renamed the
    /// extension primitive from "schema" to "capability"; the legacy
    /// shape is no longer loaded silently — the operator must rename the
    /// file (and re-run `specify init <capability>` if the project still
    /// records the old `schema:` field). `path` is the offending file
    /// the detector found so the diagnostic can name it.
    ///
    /// See: <https://github.com/augentic/specify/blob/main/rfcs/rfc-13-extensibility.md#migration>
    #[error(
        "schema-became-capability: found legacy `schema` shape at `{}` but expected the \
         post-RFC-13 `capability` shape. RFC-13 renamed the Specify extension primitive: \
         `schema` is now `capability` (rename `schema.yaml` → `capability.yaml`, and rewrite \
         `project.yaml: schema:` → `project.yaml: capability:`). Re-run \
         `specify init <capability>` if you have not already migrated. \
         See: https://github.com/augentic/specify/blob/main/rfcs/rfc-13-extensibility.md#migration",
        path.display()
    )]
    SchemaBecameCapability {
        /// Path to the file that triggered the diagnostic (a legacy
        /// `schema.yaml`, or a `project.yaml` carrying the v1 `schema:`
        /// field).
        path: std::path::PathBuf,
    },

    /// `specify init` was invoked without the post-RFC-13 capability
    /// positional and without `--hub`, or with both at once. RFC-13
    /// makes the two mutually exclusive: a regular project init takes
    /// `<capability>` as a required positional; a registry-only platform
    /// hub init takes `--hub` instead. See:
    /// <https://github.com/augentic/specify/blob/main/rfcs/rfc-13-extensibility.md#migration>.
    #[error(
        "init-requires-capability-or-hub: `specify init` requires either a capability \
         identifier or `--hub`. Run `specify init <capability>` for a regular project \
         (e.g. `specify init omnia` or `specify init https://...`), or `specify init --hub` \
         for a registry-only platform hub. The two are mutually exclusive. \
         See: https://github.com/augentic/specify/blob/main/rfcs/rfc-13-extensibility.md#migration"
    )]
    InitRequiresCapabilityOrHub,

    /// A declared WASI tool could not be resolved or fetched.
    #[error("tool resolver error: {0}")]
    ToolResolver(String),

    /// A declared WASI tool failed while compiling, linking, instantiating, or running.
    #[error("tool runtime error: {0}")]
    ToolRuntime(String),

    /// A declared WASI tool requested filesystem authority outside its manifest policy.
    #[error("tool permission denied: {0}")]
    ToolPermissionDenied(String),

    /// The requested tool name was not present in either declaration site.
    #[error("tool not declared: {name}")]
    ToolNotDeclared {
        /// Missing tool name.
        name: String,
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

/// Render the `(slice_name, lifecycle_status)` list embedded in
/// [`Error::SliceMigrationBlockedByInProgress`] as the comma-separated
/// `name (status), name (status)` form used in the diagnostic.
///
/// Kept as a free function so the `thiserror` `#[error(…)]` format
/// string can resolve it without taking on a runtime fmt closure.
fn format_in_progress(items: &[(String, String)]) -> String {
    items.iter().map(|(name, status)| format!("{name} ({status})")).collect::<Vec<_>>().join(", ")
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
    fn schema_became_capability_display() {
        let err = Error::SchemaBecameCapability {
            path: std::path::PathBuf::from("./.specify/.cache/omnia/schema.yaml"),
        };
        let s = err.to_string();
        assert!(
            s.contains("schema-became-capability"),
            "diagnostic must carry the stable kebab-case code, got: {s}"
        );
        assert!(
            s.contains("./.specify/.cache/omnia/schema.yaml"),
            "diagnostic must surface the offending path, got: {s}"
        );
        assert!(
            s.contains("capability.yaml"),
            "diagnostic must name the post-rename filename, got: {s}"
        );
        assert!(s.contains("RFC-13"), "diagnostic must cite RFC-13, got: {s}");
        assert!(
            s.contains("specify init <capability>"),
            "diagnostic must mention the post-rename init command, got: {s}"
        );
        assert!(
            s.contains("rfcs/rfc-13-extensibility.md#migration"),
            "diagnostic must link the RFC-13 §Migration anchor, got: {s}"
        );
    }

    #[test]
    fn init_requires_capability_or_hub_display() {
        let err = Error::InitRequiresCapabilityOrHub;
        let s = err.to_string();
        assert!(
            s.contains("init-requires-capability-or-hub"),
            "diagnostic must carry the stable kebab-case code, got: {s}"
        );
        assert!(
            s.contains("specify init <capability>"),
            "diagnostic must show the regular-project init form, got: {s}"
        );
        assert!(
            s.contains("specify init --hub"),
            "diagnostic must show the hub-init form, got: {s}"
        );
        assert!(
            s.contains("rfcs/rfc-13-extensibility.md#migration"),
            "diagnostic must link RFC-13 §Migration, got: {s}"
        );
    }

    #[test]
    fn slice_migration_blocked_by_in_progress_display() {
        let err = Error::SliceMigrationBlockedByInProgress {
            in_progress: vec![
                ("demo".to_string(), "defining".to_string()),
                ("other-thing".to_string(), "building".to_string()),
            ],
        };
        let s = err.to_string();
        assert!(
            s.starts_with("slice-migration-blocked-by-in-progress:"),
            "diagnostic must carry the kebab-case prefix, got: {s}"
        );
        assert!(s.contains("2 slice(s)"), "diagnostic must surface the count, got: {s}");
        assert!(
            s.contains("demo (defining)"),
            "diagnostic must surface the (name, status) pair, got: {s}"
        );
        assert!(
            s.contains("other-thing (building)"),
            "diagnostic must surface every offender, got: {s}"
        );
        assert!(
            s.contains("specify slice drop"),
            "diagnostic must point operator at the recovery verb, got: {s}"
        );
        assert!(
            s.contains("specify slice merge run"),
            "diagnostic must mention the canonical completion verb, got: {s}"
        );
    }

    #[test]
    fn slice_migration_blocked_singular_form_round_trips() {
        // Single-offender variant: the same diagnostic must still
        // render cleanly so the operator's first read names the
        // problematic slice.
        let err = Error::SliceMigrationBlockedByInProgress {
            in_progress: vec![("demo".to_string(), "defining".to_string())],
        };
        let s = err.to_string();
        assert!(s.contains("1 slice(s)"), "diagnostic must surface the count, got: {s}");
        assert!(s.contains("demo (defining)"), "diagnostic must surface the offender, got: {s}");
    }

    #[test]
    fn change_noun_migration_target_exists_display() {
        let err = Error::ChangeNounMigrationTargetExists {
            initiative: std::path::PathBuf::from("/proj/initiative.md"),
            change: std::path::PathBuf::from("/proj/change.md"),
        };
        let s = err.to_string();
        assert!(
            s.starts_with("change-noun-migration-target-exists:"),
            "diagnostic must carry the kebab-case prefix, got: {s}"
        );
        assert!(
            s.contains("/proj/initiative.md") && s.contains("/proj/change.md"),
            "diagnostic must surface both colliding paths, got: {s}"
        );
        assert!(
            s.contains("specify migrate change-noun"),
            "diagnostic must reference the migration verb, got: {s}"
        );
    }

    #[test]
    fn change_brief_became_change_md_display() {
        let err = Error::ChangeBriefBecameChangeMd {
            path: std::path::PathBuf::from("/proj/initiative.md"),
        };
        let s = err.to_string();
        assert!(
            s.starts_with("change-brief-became-change-md:"),
            "diagnostic must carry the kebab-case prefix, got: {s}"
        );
        assert!(
            s.contains("/proj/initiative.md"),
            "diagnostic must surface the offending path, got: {s}"
        );
        assert!(s.contains("change.md"), "diagnostic must name the post-rename filename, got: {s}");
        assert!(
            s.contains("specify migrate change-noun"),
            "diagnostic must point operator at the migration verb, got: {s}"
        );
        assert!(
            s.contains("rfcs/rfc-13-extensibility.md#migration"),
            "diagnostic must link the RFC-13 §Migration anchor, got: {s}"
        );
    }

    #[test]
    fn slice_migration_target_exists_display() {
        let err = Error::SliceMigrationTargetExists {
            changes: std::path::PathBuf::from("/proj/.specify/changes"),
            slices: std::path::PathBuf::from("/proj/.specify/slices"),
        };
        let s = err.to_string();
        assert!(
            s.starts_with("slice-migration-target-exists:"),
            "diagnostic must carry the kebab-case prefix, got: {s}"
        );
        assert!(
            s.contains("/proj/.specify/changes") && s.contains("/proj/.specify/slices"),
            "diagnostic must surface both colliding paths, got: {s}"
        );
        assert!(
            s.contains("specify migrate slice-layout"),
            "diagnostic must reference the migration verb, got: {s}"
        );
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
