//! Type definitions for `plan.yaml` (`Plan`, `Entry`, `EntryPatch`,
//! `Status`, `Lifecycle`). Validation findings are emitted on the
//! neutral [`specify_diagnostics::Diagnostic`] currency by the sibling
//! `validate` / `doctor` modules. Behaviour lives in the sibling
//! submodules.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use specify_model::evidence::ClaimKind;

use crate::name::{PlanName, SliceName};

/// Lifecycle state of a single entry in [`Plan::entries`].
///
/// workflow collapses the per-entry state machine to three states:
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
    /// Not yet started. Written by `plan add` / `plan amend` (forward)
    /// and `plan transition <entry> --undo` (reverse from
    /// `InProgress`).
    Pending,
    /// Currently being executed. Written by `plan next` (forward)
    /// and `plan transition <entry> --undo` (reverse from `Done`).
    InProgress,
    /// Completed successfully. Written by `slice merge` (forward
    /// only — `--undo` walks back to `InProgress` so the slice can be
    /// re-built and re-merged without inventing a `Reopened` state).
    Done,
}

/// Plan-level lifecycle state stored at the top of `plan.yaml`
/// (workflow §Workflow vocabulary).
///
/// Two stored states only — `pending` (default after `plan create`)
/// and `approved` (operator-stamped at Gate 1 via
/// `specify plan transition <plan-name> approved`). "Currently
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
    Approved,
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
    pub name: PlanName,
    /// Plan-level lifecycle gate (workflow §Workflow vocabulary).
    /// Defaults to [`Lifecycle::Pending`] on parse so 1.x fixtures
    /// without a `lifecycle:` field load cleanly.
    #[serde(default)]
    pub lifecycle: Lifecycle,
    /// Named source bindings referenced by [`Entry::sources`].
    /// Optional in the YAML; defaults to an empty map.
    ///
    /// Each value is a structured [`SourceBinding`] carrying the
    /// kebab-case source adapter name plus exactly one of `path`
    /// (filesystem path or repo location) or `value` (literal payload
    /// supplied directly to the adapter — used by `intent`).
    #[serde(default)]
    pub sources: BTreeMap<String, SourceBinding>,
    /// Ordered list of plan entries. Order is the intended execution
    /// order; `Plan::next_eligible` applies dependency eligibility.
    #[serde(rename = "slices")]
    pub entries: Vec<Entry>,
}

/// One entry in [`Plan::entries`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Entry {
    /// Stable identifier (kebab-case) unique within the plan.
    pub name: SliceName,
    /// Target registry project. Optional on disk: an omitted value
    /// resolves to the sole project in the topology (a single regular
    /// project synthesised from `project.yaml`), so single-project
    /// plans need not repeat the project name; multi-project workspace
    /// registries require an explicit value.
    ///
    /// The target adapter (`name@vN`) is **not** stored on the slice —
    /// it is resolved on demand from this project via the topology
    /// (the committed `.specify/topology.lock` for a workspace, `project.yaml.adapter` for a single
    /// regular project) by [`crate::change::plan::core::resolve_target`].
    #[serde(default)]
    pub project: Option<String>,
    /// Current lifecycle state of this entry.
    pub status: Status,
    /// Names of other plan entries that must reach `done` before this
    /// entry is eligible.
    #[serde(default)]
    pub depends_on: Vec<SliceName>,
    /// (source, lead) bindings (workflow §`Slice.sources`).
    /// Each entry pairs a `source` — referencing a top-level
    /// [`Plan::sources`] entry — with the `lead` from
    /// `discovery.md` that contributed to the slice. The bare-string
    /// shorthand `<key>` is accepted on the wire as sugar for
    /// `{ source: <key>, lead: <slice.name> }`; in memory we
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
    /// workflow §Plan-time reconciliation — closed enum capturing slice-level
    /// reconciliation outcome. Absent on disk (the default) is semantic `none`.
    /// `Likely` is set by `/spec:plan`'s `propose` sub-step on
    /// materially-disagreeing lead synopses; `Accepted` /
    /// `Rejected` are written by the operator at Gate 1 via
    /// `specify plan amend --divergence`. Advisory metadata in v1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub divergence: Option<Divergence>,
    /// per-slice authority override — optional per-slice authority override map keyed
    /// by claim kind, valued by source key. Keys are the closed
    /// [`ClaimKind`] enum; values MUST be source keys present in
    /// this slice's own [`Entry::sources`] list — orphan keys are
    /// rejected by `specify slice validate` with
    /// `slice-authority-override-orphan-source`. Empty map and
    /// missing field are equivalent.
    #[serde(default, skip_serializing_if = "slice_authority_override_is_empty")]
    pub authority_override: SliceAuthorityOverride,
}

