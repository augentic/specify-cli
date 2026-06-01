//! Slice model — `model.yaml`.
//!
//! One structured artifact per slice at
//! `.specify/slices/<slice>/model.yaml` (RFC-29 M2b). The single
//! `schemas/slice/model.schema.json` validates both the agent's
//! synthesis-response `model` and the persisted file: kernel-owned and
//! header fields are optional so the kernel re-derives/stamps them on
//! projection (normalize, never reject). Provenance is carried inline
//! on each requirement, so the provenance view is *projected* on
//! demand by `specrun slice provenance` rather than persisted as a
//! second file. See [`DECISIONS.md` §"Single slice-model artifact"][model-artifact].
//!
//! [model-artifact]: ../../../../DECISIONS.md#single-slice-model-artifact-rfc-29-m2b-simplification

use std::path::Path;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use specify_error::{Error, Result};
use specify_model::evidence::ClaimKind;
use specify_model::spec::provenance::RequirementStatus;
use specify_schema::{ValidationStatus, join_details};

use crate::schema::{SLICE_MODEL_JSON_SCHEMA, validate_value};
use crate::slice::provenance::{
    ContributingClaim, ProvenanceIndex, ProvenanceRequirement, ProvenanceResolution,
    ResolutionTrace,
};

/// In-memory view of `model.yaml`, holding the header and the
/// requirement set with inline provenance.
///
/// Non-requirements sections (`domain`, `apis`, …) are validated by the
/// schema but not modelled here — the provenance projection draws only
/// on `requirements[]`, so unknown fields are ignored on deserialise
/// (the schema enforces the closed top-level shape during
/// [`SliceModel::load`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SliceModel {
    /// Stored schema version. Kernel-stamped on the persisted file;
    /// optional because the agent response omits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// Slice name. Kernel-stamped on the persisted file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slice: Option<String>,
    /// Bound target `name@vN`. Kernel-stamped on the persisted file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Bound project, optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// The requirement set with inline provenance.
    #[serde(default)]
    pub requirements: Vec<ModelRequirement>,
}

/// One `requirements[]` entry. Kernel-owned fields (`id`, `status`,
/// `sources`, `resolution`, `resolution-trace`) are optional: the
/// agent omits them and the kernel re-derives them on projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ModelRequirement {
    /// Kernel-projected `REQ-NNN` id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Kernel-projected status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<RequirementStatus>,
    /// Kernel-projected rendered source list (highest authority first).
    #[serde(default)]
    pub sources: Vec<String>,
    /// Agent-authored contributing claims with kernel-projected
    /// `value` / `path` / `winner`.
    #[serde(default)]
    pub claims: Vec<ModelClaim>,
    /// Kernel-projected resolution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<ProvenanceResolution>,
    /// Kernel-projected resolution trace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution_trace: Option<ResolutionTrace>,
}

/// One inline claim under [`ModelRequirement::claims`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ModelClaim {
    /// Source key the claim came from.
    pub source: String,
    /// Claim id within that source's Evidence file.
    pub id: String,
    /// Claim kind (D13).
    pub kind: ClaimKind,
    /// Kernel-projected single-line claim payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Kernel-projected `<path>#L<n>` anchor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Kernel-projected winner marker (divergence only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winner: Option<bool>,
}

/// Validate a raw `model.yaml` document (parsed to JSON) against the
/// embedded `schemas/slice/model.schema.json`.
///
/// Validates the *whole* document — including the non-requirements
/// sections [`SliceModel`] does not model — so the closed top-level
/// shape is enforced even though the typed view captures only the
/// header and requirements.
///
/// # Errors
///
/// Returns [`Error::Validation`] keyed on `"slice-model-schema"` when
/// the document fails the schema.
pub fn validate_model_doc(value: &JsonValue) -> Result<()> {
    let rule = "model.yaml conforms to schemas/slice/model.schema.json";
    let failures: Vec<_> = validate_value(value, SLICE_MODEL_JSON_SCHEMA, "slice-model-schema", rule)
        .into_iter()
        .filter(|summary| summary.status == ValidationStatus::Fail)
        .collect();
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Error::Validation {
            code: "slice-model-schema".to_string(),
            detail: join_details(&failures),
        })
    }
}

impl SliceModel {
    /// Parse and schema-validate a `model.yaml` from its raw contents.
    ///
    /// The whole document is validated against the schema first, then
    /// the typed header + requirements view is deserialised from it.
    ///
    /// # Errors
    ///
    /// - [`Error::YamlDe`] when the contents are not valid YAML.
    /// - [`Error::Validation`] when the document fails the schema.
    pub fn parse_yaml(raw: &str) -> Result<Self> {
        let value: JsonValue = serde_saphyr::from_str(raw)?;
        validate_model_doc(&value)?;
        // Re-parse into the typed header + requirements view; unknown
        // sections (`domain`, `apis`, …) are ignored on deserialise,
        // having already been schema-checked on the raw document above.
        let model: Self = serde_saphyr::from_str(raw)?;
        Ok(model)
    }

