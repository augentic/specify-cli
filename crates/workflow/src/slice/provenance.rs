//! Provenance index — `provenance.yaml`.
//!
//! One file per slice at `.specify/slices/<slice>/provenance.yaml`. Lists
//! every `REQ-*` id in `spec.md` and the contributing
//! `(source, id)` pairs plus the authority outcome.
//! Validated against `schemas/slice/provenance.schema.json`. The file is
//! audit-only; see [`DECISIONS.md` §"`provenance.yaml` audit index"][provenance-audit] for the rationale (`spec.md` is the
//! authoritative artifact).
//!
//! [provenance-audit]: ../../../../DECISIONS.md#provenanceyaml-audit-index

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::Path;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use specify_diagnostics::{Artifact, Diagnostic};
use specify_error::{Error, Result};
use specify_model::spec::provenance::RequirementStatus;

use crate::schema::{PROVENANCE_JSON_SCHEMA, evidence_yaml_paths, validate_serialisable};

/// In-memory model of `provenance.yaml` (workflow §Provenance index).
///
/// Top-level shape is closed; unknown fields are rejected per the
/// matching schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ProvenanceIndex {
    /// Stored schema version. Currently `1`; additive fields land
    /// without a bump.
    pub version: u32,
    /// Slice name. MUST match the directory under `.specify/slices/`.
    pub slice: String,
    /// UTC second-precision timestamp at which `/spec:refine` wrote
    /// the file. Resolution is to the second so byte-stable diffs
    /// survive reasonably-fast clocks.
    #[serde(with = "specify_error::serde_rfc3339")]
    pub generated_at: Timestamp,
    /// CLI version that wrote the file (e.g. `specify@2.1.0`).
    pub generator: String,
    /// One entry per `REQ-*` id in `spec.md`; order matches `spec.md`
    /// order. `specrun slice validate` enforces id-set parity in both
    /// directions.
    pub requirements: Vec<ProvenanceRequirement>,
}

/// One row under [`ProvenanceIndex::requirements`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ProvenanceRequirement {
    /// Requirement id matching a `REQ-NNN` heading in `spec.md`.
    pub id: String,
    /// Mirrors the `Status:` line on the matching `spec.md` block.
    pub status: RequirementStatus,
    /// Source keys cited on the matching `spec.md` `Sources:` line.
    /// Empty when `status` is `unknown` and `resolution` is
    /// `unknown-no-evidence`.
    pub sources: Vec<String>,
    /// Every `(source, id)` pair synthesis consulted — *not*
    /// only the winning one. Operators auditing a divergence can see
    /// what was dropped.
    pub contributing_claims: Vec<ContributingClaim>,
    /// How synthesis arrived at the requirement's final value. See
    /// [`ProvenanceResolution`] for the closed variant set and meanings.
    pub resolution: ProvenanceResolution,
    /// Optional trace describing how a non-trivial resolution
    /// selected the winning claim. Present only when `resolution` is
    /// [`ProvenanceResolution::AuthorityResolved`] or
    /// [`ProvenanceResolution::PerSliceOverride`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_trace: Option<ResolutionTrace>,
}

/// One contributing-claim entry under
/// [`ProvenanceRequirement::contributing_claims`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ContributingClaim {
    /// Source key (matches a top-level `plan.yaml.sources.<key>`
    /// binding) the claim came from.
    pub source: String,
    /// Claim id within the source's Evidence file (matches
    /// `claims[].id`).
    pub id: String,
    /// Claim kind copied from the source Evidence claim — closed
    /// enum (mirrored from
    /// `schemas/evidence.schema.json#/$defs/claimKind`).
    pub kind: specify_model::evidence::ClaimKind,
    /// Optional single-line claim payload (statement / criterion /
    /// decision body). Multi-line bodies truncate to the first
    /// non-empty line with a trailing `…`; the 16 `KiB` cap is
    /// enforced by the writer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Optional `<path>#L<n>` anchor copied from the source Evidence
    /// claim so the operator can open the original line range.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Optional winner marker. `Some(true)` on the entry synthesis
    /// selected; `Some(false)` on entries dropped by authority
    /// resolution; `None` on `agreed` blocks where there is no
    /// winner / loser distinction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winner: Option<bool>,
}