/// workflow §Plan-time reconciliation — slice-level reconciliation-outcome enum.
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
/// divergence and writer-ownership contract — the CLI is the single writer of every variant of
/// this enum on `plan.yaml.slices[].divergence`. `Likely` reaches
/// disk in the `propose`-driven `/spec:plan` flow (RFC-29 D2) when the
/// agent runs `specify plan amend --divergence likely` *after*
/// `specify plan propose --from` (the slice writer), because slices
/// do not exist until projection runs. `Accepted` / `Rejected` reach
/// disk via `specify plan amend --divergence`. `plan amend` is the
/// only writer of the field — `plan create` scaffolds an empty plan
/// and never stamps divergence. `none` is the implicit-absent
/// default and is never serialised explicitly into a slice record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Divergence {
    /// No divergence — the implicit default for slice records (absent
    /// on disk) and the explicit first value of the journal
    /// `plan.amend.divergence` `from` field on the first transition.
    #[serde(rename = "none")]
    None,
    /// Staged by the `/spec:plan` agent after `propose --from`, via
    /// `specify plan amend --divergence likely`, on
    /// materially-disagreeing lead synopses.
    Likely,
    /// Operator-stamped at Gate 1 — divergence acknowledged and
    /// accepted into the plan.
    Accepted,
    /// Operator-stamped at Gate 1 — divergence rejected; the plan
    /// must be re-proposed before Gate 1 review.
    Rejected,
}

/// per-slice authority override — per-slice authority override map keyed by claim
/// kind, valued by source key.
///
/// The map is scoped to one [`Entry`]; plan-wide and project-wide
/// overrides are out of scope per authority and reconciliation contract. Keys reuse the closed
/// [`ClaimKind`] enum; values are bare source strings that MUST
/// be present in the owning slice's [`Entry::sources`] list —
/// validation refuses orphan keys with
/// `slice-authority-override-orphan-source`.
///
/// `#[serde(transparent)]` over `BTreeMap` so the on-disk shape is
/// the bare YAML map under `authority-override:`. Empty map and
/// missing field round-trip identically — both leave the slice's
/// authority resolution at the workflow default ordering.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(transparent)]
pub struct SliceAuthorityOverride {
    /// Inner map. `BTreeMap` for byte-stable diffs on serialise.
    pub by_kind: BTreeMap<ClaimKind, String>,
}

fn slice_authority_override_is_empty(o: &SliceAuthorityOverride) -> bool {
    o.by_kind.is_empty()
}

/// One top-level [`Plan::sources`] binding.
///
/// Carries the kebab-case source adapter name plus exactly one of
/// `path` (filesystem path or repo location) or `value` (literal
/// payload supplied directly to the adapter, used by the `intent`
/// source).
///
/// On the wire (workflow §Source) the binding is always the structured
/// `{ adapter, path?, value? }` object form — 2.0 dropped the
/// 1.x bare-string shorthand. The `oneOf` exclusion between `path`
/// and `value` is enforced by `plan.schema.json` and re-checked at
/// the loader boundary via [`crate::schema::validate_plan`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SourceBinding {
    /// Kebab-case source-adapter name (e.g. `intent`, `documentation`,
    /// `code-typescript`, `screenshots`).
    pub adapter: String,
    /// Filesystem path or repo location the adapter binds against.
    /// Mutually exclusive with `value`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Literal value supplied directly to the adapter (e.g. the
    /// operator brief text for `intent`). Mutually exclusive with
    /// `path`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

