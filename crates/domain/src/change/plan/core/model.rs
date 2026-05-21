//! Type definitions for `plan.yaml` (`Plan`, `Entry`, `EntryPatch`,
//! `Status`, `Severity`, `Finding`). Behaviour lives in the sibling
//! submodules.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use specify_error::Error;

/// Lifecycle state of a single entry in [`Plan::entries`].
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
#[non_exhaustive]
pub enum Status {
    /// Not yet started.
    Pending,
    /// Currently being executed.
    InProgress,
    /// Completed successfully.
    Done,
    /// Blocked on an external dependency or question.
    Blocked,
    /// Execution failed.
    Failed,
    /// Intentionally skipped.
    Skipped,
}

/// In-memory model of `plan.yaml` (at the repo root).
///
/// A `Plan` is an ordered, dependency-aware list of [`Entry`]s plus
/// a named map of [`Plan::sources`] (local paths or git URLs) that the
/// entries draw from.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Plan {
    /// Human-readable plan name, e.g. `platform-v2`.
    pub name: String,
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
    /// preserve the on-disk form via [`SliceSourceBinding`], and
    /// [`Plan::resolve_sources`] normalises both shapes for
    /// downstream extract.
    #[serde(default)]
    pub sources: Vec<SliceSourceBinding>,
    /// Baseline paths relevant to this change, relative to `.specify/`.
    /// Briefs use these as a focus hint when scanning baseline directories.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<String>,
    /// Free-form human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Operational explanation for the current non-terminal/terminal
    /// status (`failed`, `blocked`, or `skipped`). Overwritten on each
    /// status transition; cleared when the entry returns to `pending`,
    /// `in-progress`, or `done`.
    #[serde(default)]
    pub status_reason: Option<String>,
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
/// Both shapes round-trip byte-identically through serde; downstream
/// code that wants normalised `(key, candidate)` pairs reaches for
/// [`Plan::resolve_sources`].
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

/// One `(source-key, candidate-id, plan-level-binding-value)` triple
/// returned by [`Plan::resolve_sources`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSourceBinding {
    /// Source key, matches a top-level [`Plan::sources`] entry.
    pub key: String,
    /// Candidate id from `discovery.md`. For the bare-string
    /// shorthand this is the owning slice's name (RFC-25
    /// §`Slice.sources`).
    pub candidate: String,
    /// The plan-level source value (currently a bare string —
    /// W0.3 widens this to a structured `{ adapter, path?, value? }`).
    pub binding: String,
}

