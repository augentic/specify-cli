//! Type definitions for `plan.yaml` (`Plan`, `Entry`, `EntryPatch`,
//! `Status`, `Lifecycle`, `Severity`, `Finding`). Behaviour lives in
//! the sibling submodules.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::evidence::ClaimKind;

/// Lifecycle state of a single entry in [`Plan::entries`].
///
/// RFC-25 collapses the per-entry state machine to three states:
/// `pending` (default after `plan add` / `plan amend`), `in-progress`
/// (written only by `plan next`), and `done` (written by
/// `plan transition <name> done` — the final per-entry transition,
/// stamped by `/spec:merge`). Build failures and merge conflicts leave
/// the active entry `in-progress`; v1 has no per-entry `blocked`,
/// `failed`, or `skipped` state.
///
/// The enum is `Copy + Eq + Hash` so it can appear in `HashSet`s,
/// `match` guards, and hash-keyed lookups without clones. Transition
/// table methods live alongside [`Plan::transition`].
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    strum::Display,
    clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Status {
    /// Not yet started.
    Pending,
    /// Currently being executed.
    InProgress,
    /// Completed successfully.
    Done,
}

/// Plan-level lifecycle state stored at the top of `plan.yaml`
/// (RFC-25 §Workflow vocabulary).
///
/// Two stored states only — `pending` (default after `plan create`)
/// and `reviewed` (operator-stamped at Gate 1 via
/// `specify plan transition <plan-name> reviewed`). "Currently
/// executing" and "drained" are computed from per-entry [`Status`] at
/// read time via [`Plan::is_executing`] / [`Plan::is_drained`].
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
    strum::Display,
    clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Lifecycle {
    /// Default after `plan create`; awaits operator review at Gate 1.
    #[default]
    Pending,
    /// Operator has stamped Gate 1 — `/spec:execute` is now legal.
    Reviewed,
}

/// In-memory model of `plan.yaml` (at the repo root).
///
/// A `Plan` is an ordered, dependency-aware list of [`Entry`]s plus
/// a named map of [`Plan::sources`] (local paths or git URLs) that the
/// entries draw from, gated by a top-level [`Plan::lifecycle`] stamp.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Plan {
    /// Human-readable plan name, e.g. `platform-v2`.
    pub name: String,
    /// Plan-level lifecycle gate (RFC-25 §Workflow vocabulary).
    /// Defaults to [`Lifecycle::Pending`] on parse so 1.x fixtures
    /// without a `lifecycle:` field load cleanly.
    #[serde(default)]
    pub lifecycle: Lifecycle,
    /// Named source locations referenced by [`Entry::sources`].
    /// Optional in the YAML; defaults to an empty map.
    ///
    /// The on-disk shape is currently a bare-string value per key
    /// (1.x backward-compat). RFC-25 widens this to a structured
    /// `{ adapter, path?, value? }` object — that loader change is
    /// W0.3's responsibility, not W0.2's.
    #[serde(default)]
    pub sources: BTreeMap<String, String>,
    /// Ordered list of plan entries. Order is the *intended* execution
    /// order; the authoritative dependency-respecting order comes from
    /// [`Plan::topological_order`].
    #[serde(rename = "slices")]
    pub entries: Vec<Entry>,
}

/// One entry in [`Plan::entries`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Entry {
    /// Stable identifier (kebab-case) unique within the plan.
    pub name: String,
    /// Target registry project. Required for multi-project registries.
    #[serde(default)]
    pub project: Option<String>,
    /// Target-adapter identifier (RFC-25 §Adapter vocabulary) for the
    /// slice (e.g. `omnia@v1`, `contracts@v1`). Required when
    /// `project` is `None`; optional override when `project` is
    /// `Some`. Mutually enriching with `project`: `project` identifies
    /// the target codebase; `target` identifies the target adapter
    /// directly.
    ///
    /// Renamed from `adapter` in RFC-25 W0.2 — the on-disk and
    /// in-memory field is now `target`. The pre-RFC-25 `adapter`
    /// alias was dropped together with the schema tightening that
    /// shipped in the same change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Current lifecycle state of this entry.
    pub status: Status,
    /// Names of other plan entries that must reach `done` before this
    /// entry is eligible.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// (source-key, candidate-id) bindings (RFC-25 §`Slice.sources`).
    /// Each entry pairs a source key — referencing a top-level
    /// [`Plan::sources`] entry — with the `candidate` id from
    /// `discovery.md` that contributed to the slice. The bare-string
    /// shorthand `<key>` is accepted on the wire as sugar for
    /// `{ key: <key>, candidate: <slice.name> }`; in memory we
    /// preserve the on-disk form via [`SliceSourceBinding`].
    #[serde(default)]
    pub sources: Vec<SliceSourceBinding>,
    /// Baseline paths relevant to this change, relative to `.specify/`.
    /// Briefs use these as a focus hint when scanning baseline directories.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<String>,
    /// Free-form human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// RFC-25 §Plan-time fusion — closed enum capturing slice-level
    /// fusion outcome. Absent on disk (the default) is semantic `none`.
    /// `Likely` is set by `/spec:plan`'s `propose` sub-step on
    /// materially-disagreeing candidate summaries; `Accepted` /
    /// `Rejected` are written by the operator at Gate 1 via
    /// `specify plan amend --divergence`. Advisory metadata in v1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub divergence: Option<Divergence>,
    /// RFC-27 §D3 — optional per-slice authority override map keyed
    /// by claim kind, valued by source key. Keys are the closed
    /// [`ClaimKind`] enum; values MUST be source keys present in
    /// this slice's own [`Entry::sources`] list — orphan keys are
    /// rejected by `specify slice validate` with
    /// `slice-authority-override-orphan-source-key`. Empty map and
    /// missing field are equivalent.
    #[serde(default, skip_serializing_if = "slice_authority_override_is_empty")]
    pub authority_override: SliceAuthorityOverride,
}

