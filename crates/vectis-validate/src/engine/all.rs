//! `validate all` — fan out across every per-mode validator and fold
//! the per-mode envelopes into a combined `{ "mode": "all", "results":
//! [...] }` shape.

use std::path::Path;

use serde_json::{Value, json};

use super::paths::{default_project_root, resolve_default_path_with_root};
use super::run_inner;
use crate::error::VectisError;
use crate::{CommandOutcome, ValidateMode};

/// Run every per-mode validator against the supplied project root (or
/// CWD when none is given) and fold the per-mode envelopes into a
/// combined envelope.
///
/// Sub-mode order: `layout`, `composition`, `tokens`, `assets` —
/// matches the operator-friendly "structural input → wired
/// composition → cross-artifact references" pipeline. When a sub-mode's
/// default-resolved input does not exist on disk, the sub-report is a
/// synthetic `{ mode, path, errors: [], warnings: [], skipped: true,
/// message: ... }` so the combined run continues; the dispatcher's
/// `validate_exit_code` recurses through `results[*].report` and only
/// flips to non-zero when a real sub-report has errors.
///
/// # Errors
///
/// Returns [`VectisError::InvalidProject`] when a sub-mode whose input
/// IS present on disk fails to read it, and [`VectisError::Internal`]
/// if an embedded schema fails to compile.
pub(super) fn validate(path: Option<&Path>) -> Result<CommandOutcome, VectisError> {
    let project_root = path.map_or_else(default_project_root, Path::to_path_buf);

    let mut results: Vec<Value> = Vec::new();
    for mode in [
        ValidateMode::Layout,
        ValidateMode::Composition,
        ValidateMode::Tokens,
        ValidateMode::Assets,
    ] {
        let target = resolve_default_path_with_root(mode, &project_root);
        let report = if target.is_file() {
            run_inner(mode, &target)?
        } else {
            json!({
                "mode": mode.as_str(),
                "path": target.display().to_string(),
                "errors": Vec::<Value>::new(),
                "warnings": Vec::<Value>::new(),
                "skipped": true,
                "message": format!(
                    "no input found at {}; default-resolved via the artifacts: block (or its embedded fallback)",
                    target.display(),
                ),
            })
        };
        results.push(json!({
            "mode": mode.as_str(),
            "report": report,
        }));
    }

    Ok(CommandOutcome::Success(json!({
        "mode": ValidateMode::All.as_str(),
        "path": project_root.display().to_string(),
        "results": results,
    })))
}