/// Closed resolution enum per workflow §Provenance index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum ProvenanceResolution {
    /// One contributing claim only.
    SingleSource,
    /// Multiple contributors, identical value.
    SingleValueAgreement,
    /// Default authority ordering or per-Evidence override broke the
    /// tie.
    AuthorityResolved,
    /// Per-slice `authority-override` map picked the winner.
    PerSliceOverride,
    /// No contributing claims (paired with
    /// [`RequirementStatus::Unknown`]).
    UnknownNoEvidence,
    /// Same-authority disagreement with no override (paired with
    /// [`RequirementStatus::Conflict`]).
    TiedConflict,
}

/// Optional resolution trace under [`ProvenanceRequirement::resolution_trace`].
///
/// `step` is the name of the resolution step that broke the tie
/// (e.g. `per-slice-authority-override`,
/// `per-evidence-authority-override`,
/// `default-authority-ordering`). The schema keeps the field
/// free-form until the step taxonomy stabilises in v2; the optional
/// `override` map and `winner` source key narrow the audit trail
/// when present.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ResolutionTrace {
    /// Name of the resolution step that broke the tie.
    pub step: String,
    /// Optional override map consulted at this step — e.g.
    /// `{ criterion: identity-design-notes }`. Stored as raw JSON to
    /// keep the trace shape open while the taxonomy stabilises.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#override: Option<serde_json::Value>,
    /// Optional source key the step selected as the winner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winner: Option<String>,
}

impl ProvenanceIndex {
    /// Validate `self` against the embedded `schemas/slice/provenance.schema.json`.
    ///
    /// Returns `Ok(())` on a clean validation; otherwise a payload-free
    /// [`Error::Validation`] keyed on the code `"provenance-schema"`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] when the in-memory index fails
    /// the schema; falls back to [`Error::Diag`] when the value is
    /// not JSON-serialisable (unreachable in normal operation).
    pub fn validate(&self) -> Result<(), Error> {
        validate_serialisable(
            self,
            PROVENANCE_JSON_SCHEMA,
            "provenance-schema",
            "provenance.yaml conforms to schemas/slice/provenance.schema.json",
            "provenance-schema-serialise",
            "provenance.yaml",
        )
    }

