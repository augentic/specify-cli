//! Hint and discriminant tables for [`crate::Error`].
//!
//! `variant_str` is the wire contract surfaced as the JSON `error`
//! field; `hint` is the long-form recovery line the TTY renderer pulls
//! alongside it.

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
            Self::PlanIncomplete { .. } => Some(
                "complete or drop the listed entries, or rerun with --force to archive anyway.",
            ),
            Self::Diag { code, .. } => match *code {
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
            Self::ContextLockTooNew { .. } => Cow::Borrowed("context-lock-version-too-new"),
            Self::ContextLockMalformed { .. } => Cow::Borrowed("context-lock-malformed"),
            Self::Validation { .. } => Cow::Borrowed("validation"),
            Self::Lifecycle { .. } => Cow::Borrowed("lifecycle"),
            Self::CliTooOld { .. } => Cow::Borrowed("specify-version-too-old"),
            Self::PlanTransition { .. } => Cow::Borrowed("plan-transition"),
            Self::PlanIncomplete { .. } => Cow::Borrowed("plan-has-outstanding-work"),
            Self::PlanNonTerminalEntries { .. } => Cow::Borrowed("non-terminal-entries-present"),
            Self::ChangeBriefExists { .. } => Cow::Borrowed("already-exists"),
            Self::DriverBusy { .. } => Cow::Borrowed("driver-busy"),
            Self::ArtifactNotFound { .. } => Cow::Borrowed("artifact-not-found"),
            Self::SliceNotFound { .. } => Cow::Borrowed("slice-not-found"),
            Self::CapabilityManifestMissing { .. } => Cow::Borrowed("capability-manifest-missing"),
            Self::Filesystem { op, .. } => Cow::Owned(format!("filesystem-{op}")),
            Self::CapabilityCheckFailed { .. } => Cow::Borrowed("capability-check-failed"),
            Self::SliceValidationFailed { .. } => Cow::Borrowed("slice-validation-failed"),
            Self::BranchPrepareFailed { .. } => Cow::Borrowed("branch-preparation-failed"),
            Self::ToolDenied(_) => Cow::Borrowed("tool-permission-denied"),
            Self::ToolNotDeclared { .. } => Cow::Borrowed("tool-not-declared"),
            Self::InvalidName(_) => Cow::Borrowed("invalid-name"),
            Self::ChangeFinalizeBlocked { .. } => Cow::Borrowed("change-finalize-blocked"),
            Self::Io(_) => Cow::Borrowed("io"),
            Self::Yaml(_) => Cow::Borrowed("yaml"),
            Self::YamlSer(_) => Cow::Borrowed("yaml-ser"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Error;

    #[test]
    fn init_requires_capability_or_hub_display() {
        let err = Error::Diag {
            code: "init-requires-capability-or-hub",
            detail: "pass <capability> or --hub".to_string(),
        };
        assert_eq!(err.variant_str(), "init-requires-capability-or-hub");
        assert!(
            err.to_string().starts_with("init-requires-capability-or-hub: "),
            "init-requires-capability-or-hub diag must start with the kebab discriminant, got: {err}"
        );
    }

    #[test]
    fn context_diagnostic_variant_strings_are_stable() {
        let diag = |code: &'static str, detail: &str| Error::Diag {
            code,
            detail: detail.to_string(),
        };
        let cases = [
            (
                diag(
                    "context-existing-unfenced-agents-md",
                    "AGENTS.md exists without Specify fences",
                ),
                "context-existing-unfenced-agents-md",
            ),
            (
                diag(
                    "context-fenced-content-modified",
                    "AGENTS.md drifted from .specify/context.lock",
                ),
                "context-fenced-content-modified",
            ),
            (
                diag("context-lock-missing", ".specify/context.lock is missing"),
                "context-lock-missing",
            ),
            (diag("context-not-generated", "AGENTS.md is missing"), "context-not-generated"),
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
        let err = Error::Diag {
            code: "plan-structural-errors",
            detail: "plan has structural errors; run 'specify change plan validate' for detail"
                .to_string(),
        };
        assert_eq!(err.variant_str(), "plan-structural-errors");
        assert!(
            err.to_string().starts_with("plan-structural-errors"),
            "plan-structural-errors diag must start with `plan-structural-errors`, got: {err}"
        );
    }

    #[test]
    fn change_finalize_variant_strings_are_stable() {
        let cases = [
            (
                Error::Diag {
                    code: "plan-not-found",
                    detail: "no plan to finalize: plan.yaml is absent.".to_string(),
                },
                "plan-not-found",
            ),
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
            ("unknown-op", "filesystem-unknown-op"),
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
}