/// RFC-25 §Plan-time fusion — slice-level fusion-outcome enum.
///
/// Closed `none | likely | accepted | rejected` taxonomy. On disk
/// inside `plan.yaml.slices[].divergence` the field uses
/// `Option<Divergence>` with `skip_serializing_if = "Option::is_none"`,
/// so an absent line (`Option::None`) is the implicit default and the
/// `none` variant never appears in slice records. The journal wire
/// (`plan.amend.divergence` payload's `from` / `to`) does pin all
/// four values literally — `Divergence::None` serialises as the
/// kebab-case `"none"` for that channel.
///
/// RFC-27 §D5 — the CLI is the single writer of every variant of
/// this enum on `plan.yaml.slices[].divergence`. `Likely` reaches
/// disk via `specify plan create --divergence-likely <slice>` (the
/// post-`propose` staging site) and `specify plan amend --divergence
/// likely` (the bare-skill fallback); `Accepted` / `Rejected` reach
/// disk via `specify plan amend --divergence`. `none` is the
/// implicit-absent default and is never serialised explicitly into
/// a slice record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Divergence {
    /// No divergence — the implicit default for slice records (absent
    /// on disk) and the explicit first value of the journal
    /// `plan.amend.divergence` `from` field on the first transition.
    #[serde(rename = "none")]
    None,
    /// Synthesised by `/spec:plan`'s `propose` sub-step on
    /// materially-disagreeing candidate summaries.
    Likely,
    /// Operator-stamped at Gate 1 — divergence acknowledged and
    /// accepted into the plan.
    Accepted,
    /// Operator-stamped at Gate 1 — divergence rejected; the plan
    /// must be re-proposed before Gate 1 review.
    Rejected,
}

/// RFC-27 §D3 — per-slice authority override map keyed by claim
/// kind, valued by source key.
///
/// The map is scoped to one [`Entry`]; plan-wide and project-wide
/// overrides are out of scope per RFC-27. Keys reuse the closed
/// [`ClaimKind`] enum; values are bare source-key strings that MUST
/// be present in the owning slice's [`Entry::sources`] list —
/// validation refuses orphan keys with
/// `slice-authority-override-orphan-source-key`.
///
/// `#[serde(transparent)]` over `BTreeMap` so the on-disk shape is
/// the bare YAML map under `authority-override:`. Empty map and
/// missing field round-trip identically — both leave the slice's
/// authority resolution at the RFC-25 default ordering.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct SliceAuthorityOverride {
    /// Inner map. `BTreeMap` for byte-stable diffs on serialise.
    pub by_kind: BTreeMap<ClaimKind, String>,
}

fn slice_authority_override_is_empty(o: &SliceAuthorityOverride) -> bool {
    o.by_kind.is_empty()
}

/// One `(source-key, candidate-id)` binding under [`Entry::sources`].
///
/// On the wire (RFC-25 §`Slice.sources`) this is either:
///
/// - a bare string `<key>` — shorthand for the structured form
///   `{ key: <key>, candidate: <slice.name> }`; used predominantly in
///   the degenerate `intent` case (`sources: [intent]`); or
/// - a structured `{ key, candidate }` object.
///
/// Both shapes round-trip byte-identically through serde. Callers can
/// use [`SliceSourceBinding::key`] and [`SliceSourceBinding::candidate`]
/// when they need explicit `(key, candidate)` pairs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum SliceSourceBinding {
    /// Bare-string shorthand: `<key>` ≡ `{ key, candidate: <slice.name> }`.
    Bare(String),
    /// Structured form. Both fields are kebab-case identifiers.
    Structured {
        /// Source key matching a top-level [`Plan::sources`] entry.
        key: String,
        /// Candidate id from `discovery.md`.
        candidate: String,
    },
}