    /// Load and schema-validate a `provenance.yaml` at `path`.
    ///
    /// Returns the parsed [`ProvenanceIndex`] on success. Schema
    /// validation runs *after* the YAML parse so unknown-field and
    /// shape problems surface as schema findings rather than serde
    /// deserialise errors when the schema can produce a clearer
    /// message — both paths still route through [`Error::Validation`]
    /// so callers see one variant for malformed input.
    ///
    /// # Errors
    ///
    /// - [`Error::Filesystem`] when `path` cannot be read.
    /// - [`Error::YamlDe`] when the file is not valid YAML.
    /// - [`Error::Validation`] when the file fails
    ///   `schemas/slice/provenance.schema.json`.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path).map_err(|source| Error::Filesystem {
            op: "read",
            path: path.to_path_buf(),
            source,
        })?;
        let index: Self = serde_saphyr::from_str(&raw)?;
        index.validate()?;
        Ok(index)
    }

    /// Compare `self` against the slice's `spec.md` `REQ-*` ids
    /// and the per-source evidence claim ids, returning every
    /// drift finding sorted for byte-stable error output.
    ///
    /// Drift kinds (`provenance.yaml` audit index §Acceptance scenario #26-4):
    ///
    /// 1. **Requirement-id drift** — `spec.md` `REQ-*` set ≠
    ///    `provenance.yaml.requirements[].id` set, in either direction.
    /// 2. **Contributing-claim drift** — any
    ///    `contributing-claims[].(source, id)` pair that does
    ///    not resolve to a real claim in the corresponding
    ///    `.specify/slices/<slice>/evidence/<source>.yaml`.
    ///    Comparison is on `(source, id)`; `path` is
    ///    informational only.
    ///
    /// Findings sort by `(req_id, drift-kind, source, id)` so
    /// repeated runs produce byte-identical error envelopes.
    #[must_use]
    pub fn detect_drift(
        &self, spec_req_ids: &BTreeSet<String>, evidence: &EvidenceClaimIds,
    ) -> Vec<ProvenanceDrift> {
        let mut out: Vec<ProvenanceDrift> = Vec::new();

        let provenance_req_ids: BTreeSet<&str> =
            self.requirements.iter().map(|r| r.id.as_str()).collect();
        for spec_id in spec_req_ids {
            if !provenance_req_ids.contains(spec_id.as_str()) {
                out.push(ProvenanceDrift::MissingProvenanceRequirement {
                    req_id: spec_id.clone(),
                });
            }
        }
        for req in &self.requirements {
            if !spec_req_ids.contains(&req.id) {
                out.push(ProvenanceDrift::ExtraProvenanceRequirement {
                    req_id: req.id.clone(),
                });
            }
        }

        for req in &self.requirements {
            for claim in &req.contributing_claims {
                let exists = evidence
                    .get(claim.source.as_str())
                    .is_some_and(|ids| ids.contains(claim.id.as_str()));
                if !exists {
                    out.push(ProvenanceDrift::ContributingClaimNotFound {
                        req_id: req.id.clone(),
                        source: claim.source.clone(),
                        id: claim.id.clone(),
                        path: claim.path.clone(),
                    });
                }
            }
        }

        out.sort_by_key(ProvenanceDrift::sort_key);
        out
    }
}

/// One drift finding produced by [`ProvenanceIndex::detect_drift`].
///
/// Each variant maps to one `slice-provenance-drift` [`Diagnostic`]
/// `violation` via [`ProvenanceDrift::into_diagnostic`], rendered on the
/// `slice validate` surface alongside the other findings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvenanceDrift {
    /// A `REQ-*` heading exists in `spec.md` with no matching
    /// `requirements[].id` row in `provenance.yaml`. Operator must
    /// re-run `/spec:refine` to regenerate the index.
    MissingProvenanceRequirement {
        /// `REQ-NNN` id that is missing from `provenance.yaml`.
        req_id: String,
    },
    /// A `requirements[].id` row exists in `provenance.yaml` with no
    /// matching `REQ-*` heading in `spec.md` — typically a stale
    /// provenance entry after the operator hand-deleted a requirement
    /// from `spec.md`.
    ExtraProvenanceRequirement {
        /// `REQ-NNN` id that has no matching `spec.md` heading.
        req_id: String,
    },
    /// A `contributing-claims[].(source, id)` pair does not
    /// resolve to a real claim in
    /// `.specify/slices/<slice>/evidence/<source>.yaml`. Either the
    /// evidence file was hand-edited or the provenance entry references
    /// a claim from a stale extract.
    ContributingClaimNotFound {
        /// Owning requirement id (the `requirements[].id` the
        /// contributing claim sits under).
        req_id: String,
        /// Source key the claim was attributed to.
        source: String,
        /// Claim id that could not be resolved.
        id: String,
        /// Informational path copy from the contributing entry, when
        /// present; the comparison key is `(source, id)` only.
        path: Option<String>,
    },
}

impl ProvenanceDrift {
    fn sort_key(&self) -> (String, u8, String, String) {
        match self {
            Self::MissingProvenanceRequirement { req_id } => {
                (req_id.clone(), 0, String::new(), String::new())
            }
            Self::ExtraProvenanceRequirement { req_id } => {
                (req_id.clone(), 1, String::new(), String::new())
            }
            Self::ContributingClaimNotFound {
                req_id, source, id, ..
            } => (req_id.clone(), 2, source.clone(), id.clone()),
        }
    }

