//! Serde adapter for `PathBuf` rendered as `Path::display`.
//!
//! Wired by `#[serde(serialize_with = "specify_error::serde_path_display::serialize")]`
//! on a `PathBuf` field. Output is the lossy UTF-8 form of the path —
//! the same shape the CLI text writers already produce. There is no
//! deserialiser: domain types use this for one-way wire emission only.

use std::path::PathBuf;

use serde::Serializer;

/// Serialise a [`PathBuf`] as its `Path::display` lossy UTF-8 form.
///
/// The `&PathBuf` parameter is required by serde's `serialize_with`
/// contract — the field's owned type must match exactly — so the
/// `clippy::ptr_arg` lint is intentionally suppressed here.
///
/// # Errors
///
/// Propagates any error produced by the underlying [`Serializer`].
#[expect(
    clippy::ptr_arg,
    reason = "serde `serialize_with` requires `&FieldType` (here `&PathBuf`)"
)]
pub fn serialize<S: Serializer>(value: &PathBuf, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_str(&value.display())
}
