//! YAML newtype wrappers that hide `serde_saphyr` from public surfaces.
//!
//! Pairs with [`crate::Error`]'s `Yaml` / `YamlSer` variants so the
//! upstream crate name does not leak through every `specify-*` API.

/// Newtype wrapper around [`serde_saphyr::Error`].
///
/// Hides the upstream crate name from every `specify-*` public
/// surface. Implements `From<serde_saphyr::Error>` so call sites can
/// keep propagating saphyr deser errors via `?`.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct YamlError(#[from] serde_saphyr::Error);

/// Newtype wrapper around [`serde_saphyr::ser::Error`]. Pairs with
/// [`YamlError`]; same rationale (no upstream-name leak).
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct YamlSerError(#[from] serde_saphyr::ser::Error);
