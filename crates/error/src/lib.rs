//! Unified error types for the `specify` CLI and its domain crates.
//!
//! Every public function in a `specify-*` crate returns `Result<T, Error>`.
//! The variants are structured (rather than string-only) so `main.rs` can
//! pattern-match them to assign exit codes and pick an output format.

/// Kebab-case predicate shared across every workspace crate.
///
/// Mirrors the JSON Schema regex `^[a-z0-9]+(-[a-z0-9]+)*$` carried by
/// `schemas/plan/plan.schema.json` `$defs.kebabName.pattern`.
#[must_use]
pub fn is_kebab(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.ends_with('-')
        && !s.contains("--")
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Validation outcome for a single rule check.
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

    /// Capability resolution failed with the given reason.
    #[error("capability resolution failed: {0}")]
    CapabilityResolution(String),

    /// A configuration or input error.
    #[error("config error: {0}")]
    Config(String),

    /// A user-supplied CLI argument is invalid for reasons clap cannot
    /// catch (kebab-case names, mutually exclusive flag combinations,
    /// unknown enum keys, etc.). Carries the offending flag/value plus a
    /// human-readable detail. Prefer this over [`Error::Config`] for
    /// argument-shape validation so the CLI can map it onto the
    /// argument-error exit code.
    #[error("invalid argument {flag}: {detail}")]
    Argument {
        /// Argument name (e.g. `--capability`, `<name>`, `phase`).
        flag: &'static str,
        /// Human-readable explanation, suitable for display alongside `--help`.
        detail: String,
    },

    /// `specify context generate` refused to overwrite an existing
    /// hand-authored `AGENTS.md` that does not contain the managed fences.
    #[error(
        "context-existing-unfenced-agents-md: AGENTS.md exists without Specify context fences; \
         rerun with --force to rewrite it"
    )]
    ContextUnfenced,

    /// `specify context generate` refused to replace the managed block
    /// because the fenced content has diverged from `.specify/context.lock`.
    #[error(
        "context-fenced-content-modified: AGENTS.md content inside the Specify context fences \
         has changed since .specify/context.lock was written; reconcile the edits or rerun \
         with --force to replace the generated block"
    )]
    ContextDrift,

    /// `.specify/context.lock` is absent for a context check.
    #[error("context-lock-missing: .specify/context.lock is missing")]
    ContextNoLock,

    /// `AGENTS.md` is absent for a context check.
    #[error("context-not-generated: AGENTS.md is missing")]
    ContextMissing,

    /// `.specify/context.lock` declares a version newer than this CLI supports.
    #[error(
        "context-lock-version-too-new: lock version {found} is newer than supported version \
         {supported}"
    )]
    ContextLockTooNew {
        /// Version declared by the lock file.
        found: u64,
        /// Highest lock-file version supported by this CLI.
        supported: u64,
    },

    /// `.specify/context.lock` exists but is not a well-formed current lock.
    #[error("context-lock-malformed: {detail}")]
    ContextLockMalformed {
        /// Human-readable malformed-lock detail.
        detail: String,
    },

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
    CliTooOld {
        /// Minimum version the project requires.
        required: String,
        /// Version currently installed.
        found: String,
    },

    /// Illegal plan entry status transition.
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
    PlanIncomplete {
        /// Names of plan entries not yet in a terminal state.
        entries: Vec<String>,
    },

    /// Another live `/change:execute` driver holds `.specify/plan.lock`.
    /// Stale locks (dead PID / malformed content) are reclaimed silently.
    #[error("another /change:execute driver is running (pid {pid}); refusing to proceed")]
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

    /// `specify init` requires either a `<capability>` positional or
    /// `--hub` (mutually exclusive).
    #[error(
        "init-requires-capability-or-hub: `specify init` requires either a capability \
         identifier or `--hub`. Run `specify init <capability>` for a regular project \
         (e.g. `specify init omnia` or `specify init https://...`), or `specify init --hub` \
         for a registry-only platform hub. The two are mutually exclusive. \
         See: https://github.com/augentic/specify/blob/main/rfcs/rfc-13-extensibility.md#migration"
    )]
    InitNeedsCapability,

    /// A declared WASI tool could not be resolved or fetched.
    #[error("tool resolver error: {0}")]
    ToolResolver(String),

    /// A declared WASI tool failed while compiling, linking, instantiating, or running.
    #[error("tool runtime error: {0}")]
    ToolRuntime(String),

    /// A declared WASI tool requested filesystem authority outside its manifest policy.
    #[error("tool permission denied: {0}")]
    ToolDenied(String),

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

