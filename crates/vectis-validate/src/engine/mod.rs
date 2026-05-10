//! Deterministic validation engine for the `vectis-validate` command.
//!
//! Public surface: [`run`] dispatches a parsed [`Args`] to the per-mode
//! handler. Each per-mode envelope carries a uniform shape:
//!
//! ```json
//! {
//!   "mode": "assets",
//!   "path": "design-system/assets.yaml",
//!   "errors":   [{ "path": "/assets/foo/sources/ios/1x", "message": "..." }],
//!   "warnings": [{ "path": "/assets/foo/sources/android", "message": "..." }]
//! }
//! ```
//!
//! Errors / warnings entries carry a JSON Pointer-shaped `path` so the
//! operator can locate the offending sub-document. The dispatcher
//! exits non-zero only when a real sub-report carries errors. Provenance
//! and the rationale behind every rule live in
//! `crates/vectis-validate/DECISIONS.md`.

mod all;
mod assets;
mod composition;
mod layout;
mod paths;
mod shared;
mod tokens;

use std::path::Path;

pub use paths::{
    discover_artifact, expand_path_template, find_project_root, paths_for_key,
    resolve_default_path_with_root,
};
use serde_json::Value;
pub use shared::{assets_validator, composition_validator, tokens_validator};

use crate::error::VectisError;
use crate::{Args, CommandOutcome, ValidateMode};

/// Dispatch a `vectis validate` invocation to the per-mode handler.
///
/// # Errors
///
/// Returns [`VectisError::InvalidProject`] when the resolved
/// `tokens.yaml` / `assets.yaml` / `layout.yaml` / `composition.yaml`
/// is unreadable in single-mode runs (missing file, permission
/// denied; `validate all` instead surfaces the missing input as a
/// synthetic `skipped: true` sub-report) and [`VectisError::Internal`]
/// if an embedded schema fails to compile. YAML parse failures and
/// schema validation failures are *not* errors at this layer; they are
/// folded into the `errors` array of the per-mode envelope so the
/// operator sees the full report alongside any other findings.
pub fn run(args: &Args) -> Result<CommandOutcome, VectisError> {
    match args.mode {
        ValidateMode::Tokens => tokens::validate(args.path.as_deref()),
        ValidateMode::Assets => assets::validate(args.path.as_deref()),
        ValidateMode::Layout => layout::validate(args.path.as_deref()),
        ValidateMode::Composition => composition::validate(args.path.as_deref()),
        ValidateMode::All => all::validate(args.path.as_deref()),
    }
}

/// Re-enter [`run`] for the auto-invoke path. Runs the named sub-mode
/// against the supplied path and returns its envelope (the `Value`
/// inside [`CommandOutcome::Success`]). Used by composition mode to
/// fold sibling tokens / assets envelopes, and by `validate all` to
/// dispatch each sub-mode in turn.
pub fn run_inner(mode: ValidateMode, path: &Path) -> Result<Value, VectisError> {
    let inner_args = Args {
        mode,
        path: Some(path.to_path_buf()),
    };
    let CommandOutcome::Success(value) = run(&inner_args)?;
    Ok(value)
}
