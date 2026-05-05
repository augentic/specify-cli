//! `specify vectis validate <mode> [path]` -- schema and cross-artifact
//! validation surface (RFC-11 §H, §I).
//!
//! Phase 1.5 (RFC-11 implementation plan) lands the dispatcher and
//! `clap` plumbing only. Every mode returns
//! [`CommandOutcome::Stub`] so the dispatcher emits the v2
//! `not-implemented` envelope and the caller exits non-zero. The five
//! modes are filled in incrementally:
//!
//! - **Phase 1.6** -- `tokens` mode validates against
//!   `schemas/vectis/tokens.schema.json` (Appendix A).
//! - **Phase 1.7** -- `assets` mode validates against
//!   `schemas/vectis/assets.schema.json` (Appendix B) plus referenced
//!   file existence and per-platform density coverage (§E).
//! - **Phase 1.8** -- `layout` mode validates as the unwired subset of
//!   `composition.schema.json`, including the §G structural-identity
//!   rule for any `component:` directives present.
//! - **Phase 1.9** -- `composition` mode adds cross-artifact resolution
//!   and auto-invokes `tokens` / `assets` when sibling files exist.
//! - **Phase 1.10** -- `all` runs the four modes in turn, plus the
//!   `artifacts:`-block default-path resolution every mode shares.
//!
//! Until those phases land, the JSON Schema crate the rest of the
//! workspace already depends on (`jsonschema = { version = "0.46",
//! default-features = false }`, declared in the root `Cargo.toml`) is
//! the canonical choice. Phases 1.6+ should add it to this crate's
//! direct dependencies and reuse the validator pattern in
//! `crates/schema/src/schema.rs::validate_against_meta` rather than
//! introducing a sibling.

use crate::error::VectisError;
use crate::{CommandOutcome, ValidateArgs, ValidateMode};

/// Dispatch a `vectis validate` invocation to the per-mode handler.
///
/// Phase 1.5 every variant returns
/// [`CommandOutcome::Stub`] with a `command` string of the form
/// `"validate <mode>"`. The dispatcher in `src/commands/vectis.rs`
/// turns that into the v2 `not-implemented` JSON envelope (with a
/// `"command": "validate <mode>"` field) and a humanised text
/// fallback. The five mode strings (`layout`, `composition`, `tokens`,
/// `assets`, `all`) are kebab-case identical to the
/// [`ValidateMode`] discriminant so JSON consumers and text operators
/// see the same identifier.
///
/// # Errors
///
/// This stub never errors. Phases 1.6-1.10 will return
/// [`VectisError::Io`] / [`VectisError::InvalidProject`] /
/// [`VectisError::Internal`] when YAML parsing, schema compilation,
/// or fixture loading fails.
// `const fn` looks tempting here today (the body is pure) but every
// later phase's body has to read files, parse YAML, and compile JSON
// Schemas -- so this MUST stay non-const to avoid a churny signature
// flip when Phase 1.6 lands.
#[allow(
    clippy::missing_const_for_fn,
    reason = "stub today; Phases 1.6-1.10 add IO and YAML parsing."
)]
pub fn run(args: &ValidateArgs) -> Result<CommandOutcome, VectisError> {
    let command = match args.mode {
        ValidateMode::Layout => "validate layout",
        ValidateMode::Composition => "validate composition",
        ValidateMode::Tokens => "validate tokens",
        ValidateMode::Assets => "validate assets",
        ValidateMode::All => "validate all",
    };
    Ok(CommandOutcome::Stub { command })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_returns_a_stub_with_kebab_command() {
        for (mode, expected) in [
            (ValidateMode::Layout, "validate layout"),
            (ValidateMode::Composition, "validate composition"),
            (ValidateMode::Tokens, "validate tokens"),
            (ValidateMode::Assets, "validate assets"),
            (ValidateMode::All, "validate all"),
        ] {
            let args = ValidateArgs { mode, path: None };
            let outcome = run(&args).expect("stub never errors");
            match outcome {
                CommandOutcome::Stub { command } => assert_eq!(command, expected),
                CommandOutcome::Success(value) => {
                    panic!("expected Stub for {mode:?}, got Success({value})")
                }
            }
        }
    }

    #[test]
    fn explicit_path_does_not_change_stub_outcome() {
        let args = ValidateArgs {
            mode: ValidateMode::Tokens,
            path: Some(std::path::PathBuf::from("design-system/tokens.yaml")),
        };
        let outcome = run(&args).expect("stub never errors");
        match outcome {
            CommandOutcome::Stub { command } => assert_eq!(command, "validate tokens"),
            CommandOutcome::Success(_) => panic!("expected Stub, got Success"),
        }
    }

    #[test]
    fn validate_mode_as_str_matches_value_enum_spelling() {
        for (mode, expected) in [
            (ValidateMode::Layout, "layout"),
            (ValidateMode::Composition, "composition"),
            (ValidateMode::Tokens, "tokens"),
            (ValidateMode::Assets, "assets"),
            (ValidateMode::All, "all"),
        ] {
            assert_eq!(mode.as_str(), expected);
        }
    }
}
