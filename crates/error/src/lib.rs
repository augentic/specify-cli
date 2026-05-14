//! Unified error types for the `specify` CLI and its domain crates.
//! Every public function returns `Result<T, Error>`; variants are
//! structured so the binary can route them to exit codes and formats.

pub mod display;
pub mod error;
pub mod serde_rfc3339;
pub mod validation;
pub mod yaml;

pub use error::Error;
pub use validation::{Status as ValidationStatus, Summary as ValidationSummary};
pub use yaml::YamlError;

/// Workspace-wide `Result` alias bound to [`Error`].
///
/// Lets call sites write `specify_error::Result<T>` (or `Result<T>`
/// after `use specify_error::Result`) without restating the error
/// parameter; supply an explicit `E` to override on the rare path that
/// returns a non-[`Error`] failure.
pub type Result<T, E = Error> = std::result::Result<T, E>;

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

#[cfg(test)]
#[test]
fn is_kebab_accepts_and_rejects() {
    for ok in ["a", "abc", "alpha-gateway", "x-1", "a1-b2"] {
        assert!(is_kebab(ok), "expected `{ok}` to pass");
    }
    for bad in ["", "-a", "a-", "a--b", "A", "alpha_gateway", "alpha gateway"] {
        assert!(!is_kebab(bad), "expected `{bad}` to fail");
    }
}
