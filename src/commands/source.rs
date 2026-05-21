//! `specify source {resolve}` — RFC-25 source adapter operations.
//!
//! Source adapters carry `axis: source` and the `enumerate` +
//! `extract` capabilities. The run-side dispatch lives on the unified
//! `commands::resolve_plugin` helper (it is byte-identical to the
//! target-axis path apart from the `@version` peel); this module only
//! owns the clap derive surface under [`cli`].

pub mod cli;
