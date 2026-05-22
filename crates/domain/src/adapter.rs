//! Adapter manifest resolution and brief frontmatter parsing.
//!
//! RFC-25 §"Adapter implementation shape" / §"Resolver and cache".
//! Source and target adapters share the [`Adapter`] manifest shape (a
//! kebab-case `name`, integer `version`, the `axis: source | target`
//! discriminator, the closed `operations[]` list, the `briefs.<operation>`
//! map, and an optional `tools[]` declaration) validated against
//! `schemas/adapter.schema.json` and refined per-axis by
//! `schemas/source.schema.json` / `schemas/target.schema.json`.
//!
//! Resolution is path-agnostic: the loader probes
//! `<project_dir>/.specify/.cache/{sources,targets}/<name>/` first
//! (agent-populated cache) and then `<project_dir>/{sources,targets}/<name>/`
//! (in-repo). The cache layout matches RFC-25 §Resolver and cache verbatim
//! so source and target adapters with colliding names disambiguate by axis.

mod brief;
pub mod cache;
mod core;
mod operation;

pub use core::{
    ADAPTER_FILENAME, Adapter, AdapterLocation, AdapterToolDeclaration, Axis, CacheMode,
    ResolvedAdapter, cache_dir,
};

pub use brief::{Brief, BriefFrontmatter, split_on_closing_delimiter};
pub use cache::{
    CacheFingerprint, CacheIndexEntry, CacheLayout, CacheLookup, CacheMissReason,
    FingerprintRecord, FingerprintSource, FingerprintToolVersion, LookupOutcome, SourceOperation,
    append_index, lookup as cache_lookup, read_index as cache_read_index, sha256_file,
    sha256_prefixed, write as cache_write,
};
pub use operation::Operation;