    /// Lift a drift finding into the `slice-provenance-drift`
    /// [`Diagnostic`] the CLI renders on the `slice validate` surface.
    #[must_use]
    pub fn into_diagnostic(self) -> Diagnostic {
        let detail = match self {
            Self::MissingProvenanceRequirement { req_id } => {
                format!(
                    "{req_id} appears in spec.md but is missing from provenance.yaml; re-run `/spec:refine` to regenerate the provenance index"
                )
            }
            Self::ExtraProvenanceRequirement { req_id } => {
                format!(
                    "{req_id} appears in provenance.yaml but no matching `REQ-*` heading exists in spec.md; re-run `/spec:refine` to regenerate the provenance index"
                )
            }
            Self::ContributingClaimNotFound {
                req_id,
                source,
                id,
                path,
            } => {
                let suffix = path.map_or_else(String::new, |p| format!(" (path: {p})"));
                format!(
                    "{req_id}: contributing-claim source `{source}` id `{id}` does not resolve to a claim in evidence/{source}.yaml{suffix}"
                )
            }
        };
        Diagnostic::violation(
            "slice-provenance-drift",
            "provenance.yaml stays in sync with spec.md REQ ids and per-source evidence claims",
            detail,
            Artifact::Specs,
            None,
        )
    }
}

/// Map of source key → set of `id` strings found in that
/// source's evidence file. Built by [`collect_evidence_claim_ids`]
/// and consumed by [`ProvenanceIndex::detect_drift`].
pub type EvidenceClaimIds = BTreeMap<String, BTreeSet<String>>;

/// Build the `(source → id set)` lookup the drift gate
/// consumes.
///
/// Walks `<slice_dir>/evidence/` and collects every `id` value
/// keyed by the source key inferred from the filename stem
/// (`<source>.yaml` → `<source>`). Files without a `claims:`
/// array or without `id` entries contribute an empty set so
/// drift detection can still report missing claims against the
/// known source key.
///
/// The evidence schema is `additionalProperties: true` on every
/// claim, so this helper deliberately uses `serde_json::Value`
/// rather than the typed [`specify_model::evidence`] surface: drift
/// detection cares only about the `(source, id)` join keys,
/// and tolerating unknown per-kind body fields here keeps the
/// helper forward-compatible with future claim kinds.
///
/// # Errors
///
/// - [`Error::Filesystem`] when `evidence/` exists but cannot be
///   read.
/// - [`Error::YamlDe`] when an evidence file does not parse as
///   YAML (the same file would also fail
///   [`crate::schema::validate_evidence_dir`] — both checks run via
///   `slice validate`).
pub fn collect_evidence_claim_ids(slice_dir: &Path) -> Result<EvidenceClaimIds> {
    let mut out: EvidenceClaimIds = BTreeMap::new();
    let paths = evidence_yaml_paths(slice_dir)?;

    for path in paths {
        let Some(stem) = path.file_stem().and_then(OsStr::to_str) else { continue };
        let source = stem.to_string();
        let raw = std::fs::read_to_string(&path).map_err(|source| Error::Filesystem {
            op: "read",
            path: path.clone(),
            source,
        })?;
        let value: JsonValue = serde_saphyr::from_str(&raw)?;
        let claim_ids = extract_claim_ids(&value);
        out.entry(source).or_default().extend(claim_ids);
    }
    Ok(out)
}

