//! `Error` enum and saphyr-error conversions. Hint and discriminant
//! tables live in [`crate::display`]; the YAML wrappers behind
//! `Yaml` / `YamlSer` live in [`crate::yaml`].

use crate::validation::Summary as ValidationSummary;
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

    /// Validation failed with one or more findings.
    #[error("validation failed: {} errors", results.len())]
    Validation {
        /// Individual validation results.
        results: Vec<ValidationSummary>,
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