    /// Load and schema-validate a `model.yaml` at `path`.
    ///
    /// # Errors
    ///
    /// - [`Error::Filesystem`] when `path` cannot be read.
    /// - [`Error::YamlDe`] when the file is not valid YAML.
    /// - [`Error::Validation`] when the file fails the schema.
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path).map_err(|source| Error::Filesystem {
            op: "read",
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse_yaml(&raw)
    }

    /// Project the audit-only provenance view from this model
    /// (RFC-29c §"Provenance projection"). The reshape is mechanical:
    /// provenance is already inline on each requirement, so the model
    /// and its projection can never drift.
    ///
    /// `generated_at` and `generator` stamp the projection's header so
    /// it round-trips against `schemas/slice/provenance.schema.json`;
    /// the per-requirement body is byte-stable given the same model.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] keyed on `"slice-model-incomplete"`
    /// when a persisted requirement is missing the kernel-owned
    /// `id` / `status` / `resolution` fields the projection requires
    /// (i.e. the model was a pre-projection agent draft, not a
    /// persisted artifact).
    pub fn to_provenance_index(
        &self, generated_at: Timestamp, generator: String,
    ) -> Result<ProvenanceIndex> {
        let mut requirements = Vec::with_capacity(self.requirements.len());
        for req in &self.requirements {
            let id = req.id.clone().ok_or_else(|| missing_field("requirements[].id"))?;
            let status = req.status.ok_or_else(|| missing_field("requirements[].status"))?;
            let resolution =
                req.resolution.ok_or_else(|| missing_field("requirements[].resolution"))?;
            let contributing_claims = req
                .claims
                .iter()
                .map(|c| ContributingClaim {
                    source: c.source.clone(),
                    id: c.id.clone(),
                    kind: c.kind,
                    value: c.value.clone(),
                    path: c.path.clone(),
                    winner: c.winner,
                })
                .collect();
            requirements.push(ProvenanceRequirement {
                id,
                status,
                sources: req.sources.clone(),
                contributing_claims,
                resolution,
                resolution_trace: req.resolution_trace.clone(),
            });
        }
        let index = ProvenanceIndex {
            version: 1,
            slice: self.slice.clone().unwrap_or_default(),
            generated_at,
            generator,
            requirements,
        };
        index.validate()?;
        Ok(index)
    }
}

fn missing_field(field: &str) -> Error {
    Error::validation_failed(
        "slice-model-incomplete",
        "a persisted model.yaml carries kernel-projected provenance fields",
        format!(
            "{field} is absent; the provenance projection requires a persisted (projected) \
             model.yaml, not a pre-projection synthesis draft"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::test_timestamp;

    /// A fully-projected `model.yaml` (kernel-owned fields present) with
    /// the seven required sections so it validates against the schema.
    const PROJECTED_MODEL: &str = "version: 1
slice: identity-service
target: omnia@v1
project: identity-service
requirements:
  - id: REQ-001
    title: Request password reset
    status: agreed
    sources: [docs, legacy]
    claims:
      - source: docs
        id: password-reset.request
        kind: requirement
        value: \"The system lets a user request a reset link.\"
        path: docs/identity/reset.md#L4
    resolution: single-value-agreement
    statement: The system lets a user request a reset link.
domain:
  types: []
apis:
  surfaces: []
configuration: []
technical-logic:
  decisions: []
observability: []
tasks: []
";

    #[test]
    fn parses_and_validates_projected_model() {
        let model = SliceModel::parse_yaml(PROJECTED_MODEL).expect("projected model must validate");
        assert_eq!(model.slice.as_deref(), Some("identity-service"));
        assert_eq!(model.requirements.len(), 1);
    }

    #[test]
    fn projects_provenance_from_inline_data() {
        let model = SliceModel::parse_yaml(PROJECTED_MODEL).expect("parse");
        let index = model
            .to_provenance_index(test_timestamp("2026-05-28T05:45:00Z"), "specify@2.1.0".to_string())
            .expect("projection succeeds");
        assert_eq!(index.slice, "identity-service");
        assert_eq!(index.requirements.len(), 1);
        let req = &index.requirements[0];
        assert_eq!(req.id, "REQ-001");
        assert_eq!(req.resolution, ProvenanceResolution::SingleValueAgreement);
        assert_eq!(req.contributing_claims.len(), 1);
        index.validate().expect("projected index must validate");
    }

    #[test]
    fn projection_rejects_pre_projection_draft() {
        let mut model = SliceModel::parse_yaml(PROJECTED_MODEL).expect("parse");
        model.requirements[0].id = None;
        let err = model
            .to_provenance_index(test_timestamp("2026-05-28T05:45:00Z"), "specify@2.1.0".to_string())
            .expect_err("a draft without projected ids cannot project provenance");
        assert!(matches!(err, Error::Validation { .. }));
    }

    #[test]
    fn rejects_document_missing_required_sections() {
        let err = SliceModel::parse_yaml(
            "version: 1\nslice: x\ntarget: omnia@v1\nrequirements: []\n",
        )
        .expect_err("a document missing the required sections must fail the schema");
        assert!(matches!(err, Error::Validation { .. }));
    }
}
