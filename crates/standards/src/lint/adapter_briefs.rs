//! Shared `set-coverage` / `set-eq` adapter-briefs support.
//!
//! Both interpreters check an adapter manifest's `briefs.keys()`
//! against the operations its axis must declare. The expected
//! operation sets are **policy supplied by the rule's
//! `config: { expected-operations }`**, keyed by axis — never a `const`
//! in the engine (per the standards-layer policy-in-`specify` rule).
//! This module holds the shared config shape so the two interpreters
//! parse it identically; the only inline knowledge that survives is the
//! mechanism mapping a closed [`AdapterAxis`] to its kebab-case token.
//!
//! Lives one level above `lint/eval/` so the `every_interpreter_maps_to_kind`
//! parity test (which treats every `lint/eval/<kind>.rs` module as a
//! hint-kind interpreter) does not mistake this shared helper for an
//! orphan interpreter.

use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::Value as JsonValue;

use crate::lint::AdapterAxis;

/// Parsed `expected-operations` hint configuration shared by
/// `set-coverage` (`adapter-briefs`) and `set-eq` (`adapter-briefs`).
/// The per-axis operation lists are policy supplied by the rule.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) struct ExpectedOperationsConfig {
    expected_operations: AxisOperations,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct AxisOperations {
    #[serde(default)]
    sources: Vec<String>,
    #[serde(default)]
    targets: Vec<String>,
}

impl ExpectedOperationsConfig {
    /// Parse the rule's `config: { expected-operations }`. `None` signals
    /// a missing or malformed config so the caller can raise an
    /// `Unsupported` hint error against its own kind.
    pub(crate) fn parse(config: Option<&JsonValue>) -> Option<Self> {
        serde_json::from_value(config?.clone()).ok()
    }

    /// The operation set a manifest on `axis` must declare in `briefs`,
    /// taken from the rule-supplied config.
    pub(crate) fn expected_for(&self, axis: AdapterAxis) -> BTreeSet<&str> {
        let ops = match axis {
            AdapterAxis::Sources => &self.expected_operations.sources,
            AdapterAxis::Targets => &self.expected_operations.targets,
        };
        ops.iter().map(String::as_str).collect()
    }
}

/// Kebab-case axis token surfaced in the `set-coverage` / `set-eq`
/// structured evidence payloads.
pub(crate) const fn axis_token(axis: AdapterAxis) -> &'static str {
    match axis {
        AdapterAxis::Sources => "sources",
        AdapterAxis::Targets => "targets",
    }
}
