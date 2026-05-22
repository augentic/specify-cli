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
    #[serde(default, skip_serializing_if = "SliceAuthorityOverride::is_empty")]
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

impl SliceAuthorityOverride {
    /// Build a [`SliceAuthorityOverride`] from an iterator of
    /// `(kind, source-key)` pairs. Duplicate keys take the last
    /// value per [`BTreeMap::insert`].
    #[must_use]
    pub fn from_pairs<I, S>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (ClaimKind, S)>,
        S: Into<String>,
    {
        Self {
            by_kind: pairs.into_iter().map(|(k, v)| (k, v.into())).collect(),
        }
    }

    /// `true` when the override map carries no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_kind.is_empty()
    }

    /// Resolve the source key, if any, pinned for `kind`.
    #[must_use]
    pub fn resolve(&self, kind: ClaimKind) -> Option<&str> {
        self.by_kind.get(&kind).map(String::as_str)
    }
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

impl From<&str> for SliceSourceBinding {
    fn from(value: &str) -> Self {
        Self::Bare(value.to_string())
    }
}

impl From<String> for SliceSourceBinding {
    fn from(value: String) -> Self {
        Self::Bare(value)
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
mod tests {
    use super::*;

    /// Verbatim §The Plan reference fixture, post-RFC-25 collapse.
    /// All entries use the simplified per-entry `Status` enum
    /// (`pending | in-progress | done`); v1 has no per-entry
    /// `blocked`, `failed`, or `skipped` state.
    const RFC_EXAMPLE_YAML: &str = r"name: platform-v2
sources:
  monolith: /path/to/legacy-codebase
  orders: git@github.com:org/orders-service.git
  payments: git@github.com:org/payments-service.git
  frontend: git@github.com:org/web-app.git
slices:
  - name: user-registration
    project: platform
    sources: [monolith]
    status: done
  - name: email-verification
    project: platform
    sources: [monolith]
    depends-on: [user-registration]
    status: in-progress
  - name: registration-duplicate-email-crash
    project: platform
    description: >
      Duplicate email submission returns 500 instead of 409.
      Discovered during email-verification extraction.
    status: pending
";

    #[test]
    fn round_trips_rfc_fixture() {
        let original: Plan = serde_saphyr::from_str(RFC_EXAMPLE_YAML).expect("parse rfc fixture");
        let yaml = serde_saphyr::to_string(&original).expect("serialize plan");
        let reparsed: Plan = serde_saphyr::from_str(&yaml).expect("reparse plan");
        assert_eq!(original, reparsed, "plan should survive a serialize/parse round-trip");

        assert_eq!(original.name, "platform-v2");
        assert_eq!(original.sources.len(), 4);
        assert_eq!(original.entries.len(), 3);
        assert_eq!(original.entries[0].status, Status::Done);
        assert_eq!(original.entries[1].status, Status::InProgress);
        assert_eq!(original.entries[2].status, Status::Pending);
    }

    #[test]
    fn lifecycle_defaults_to_pending() {
        let yaml = "name: foo\nslices: []\n";
        let plan: Plan = serde_saphyr::from_str(yaml).expect("parse minimal plan");
        assert_eq!(
            plan.lifecycle,
            Lifecycle::Pending,
            "missing lifecycle field must default to pending"
        );
    }

    #[test]
    fn lifecycle_round_trips() {
        let yaml = "name: foo\nlifecycle: reviewed\nslices: []\n";
        let plan: Plan = serde_saphyr::from_str(yaml).expect("parse reviewed");
        assert_eq!(plan.lifecycle, Lifecycle::Reviewed);

        let rendered = serde_saphyr::to_string(&plan).expect("serialize");
        assert!(
            rendered.contains("lifecycle: reviewed"),
            "serialised plan must carry kebab-case lifecycle: reviewed, got:\n{rendered}"
        );
    }

    #[test]
    fn serializes_kebab_case() {
        let plan = Plan {
            name: "demo".to_string(),
            lifecycle: Lifecycle::Pending,
            sources: BTreeMap::new(),
            entries: vec![Entry {
                name: "entry-one".to_string(),
                project: Some("default".into()),
                target: None,
                status: Status::InProgress,
                depends_on: vec!["entry-zero".to_string()],
                sources: vec![],
                context: vec![],
                description: None,
                divergence: None,
                authority_override: SliceAuthorityOverride::default(),
            }],
        };
        let yaml = serde_saphyr::to_string(&plan).expect("serialize plan");
        assert!(yaml.contains("depends-on:"), "expected kebab-case depends-on in:\n{yaml}");
        assert!(
            yaml.contains("status: in-progress"),
            "expected kebab-case enum value in-progress in:\n{yaml}"
        );
        assert!(!yaml.contains("depends_on"), "snake_case depends_on leaked into output:\n{yaml}");
    }

    #[test]
    fn missing_fields_default() {
        let yaml = "name: foo\nslices: []\n";
        let plan: Plan = serde_saphyr::from_str(yaml).expect("parse minimal plan");
        assert_eq!(plan.name, "foo");
        assert_eq!(plan.lifecycle, Lifecycle::Pending);
        assert!(plan.sources.is_empty(), "sources should default to empty map");
        assert!(plan.entries.is_empty(), "slices should be empty");
    }

    #[test]
    fn project_round_trips() {
        let yaml = "\
name: foo
project: traffic
status: pending
";
        let parsed: Entry = serde_saphyr::from_str(yaml).expect("parses with project");
        assert_eq!(parsed.project.as_deref(), Some("traffic"));
        let round_tripped = serde_saphyr::to_string(&parsed).expect("serialize");
        let re_parsed: Entry = serde_saphyr::from_str(&round_tripped).expect("re-parse");
        assert_eq!(re_parsed.project, parsed.project);
    }

    #[test]
    fn project_defaults_to_none() {
        let yaml = "\
name: foo
status: pending
";
        let parsed: Entry = serde_saphyr::from_str(yaml).expect("parses without project");
        assert_eq!(parsed.project, None);
    }

    #[test]
    fn target_field_round_trips() {
        let yaml = r"name: test
slices:
  - name: define-contracts
    target: contracts@v1
    status: pending
  - name: impl-auth
    project: auth-service
    target: omnia@v1
    status: pending
";
        let plan: Plan = serde_saphyr::from_str(yaml).expect("parse");
        assert_eq!(plan.entries[0].target.as_deref(), Some("contracts@v1"));
        assert_eq!(plan.entries[0].project, None);
        assert_eq!(plan.entries[1].target.as_deref(), Some("omnia@v1"));
        assert_eq!(plan.entries[1].project.as_deref(), Some("auth-service"));

        let rendered = serde_saphyr::to_string(&plan).expect("serialize");
        let reparsed: Plan = serde_saphyr::from_str(&rendered).expect("reparse");
        assert_eq!(plan, reparsed, "plan must survive a YAML round-trip");
    }

    #[test]
    fn context_round_trips() {
        let yaml = r"
name: ctx-test
slices:
  - name: with-ctx
    project: default
    status: pending
    context:
      - contracts/http/user-api.yaml
      - specs/user-registration/spec.md
  - name: without-ctx
    project: default
    status: pending
";
        let plan: Plan = serde_saphyr::from_str(yaml).expect("parse yaml");
        assert_eq!(
            plan.entries[0].context,
            vec!["contracts/http/user-api.yaml", "specs/user-registration/spec.md"],
        );
        assert!(plan.entries[1].context.is_empty(), "missing context defaults to empty");

        let serialized = serde_saphyr::to_string(&plan).expect("serialize");
        assert!(
            serialized.contains("contracts/http/user-api.yaml"),
            "populated context must appear in serialized output"
        );
        assert!(
            !serialized.contains("without-ctx")
                || !serialized.split("without-ctx").nth(1).unwrap_or("").contains("context"),
            "empty context must be omitted from serialized output"
        );
    }

    #[test]
    fn patch_omits_status() {
        let patch = EntryPatch::default();
        assert!(patch.depends_on.is_none());
        assert!(patch.sources.is_none());
        assert_eq!(patch.project, Patch::Keep);
        assert_eq!(patch.target, Patch::Keep);
        assert_eq!(patch.description, Patch::Keep);
        assert!(patch.context.is_none());
    }

    #[test]
    fn slice_source_binding_round_trips_both_shapes() {
        let yaml = r"
name: bindings
slices:
  - name: pure-intent
    target: omnia
    sources: [intent]
    status: pending
  - name: combined
    target: omnia
    sources:
      - key: docs
        candidate: account-pwd-reset
      - key: legacy
        candidate: account-pwd-reset
    status: pending
";
        let plan: Plan = serde_saphyr::from_str(yaml).expect("parse");
        assert!(
            matches!(&plan.entries[0].sources[0], SliceSourceBinding::Bare(k) if k == "intent")
        );
        assert!(matches!(
            &plan.entries[1].sources[0],
            SliceSourceBinding::Structured { key, candidate }
                if key == "docs" && candidate == "account-pwd-reset"
        ));
        let rendered = serde_saphyr::to_string(&plan).expect("serialize");
        let reparsed: Plan = serde_saphyr::from_str(&rendered).expect("reparse");
        assert_eq!(plan, reparsed, "both binding shapes must survive a round-trip");
    }

    #[test]
    fn slice_source_binding_helpers_normalise_bare_shorthand() {
        let bare = SliceSourceBinding::Bare("intent".into());
        assert_eq!(bare.key(), "intent");
        assert_eq!(bare.candidate("add-search-filter"), "add-search-filter");

        let structured = SliceSourceBinding::Structured {
            key: "docs".into(),
            candidate: "user-reg".into(),
        };
        assert_eq!(structured.key(), "docs");
        assert_eq!(structured.candidate("ignored-slice-name"), "user-reg");
    }

    #[test]
    fn is_drained_only_when_all_done() {
        let plan = Plan {
            name: "demo".into(),
            lifecycle: Lifecycle::Reviewed,
            sources: BTreeMap::new(),
            entries: vec![
                Entry {
                    name: "a".into(),
                    project: Some("default".into()),
                    target: None,
                    status: Status::Done,
                    depends_on: vec![],
                    sources: vec![],
                    context: vec![],
                    description: None,
                    divergence: None,
                    authority_override: SliceAuthorityOverride::default(),
                },
                Entry {
                    name: "b".into(),
                    project: Some("default".into()),
                    target: None,
                    status: Status::Done,
                    depends_on: vec![],
                    sources: vec![],
                    context: vec![],
                    description: None,
                    divergence: None,
                    authority_override: SliceAuthorityOverride::default(),
                },
            ],
        };
        assert!(plan.is_drained(), "all-done plan must report drained");
        assert!(!plan.is_executing(), "no in-progress entry => not executing");
    }

    #[test]
    fn is_executing_when_any_in_progress() {
        let plan = Plan {
            name: "demo".into(),
            lifecycle: Lifecycle::Reviewed,
            sources: BTreeMap::new(),
            entries: vec![
                Entry {
                    name: "a".into(),
                    project: Some("default".into()),
                    target: None,
                    status: Status::Done,
                    depends_on: vec![],
                    sources: vec![],
                    context: vec![],
                    description: None,
                    divergence: None,
                    authority_override: SliceAuthorityOverride::default(),
                },
                Entry {
                    name: "b".into(),
                    project: Some("default".into()),
                    target: None,
                    status: Status::InProgress,
                    depends_on: vec![],
                    sources: vec![],
                    context: vec![],
                    description: None,
                    divergence: None,
                    authority_override: SliceAuthorityOverride::default(),
                },
            ],
        };
        assert!(plan.is_executing(), "any in-progress => executing");
        assert!(!plan.is_drained(), "in-progress entry => not drained");
    }

    #[test]
    fn authority_override_round_trips() {
        let yaml = r"name: rfc-27
slices:
  - name: identity-user-registration
    target: omnia
    project: identity-svc
    status: pending
    sources:
      - key: runtime
        candidate: user-registration
      - key: legacy-monolith
        candidate: user-registration
    authority-override:
      requirement: runtime
      criterion: legacy-monolith
";
        let plan: Plan = serde_saphyr::from_str(yaml).expect("parse");
        let entry = &plan.entries[0];
        assert_eq!(entry.authority_override.resolve(ClaimKind::Requirement), Some("runtime"));
        assert_eq!(entry.authority_override.resolve(ClaimKind::Criterion), Some("legacy-monolith"));

        let rendered = serde_saphyr::to_string(&plan).expect("serialize");
        assert!(rendered.contains("authority-override:"));
        assert!(rendered.contains("requirement: runtime"));
        let reparsed: Plan = serde_saphyr::from_str(&rendered).expect("reparse");
        assert_eq!(plan, reparsed);
    }

    #[test]
    fn empty_authority_override_elides() {
        let yaml = r"name: tiny
slices:
  - name: x
    target: omnia
    status: pending
";
        let plan: Plan = serde_saphyr::from_str(yaml).expect("parse");
        assert!(plan.entries[0].authority_override.is_empty());
        let rendered = serde_saphyr::to_string(&plan).expect("serialize");
        assert!(
            !rendered.contains("authority-override"),
            "empty override map must elide on write, got:\n{rendered}"
        );
    }

    #[test]
    fn divergence_likely_round_trips_to_byte_identical_yaml() {
        // RFC-27 §D5: the CLI is the single writer of every variant
        // of `slices[].divergence`. The on-disk shape for `Likely`
        // is one kebab-case line on the slice entry, byte-identical
        // to the legacy skill-written output we are retiring.
        let reference = r"name: demo
slices:
  - name: checkout
    project: default
    status: pending
    divergence: likely
";
        let plan: Plan = serde_saphyr::from_str(reference).expect("parse reference yaml");
        assert_eq!(plan.entries[0].divergence, Some(Divergence::Likely));
        let rendered = serde_saphyr::to_string(&plan).expect("serialize");
        assert!(
            rendered.contains("divergence: likely"),
            "Divergence::Likely must serialise as kebab-case `divergence: likely`, got:\n{rendered}"
        );
        let reparsed: Plan = serde_saphyr::from_str(&rendered).expect("reparse");
        assert_eq!(plan, reparsed, "plan with divergence: likely must round-trip");
    }

    #[test]
    fn empty_plan_is_drained_vacuously() {
        let plan = Plan {
            name: "demo".into(),
            lifecycle: Lifecycle::Pending,
            sources: BTreeMap::new(),
            entries: vec![],
        };
        assert!(plan.is_drained(), "empty plan reports drained vacuously");
        assert!(!plan.is_executing(), "empty plan is not executing");
    }
}
