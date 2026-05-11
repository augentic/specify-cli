//! `specify change plan doctor` — superset of `specify change plan validate` plus four
//! health diagnostics:
//!
//!   - `cycle-in-depends-on`   (error, payload: cycle path)
//!   - `orphan-source-key`     (warning, payload: unreferenced source key)
//!   - `stale-workspace-clone` (warning, payload: project / reason / signatures)
//!   - `unreachable-entry`     (error, payload: blocking predecessors)
//!
//! Doctor is purely additive: it runs every check `Plan::validate` runs,
//! preserves the existing diagnostic codes (`dependency-cycle`,
//! `unknown-depends-on`, `unknown-source`, `multiple-in-progress`,
//! `project-*`, `capability-mismatch-workspace`, …) bit-for-bit, and then
//! layers the four codes above on top with structured payloads. The
//! `Plan::validate` and `Plan::next_eligible` runtime semantics are not
//! changed by anything in this module.
//!
//! ## Stale workspace slot contract
//!
//! `workspace sync` is the authority for whether an existing slot
//! matches `registry.yaml`: remote-backed slots must be git work trees
//! whose `origin` equals the registry URL, and local/relative slots must
//! be symlinks whose canonical target equals the registry target.
//! Doctor reads the same slot-problem inspector from `specify-registry`.
//! A missing `.specify-sync.yaml` stamp is not a warning; only an actual
//! mismatch that sync would refuse is reported.
//!
//! ## Schema-mismatch overlap
//!
//! `Plan::validate` already emits `capability-mismatch-workspace` when a
//! clone's `.specify/project.yaml:capability` disagrees with the registry's
//! declared capability. Doctor's `stale-workspace-clone` is a *URL* check;
//! the capability check stays on `validate`. Operators see both signals
//! when the clone is out of sync on both axes, and the codes are
//! orthogonal so dashboards can route each to the right runbook.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::core::{Finding, Plan, Severity};
use crate::registry::Registry;

mod cycle;
mod orphan_source;
mod stale_clone;
mod unreachable;

#[cfg(test)]
mod tests;

/// Stable code for the cycle-detection diagnostic.
///
/// Distinct from validate's `dependency-cycle` so dashboards can route
/// the doctor-only structured payload separately from validate's
/// message-only string.
pub const CYCLE: &str = "cycle-in-depends-on";
/// Stable code for the orphan-source-key diagnostic — top-level
/// `sources:` key declared but unreferenced by any entry.
pub const ORPHAN_SOURCE: &str = "orphan-source-key";
/// Stable code for the stale-workspace-clone diagnostic. See
/// [`StaleReason`] for the two ways a clone is classified stale.
pub const STALE_CLONE: &str = "stale-workspace-clone";
/// Stable code for the unreachable-entry diagnostic — pending entry
/// whose dependency closure is rooted in `failed`/`skipped`.
pub const UNREACHABLE: &str = "unreachable-entry";

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
    pub severity: DiagnosticSeverity,
    /// Stable machine-readable code. The four doctor-only codes are the
    /// constants on this module (`CODE_*`); validate's codes come
    /// through unchanged.
    pub code: String,
    /// Human-readable description.
    pub message: String,
    /// Offending plan entry name when the finding is entry-local;
    /// `None` for plan-wide findings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    /// Structured payload — `Some` only on the four doctor-specific
    /// codes; `None` for findings forwarded from `Plan::validate`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<DiagnosticPayload>,
}

crate::kebab_enum! {
    /// JSON-shape mirror of [`Severity`] with kebab-case casing for wire
    /// output.
    #[derive(Debug)]
    pub enum DiagnosticSeverity {
        /// Blocking problem.
        Error => "error",
        /// Non-blocking advisory.
        Warning => "warning",
    }
}

impl DiagnosticSeverity {
    /// Fixed wire string. Alias for [`Self::as_str`] preserved for
    /// back-compat.
    #[must_use]
    pub const fn label(self) -> &'static str {
        self.as_str()
    }
}

impl From<&Severity> for DiagnosticSeverity {
    fn from(value: &Severity) -> Self {
        match value {
            Severity::Error => Self::Error,
            Severity::Warning => Self::Warning,
        }
    }
}

/// Structured payload carried by the four doctor-only diagnostics.
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
    /// Payload for [`UNREACHABLE`].
    UnreachableEntry {
        /// The unreachable plan entry.
        entry: String,
        /// Each immediate `depends-on` predecessor that contributes to
        /// the unreachability — either by being terminal-blocking
        /// (`failed`/`skipped`) or by itself being unreachable.
        blocking: Vec<BlockingPredecessor>,
    },
}

crate::kebab_enum! {
    /// Why a workspace clone is classified stale by [`STALE_CLONE`].
    #[derive(Debug)]
    pub enum StaleReason {
        /// A remote-backed clone's `origin` differs from the registry URL.
        SignatureChanged => "signature-changed",
        /// Slot materialisation does not match the registry URL class or target.
        SlotMismatch => "slot-mismatch",
        /// Retained for old JSON consumers. Doctor no longer emits this
        /// reason because sync does not write `.specify-sync.yaml`.
        MissingSyncStamp => "missing-sync-stamp",
    }
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
    /// Capability identifier from the registry's `capability` field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    /// Canonical filesystem target for symlink-backed slots.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

/// One immediate predecessor of an unreachable entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct BlockingPredecessor {
    /// Predecessor plan-entry name.
    pub name: String,
    /// Predecessor's current plan-entry status (always one of
    /// `failed`, `skipped`, or `pending` — pending appears when the
    /// predecessor is itself unreachable; the chain is reported via
    /// the predecessor's own `unreachable-entry` diagnostic).
    pub status: String,
}

impl Diagnostic {
    /// Forward a `Plan::validate` finding to the doctor stream
    /// without payload data, preserving the original code and
    /// severity.
    fn from_finding(f: &Finding) -> Self {
        Self {
            severity: DiagnosticSeverity::from(&f.level),
            code: f.code.to_string(),
            message: f.message.clone(),
            entry: f.entry.clone(),
            data: None,
        }
    }
}

/// Run every `Plan::validate` check, then layer the four doctor-only
/// diagnostics on top.
///
/// `slices_dir` and `registry` are forwarded to `Plan::validate` so
/// the validate-level findings are bit-identical to those emitted by
/// `specify change plan validate`. `project_dir` is consulted only by the
/// stale-workspace-clone check; pass `None` to skip that check
/// (`Plan::doctor_pure` does the same — see the unit tests).
///
/// The order in the returned vector is stable:
///
///   1. Every `Plan::validate` finding, in the existing order.
///   2. Cycle diagnostics (one per cycle, deduplicated by node-set).
///   3. Orphan source-key diagnostics (sorted by key).
///   4. Stale workspace clone diagnostics (sorted by project name).
///   5. Unreachable-entry diagnostics (sorted by entry name).
#[must_use]
pub fn doctor(
    plan: &Plan, slices_dir: Option<&Path>, registry: Option<&Registry>, project_dir: Option<&Path>,
) -> Vec<Diagnostic> {
    let mut out: Vec<Diagnostic> =
        plan.validate(slices_dir, registry).iter().map(Diagnostic::from_finding).collect();

    out.extend(cycle::detect(&plan.entries));
    out.extend(orphan_source::detect(plan));
    if let (Some(reg), Some(dir)) = (registry, project_dir) {
        out.extend(stale_clone::detect(reg, dir));
    }
    out.extend(unreachable::detect(&plan.entries));

    out
}