impl SourceBinding {
    /// Construct a path-bound binding for the named adapter.
    #[must_use]
    pub fn path(adapter: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            adapter: adapter.into(),
            path: Some(path.into()),
            value: None,
        }
    }

    /// Construct a value-bound binding for the named adapter.
    #[must_use]
    pub fn value(adapter: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            adapter: adapter.into(),
            path: None,
            value: Some(value.into()),
        }
    }
}

/// Parsed `<name>@v<version>` target-adapter identifier (workflow §Adapter
/// vocabulary).
///
/// This is the *resolved* target form, produced by
/// [`crate::change::plan::core::resolve_target`] from a slice's bound
/// project topology and surfaced by `specify plan next`, the slice
/// `.metadata.yaml`, and the build request. It is no longer a stored
/// `plan.yaml` field — a slice binds only a `project`, and the target
/// adapter is resolved on demand.
///
/// Wire form is the single kebab string `name@vN` (e.g. `omnia@v1`),
/// with `name` matching `^[a-z][a-z0-9-]*$` and `N` a non-negative
/// integer. Deserialisation goes through [`TargetRef::parse`] so any
/// payload that survives serde already has the `@vN` suffix in valid
/// form. Components are private so every `TargetRef` value satisfies
/// the wire regex by construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TargetRef {
    name: String,
    version: u32,
}

impl TargetRef {
    /// Parse a wire-form `<name>@v<version>` string.
    ///
    /// # Errors
    ///
    /// Returns [`TargetRefParseError`] when the string does not match
    /// the wire regex `^[a-z][a-z0-9-]*@v\d+$` — wrong shape, empty
    /// segment, mixed case, missing `@v`, non-digit version, etc.
    pub fn parse(input: &str) -> Result<Self, TargetRefParseError> {
        let (name, version_part) =
            input.split_once("@v").ok_or_else(|| TargetRefParseError::new(input))?;
        if !is_kebab_target_name(name) {
            return Err(TargetRefParseError::new(input));
        }
        if version_part.is_empty() || !version_part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(TargetRefParseError::new(input));
        }
        let version: u32 = version_part.parse().map_err(|_err| TargetRefParseError::new(input))?;
        Ok(Self {
            name: name.to_string(),
            version,
        })
    }
}

fn is_kebab_target_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    let mut prev_hyphen = false;
    for b in bytes {
        match b {
            b'a'..=b'z' | b'0'..=b'9' => prev_hyphen = false,
            b'-' if !prev_hyphen => prev_hyphen = true,
            _ => return false,
        }
    }
    !prev_hyphen
}

impl fmt::Display for TargetRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@v{}", self.name, self.version)
    }
}

impl Serialize for TargetRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TargetRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

/// Error returned by [`TargetRef::parse`] when the input does not
/// match the `name@vN` wire form.
///
/// Carries the offending input verbatim so callers can surface it in
/// diagnostics without re-formatting; the [`fmt::Display`] body is
/// already the kebab discriminant prose used by
/// `plan-target-malformed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetRefParseError {
    /// The original (rejected) input.
    pub input: String,
}

impl TargetRefParseError {
    fn new(input: &str) -> Self {
        Self {
            input: input.to_string(),
        }
    }
}

impl fmt::Display for TargetRefParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "target `{}` is not of the form `<name>@v<version>` (kebab name, lowercase `v`, integer version)",
            self.input,
        )
    }
}

impl std::error::Error for TargetRefParseError {}

