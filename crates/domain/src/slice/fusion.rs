//! RFC-27 §D4 reconciliation index — `fusion.yaml`.
//!
//! One file per slice at `.specify/slices/<slice>/fusion.yaml`. Lists
//! every `REQ-*` id in `spec.md` and the contributing
//! `(source-key, claim-id)` pairs plus the authority outcome.
//! Validated against `schemas/slice/fusion.schema.json`; the file is
//! audit-only — `spec.md` remains the authoritative artifact for
//! downstream verbs.
//!
//! Change 2.6 wires up the YAML read and validation envelope
//! ([`FusionIndex::load`]) and drift detection
//! ([`FusionIndex::detect_drift`]) consumed by `specify slice
//! validate`. Agent-side authoring of `fusion.yaml` itself lands in
//! Change 3.2; the CLI half owns validation and inspection only.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use specify_error::{Error, Result, ValidationStatus, ValidationSummary};

use crate::schema::{fusion_schema_source, validate_value};
use crate::spec::provenance::RequirementStatus;

/// In-memory model of `fusion.yaml` (RFC-27 §Reconciliation index).
///
/// Top-level shape is closed; unknown fields are rejected per the
/// matching schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FusionIndex {
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
    /// order. `specify slice validate` enforces id-set parity in both
    /// directions.
    pub requirements: Vec<FusionRequirement>,
}

/// One row under [`FusionIndex::requirements`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FusionRequirement {
    /// Requirement id matching a `REQ-NNN` heading in `spec.md`.
    pub id: String,
    /// Mirrors the `Status:` line on the matching `spec.md` block.
    pub status: RequirementStatus,
    /// Source keys cited on the matching `spec.md` `Sources:` line.
    /// Empty when `status` is `unknown` and `resolution` is
    /// `unknown-no-evidence`.
    pub sources: Vec<String>,
    /// Every `(source, claim-id)` pair synthesis consulted — *not*
    /// only the winning one. Operators auditing a divergence can see
    /// what was dropped.
    pub contributing_claims: Vec<ContributingClaim>,
    /// How synthesis arrived at the requirement's final value.
    pub resolution: FusionResolution,
    /// Optional trace describing how a non-trivial resolution
    /// selected the winning claim. Present only when `resolution` is
    /// [`FusionResolution::AuthorityResolved`] or
    /// [`FusionResolution::PerSliceOverride`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_trace: Option<ResolutionTrace>,
}

/// One contributing-claim entry under
/// [`FusionRequirement::contributing_claims`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ContributingClaim {
    /// Source key (matches a top-level `plan.yaml.sources.<key>`
    /// binding) the claim came from.
    pub source: String,
    /// Claim id within the source's Evidence file (matches
    /// `claims[].claim-id`).
    pub claim_id: String,
    /// Claim kind copied from the source Evidence claim — closed
    /// enum (mirrored from
    /// `schemas/evidence.schema.json#/$defs/claimKind`).
    pub kind: crate::evidence::ClaimKind,
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

/// Closed resolution enum per RFC-27 §Reconciliation index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum FusionResolution {
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

/// Optional resolution trace under [`FusionRequirement::resolution_trace`].
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

impl FusionIndex {
    /// Validate `self` against the embedded `schemas/slice/fusion.schema.json`.
    ///
    /// Returns `Ok(())` on a clean validation; otherwise an
    /// [`Error::Validation`] whose single [`ValidationSummary`]
    /// carries the rule id `"fusion-schema"`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] when the in-memory index fails
    /// the schema; falls back to [`Error::Diag`] when the value is
    /// not JSON-serialisable (unreachable in normal operation).
    pub fn validate(&self) -> Result<(), Error> {
        let instance = serde_json::to_value(self).map_err(|err| Error::Diag {
            code: "fusion-schema-serialise",
            detail: format!("failed to serialise fusion.yaml to JSON for schema validation: {err}"),
        })?;
        let results: Vec<ValidationSummary> = validate_value(
            &instance,
            fusion_schema_source(),
            "fusion-schema",
            "fusion.yaml conforms to schemas/slice/fusion.schema.json",
        )
        .into_iter()
        .filter(|s| s.status == ValidationStatus::Fail)
        .collect();
        if results.is_empty() { Ok(()) } else { Err(Error::Validation { results }) }
    }

