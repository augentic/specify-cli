//! `specify vectis versions` — resolve and emit the active version pins.
//!
//! A read-only subcommand that resolves the version pin hierarchy
//! (embedded → user → project → `--version-file` override) and emits the
//! full resolved set as JSON. Skills and briefs shell out to this instead
//! of hardcoding dependency versions.

use crate::error::VectisError;
use crate::versions::Versions;
use crate::{CommandOutcome, VersionsArgs};

/// Resolve the active version pins and return them as JSON.
///
/// # Errors
///
/// Returns `VectisError::InvalidProject` when the version file is missing
/// or malformed, and `VectisError::Internal` if serialization fails.
pub fn run(args: &VersionsArgs) -> Result<CommandOutcome, VectisError> {
    let project_dir =
        args.dir.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
    let versions = Versions::resolve(&project_dir, args.version_file.as_deref())?;
    let value = serde_json::to_value(&versions).map_err(|e| VectisError::Internal {
        message: format!("failed to serialize versions: {e}"),
    })?;
    Ok(CommandOutcome::Success(value))
}
