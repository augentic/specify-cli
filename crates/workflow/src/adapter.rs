//! Adapter manifest resolution.
//!
//! workflow §"Adapter implementation shape" / §"Resolver and cache".
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
//! extraction cache fingerprint contract per-source extraction result cache lives in a sibling
//! tree under `<project_dir>/.specify/.cache/extractions/<adapter>/`
//! (with `index.jsonl` at the adapter root) — see
//! [DECISIONS.md §"Cache layout"].
//!
//! Brief bodies are read by the agent from paths declared in each
//! manifest's typed `briefs` map; the CLI never parses brief markdown.
//! Per the plugin-repo standard
//! ([`docs/standards/skill-authoring.md`](https://github.com/augentic/specify/blob/main/docs/standards/skill-authoring.md)
//! §"Brief authoring"), briefs carry no YAML frontmatter.
//!
//! [DECISIONS.md §"Operations typed at parse boundary"]: ../../../DECISIONS.md#operations-typed-at-parse-boundary
//! [DECISIONS.md §"Cache layout"]: ../../../DECISIONS.md#cache-layout

pub mod cache;
mod core;
pub(crate) mod operation;

pub use core::{
    ADAPTER_FILENAME, AdapterLocation, Axis, CacheMode, Execution, ResolvedTargetAdapter,
    SourceAdapter, TargetAdapter, cache_dir, check_axis_unique_for_name,
};

pub use cache::{CacheLayout, SourceOperation, read_index as cache_read_index};
pub use operation::TargetOperation;
