//! Embedded schema sources shared between `validate` (lazy-compiled
//! validators) and `schema` (raw schema retrieval).

/// Canonical tool-owned `tokens.schema.json` (the tool-owned schema and catalog decisions D1).
pub(crate) const TOKENS_SCHEMA_SOURCE: &str = include_str!("../embedded/tokens.schema.json");

/// Canonical tool-owned `assets.schema.json` (the tool-owned schema and catalog decisions D1).
pub(crate) const ASSETS_SCHEMA_SOURCE: &str = include_str!("../embedded/assets.schema.json");

/// Canonical tool-owned `composition.schema.json` (the tool-owned schema and catalog decisions D1).
/// Shared between `layout` mode (unwired-subset runtime) and
/// `composition` mode (full lifecycle runtime).
pub(crate) const COMPOSITION_SCHEMA_SOURCE: &str =
    include_str!("../embedded/composition.schema.json");