/// One `(source, lead)` binding under [`Entry::sources`].
///
/// On the wire (workflow §`Slice.sources`) this is either:
///
/// - a bare string `<key>` — shorthand for the structured form
///   `{ source: <key>, lead: <slice.name> }`; used
///   predominantly in the degenerate `intent` case
///   (`sources: [intent]`); or
/// - a structured `{ source, lead }` object.
///
/// Both shapes round-trip byte-identically: the bare shorthand is
/// normalised at parse time into `lead == None`, and `Serialize`
/// emits the same shape the operator authored. Use
/// [`SliceSourceBinding::bare`] / [`SliceSourceBinding::structured`] in
/// tests instead of constructing the struct literal directly so the
/// shorthand discipline stays consistent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SliceSourceBinding {
    /// Source key matching a top-level [`Plan::sources`] entry. Always
    /// present, regardless of which wire shape produced this value.
    pub source: String,
    /// Lead id from `discovery.md`, resolved within `source`.
    /// `None` denotes the bare-string shorthand — the lead falls
    /// back to the owning slice's name via
    /// [`SliceSourceBinding::lead`].
    pub lead: Option<String>,
}

impl SliceSourceBinding {
    /// Construct the bare-string shorthand form: lead defaults to
    /// the owning slice's name at lookup time.
    #[must_use]
    pub fn bare(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            lead: None,
        }
    }

    /// Construct the structured form with an explicit lead.
    #[must_use]
    pub fn structured(source: impl Into<String>, lead: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            lead: Some(lead.into()),
        }
    }

    /// The source key this binding references in [`Plan::sources`].
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The lead this binding pairs with, falling back to the
    /// owning slice's name for the bare-string shorthand per the
    /// workflow contract §`Slice.sources`.
    #[must_use]
    pub fn lead<'a>(&'a self, slice_name: &'a str) -> &'a str {
        self.lead.as_deref().unwrap_or(slice_name)
    }
}

impl Serialize for SliceSourceBinding {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match &self.lead {
            None => serializer.serialize_str(&self.source),
            Some(lead) => {
                use serde::ser::SerializeStruct;
                let mut state = serializer.serialize_struct("SliceSourceBinding", 2)?;
                state.serialize_field("source", &self.source)?;
                state.serialize_field("lead", lead)?;
                state.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for SliceSourceBinding {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Bare(String),
            Structured {
                #[serde(rename = "source")]
                source: String,
                #[serde(rename = "lead")]
                lead: String,
            },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Bare(source) => Self::bare(source),
            Wire::Structured { source, lead } => Self::structured(source, lead),
        })
    }
}

impl Plan {
    /// Computed predicate (workflow §Workflow vocabulary): `true` when
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

    /// Computed predicate (workflow §Workflow vocabulary): `true` when
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

impl Patch<String> {
    /// Materialise the wire convention shared by every `Patch<String>`
    /// CLI flag: a missing flag means `Keep`, an empty-string flag
    /// (`--field ""`) means `Clear`, and any non-empty value means
    /// `Set(value)`.
    #[must_use]
    pub fn from_string_option(value: Option<String>) -> Self {
        match value {
            None => Self::Keep,
            Some(s) if s.is_empty() => Self::Clear,
            Some(s) => Self::Set(s),
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
    pub depends_on: Option<Vec<SliceName>>,
    /// Replace `sources` wholesale when `Some`.
    pub sources: Option<Vec<SliceSourceBinding>>,
    /// Three-way patch over `project`.
    pub project: Patch<String>,
    /// Three-way patch over `description`.
    pub description: Patch<String>,
    /// Replace `context` wholesale when `Some`.
    pub context: Option<Vec<String>>,
    /// Set `divergence` when `Some`. `None` leaves the field
    /// untouched. The CLI is the only caller that materialises this
    /// patch (`specify plan amend --divergence`) — divergence and writer-ownership contract
    /// widens the accepted operator surface to include `Likely`
    /// alongside `Accepted` / `Rejected`; the implicit `None` value
    /// is still rejected at the flag-parser level (omit
    /// `--divergence` to leave the field alone).
    pub divergence: Option<Divergence>,
}

#[cfg(test)]
mod tests;