impl SliceSourceBinding {
    /// The source key this binding references in [`Plan::sources`].
    #[must_use]
    pub const fn key(&self) -> &str {
        match self {
            Self::Bare(k) | Self::Structured { key: k, .. } => k.as_str(),
        }
    }

    /// The candidate id this binding pairs with, falling back to the
    /// owning slice's name for the bare-string shorthand per RFC-25
    /// §`Slice.sources`.
    #[must_use]
    pub const fn candidate<'a>(&'a self, slice_name: &'a str) -> &'a str {
        match self {
            Self::Bare(_) => slice_name,
            Self::Structured { candidate, .. } => candidate.as_str(),
        }
    }
}

impl Plan {
    /// Computed predicate (RFC-25 §Workflow vocabulary): `true` when
    /// at least one entry is currently `in-progress`.
    ///
    /// "Currently executing" is not stored — it's derived from
    /// per-entry [`Status`] every time it's read, so race-prone
    /// duplication between plan-level and per-entry state is
    /// impossible by construction.
    #[must_use]
    pub fn is_executing(&self) -> bool {
        self.entries.iter().any(|e| e.status == Status::InProgress)
    }

    /// Computed predicate (RFC-25 §Workflow vocabulary): `true` when
    /// every entry has reached terminal `done` status.
    ///
    /// Empty plans report drained vacuously — there is no work left
    /// to drain. Like [`Plan::is_executing`], "drained" is derived
    /// from per-entry [`Status`] at read time and never stored.
    #[must_use]
    pub fn is_drained(&self) -> bool {
        self.entries.iter().all(|e| e.status == Status::Done)
    }
}

/// Three-way patch over a nullable field: `Keep` leaves the field
/// untouched, `Clear` sets it to `None`, `Set(v)` replaces it with
/// `Some(v)`.
///
/// This is the in-memory builder shape consumed by [`Plan::amend`]; it
/// does **not** appear on the wire.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub enum Patch<T> {
    /// Leave the field unchanged.
    #[default]
    Keep,
    /// Replace the field with `None`.
    Clear,
    /// Replace the field with `Some(v)`.
    Set(T),
}

impl<T> Patch<T> {
    /// Apply the patch to an `Option<T>` field in place.
    pub fn apply(self, field: &mut Option<T>) {
        match self {
            Self::Keep => {}
            Self::Clear => *field = None,
            Self::Set(v) => *field = Some(v),
        }
    }
}

/// Patch applied by [`Plan::amend`] to an existing entry.
///
/// Wholesale-replacement fields are `Option<Vec<...>>`; nullable fields use
/// the three-way [`Patch`] enum. `status` is deliberately absent —
/// status transitions are made via [`Plan::transition`], never through
/// `amend`.
///
/// The absence of a `status` field is a type-system guarantee: `amend`
/// cannot mutate status.
#[derive(Debug, Default, Clone)]
pub struct EntryPatch {
    /// Replace `depends_on` wholesale when `Some`.
    pub depends_on: Option<Vec<String>>,
    /// Replace `sources` wholesale when `Some`.
    pub sources: Option<Vec<SliceSourceBinding>>,
    /// Three-way patch over `project`.
    pub project: Patch<String>,
    /// Three-way patch over `target` (the RFC-25 target-adapter
    /// identifier — renamed from `adapter`).
    pub target: Patch<String>,
    /// Three-way patch over `description`.
    pub description: Patch<String>,
    /// Replace `context` wholesale when `Some`.
    pub context: Option<Vec<String>>,
    /// Set `divergence` when `Some`. `None` leaves the field
    /// untouched. The CLI is the only caller that materialises this
    /// patch (`specify plan amend --divergence`) — RFC-27 §D5
    /// widens the accepted operator surface to include `Likely`
    /// alongside `Accepted` / `Rejected`; the implicit `None` value
    /// is still rejected at the flag-parser level (omit
    /// `--divergence` to leave the field alone).
    pub divergence: Option<Divergence>,
}

/// Severity of a validation finding produced by
/// [`Plan::validate`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, strum::Display,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Severity {
    /// Blocking problem — the plan is not usable as-is.
    Error,
    /// Non-blocking advisory — the plan is usable but something looks
    /// off (e.g. a source key is defined but unreferenced).
    Warning,
}

/// A single finding reported by [`Plan::validate`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Finding {
    /// Severity bucket.
    pub level: Severity,
    /// Stable machine-readable code, e.g. `"plan.cycle"`.
    pub code: &'static str,
    /// Human-readable description.
    pub message: String,
    /// Name of the offending entry, when the finding is entry-local.
    pub entry: Option<String>,
}

#[cfg(test)]
mod tests;
