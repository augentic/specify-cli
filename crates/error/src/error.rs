//! `Error` enum and saphyr-error conversions.
//!
//! Hint and discriminant tables live in [`crate::display`]; the YAML
//! wrappers behind `Yaml` / `YamlSer` live in [`crate::yaml`].

use crate::validation::ValidationSummary;
use crate::yaml::{YamlError, YamlSerError};

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
    #[error("validation failed: {} errors", results.len())]
    Validation {
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

    /// `specify workspace prepare-branch` refused to land a branch
    /// because `specify_registry::branch::prepare` returned a
    /// diagnostic. The renderer surfaces the diagnostic key + paths
    /// alongside the human-readable detail.
    #[error("branch-preparation-failed: project `{project}`: {detail} ({key})")]
    BranchPrepareFailed {
        /// Project (registry slot) name.
        project: String,
        /// Stable diagnostic key from `specify_registry::branch`.
        key: String,
        /// Human-readable diagnostic message.
        detail: String,
        /// Repository-relative paths the diagnostic points at (may be
        /// empty when the diagnostic is whole-clone scoped).
        paths: Vec<String>,
    },

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

    /// `specify change finalize` ran successfully but the per-project
    /// probes reported at least one blocker. The structured summary
    /// (unmerged PRs, dirty clones, etc.) lives in the stdout body;
    /// this variant carries the change name and a human-readable
    /// summary of the blockers.
    #[error("change-finalize-blocked: change `{change}` blocked: {summary}")]
    ChangeFinalizeBlocked {
        /// Change name (= `plan.yaml:name`).
        change: String,
        /// Human-readable summary of the per-project blockers.
        summary: String,
    },

    /// An I/O error propagated from the standard library.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A YAML deserialization error.
    #[error(transparent)]
    Yaml(#[from] YamlError),

    /// A YAML serialization error.
    #[error(transparent)]
    YamlSer(#[from] YamlSerError),
}

impl From<serde_saphyr::Error> for Error {
    fn from(value: serde_saphyr::Error) -> Self {
        Self::Yaml(YamlError::from(value))
    }
}

impl From<serde_saphyr::ser::Error> for Error {
    fn from(value: serde_saphyr::ser::Error) -> Self {
        Self::YamlSer(YamlSerError::from(value))
    }
}

#[cfg(test)]
mod tests {
    use super::Error;

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