impl Plan {
    /// Resolve `slice.sources` against [`Plan::sources`], normalising
    /// the bare-string shorthand into explicit `(key, candidate)`
    /// pairs per RFC-25 §`Slice.sources`.
    ///
    /// The returned vector preserves declaration order — `/spec:refine`
    /// runs `extract` serially in this order, per RFC-25 §Execution
    /// model.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Diag`] with code `plan-source-key-undefined`
    /// when a slice references a key that is not declared under
    /// [`Plan::sources`].
    pub fn resolve_sources(&self, slice: &Entry) -> Result<Vec<ResolvedSourceBinding>, Error> {
        let mut out = Vec::with_capacity(slice.sources.len());
        for binding in &slice.sources {
            let key = binding.key().to_string();
            let candidate = binding.candidate(&slice.name).to_string();
            let value = self.sources.get(&key).cloned().ok_or_else(|| Error::Diag {
                code: "plan-source-key-undefined",
                detail: format!(
                    "slice `{}` references source key `{key}`, which is not declared under plan.sources",
                    slice.name
                ),
            })?;
            out.push(ResolvedSourceBinding {
                key,
                candidate,
                binding: value,
            });
        }
        Ok(out)
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
/// the three-way [`Patch`] enum. `status` and `status_reason` are
/// deliberately absent — status transitions are made via
/// [`Plan::transition`], never through `amend`, and the reason field
/// travels with the transition.
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

    #[test]
    fn rfc_example_round_trips() {
        let original: Plan = serde_saphyr::from_str(super::super::test_support::RFC_EXAMPLE_YAML)
            .expect("parse rfc fixture");
        let rendered = serde_saphyr::to_string(&original).expect("serialize plan");
        let reparsed: Plan = serde_saphyr::from_str(&rendered).expect("reparse rendered plan");
        assert_eq!(original, reparsed, "plan should survive a serialize/parse round-trip");

        assert_eq!(original.name, "platform-v2");
        assert_eq!(original.sources.len(), 4);
        assert_eq!(original.entries.len(), 9);
        assert_eq!(original.entries[0].status, Status::Done);
        assert_eq!(original.entries[1].status, Status::InProgress);
        assert_eq!(original.entries[7].status, Status::Failed);
        assert!(original.entries[7].status_reason.is_some());
    }

    #[test]
    fn serializes_kebab_case() {
        let plan = Plan {
            name: "demo".to_string(),
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
                status_reason: Some("awaiting upstream fix".to_string()),
            }],
        };
        let yaml = serde_saphyr::to_string(&plan).expect("serialize plan");
        assert!(yaml.contains("depends-on:"), "expected kebab-case depends-on in:\n{yaml}");
        assert!(
            yaml.contains("status: in-progress"),
            "expected kebab-case enum value in-progress in:\n{yaml}"
        );
        assert!(yaml.contains("status-reason:"), "expected kebab-case status-reason in:\n{yaml}");
        assert!(!yaml.contains("depends_on"), "snake_case depends_on leaked into output:\n{yaml}");
        assert!(
            !yaml.contains("status_reason"),
            "snake_case status_reason leaked into output:\n{yaml}"
        );
    }

    #[test]
    fn missing_fields_default() {
        let yaml = "name: foo\nslices: []\n";
        let plan: Plan = serde_saphyr::from_str(yaml).expect("parse minimal plan");
        assert_eq!(plan.name, "foo");
        assert!(plan.sources.is_empty(), "sources should default to empty map");
        assert!(plan.entries.is_empty(), "slices should be empty");
    }

    #[test]
    fn status_reason_round_trips() {
        let yaml = r"name: demo
slices:
  - name: checkout-api
    sources: [payments]
    depends-on: [shopping-cart]
    status: failed
    status-reason: >
      Type mismatch between cart line-item schema and payment gateway contract.
      Needs design revision after shopping-cart specs are updated.
";
        let plan: Plan = serde_saphyr::from_str(yaml).expect("parse");
        let entry = &plan.entries[0];
        assert_eq!(entry.status, Status::Failed);
        let reason = entry.status_reason.as_deref().expect("status_reason populated");
        assert!(
            reason.contains("Type mismatch"),
            "status_reason should preserve folded text, got: {reason:?}"
        );

        let rendered = serde_saphyr::to_string(&plan).expect("serialize");
        let reparsed: Plan = serde_saphyr::from_str(&rendered).expect("reparse");
        assert_eq!(plan, reparsed);
        assert_eq!(
            reparsed.entries[0].status_reason, entry.status_reason,
            "status_reason should be byte-identical after round-trip"
        );
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
    fn resolve_sources_normalises_and_rejects_unknown_keys() {
        let mut sources = BTreeMap::new();
        sources.insert("intent".to_string(), "do the thing".to_string());
        sources.insert("docs".to_string(), "./design-notes".to_string());
        let plan = Plan {
            name: "demo".into(),
            sources,
            entries: vec![Entry {
                name: "add-search-filter".into(),
                project: None,
                target: Some("omnia".into()),
                status: Status::Pending,
                depends_on: vec![],
                sources: vec![
                    SliceSourceBinding::Bare("intent".into()),
                    SliceSourceBinding::Structured {
                        key: "docs".into(),
                        candidate: "search-filter".into(),
                    },
                ],
                context: vec![],
                description: None,
                status_reason: None,
            }],
        };

        let resolved = plan.resolve_sources(&plan.entries[0]).expect("resolve ok");
        assert_eq!(resolved.len(), 2);
        assert_eq!(resolved[0].key, "intent");
        assert_eq!(
            resolved[0].candidate, "add-search-filter",
            "bare shorthand resolves candidate to slice.name"
        );
        assert_eq!(resolved[0].binding, "do the thing");
        assert_eq!(resolved[1].key, "docs");
        assert_eq!(resolved[1].candidate, "search-filter");
        assert_eq!(resolved[1].binding, "./design-notes");

        let mut broken = plan.entries[0].clone();
        broken.sources.push(SliceSourceBinding::Bare("nope".into()));
        let err = plan.resolve_sources(&broken).expect_err("unknown key must error");
        match err {
            Error::Diag { code, detail } => {
                assert_eq!(code, "plan-source-key-undefined");
                assert!(detail.contains("nope"), "detail should mention key: {detail}");
            }
            other => panic!("expected plan-source-key-undefined, got {other:?}"),
        }
    }
}
