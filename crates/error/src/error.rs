//! `Error` enum and saphyr-error conversions.
//!
//! The `YamlDe` / `YamlSer` variants flatten `serde_saphyr`'s two error
//! types directly into the crate's error surface; callers that don't
//! care which API tripped can continue to `?`-propagate.

use crate::validation::{Status as ValidationStatus, Summary as ValidationSummary};

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

    /// A required artifact was not found at the expected path.
    #[error("{kind} not found at {}", path.display())]
    ArtifactNotFound {
        /// Kind of artifact (e.g. `.metadata.yaml`).
        kind: &'static str,
        /// Path where the artifact was expected.
        path: std::path::PathBuf,
    },

    /// A filesystem operation failed. The `op` field is a stable
    /// kebab-case suffix that, prefixed with `filesystem-`, becomes the
    /// JSON envelope's `error` discriminant (e.g. `filesystem-readdir`).
    /// Canonical call sites: the slice-merge engine
    /// (`specify_domain::merge::slice::{read, write}`), where every
    /// recursive directory walk and file copy needs a stable, testable
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

    /// A YAML deserialization error (e.g. `serde_saphyr::from_str`).
    /// Library crates rely on `?`-propagation; the variant docstring is
    /// the canonical "you don't have to care which `serde_saphyr` API
    /// tripped" — match on either YAML variant when that distinction is
    /// irrelevant.
    #[error(transparent)]
    YamlDe(#[from] serde_saphyr::Error),

    /// A YAML serialization error (e.g. `serde_saphyr::to_string`).
    #[error(transparent)]
    YamlSer(#[from] serde_saphyr::ser::Error),
}

impl Error {
    /// Long-form recovery hint for tightened diagnostics. Returns
    /// `None` when the variant has no actionable follow-up beyond the
    /// `#[error("…")]` body.
    ///
    /// The renderer in `src/output.rs` calls this to surface guidance
    /// alongside the kebab discriminant on a TTY, while keeping the
    /// machine-readable JSON envelope compact. New hints land here
    /// (typed-arm for typed variants; `Self::Diag { code, .. }` arm for
    /// `Diag`-routed sites), not in the renderer.
    #[must_use]
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::Diag { code, .. } => match *code {
                "plan-has-outstanding-work" => Some(
                    "complete or drop the listed entries, or rerun with --force to archive anyway.",
                ),
                "init-requires-capability-or-hub" => Some(
                    "`specify init <capability>` for a regular project, or `specify init --hub` for a platform hub.\nsee: docs/init.md",
                ),
                "context-existing-unfenced-agents-md" => {
                    Some("rerun with --force to rewrite AGENTS.md.")
                }
                "context-fenced-content-modified" => Some(
                    "reconcile the edits or rerun with --force to replace the generated block.",
                ),
                _ => None,
            },
            _ => None,
        }
    }

    /// Kebab-case identifier used in structured CLI error payloads.
    ///
    /// Most arms return a `&'static str` literal; [`Self::Filesystem`]
    /// composes `filesystem-<op>` and returns the owned form.
    #[must_use]
    pub fn variant_str(&self) -> String {
        match self {
            Self::NotInitialized => "not-initialized".to_string(),
            Self::Diag { code, .. } => (*code).to_string(),
            Self::Argument { .. } => "argument".to_string(),
            Self::Validation { .. } => "validation".to_string(),
            Self::CliTooOld { .. } => "specify-version-too-old".to_string(),
            Self::ArtifactNotFound { .. } => "artifact-not-found".to_string(),
            Self::Filesystem { op, .. } => format!("filesystem-{op}"),
            Self::BranchPrepareFailed { .. } => "branch-preparation-failed".to_string(),
            Self::Io(_) => "io".to_string(),
            Self::YamlDe(_) | Self::YamlSer(_) => "yaml".to_string(),
        }
    }

    /// Build a single-finding `Validation` failure. Use at sites that
    /// previously routed through `Error::Diag` to land on `Exit::ValidationFailed`:
    /// the kebab `rule_id` becomes the wire-visible discriminant inside
    /// the `results[]` array, and `Exit::from(&Error)` matches on the
    /// `Validation` variant rather than a magic code list. Compose
    /// multi-finding payloads with `Error::Validation { results }`
    /// directly.
    #[must_use]
    pub fn validation_failed(
        rule_id: impl Into<String>, rule: impl Into<String>, detail: impl Into<String>,
    ) -> Self {
        Self::Validation {
            results: vec![ValidationSummary {
                status: ValidationStatus::Fail,
                rule_id: rule_id.into(),
                rule: rule.into(),
                detail: Some(detail.into()),
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn diag_round_trip() {
        let err = Error::Diag {
            code: "kebab-prefix",
            detail: "specific detail".to_string(),
        };
        assert_eq!(err.variant_str(), "kebab-prefix");
        assert_eq!(err.to_string(), "kebab-prefix: specific detail");
    }
}

#[cfg(test)]
mod tests {
    use super::Error;

    #[test]
    fn diag_round_trip() {
        let err = Error::Diag {
            code: "kebab-prefix",
            detail: "specific detail".to_string(),
        };
        assert_eq!(err.variant_str(), "kebab-prefix");
        assert_eq!(err.to_string(), "kebab-prefix: specific detail");
    }
}