fn extract_claim_ids(doc: &JsonValue) -> BTreeSet<String> {
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let Some(claims) = doc.get("claims").and_then(JsonValue::as_array) else {
        return ids;
    };
    for claim in claims {
        if let Some(id) = claim.get("id").and_then(JsonValue::as_str) {
            ids.insert(id.to_string());
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use specify_model::evidence::ClaimKind;

    use super::*;
    use crate::journal::test_timestamp;

    fn sample() -> ProvenanceIndex {
        ProvenanceIndex {
            version: 1,
            slice: "identity-user-registration".to_string(),
            generated_at: test_timestamp("2026-05-22T13:15:00Z"),
            generator: "specify@2.1.0".to_string(),
            requirements: vec![
                ProvenanceRequirement {
                    id: "REQ-001".to_string(),
                    status: RequirementStatus::Agreed,
                    sources: vec!["identity-design-notes".to_string(), "runtime".to_string()],
                    contributing_claims: vec![
                        ContributingClaim {
                            source: "identity-design-notes".to_string(),
                            id: "password-reset.request".to_string(),
                            kind: ClaimKind::Requirement,
                            value: None,
                            path: None,
                            winner: None,
                        },
                        ContributingClaim {
                            source: "runtime".to_string(),
                            id: "users.register.happy-path".to_string(),
                            kind: ClaimKind::Example,
                            value: None,
                            path: None,
                            winner: None,
                        },
                    ],
                    resolution: ProvenanceResolution::SingleValueAgreement,
                    resolution_trace: None,
                },
                ProvenanceRequirement {
                    id: "REQ-007".to_string(),
                    status: RequirementStatus::Divergence,
                    sources: vec![
                        "identity-design-notes".to_string(),
                        "legacy-monolith".to_string(),
                    ],
                    contributing_claims: vec![
                        ContributingClaim {
                            source: "identity-design-notes".to_string(),
                            id: "password-reset.expiry".to_string(),
                            kind: ClaimKind::Criterion,
                            value: Some("Reset links expire after 30 minutes.".to_string()),
                            path: Some("docs/account.md#L7".to_string()),
                            winner: Some(true),
                        },
                        ContributingClaim {
                            source: "legacy-monolith".to_string(),
                            id: "password-reset.expiry".to_string(),
                            kind: ClaimKind::Criterion,
                            value: Some("expiresAt = createdAt + 24h".to_string()),
                            path: Some("src/users/reset.ts#L42".to_string()),
                            winner: Some(false),
                        },
                    ],
                    resolution: ProvenanceResolution::PerSliceOverride,
                    resolution_trace: Some(ResolutionTrace {
                        step: "per-slice-authority-override".to_string(),
                        r#override: Some(serde_json::json!({
                            "criterion": "identity-design-notes",
                        })),
                        winner: Some("identity-design-notes".to_string()),
                    }),
                },
            ],
        }
    }

    #[test]
    fn round_trips_through_yaml() {
        let original = sample();
        let yaml = serde_saphyr::to_string(&original).expect("serialise");
        assert!(yaml.contains("generated-at: 2026-05-22T13:15:00Z"));
        assert!(yaml.contains("contributing-claims:"));
        assert!(yaml.contains("resolution: per-slice-override"));
        let reparsed: ProvenanceIndex = serde_saphyr::from_str(&yaml).expect("reparse");
        assert_eq!(original, reparsed);
    }

    #[test]
    fn validates_against_embedded_schema() {
        sample()
            .validate()
            .expect("sample provenance index must validate against the embedded schema");
    }

    #[test]
    fn resolution_round_trips_kebab_case() {
        for (variant, wire) in [
            (ProvenanceResolution::SingleSource, "single-source"),
            (ProvenanceResolution::SingleValueAgreement, "single-value-agreement"),
            (ProvenanceResolution::AuthorityResolved, "authority-resolved"),
            (ProvenanceResolution::PerSliceOverride, "per-slice-override"),
            (ProvenanceResolution::UnknownNoEvidence, "unknown-no-evidence"),
            (ProvenanceResolution::TiedConflict, "tied-conflict"),
        ] {
            assert_eq!(serde_json::to_string(&variant).expect("serialise"), format!("\"{wire}\""));
        }
    }

    #[test]
    fn rejects_unknown_top_level_fields() {
        let yaml = r"version: 1
slice: x
generated-at: 2026-05-22T13:15:00Z
generator: specify@2.1.0
requirements: []
rogue: true
";
        let err = serde_saphyr::from_str::<ProvenanceIndex>(yaml)
            .expect_err("deny_unknown_fields must reject rogue");
        assert!(err.to_string().contains("rogue"), "expected error to name rogue, got: {err}");
    }

    #[test]
    fn load_reports_schema_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("provenance.yaml");
        // `version: 0` parses cleanly (u32) but trips the schema's
        // `minimum: 1` so the failure surfaces as Validation rather
        // than YAML deserialise.
        std::fs::write(
            &path,
            "version: 0\n\
             slice: my-slice\n\
             generated-at: 2026-05-22T13:15:00Z\n\
             generator: specify@2.1.0\n\
             requirements: []\n",
        )
        .expect("write");
        let err = ProvenanceIndex::load(&path).expect_err("schema must reject");
        assert!(matches!(err, Error::Validation { .. }), "expected Validation, got: {err}");
    }

    #[test]
    fn load_reports_yaml_parse_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("provenance.yaml");
        // Missing required `generator` field — serde catches this
        // before the schema validator runs; the failure routes
        // through `Error::YamlDe`, not `Error::Validation`, but is
        // still surfaced (the slice validate caller bundles both).
        std::fs::write(
            &path,
            "version: 1\n\
             slice: my-slice\n\
             generated-at: 2026-05-22T13:15:00Z\n\
             requirements: []\n",
        )
        .expect("write");
        let err = ProvenanceIndex::load(&path).expect_err("missing field must error");
        assert!(matches!(err, Error::YamlDe(_)), "expected YamlDe, got: {err}");
    }

    fn req_id_set<const N: usize>(ids: [&str; N]) -> BTreeSet<String> {
        ids.into_iter().map(str::to_string).collect()
    }

    fn evidence_map<const N: usize>(rows: [(&str, &[&str]); N]) -> EvidenceClaimIds {
        rows.into_iter()
            .map(|(src, ids)| (src.to_string(), ids.iter().map(|s| (*s).to_string()).collect()))
            .collect()
    }

    #[test]
    fn detect_drift_clean_no_findings() {
        let index = sample();
        let spec_ids = req_id_set(["REQ-001", "REQ-007"]);
        let evidence = evidence_map([
            ("identity-design-notes", &["password-reset.request", "password-reset.expiry"][..]),
            ("runtime", &["users.register.happy-path"][..]),
            ("legacy-monolith", &["password-reset.expiry"][..]),
        ]);
        assert!(index.detect_drift(&spec_ids, &evidence).is_empty());
    }

    #[test]
    fn collect_claim_ids_walks_yaml_and_yml() {
        let dir = tempfile::tempdir().expect("tempdir");
        let evidence_dir = dir.path().join("evidence");
        std::fs::create_dir_all(&evidence_dir).expect("mkdir");
        std::fs::write(
            evidence_dir.join("runtime.yaml"),
            r"source: runtime
adapter: captures
authority: behaviour
lead: user-registration
claims:
  - kind: example
    id: users.register.happy-path
  - kind: example
    id: users.register.minimal
",
        )
        .expect("write runtime");
        std::fs::write(
            evidence_dir.join("legacy.yml"),
            r"source: legacy
adapter: code-typescript
authority: behaviour
lead: user-registration
claims:
  - kind: excerpt
    id: users.register.email-validation
  - kind: requirement
    id: users.register.requires-email
",
        )
        .expect("write legacy");
        // A non-YAML sibling must be ignored.
        std::fs::write(evidence_dir.join("ignore.txt"), "not yaml").expect("ignored");
        let map = collect_evidence_claim_ids(dir.path()).expect("collect");
        let runtime = map.get("runtime").expect("runtime row");
        assert_eq!(runtime.len(), 2);
        assert!(runtime.contains("users.register.happy-path"));
        assert!(runtime.contains("users.register.minimal"));
        let legacy = map.get("legacy").expect("legacy row");
        assert_eq!(legacy.len(), 2);
        assert!(legacy.contains("users.register.email-validation"));
        assert!(legacy.contains("users.register.requires-email"));
        assert!(!map.contains_key("ignore"));
    }

    #[test]
    fn collect_claim_ids_missing_dir_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let map = collect_evidence_claim_ids(dir.path()).expect("collect");
        assert!(map.is_empty());
    }
}
