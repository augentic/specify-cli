//! Hint and discriminant tables for [`crate::Error`]. `variant_str` is
//! the wire-contract `error` field; `hint` is the long-form recovery
//! line the TTY renderer pulls alongside it.

use std::borrow::Cow;

use crate::error::Error;

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
    /// Most arms borrow a `&'static str`; [`Self::Filesystem`] composes
    /// `filesystem-<op>` and returns the owned form.
    #[must_use]
    pub fn variant_str(&self) -> Cow<'static, str> {
        match self {
            Self::NotInitialized => Cow::Borrowed("not-initialized"),
            Self::Diag { code, .. } => Cow::Borrowed(code),
            Self::Argument { .. } => Cow::Borrowed("argument"),
            Self::Validation { .. } => Cow::Borrowed("validation"),
            Self::CliTooOld { .. } => Cow::Borrowed("specify-version-too-old"),
            Self::PlanTransition { .. } => Cow::Borrowed("plan-transition"),
            Self::DriverBusy { .. } => Cow::Borrowed("driver-busy"),
            Self::ArtifactNotFound { .. } => Cow::Borrowed("artifact-not-found"),
            Self::SliceNotFound { .. } => Cow::Borrowed("slice-not-found"),
            Self::Filesystem { op, .. } => Cow::Owned(format!("filesystem-{op}")),
            Self::BranchPrepareFailed { .. } => Cow::Borrowed("branch-preparation-failed"),
            Self::Io(_) => Cow::Borrowed("io"),
            Self::Yaml(_) => Cow::Borrowed("yaml"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Error;

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
