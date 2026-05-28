//! `specrun rules export` handler — `ResolvedRules` export contract.
//!
//! Read-only. Builds the `ResolveInputs` struct from CLI args,
//! delegates to [`specify_lints::build_resolved_rules`], and
//! streams the resulting envelope to stdout as JSON. v1 supports
//! JSON output only; the global `--format text` default at the
//! `Cli` level surfaces as `Error::Argument` (exit 2) so the
//! JSON-only contract is explicit.
//!
//! Failure modes round-trip through `Exit::from(&Error)` per
//! `docs/standards/handler-shape.md`:
//!
//! | `ResolveError` variant   | `Error` variant         | Exit |
//! |--------------------------|-------------------------|------|
//! | `RulesRootRequired`      | `Validation`            | 2    |
//! | `DuplicateRuleId`        | `Validation`            | 2    |
//! | `Parse`                  | `Validation`            | 2    |
//! | `Filesystem`             | `Filesystem { op }`     | 1    |

use std::path::{Path, PathBuf};

use specify_error::{Error, Result};
use specify_lints::{ResolveInputs, build_resolved_rules, map_resolve_error};

use crate::runtime::cli::Format;
use crate::runtime::output;

/// Run the export with explicit pre-parsed arguments. Splitting the
/// signature off the clap-struct keeps the dispatcher arm flat.
#[expect(
    clippy::too_many_arguments,
    reason = "Arguments mirror the closed rules resolution input set; the handler threads the clap-derived surface through verbatim into ResolveInputs."
)]
pub fn run(
    format: Format, rules_root: Option<&Path>, target: &str, sources: &[String],
    artifacts: &[PathBuf], languages: &[String], include_deprecated: bool, include_unmatched: bool,
    include_core: bool, project_dir: &Path,
) -> Result<()> {
    require_json(format)?;

    let inputs = ResolveInputs {
        project_dir,
        rules_root,
        target_adapter: target,
        source_adapters: sources,
        artifact_paths: artifacts,
        languages,
        include_deprecated,
        include_unmatched,
        include_core,
    };

    let resolved = build_resolved_rules(&inputs).map_err(map_resolve_error)?;

    output::emit(&mut std::io::stdout().lock(), Format::Json, &resolved, |_, _| Ok(()))?;
    Ok(())
}

/// Reject `--format text` with an argument-shape error (exit 2).
fn require_json(format: Format) -> Result<()> {
    match format {
        Format::Json => Ok(()),
        Format::Text => Err(Error::Argument {
            flag: "--format",
            detail: "specrun rules export currently emits JSON only; rerun with --format json"
                .to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// `--format text` must fail with `Error::Argument` so
    /// `Exit::from` lands on exit 2 (argument shape).
    #[test]
    fn rejects_text_format_with_argument_error() {
        let err = run(
            Format::Text,
            None,
            "omnia",
            &[],
            &[],
            &[],
            false,
            false,
            false,
            &PathBuf::from("."),
        )
        .expect_err("text format must be rejected");
        match err {
            Error::Argument { flag, detail } => {
                assert_eq!(flag, "--format");
                assert!(detail.contains("--format json"), "detail missing hint: {detail}");
            }
            other => panic!("expected Error::Argument, got {other:?}"),
        }
    }
}
