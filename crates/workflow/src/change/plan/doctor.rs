//! Health diagnostics layered on top of `Plan::validate`:
//! `cycle-in-depends-on`, `orphan-source-key`,
//! and `stale-workspace-clone`. Surfaced through
//! `specrun plan validate`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::core::{Finding, Plan, Severity};
use crate::registry::Registry;

mod cycle;
mod orphan_source;
mod stale_clone;

pub use cycle::detect;

#[cfg(test)]
mod tests;

/// Stable code for the cycle-detection diagnostic.
pub const CYCLE: &str = "cycle-in-depends-on";
/// Stable code for the orphan-source-key diagnostic — top-level
/// `sources:` key declared but unreferenced by any entry.
pub const ORPHAN_SOURCE: &str = "orphan-source-key";
/// Stable code for the stale-workspace-clone diagnostic. See
/// [`StaleReason`] for the two ways a clone is classified stale.
pub const STALE_CLONE: &str = "stale-workspace-clone";

/// One row in the doctor diagnostic stream.
///
/// Wire shape (kebab-case):
///
/// ```json
/// {
///   "severity": "error" | "warning",
///   "code": "<stable code>",
///   "message": "<human readable>",
///   "entry": null | "<plan entry name>",
///   "data": null | { ... payload ... }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Diagnostic {
    /// Severity bucket.
    pub severity: Severity,
    /// Stable machine-readable code. The four doctor-only codes are the
    /// constants on this module ; validate's codes come
    /// through unchanged.
    pub code: String,
    /// Human-readable description.
    pub message: String,
    /// Offending plan entry name when the finding is entry-local;
    /// `None` for plan-wide findings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    /// Structured payload — `Some` only on the three doctor-specific
    /// codes; `None` for findings forwarded from `Plan::validate`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<DiagnosticPayload>,
}

/// Structured payload carried by the three doctor-only diagnostics.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DiagnosticPayload {
    /// Payload for [`CYCLE`].
    ///
    /// `cycle` is the dependency cycle in stable, alphabetically-sorted
    /// order with the first node repeated at the end so reviewers can
    /// read the loop without mentally closing it.
    Cycle {
        /// Cycle path: `[a, b, c, a]`.
        cycle: Vec<String>,
    },
    /// Payload for [`ORPHAN_SOURCE`].
    OrphanSource {
        /// Top-level `sources:` key that no entry references.
        key: String,
    },
    /// Payload for [`STALE_CLONE`].
    StaleClone {
        /// Registry project name whose `.specify/workspace/<project>/`
        /// slot is out of sync.
        project: String,
        /// Why the slot is classified stale.
        reason: StaleReason,
        /// Registry's expected signature for the slot.
        #[serde(skip_serializing_if = "Option::is_none")]
        expected: Option<CloneSignature>,
        /// Slot's observed signature, when inspectable.
        #[serde(skip_serializing_if = "Option::is_none")]
        observed: Option<CloneSignature>,
    },
}

/// Why a workspace clone is classified stale by [`STALE_CLONE`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, strum::Display,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum StaleReason {
    /// A remote-backed clone's `origin` differs from the registry URL.
    SignatureChanged,
    /// Slot materialisation does not match the registry URL class or target.
    SlotMismatch,
}

/// Snapshot of the registry or slot signature for staleness comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct CloneSignature {
    /// Materialisation kind (`git-clone`, `symlink`, or `other`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slot_kind: Option<String>,
    /// Repo URL — registry's `url` for the expected signature; git
    /// `origin` for observed remote-backed slots.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Adapter identifier from the registry's `adapter` field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    /// Canonical filesystem target for symlink-backed slots.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

impl Diagnostic {
    /// Forward a `Plan::validate` finding to the doctor stream
    /// without payload data, preserving the original code and
    /// severity.
    fn from_finding(f: &Finding) -> Self {
        Self {
            severity: f.level,
            code: f.code.to_string(),
            message: f.message.clone(),
            entry: f.entry.clone(),
            data: None,
        }
    }
}

/// Run every `Plan::validate` check, then layer the three doctor-only
/// diagnostics on top.
///
/// `slices_dir` and `registry` are forwarded to `Plan::validate` so
/// the validate-level findings are bit-identical to those emitted by
/// `specrun plan validate`. `project_dir` is consulted only by the
/// stale-workspace-clone check; pass `None` to skip that check
/// (`Plan::doctor_pure` does the same — see the unit tests).
///
/// The order in the returned vector is stable:
///
///   1. Every `Plan::validate` finding, in the existing order.
///   2. Cycle diagnostics (one per cycle, deduplicated by node-set).
///   3. Orphan source-key diagnostics (sorted by key).
///   4. Stale workspace clone diagnostics (sorted by project name).
#[must_use]
pub fn doctor(
    plan: &Plan, slices_dir: Option<&Path>, registry: Option<&Registry>, project_dir: Option<&Path>,
) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> =
        plan.validate(slices_dir, registry).iter().map(Diagnostic::from_finding).collect();

    out.extend(detect(&plan.entries));
    out.extend(orphan_source::detect(plan));
    if let (Some(reg), Some(dir)) = (registry, project_dir) {
        out.extend(stale_clone::detect(reg, dir));
    }

    out
}
