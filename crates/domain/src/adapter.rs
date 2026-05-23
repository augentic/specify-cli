//! Adapter manifest resolution.
//!
//! RFC-25 §"Adapter implementation shape" / §"Resolver and cache".
//! Source and target adapters share the `adapter.yaml` wire shape but
//! split into [`SourceAdapter`] / [`TargetAdapter`] in memory, each
//! carrying its closed operation set ([`SourceOperation`] /
//! [`TargetOperation`]) as the typed `briefs.keys()` source-of-truth.
//! The split pushes the string boundary out to the YAML parse step;
//! see [DECISIONS.md §"Operations typed at parse boundary"] for the
//! rationale.
//!
//! Resolution is path-agnostic: each axis-specific loader probes
//! `<project_dir>/.specify/.cache/manifests/{sources,targets}/<name>/`
//! first (the agent-populated manifest cache) and then
//! `<project_dir>/adapters/{sources,targets}/<name>/` (in-repo). The
//! manifest cache mirrors the in-repo adapter tree so source and
//! target adapters with colliding names disambiguate by axis. The
//! RFC-27 §D8 per-source extraction result cache lives in a sibling
//! tree under `<project_dir>/.specify/.cache/extractions/<adapter>/`
//! (with `index.jsonl` at the adapter root) — see
//! [DECISIONS.md §"Cache layout"].
//!
//! Brief bodies are read by the agent at the path returned by
//! [`ResolvedSourceAdapter::brief_path`] / [`ResolvedTargetAdapter::brief_path`];
//! the CLI never parses brief markdown. Per the plugin-repo standard
//! ([`docs/standards/skill-authoring.md`](https://github.com/augentic/specify/blob/main/docs/standards/skill-authoring.md)
//! §"Brief authoring"), briefs carry no YAML frontmatter.
//!
//! [DECISIONS.md §"Operations typed at parse boundary"]: ../../../DECISIONS.md#operations-typed-at-parse-boundary
//! [DECISIONS.md §"Cache layout"]: ../../../DECISIONS.md#cache-layout

pub mod cache;
mod core;
pub(crate) mod operation;

pub use core::{
    ADAPTER_FILENAME, ADAPTERS_DIR, AdapterLocation, AdapterToolDeclaration, Axis, CacheMode,
    EXTRACTIONS_CACHE_DIR, MANIFESTS_CACHE_DIR, ResolvedSourceAdapter, ResolvedTargetAdapter,
    SourceAdapter, TargetAdapter, adapter_axis_dir, cache_dir, check_axis_unique_for_name,
};

pub use cache::{
    CacheFingerprint, CacheIndexEntry, CacheLayout, CacheLookup, CacheMissReason,
    FingerprintRecord, FingerprintSource, FingerprintToolVersion, LookupOutcome, SourceOperation,
    append_index, lookup as cache_lookup, read_index as cache_read_index, sha256_file,
    sha256_prefixed, write as cache_write,
};
pub use operation::TargetOperation;