impl Error {
    /// Kebab-case identifier used in structured CLI error payloads.
    #[must_use]
    pub const fn variant_str(&self) -> &'static str {
        match self {
            Self::NotInitialized => "not-initialized",
            Self::CapabilityResolution(_) => "capability-resolution",
            Self::Config(_) => "config",
            Self::Argument { .. } => "argument",
            Self::ContextUnfenced => "context-existing-unfenced-agents-md",
            Self::ContextDrift => "context-fenced-content-modified",
            Self::ContextNoLock => "context-lock-missing",
            Self::ContextMissing => "context-not-generated",
            Self::ContextLockTooNew { .. } => "context-lock-version-too-new",
            Self::ContextLockMalformed { .. } => "context-lock-malformed",
            Self::Validation { .. } => "validation",
            Self::Merge(_) => "merge",
            Self::Lifecycle { .. } => "lifecycle",
            Self::CliTooOld { .. } => "specify-version-too-old",
            Self::PlanTransition { .. } => "plan-transition",
            Self::PlanIncomplete { .. } => "plan-has-outstanding-work",
            Self::DriverBusy { .. } => "driver-busy",
            Self::ArtifactNotFound { .. } => "artifact-not-found",
            Self::SliceNotFound { .. } => "slice-not-found",
            Self::RegistryMissing => "registry-missing",
            Self::InitNeedsCapability => "init-requires-capability-or-hub",
            Self::ToolResolver(_) => "tool-resolver",
            Self::ToolRuntime(_) => "tool-runtime",
            Self::ToolDenied(_) => "tool-permission-denied",
            Self::ToolNotDeclared { .. } => "tool-not-declared",
            Self::InvalidName(_) => "invalid-name",
            Self::Io(_) => "io",
            Self::Yaml(_) => "yaml",
            Self::YamlSer(_) => "yaml-ser",
        }
    }
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

    /// Spot-check that representative variants render their expected
    /// prefix. Per-variant assertions on `Display` output are noise
    /// — `thiserror` already drives the format strings.
    #[test]
    fn display_spot_checks() {
        let cases: [(Error, &str); 5] = [
            (Error::NotInitialized, "not initialized"),
            (Error::CapabilityResolution("boom".into()), "capability resolution failed: boom"),
            (Error::Config("bad key".into()), "config error: bad key"),
            (Error::Merge("conflict".into()), "merge failed: conflict"),
            (Error::InvalidName("bad--name".into()), "invalid name: bad--name"),
        ];
        for (err, expected_prefix) in cases {
            assert!(
                err.to_string().starts_with(expected_prefix),
                "{err} should start with {expected_prefix}"
            );
        }
    }

    #[test]
    fn validation_payload_round_trips() {
        let err = Error::Validation {
            count: 2,
            results: vec![summary(ValidationStatus::Fail), summary(ValidationStatus::Deferred)],
        };
        assert_eq!(err.to_string(), "validation failed: 2 errors");
        let Error::Validation { count, results } = err else {
            panic!("expected Validation variant");
        };
        assert_eq!(count, 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].status, ValidationStatus::Fail);
        assert_eq!(results[1].status, ValidationStatus::Deferred);
    }

    #[test]
    fn init_requires_capability_or_hub_display() {
        let err = Error::InitNeedsCapability;
        let s = err.to_string();
        for needle in
            ["init-requires-capability-or-hub", "specify init <capability>", "specify init --hub"]
        {
            assert!(s.contains(needle), "diagnostic must include `{needle}`, got: {s}");
        }
    }

    #[test]
    fn context_diagnostic_variant_strings_are_stable() {
        let cases = [
            (Error::ContextUnfenced, "context-existing-unfenced-agents-md"),
            (Error::ContextDrift, "context-fenced-content-modified"),
            (Error::ContextNoLock, "context-lock-missing"),
            (Error::ContextMissing, "context-not-generated"),
            (
                Error::ContextLockTooNew {
                    found: 2,
                    supported: 1,
                },
                "context-lock-version-too-new",
            ),
            (
                Error::ContextLockMalformed {
                    detail: "missing inputs".to_string(),
                },
                "context-lock-malformed",
            ),
        ];

        for (err, expected) in cases {
            assert_eq!(err.variant_str(), expected);
            assert!(
                err.to_string().starts_with(expected),
                "context diagnostic display must start with `{expected}`, got: {err}"
            );
        }
    }

    #[test]
    fn io_from() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err: Error = io.into();
        assert!(matches!(err, Error::Io(_)));
        assert_eq!(err.to_string(), "missing");
    }

    #[test]
    fn is_kebab_accepts_and_rejects() {
        for ok in ["a", "abc", "alpha-gateway", "x-1", "a1-b2"] {
            assert!(is_kebab(ok), "expected `{ok}` to pass");
        }
        for bad in ["", "-a", "a-", "a--b", "A", "alpha_gateway", "alpha gateway"] {
            assert!(!is_kebab(bad), "expected `{bad}` to fail");
        }
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
