//! Plugin manifest resolution and axis-aware cache routing.
//!
//! RFC-25 §"Adapter implementation shape" / §"Resolver and cache".
//! `crates/domain/src/plugin/` replaces the pre-RFC-25 shared-shape
//! [`crate::adapter::Adapter`] loader: source and target adapters carry
//! `axis: source | target`, closed `operations[]`, `briefs.<operation>`,
//! and an optional `tools[]`, validated against `schemas/plugin.schema.json`
//! (refined by `schemas/source.schema.json` and `schemas/target.schema.json`).
//!
//! Resolution is path-agnostic: the loader probes
//! `<project_dir>/.specify/.cache/{sources,targets}/<name>/` first
//! (agent-populated cache) and then `<project_dir>/{sources,targets}/<name>/`
//! (in-repo). The cache layout matches RFC-25 §Resolver and cache verbatim
//! so source and target adapters with colliding names disambiguate by axis.

mod core;

pub use core::{
    ADAPTER_FILENAME, Axis, Plugin, PluginLocation, PluginToolDeclaration, ResolvedPlugin,
    cache_dir,
};