    /// Load and schema-validate a `fusion.yaml` at `path`.
    ///
    /// Returns the parsed [`FusionIndex`] on success. Schema
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
    ///   `schemas/slice/fusion.schema.json`.
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
    /// Drift kinds (RFC-27 §D4 §Acceptance scenario #26-4):
    ///
    /// 1. **Requirement-id drift** — `spec.md` `REQ-*` set ≠
    ///    `fusion.yaml.requirements[].id` set, in either direction.
    /// 2. **Contributing-claim drift** — any
    ///    `contributing-claims[].(source, claim-id)` pair that does
    ///    not resolve to a real claim in the corresponding
    ///    `.specify/slices/<slice>/evidence/<source-key>.yaml`.
    ///    Comparison is on `(source, claim-id)`; `path` is
    ///    informational only.
    ///
    /// Findings sort by `(req_id, drift-kind, source, claim_id)` so
    /// repeated runs produce byte-identical error envelopes.
    #[must_use]
    pub fn detect_drift(
        &self, spec_req_ids: &BTreeSet<String>, evidence: &EvidenceClaimIds,
    ) -> Vec<FusionDrift> {
        let mut out: Vec<FusionDrift> = Vec::new();

        let fusion_req_ids: BTreeSet<&str> =
            self.requirements.iter().map(|r| r.id.as_str()).collect();
        for spec_id in spec_req_ids {
            if !fusion_req_ids.contains(spec_id.as_str()) {
                out.push(FusionDrift::MissingFusionRequirement {
                    req_id: spec_id.clone(),
                });
            }
        }
        for req in &self.requirements {
            if !spec_req_ids.contains(&req.id) {
                out.push(FusionDrift::ExtraFusionRequirement {
                    req_id: req.id.clone(),
                });
            }
        }

        for req in &self.requirements {
            for claim in &req.contributing_claims {
                let exists = evidence
                    .get(claim.source.as_str())
                    .is_some_and(|ids| ids.contains(claim.claim_id.as_str()));
                if !exists {
                    out.push(FusionDrift::ContributingClaimNotFound {
                        req_id: req.id.clone(),
                        source: claim.source.clone(),
                        claim_id: claim.claim_id.clone(),
                        path: claim.path.clone(),
                    });
                }
            }
        }

        out.sort_by_key(FusionDrift::sort_key);
        out
    }
}

/// One drift finding produced by [`FusionIndex::detect_drift`].
///
/// Each variant maps to one `slice-fusion-drift`
/// [`ValidationSummary`] line via [`FusionDrift::into_summary`] so
/// the wire envelope shape stays compatible with the existing
/// `slice validate` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FusionDrift {
    /// A `REQ-*` heading exists in `spec.md` with no matching
    /// `requirements[].id` row in `fusion.yaml`. Operator must
    /// re-run `/spec:refine` to regenerate the index.
    MissingFusionRequirement {
        /// `REQ-NNN` id that is missing from `fusion.yaml`.
        req_id: String,
    },
    /// A `requirements[].id` row exists in `fusion.yaml` with no
    /// matching `REQ-*` heading in `spec.md` — typically a stale
    /// fusion entry after the operator hand-deleted a requirement
    /// from `spec.md`.
    ExtraFusionRequirement {
        /// `REQ-NNN` id that has no matching `spec.md` heading.
        req_id: String,
    },
    /// A `contributing-claims[].(source, claim-id)` pair does not
    /// resolve to a real claim in
    /// `.specify/slices/<slice>/evidence/<source>.yaml`. Either the
    /// evidence file was hand-edited or the fusion entry references
    /// a claim from a stale extract.
    ContributingClaimNotFound {
        /// Owning requirement id (the `requirements[].id` the
        /// contributing claim sits under).
        req_id: String,
        /// Source key the claim was attributed to.
        source: String,
        /// Claim id that could not be resolved.
        claim_id: String,
        /// Informational path copy from the contributing entry, when
        /// present; the comparison key is `(source, claim_id)` only.
        path: Option<String>,
    },
}

