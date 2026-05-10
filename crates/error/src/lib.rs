//! Unified error types for the `specify` CLI and its domain crates.
//!
//! Every public function in a `specify-*` crate returns `Result<T, Error>`.
//! The variants are structured (rather than string-only) so `main.rs` can
//! pattern-match them to assign exit codes and pick an output format.

/// Kebab-case predicate shared across every workspace crate.
///
/// Mirrors the JSON Schema regex `^[a-z0-9]+(-[a-z0-9]+)*$` carried by
/// `schemas/plan/plan.schema.json` `$defs.kebabName.pattern`: one or
/// more hyphen-separated segments; each segment is non-empty and
/// contains only ASCII lowercase letters and digits.
#[must_use]
pub fn is_kebab(s: &str) -> bool {
    !s.is_empty()
        && s.split('-').all(|seg| {
            !seg.is_empty() && seg.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
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

    /// Structured catch-all for diagnostics that don't have a dedicated
    /// variant. The `code` is a stable kebab-case discriminant surfaced
    /// in JSON envelopes; `detail` is the human-readable message.
    /// Promote a recurring `Diag` site to its own variant once the call
    /// shape stabilises.
    #[error("{code}: {detail}")]
    Diag {
        /// Stable kebab-case discriminant surfaced as the JSON `error` field.
        code: &'static str,
        /// Human-readable message.
        detail: String,
    },

    /// A user-supplied CLI argument is invalid for reasons clap cannot
    /// catch (kebab-case names, mutually exclusive flag combinations,
    /// unknown enum keys, etc.). Carries the offending flag/value plus a
    /// human-readable detail. Prefer this over [`Error::Diag`] for
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
    /// The renderer adds a `--force` recovery hint.
    #[error("context-existing-unfenced-agents-md: AGENTS.md exists without Specify fences")]
    ContextUnfenced,

    /// `specify context generate` refused to replace the managed block
    /// because the fenced content has diverged from `.specify/context.lock`.
    /// The renderer adds a reconcile-or-`--force` recovery hint.
    #[error("context-fenced-content-modified: AGENTS.md drifted from .specify/context.lock")]
    ContextDrift,

    /// `.specify/context.lock` is absent for a context check.
    #[error("context-lock-missing: .specify/context.lock is missing")]
    ContextNoLock,

    /// `AGENTS.md` is absent for a context check.
    #[error("context-not-generated: AGENTS.md is missing")]
    ContextMissing,

    /// `.specify/context.lock` declares a version newer than this CLI supports.
    #[error("context-lock-version-too-new: lock version {found} > supported {supported}")]
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

    /// `specify change finalize` was invoked but `plan.yaml` is absent.
    /// Canonical "change is already finalized" signal — the recovery
    /// is to start a new loop via `specify change plan create`.
    #[error(
        "plan-not-found: no plan to finalize: plan.yaml is absent. \
         If the change was already finalized, the archive is at \
         .specify/archive/plans/. Otherwise run \
         `specify change plan create` (and `specify change create` \
         if the change brief is also missing) to start the loop."
    )]
    PlanNotFound,

    /// `specify change plan {next, status}` short-circuited because
    /// the plan has structural errors (anything `Plan::validate`
    /// surfaces at error severity except `dependency-cycle`, which
    /// `status` falls back through). Operator follow-up is to run
    /// `specify change plan validate` for the per-finding detail.
    #[error(
        "plan-structural-errors: plan has structural errors; run 'specify change plan validate' \
         for detail"
    )]
    PlanStructural,

    /// `specify change finalize` refused because the plan still has
    /// non-terminal entries. `change` is the plan name; `entries`
    /// lists the offending entry names. The handler at
    /// `commands::change` renders a non-standard envelope so the
    /// entries appear alongside the discriminant.
    #[error("non-terminal-entries-present: plan `{change}` has non-terminal entries: {entries:?}")]
    PlanNonTerminalEntries {
        /// Change name (= `plan.yaml:name`).
        change: String,
        /// Names of plan entries not in a terminal state.
        entries: Vec<String>,
    },

    /// `specify change create` refused to overwrite an existing
    /// `change.md`. The handler at `commands::change` renders a
    /// non-standard envelope (`action`/`ok`/`path`) so the brief path
    /// surfaces alongside the `already-exists` discriminant.
    #[error("already-exists: change brief already exists at {}", path.display())]
    ChangeBriefExists {
        /// Path of the existing change brief.
        path: std::path::PathBuf,
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

    /// A capability directory does not contain `capability.yaml`.
    /// Canonical call site: `specify_capability::Capability::resolve`
    /// (and the `capability` / `tool` dispatchers under `src/commands`,
    /// which probe the resolved directory before delegating).
    #[error("capability-manifest-missing: no `capability.yaml` at {}", dir.display())]
    CapabilityManifestMissing {
        /// Directory expected to contain `capability.yaml`.
        dir: std::path::PathBuf,
    },

    /// A filesystem operation failed. The `op` field is a stable
    /// kebab-case suffix that, prefixed with `filesystem-`, becomes the
    /// JSON envelope's `error` discriminant (e.g. `filesystem-readdir`).
    /// Canonical call sites: the slice-merge engine
    /// (`specify_merge::slice::{read, write}`), where every recursive
    /// directory walk and file copy needs a stable, testable
    /// discriminant for operator follow-up.
    #[error("filesystem-{op}: {} ({source})", path.display())]
    Filesystem {
        /// Operation kind; the JSON discriminant is `filesystem-<op>`.
        op: &'static str,
        /// Path the operation acted on (or attempted to).
        path: std::path::PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// `specify init` requires either a `<capability>` positional or
    /// `--hub` (mutually exclusive). The renderer adds an
    /// invocation-shape hint pointing at `docs/init.md`.
    #[error("init-requires-capability-or-hub: pass <capability> or --hub")]
    InitNeedsCapability,

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
    ///
    /// Not `const` because `Self::Filesystem` matches on its
    /// `op: &'static str` to derive a stable `filesystem-<op>`
    /// discriminant, and `match`ing on `&str` is not yet stable in
    /// `const` context.
    #[must_use]
    pub fn variant_str(&self) -> &'static str {
        match self {
            Self::NotInitialized => "not-initialized",
            Self::Diag { code, .. } => code,
            Self::Argument { .. } => "argument",
            Self::ContextUnfenced => "context-existing-unfenced-agents-md",
            Self::ContextDrift => "context-fenced-content-modified",
            Self::ContextNoLock => "context-lock-missing",
            Self::ContextMissing => "context-not-generated",
            Self::ContextLockTooNew { .. } => "context-lock-version-too-new",
            Self::ContextLockMalformed { .. } => "context-lock-malformed",
            Self::Validation { .. } => "validation",
            Self::Lifecycle { .. } => "lifecycle",
            Self::CliTooOld { .. } => "specify-version-too-old",
            Self::PlanTransition { .. } => "plan-transition",
            Self::PlanIncomplete { .. } => "plan-has-outstanding-work",
            Self::PlanNotFound => "plan-not-found",
            Self::PlanStructural => "plan-structural-errors",
            Self::PlanNonTerminalEntries { .. } => "non-terminal-entries-present",
            Self::ChangeBriefExists { .. } => "already-exists",
            Self::DriverBusy { .. } => "driver-busy",
            Self::ArtifactNotFound { .. } => "artifact-not-found",
            Self::SliceNotFound { .. } => "slice-not-found",
            Self::RegistryMissing => "registry-missing",
            Self::CapabilityManifestMissing { .. } => "capability-manifest-missing",
            Self::Filesystem { op, .. } => match *op {
                "readdir" => "filesystem-readdir",
                "dir-entry" => "filesystem-dir-entry",
                "mkdir" => "filesystem-mkdir",
                "copy" => "filesystem-copy",
                "path-prefix" => "filesystem-path-prefix",
                _ => "filesystem",
            },
            Self::InitNeedsCapability => "init-requires-capability-or-hub",
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
        let cases: [(Error, &str); 2] = [
            (Error::NotInitialized, "not initialized"),
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
        assert_eq!(err.variant_str(), "init-requires-capability-or-hub");
        assert!(
            err.to_string().starts_with("init-requires-capability-or-hub: "),
            "InitNeedsCapability display must start with the kebab discriminant, got: {err}"
        );
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
    fn plan_structural_variant_string_is_stable() {
        let err = Error::PlanStructural;
        assert_eq!(err.variant_str(), "plan-structural-errors");
        assert!(
            err.to_string().starts_with("plan-structural-errors"),
            "PlanStructural display must start with `plan-structural-errors`, got: {err}"
        );
    }

    #[test]
    fn change_finalize_variant_strings_are_stable() {
        let cases = [
            (Error::PlanNotFound, "plan-not-found"),
            (
                Error::PlanNonTerminalEntries {
                    change: "foo".to_string(),
                    entries: vec!["b".to_string()],
                },
                "non-terminal-entries-present",
            ),
            (
                Error::ChangeBriefExists {
                    path: std::path::PathBuf::from("/tmp/change.md"),
                },
                "already-exists",
            ),
        ];

        for (err, expected) in cases {
            assert_eq!(err.variant_str(), expected);
            assert!(
                err.to_string().starts_with(expected),
                "change-finalize diagnostic display must start with `{expected}`, got: {err}"
            );
        }
    }

    #[test]
    fn filesystem_variant_strings_are_stable() {
        let cases: [(&'static str, &'static str); 6] = [
            ("readdir", "filesystem-readdir"),
            ("dir-entry", "filesystem-dir-entry"),
            ("mkdir", "filesystem-mkdir"),
            ("copy", "filesystem-copy"),
            ("path-prefix", "filesystem-path-prefix"),
            ("unknown-op", "filesystem"),
        ];
        for (op, expected) in cases {
            let err = Error::Filesystem {
                op,
                path: std::path::PathBuf::from("/tmp/x"),
                source: std::io::Error::other("boom"),
            };
            assert_eq!(err.variant_str(), expected, "op `{op}` → variant_str");
            assert!(
                err.to_string().starts_with(&format!("filesystem-{op}: ")),
                "op `{op}` display: {err}"
            );
        }
    }

    #[test]
    fn capability_manifest_missing_variant_string_is_stable() {
        let err = Error::CapabilityManifestMissing {
            dir: std::path::PathBuf::from("/tmp/cap"),
        };
        assert_eq!(err.variant_str(), "capability-manifest-missing");
        assert!(err.to_string().starts_with("capability-manifest-missing: "), "display: {err}");
    }

    #[test]
    fn diag_round_trip() {
        let err = Error::Diag {
            code: "kebab-prefix",
            detail: "specific detail".to_string(),
        };
        assert_eq!(err.variant_str(), "kebab-prefix");
        assert_eq!(err.to_string(), "kebab-prefix: specific detail");
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