impl FusionDrift {
    fn sort_key(&self) -> (String, u8, String, String) {
        match self {
            Self::MissingFusionRequirement { req_id } => {
                (req_id.clone(), 0, String::new(), String::new())
            }
            Self::ExtraFusionRequirement { req_id } => {
                (req_id.clone(), 1, String::new(), String::new())
            }
            Self::ContributingClaimNotFound {
                req_id,
                source,
                claim_id,
                ..
            } => (req_id.clone(), 2, source.clone(), claim_id.clone()),
        }
    }

    /// Lift a drift finding into the `slice-fusion-drift`
    /// [`ValidationSummary`] line the CLI emits under the existing
    /// `slice validate` envelope.
    #[must_use]
    pub fn into_summary(self) -> ValidationSummary {
        let detail = match self {
            Self::MissingFusionRequirement { req_id } => {
                format!(
                    "{req_id} appears in spec.md but is missing from fusion.yaml; re-run `/spec:refine` to regenerate the reconciliation index"
                )
            }
            Self::ExtraFusionRequirement { req_id } => {
                format!(
                    "{req_id} appears in fusion.yaml but no matching `REQ-*` heading exists in spec.md; re-run `/spec:refine` to regenerate the reconciliation index"
                )
            }
            Self::ContributingClaimNotFound {
                req_id,
                source,
                claim_id,
                path,
            } => {
                let suffix = path.map_or_else(String::new, |p| format!(" (path: {p})"));
                format!(
                    "{req_id}: contributing-claim source `{source}` claim-id `{claim_id}` does not resolve to a claim in evidence/{source}.yaml{suffix}"
                )
            }
        };
        ValidationSummary {
            status: ValidationStatus::Fail,
            rule_id: "slice-fusion-drift".into(),
            rule: "fusion.yaml stays in sync with spec.md REQ ids and per-source evidence claims"
                .into(),
            detail: Some(detail),
        }
    }
}

/// Map of source key → set of `claim-id` strings found in that
/// source's evidence file. Built by [`collect_evidence_claim_ids`]
/// and consumed by [`FusionIndex::detect_drift`].
pub type EvidenceClaimIds = BTreeMap<String, BTreeSet<String>>;

/// Build the `(source-key → claim-id set)` lookup the drift gate
/// consumes.
///
/// Walks `<slice_dir>/evidence/` and collects every `claim-id` value
/// keyed by the source key inferred from the filename stem
/// (`<source-key>.yaml` → `<source-key>`). Files without a `claims:`
/// array or without `claim-id` entries contribute an empty set so
/// drift detection can still report missing claims against the
/// known source key.
///
/// The evidence schema is `additionalProperties: true` on every
/// claim, so this helper deliberately uses `serde_json::Value`
/// rather than the typed [`crate::evidence`] surface: drift
/// detection cares only about the `(source, claim-id)` join keys,
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
    let evidence_dir = slice_dir.join("evidence");
    if !evidence_dir.is_dir() {
        return Ok(out);
    }
    let entries = std::fs::read_dir(&evidence_dir).map_err(|source| Error::Filesystem {
        op: "readdir",
        path: evidence_dir.clone(),
        source,
    })?;
    let mut paths: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| Error::Filesystem {
            op: "readdir-entry",
            path: evidence_dir.clone(),
            source,
        })?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path.extension().and_then(OsStr::to_str).unwrap_or("");
        if ext.eq_ignore_ascii_case("yaml") || ext.eq_ignore_ascii_case("yml") {
            paths.push(path);
        }
    }
    paths.sort();

    for path in paths {
        let Some(stem) = path.file_stem().and_then(OsStr::to_str) else { continue };
        let source_key = stem.to_string();
        let raw = std::fs::read_to_string(&path).map_err(|source| Error::Filesystem {
            op: "read",
            path: path.clone(),
            source,
        })?;
        let value: JsonValue = serde_saphyr::from_str(&raw)?;
        let claim_ids = extract_claim_ids(&value);
        out.entry(source_key).or_default().extend(claim_ids);
    }
    Ok(out)
}

fn extract_claim_ids(doc: &JsonValue) -> BTreeSet<String> {
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let Some(claims) = doc.get("claims").and_then(JsonValue::as_array) else {
        return ids;
    };
    for claim in claims {
        if let Some(id) = claim.get("claim-id").and_then(JsonValue::as_str) {
            ids.insert(id.to_string());
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evidence::ClaimKind;
    use crate::journal::test_timestamp;

    fn sample() -> FusionIndex {
        FusionIndex {
            version: 1,
            slice: "identity-user-registration".to_string(),
            generated_at: test_timestamp("2026-05-22T13:15:00Z"),
            generator: "specify@2.1.0".to_string(),
            requirements: vec![
                FusionRequirement {
                    id: "REQ-001".to_string(),
                    status: RequirementStatus::Agreed,
                    sources: vec!["identity-design-notes".to_string(), "runtime".to_string()],
                    contributing_claims: vec![
                        ContributingClaim {
                            source: "identity-design-notes".to_string(),
                            claim_id: "password-reset.request".to_string(),
                            kind: ClaimKind::Requirement,
                            value: None,
                            path: None,
                            winner: None,
                        },
                        ContributingClaim {
                            source: "runtime".to_string(),
                            claim_id: "users.register.happy-path".to_string(),
                            kind: ClaimKind::Example,
                            value: None,
                            path: None,
                            winner: None,
                        },
                    ],
                    resolution: FusionResolution::SingleValueAgreement,
                    resolution_trace: None,
                },
                FusionRequirement {
                    id: "REQ-007".to_string(),
                    status: RequirementStatus::Divergence,
                    sources: vec![
                        "identity-design-notes".to_string(),
                        "legacy-monolith".to_string(),
                    ],
                    contributing_claims: vec![
                        ContributingClaim {
                            source: "identity-design-notes".to_string(),
                            claim_id: "password-reset.expiry".to_string(),
                            kind: ClaimKind::Criterion,
                            value: Some("Reset links expire after 30 minutes.".to_string()),
                            path: Some("docs/account.md#L7".to_string()),
                            winner: Some(true),
                        },
                        ContributingClaim {
                            source: "legacy-monolith".to_string(),
                            claim_id: "password-reset.expiry".to_string(),
                            kind: ClaimKind::Criterion,
                            value: Some("expiresAt = createdAt + 24h".to_string()),
                            path: Some("src/users/reset.ts#L42".to_string()),
                            winner: Some(false),
                        },
                    ],
                    resolution: FusionResolution::PerSliceOverride,
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
        let reparsed: FusionIndex = serde_saphyr::from_str(&yaml).expect("reparse");
        assert_eq!(original, reparsed);
    }

    #[test]
    fn validates_against_embedded_schema() {
        sample().validate().expect("sample fusion index must validate against the embedded schema");
    }

    #[test]
    fn fusion_resolution_round_trips_kebab_case() {
        for (variant, wire) in [
            (FusionResolution::SingleSource, "single-source"),
            (FusionResolution::SingleValueAgreement, "single-value-agreement"),
            (FusionResolution::AuthorityResolved, "authority-resolved"),
            (FusionResolution::PerSliceOverride, "per-slice-override"),
            (FusionResolution::UnknownNoEvidence, "unknown-no-evidence"),
            (FusionResolution::TiedConflict, "tied-conflict"),
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
        let err = serde_saphyr::from_str::<FusionIndex>(yaml)
            .expect_err("deny_unknown_fields must reject rogue");
        assert!(err.to_string().contains("rogue"), "expected error to name rogue, got: {err}");
    }

    #[test]
    fn load_reports_schema_failure_for_hand_edited_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fusion.yaml");
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
        let err = FusionIndex::load(&path).expect_err("schema must reject");
        assert!(matches!(err, Error::Validation { .. }), "expected Validation, got: {err}");
    }

    #[test]
    fn load_reports_yaml_parse_failure_when_required_field_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("fusion.yaml");
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
        let err = FusionIndex::load(&path).expect_err("missing field must error");
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
    fn detect_drift_clean_inputs_yield_no_findings() {
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
    fn detect_drift_flags_missing_fusion_entry() {
        let index = sample();
        let spec_ids = req_id_set(["REQ-001", "REQ-007", "REQ-042"]);
        let evidence = evidence_map([
            ("identity-design-notes", &["password-reset.request", "password-reset.expiry"][..]),
            ("runtime", &["users.register.happy-path"][..]),
            ("legacy-monolith", &["password-reset.expiry"][..]),
        ]);
        let drift = index.detect_drift(&spec_ids, &evidence);
        assert_eq!(drift.len(), 1);
        assert_eq!(
            drift[0],
            FusionDrift::MissingFusionRequirement {
                req_id: "REQ-042".to_string(),
            }
        );
    }

    #[test]
    fn detect_drift_flags_extra_fusion_entry() {
        let index = sample();
        let spec_ids = req_id_set(["REQ-001"]); // REQ-007 dropped from spec.md
        let evidence = evidence_map([
            ("identity-design-notes", &["password-reset.request", "password-reset.expiry"][..]),
            ("runtime", &["users.register.happy-path"][..]),
            ("legacy-monolith", &["password-reset.expiry"][..]),
        ]);
        let drift = index.detect_drift(&spec_ids, &evidence);
        assert_eq!(drift.len(), 1);
        assert_eq!(
            drift[0],
            FusionDrift::ExtraFusionRequirement {
                req_id: "REQ-007".to_string(),
            }
        );
    }

    #[test]
    fn detect_drift_flags_contributing_claim_with_no_evidence_row() {
        let index = sample();
        let spec_ids = req_id_set(["REQ-001", "REQ-007"]);
        // legacy-monolith file present but missing the expiry claim id.
        let evidence = evidence_map([
            ("identity-design-notes", &["password-reset.request", "password-reset.expiry"][..]),
            ("runtime", &["users.register.happy-path"][..]),
            ("legacy-monolith", &[][..]),
        ]);
        let drift = index.detect_drift(&spec_ids, &evidence);
        assert_eq!(drift.len(), 1);
        assert!(
            matches!(
                &drift[0],
                FusionDrift::ContributingClaimNotFound { req_id, source, claim_id, .. }
                if req_id == "REQ-007" && source == "legacy-monolith" && claim_id == "password-reset.expiry"
            ),
            "unexpected drift: {:?}",
            drift[0]
        );
    }

    #[test]
    fn detect_drift_flags_contributing_claim_with_missing_source_file() {
        let index = sample();
        let spec_ids = req_id_set(["REQ-001", "REQ-007"]);
        // The runtime evidence file is missing entirely; both claims that cite it must drift.
        let evidence = evidence_map([
            ("identity-design-notes", &["password-reset.request", "password-reset.expiry"][..]),
            ("legacy-monolith", &["password-reset.expiry"][..]),
        ]);
        let drift = index.detect_drift(&spec_ids, &evidence);
        assert_eq!(drift.len(), 1, "drift: {drift:?}");
        assert!(matches!(
            &drift[0],
            FusionDrift::ContributingClaimNotFound { source, claim_id, .. }
            if source == "runtime" && claim_id == "users.register.happy-path"
        ));
    }

    #[test]
    fn detect_drift_findings_sort_byte_stable() {
        // Build a deliberately scrambled set of drifts: contributing-claim
        // problems on REQ-007 (multiple sources), an extra fusion entry,
        // a missing fusion entry. Verify the sort order is
        // (req_id, kind, source, claim_id).
        let mut index = sample();
        // Add a third requirement that has no matching spec entry → extra drift.
        index.requirements.push(FusionRequirement {
            id: "REQ-999".to_string(),
            status: RequirementStatus::Agreed,
            sources: vec!["runtime".to_string()],
            contributing_claims: vec![ContributingClaim {
                source: "runtime".to_string(),
                claim_id: "ghost".to_string(),
                kind: ClaimKind::Example,
                value: None,
                path: None,
                winner: None,
            }],
            resolution: FusionResolution::SingleSource,
            resolution_trace: None,
        });
        let spec_ids = req_id_set(["REQ-001", "REQ-007", "REQ-MIS"]); // REQ-MIS not in fusion
        let evidence = evidence_map([
            ("identity-design-notes", &["password-reset.request"][..]),
            ("runtime", &[][..]),
            ("legacy-monolith", &[][..]),
        ]);
        let drift = index.detect_drift(&spec_ids, &evidence);
        let kinds: Vec<&'static str> = drift
            .iter()
            .map(|d| match d {
                FusionDrift::MissingFusionRequirement { .. } => "missing",
                FusionDrift::ExtraFusionRequirement { .. } => "extra",
                FusionDrift::ContributingClaimNotFound { .. } => "claim",
            })
            .collect();
        let req_ids: Vec<&str> = drift
            .iter()
            .map(|d| match d {
                FusionDrift::MissingFusionRequirement { req_id }
                | FusionDrift::ExtraFusionRequirement { req_id }
                | FusionDrift::ContributingClaimNotFound { req_id, .. } => req_id.as_str(),
            })
            .collect();
        // Sort order is (req_id, kind, source, claim_id); the kind
        // marker orders missing < extra < claim within one req_id.
        // Across req_ids, `REQ-9` sorts before `REQ-M` because
        // '9' (0x39) < 'M' (0x4D), so REQ-999 lands before REQ-MIS.
        assert_eq!(
            (kinds.clone(), req_ids.clone()),
            (
                vec![
                    "claim",   // REQ-001 contributing-claim drift on `runtime`
                    "claim",   // REQ-007 source=identity-design-notes (sorted first)
                    "claim",   // REQ-007 source=legacy-monolith
                    "extra",   // REQ-999 extra-fusion-requirement (kind=1 before kind=2)
                    "claim",   // REQ-999 ghost contributing claim
                    "missing", // REQ-MIS missing-fusion-requirement
                ],
                vec!["REQ-001", "REQ-007", "REQ-007", "REQ-999", "REQ-999", "REQ-MIS"],
            )
        );
    }

    #[test]
    fn collect_evidence_claim_ids_walks_yaml_and_yml() {
        let dir = tempfile::tempdir().expect("tempdir");
        let evidence_dir = dir.path().join("evidence");
        std::fs::create_dir_all(&evidence_dir).expect("mkdir");
        std::fs::write(
            evidence_dir.join("runtime.yaml"),
            r"source: runtime
adapter: code-runtime
authority: behaviour
candidate: user-registration
claims:
  - kind: example
    claim-id: users.register.happy-path
  - kind: example
    claim-id: users.register.minimal
",
        )
        .expect("write runtime");
        std::fs::write(
            evidence_dir.join("legacy.yml"),
            r"source: legacy
adapter: code-typescript
authority: behaviour
candidate: user-registration
claims:
  - kind: excerpt
    claim-id: users.register.email-validation
  - kind: requirement
    claim-id: users.register.requires-email
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
    fn collect_evidence_claim_ids_missing_dir_is_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let map = collect_evidence_claim_ids(dir.path()).expect("collect");
        assert!(map.is_empty());
    }
}
