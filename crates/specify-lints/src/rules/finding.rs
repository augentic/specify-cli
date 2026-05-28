//! CH-16: `LintFinding` validation helpers.
//!
//! Public validation surface consumed by CH-21's `specdev`-to-finding
//! mapper, CH-22's `specdev lint --format json` envelope, target
//! adapter review briefs, and future `specrun lint` producers.
//!
//! Three orthogonal checks per the rules contract §"Structured review finding
//! schema":
//!
//! 1. **JSON Schema validation** — every wire field conforms to
//!    `schemas/lint/finding.schema.json` (kebab-case keys, closed
//!    enums, evidence `oneOf`, fingerprint pattern, etc.).
//! 2. **Evidence cap** — the serialized `evidence` object is bounded
//!    at 16 `KiB` per the structured evidence union §"Size constraints".
//!    The cap covers the full evidence object (`kind` + payload),
//!    not individual fields.
//! 3. **Fingerprint** — the stored `fingerprint` matches the CH-15
//!    recomputation byte-for-byte. The format pre-check
//!    (`^sha256:[0-9a-f]{64}$`) lives both schema-side and here so
//!    callers that bypass `validate_finding_json` still get the
//!    closed shape.
//!
//! The aggregate [`validate`] short-circuits on the first failure;
//! callers that need every failure at once may invoke the four
//! validators individually.

use serde_json::Value as JsonValue;
use specify_schema::{LINT_FINDING_JSON_SCHEMA, compile_schema};

use super::LintFinding;
use super::fingerprint::fingerprint;

/// 16 `KiB` cap on the serialized evidence object per the rules contract
/// §"Evidence union".
const EVIDENCE_MAX_BYTES: usize = 16 * 1024;

/// Closed failure mode for the CH-16 validators.
#[derive(Debug, thiserror::Error)]
pub enum FindingError {
    /// JSON-schema validation failed. The string carries every
    /// JSON-pointer + reason pair joined by `; `, mirroring the
    /// `specify_schema::validate_value` aggregation style.
    #[error("review finding schema validation failed: {0}")]
    Schema(String),
    /// Serialized evidence object exceeds the 16 `KiB` cap.
    #[error("review finding evidence exceeds 16 KiB cap (got {actual} bytes)")]
    EvidenceTooLarge {
        /// Byte length of the UTF-8 serialized evidence object.
        actual: usize,
    },
    /// Stored fingerprint does not match the recomputed value.
    #[error("review finding fingerprint mismatch: expected {expected}, got {actual}")]
    FingerprintMismatch {
        /// Recomputed canonical fingerprint.
        expected: String,
        /// Value stored on the finding.
        actual: String,
    },
    /// Stored fingerprint does not match `^sha256:[0-9a-f]{64}$`.
    #[error("review finding fingerprint malformed: {0}")]
    FingerprintMalformed(String),
    /// Finding could not be serialized to JSON (unreachable for the
    /// derive-`Serialize` typed shape; surfaced so callers can
    /// distinguish a corrupted in-memory value from a real schema
    /// failure).
    #[error("review finding JSON serialization failed: {0}")]
    Serialize(String),
}

/// Run every CH-16 validator and short-circuit on the first failure.
///
/// Order matches the on-disk reading order — schema first (so callers
/// see structural errors before semantic ones), then the evidence cap,
/// then the fingerprint. Callers that need a complete failure list
/// may invoke the four helpers individually.
///
/// # Errors
///
/// Returns the first [`FindingError`] from [`validate_finding`],
/// [`validate_evidence_size`], or [`validate_fingerprint`].
pub fn validate(finding: &LintFinding) -> Result<(), FindingError> {
    validate_finding(finding)?;
    validate_evidence_size(finding)?;
    validate_fingerprint(finding)?;
    Ok(())
}

/// Validate a typed [`LintFinding`] against the embedded
/// `schemas/lint/finding.schema.json`.
///
/// Serializes the finding to a [`serde_json::Value`] and delegates to
/// [`validate_finding_json`].
///
/// # Errors
///
/// - [`FindingError::Serialize`] if the typed finding cannot be
///   serialized (unreachable for the derived `Serialize` impl).
/// - [`FindingError::Schema`] when the wire shape violates the
///   embedded schema.
pub fn validate_finding(finding: &LintFinding) -> Result<(), FindingError> {
    let value =
        serde_json::to_value(finding).map_err(|err| FindingError::Serialize(err.to_string()))?;
    validate_finding_json(&value)
}

/// Validate a raw [`serde_json::Value`] against the embedded
/// `schemas/lint/finding.schema.json`.
///
/// Used by callers that need to validate hand-built JSON (e.g. when
/// asserting the strict evidence `oneOf` rejects shapes that the
/// typed [`super::FindingEvidence`] enum cannot construct).
///
/// # Errors
///
/// Returns [`FindingError::Schema`] with a `; `-joined error list when
/// the instance fails validation.
pub fn validate_finding_json(value: &JsonValue) -> Result<(), FindingError> {
    let validator = compile_schema(LINT_FINDING_JSON_SCHEMA)
        .map_err(|err| FindingError::Schema(err.to_string()))?;
    let errors: Vec<String> =
        validator.iter_errors(value).map(|err| format!("{}: {err}", err.instance_path())).collect();
    if errors.is_empty() { Ok(()) } else { Err(FindingError::Schema(errors.join("; "))) }
}

/// Enforce the 16 `KiB` structured evidence cap.
///
/// The cap applies to the **full serialized `evidence` object** — the
/// `kind` discriminator plus every variant-specific field, encoded as
/// canonical UTF-8 JSON via [`serde_json::to_string`]. Individual
/// fields are not capped separately; producers MAY emit a small
/// `summary` next to a large `data` payload only while the combined
/// JSON stays under 16 `KiB`.
///
/// # Errors
///
/// - [`FindingError::Serialize`] if the evidence cannot be serialized
///   (unreachable for the derived `Serialize` impl).
/// - [`FindingError::EvidenceTooLarge`] when the serialized form
///   exceeds 16 `KiB`.
pub fn validate_evidence_size(finding: &LintFinding) -> Result<(), FindingError> {
    let serialized = serde_json::to_string(&finding.evidence)
        .map_err(|err| FindingError::Serialize(err.to_string()))?;
    let actual = serialized.len();
    if actual > EVIDENCE_MAX_BYTES {
        Err(FindingError::EvidenceTooLarge { actual })
    } else {
        Ok(())
    }
}

/// Verify that `finding.fingerprint` matches the CH-15 recomputation.
///
/// Performs the format pre-check (`^sha256:[0-9a-f]{64}$`) defensively
/// before recomputing, so callers that bypass `validate_finding_json`
/// still get a closed shape on the wire field. Uppercase hex fails
/// the pre-check — CH-15 always emits lowercase and the schema mirrors
/// that constraint.
///
/// # Errors
///
/// - [`FindingError::FingerprintMalformed`] when the prefix or hex
///   shape is wrong.
/// - [`FindingError::FingerprintMismatch`] when the recomputed value
///   differs from the stored one.
pub fn validate_fingerprint(finding: &LintFinding) -> Result<(), FindingError> {
    let Some(hex) = finding.fingerprint.strip_prefix("sha256:") else {
        return Err(FindingError::FingerprintMalformed(finding.fingerprint.clone()));
    };
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        return Err(FindingError::FingerprintMalformed(finding.fingerprint.clone()));
    }
    let expected = fingerprint(finding);
    if expected == finding.fingerprint {
        Ok(())
    } else {
        Err(FindingError::FingerprintMismatch {
            expected,
            actual: finding.fingerprint.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        EVIDENCE_MAX_BYTES, FindingError, validate, validate_evidence_size, validate_finding,
        validate_finding_json, validate_fingerprint,
    };
    use crate::rules::fingerprint::fingerprint;
    use crate::rules::{
        Artifact, Confidence, FindingEvidence, FindingLocation, FindingSource, LintFinding,
        Severity,
    };

    /// Minimal valid finding template; callers stamp the fingerprint
    /// from CH-15 and mutate the rest as needed.
    fn sample_finding() -> LintFinding {
        let mut finding = LintFinding {
            id: "FIND-0001".into(),
            rule_id: Some("UNI-014".into()),
            related_rule_ids: None,
            title: "Literal deployment URL in generated handler".into(),
            severity: Severity::Important,
            source: FindingSource::Hybrid,
            target_adapter: Some("omnia".into()),
            source_adapter: None,
            slice: Some("billing-invoice-export".into()),
            change: None,
            artifact: Artifact::Code,
            location: Some(FindingLocation {
                path: "crates/invoice_export/src/config.rs".into(),
                line: Some(18),
                column: Some(5),
                end_line: None,
                end_column: None,
            }),
            evidence: FindingEvidence::Snippet {
                value: "const BASE_URL: &str = \"https://api.example.com\";".into(),
            },
            impact: "Generated code will point every deployment at the same external endpoint."
                .into(),
            remediation:
                "Read the endpoint from Omnia configuration and add a required config key to the design."
                    .into(),
            confidence: Some(Confidence::High),
            fingerprint: String::new(),
            status: None,
            disposition: None,
        };
        finding.fingerprint = fingerprint(&finding);
        finding
    }

    /// Hand-built JSON matching the FIND-0001 example; mutated by
    /// the negative schema tests below to assert strict `oneOf` and
    /// pattern enforcement.
    fn sample_finding_json() -> serde_json::Value {
        json!({
            "id": "FIND-0001",
            "rule-id": "UNI-014",
            "title": "Literal deployment URL in generated handler",
            "severity": "important",
            "source": "hybrid",
            "target-adapter": "omnia",
            "slice": "billing-invoice-export",
            "artifact": "code",
            "location": {
                "path": "crates/invoice_export/src/config.rs",
                "line": 18
            },
            "evidence": {
                "kind": "snippet",
                "value": "const BASE_URL: &str = \"https://api.example.com\";"
            },
            "impact": "Generated code will point every deployment at the same external endpoint.",
            "remediation": "Read the endpoint from Omnia configuration and add a required config key to the design.",
            "confidence": "high",
            "fingerprint": "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        })
    }

    /// (1) Fully populated, schema-conformant finding with a matching
    /// CH-15 fingerprint passes every CH-16 validator.
    #[test]
    fn validate_accepts_valid_finding() {
        let finding = sample_finding();
        validate(&finding).expect("valid finding must pass every validator");
    }

    /// (2) Missing required field — `title: ""` violates
    /// `minLength: 1` per the embedded schema.
    #[test]
    fn validate_finding_rejects_empty_title() {
        let mut finding = sample_finding();
        finding.title = String::new();
        finding.fingerprint = fingerprint(&finding);
        match validate_finding(&finding) {
            Err(FindingError::Schema(detail)) => {
                assert!(detail.contains("title"), "error must cite title: {detail}");
            }
            other => panic!("expected Schema error, got {other:?}"),
        }
    }

    /// (3) Invalid severity (`"high"` instead of `important`) fails
    /// the closed-enum schema check.
    #[test]
    fn validate_finding_json_rejects_invalid_severity() {
        let mut value = sample_finding_json();
        value["severity"] = json!("high");
        match validate_finding_json(&value) {
            Err(FindingError::Schema(detail)) => {
                assert!(detail.contains("severity"), "error must cite severity: {detail}");
            }
            other => panic!("expected Schema error, got {other:?}"),
        }
    }

    /// (4) Oversize snippet evidence trips the 16 `KiB` cap. Use a
    /// 20 `KiB` payload so the serialized object comfortably exceeds
    /// the ceiling.
    #[test]
    fn validate_evidence_size_rejects_oversize_snippet() {
        let mut finding = sample_finding();
        let big = "a".repeat(20 * 1024);
        finding.evidence = FindingEvidence::Snippet { value: big };
        finding.fingerprint = fingerprint(&finding);
        match validate_evidence_size(&finding) {
            Err(FindingError::EvidenceTooLarge { actual }) => {
                assert!(
                    actual > EVIDENCE_MAX_BYTES,
                    "actual {actual} must exceed cap {EVIDENCE_MAX_BYTES}"
                );
            }
            other => panic!("expected EvidenceTooLarge, got {other:?}"),
        }
    }

    /// (4b) Evidence sitting just under the cap passes — boundary
    /// guard so future drift on the serialized framing surfaces here.
    #[test]
    fn validate_evidence_size_accepts_payload_just_under_cap() {
        let mut finding = sample_finding();
        // Snippet payload framing: {"kind":"snippet","value":"..."}.
        // 28 bytes of framing plus the JSON-escaped value bytes.
        let framing = r#"{"kind":"snippet","value":""}"#.len();
        let payload = "a".repeat(EVIDENCE_MAX_BYTES - framing);
        finding.evidence = FindingEvidence::Snippet { value: payload };
        finding.fingerprint = fingerprint(&finding);
        validate_evidence_size(&finding).expect("payload exactly at cap must pass");
    }

    /// (5) Fingerprint that does not match `^sha256:[0-9a-f]{64}$`
    /// surfaces as `FingerprintMalformed`. The typed validator
    /// rejects pre-`sha256:` strings before the schema sees them
    /// because callers may invoke `validate_fingerprint` directly.
    #[test]
    fn validate_fingerprint_rejects_malformed_value() {
        let mut finding = sample_finding();
        finding.fingerprint = "not-a-sha256".into();
        match validate_fingerprint(&finding) {
            Err(FindingError::FingerprintMalformed(detail)) => {
                assert_eq!(detail, "not-a-sha256");
            }
            other => panic!("expected FingerprintMalformed, got {other:?}"),
        }
    }

    /// (5b) Malformed fingerprint also fails the typed
    /// `validate_finding` via the schema's pattern check.
    #[test]
    fn validate_finding_rejects_malformed_fingerprint_via_schema() {
        let mut finding = sample_finding();
        finding.fingerprint = "not-a-sha256".into();
        match validate_finding(&finding) {
            Err(FindingError::Schema(detail)) => {
                assert!(detail.contains("fingerprint"), "error must cite fingerprint: {detail}");
            }
            other => panic!("expected Schema error, got {other:?}"),
        }
    }

    /// (6) Well-formed fingerprint that does not match the recompute
    /// surfaces as `FingerprintMismatch`.
    #[test]
    fn validate_fingerprint_rejects_mismatch() {
        let finding = sample_finding();
        let mut tampered = finding.clone();
        tampered.fingerprint =
            "sha256:1111111111111111111111111111111111111111111111111111111111111111".into();
        match validate_fingerprint(&tampered) {
            Err(FindingError::FingerprintMismatch { expected, actual }) => {
                assert_eq!(expected, finding.fingerprint);
                assert_eq!(actual, tampered.fingerprint);
            }
            other => panic!("expected FingerprintMismatch, got {other:?}"),
        }
    }

    /// (6b) Uppercase hex is non-canonical per CH-15 and must surface
    /// as `FingerprintMalformed` even though it passes the loose
    /// `is_ascii_hexdigit` check.
    #[test]
    fn validate_fingerprint_rejects_uppercase_hex() {
        let mut finding = sample_finding();
        let stored_hex =
            finding.fingerprint.strip_prefix("sha256:").expect("baseline has prefix").to_owned();
        finding.fingerprint = format!("sha256:{}", stored_hex.to_ascii_uppercase());
        match validate_fingerprint(&finding) {
            Err(FindingError::FingerprintMalformed(_)) => {}
            other => panic!("expected FingerprintMalformed for uppercase hex, got {other:?}"),
        }
    }

    /// (7) Invalid rule-id (`OMNIA-1` instead of `OMNIA-001`) fails
    /// the closed `ruleId` pattern.
    #[test]
    fn validate_finding_json_rejects_invalid_rule_id() {
        let mut value = sample_finding_json();
        value["rule-id"] = json!("OMNIA-1");
        match validate_finding_json(&value) {
            Err(FindingError::Schema(detail)) => {
                assert!(detail.contains("rule-id"), "error must cite rule-id: {detail}");
            }
            other => panic!("expected Schema error, got {other:?}"),
        }
    }

    /// (8a) Strict evidence `oneOf` — snippet with extra `sha256`
    /// field violates `additionalProperties: false` on the snippet
    /// branch.
    #[test]
    fn validate_finding_json_rejects_snippet_with_extra_sha256() {
        let mut value = sample_finding_json();
        value["evidence"] = json!({
            "kind": "snippet",
            "value": "let x = 1;",
            "sha256": "a".repeat(64),
        });
        match validate_finding_json(&value) {
            Err(FindingError::Schema(detail)) => {
                assert!(detail.contains("evidence"), "error must cite evidence: {detail}");
            }
            other => panic!("expected Schema error, got {other:?}"),
        }
    }

    /// (8b) Strict evidence `oneOf` — digest variant missing required
    /// `sha256`.
    #[test]
    fn validate_finding_json_rejects_digest_missing_sha256() {
        let mut value = sample_finding_json();
        value["evidence"] = json!({
            "kind": "digest",
            "summary": "binary blob",
        });
        match validate_finding_json(&value) {
            Err(FindingError::Schema(detail)) => {
                assert!(detail.contains("evidence"), "error must cite evidence: {detail}");
            }
            other => panic!("expected Schema error, got {other:?}"),
        }
    }

    /// (8c) Strict evidence `oneOf` — structured variant missing
    /// required `data`.
    #[test]
    fn validate_finding_json_rejects_structured_missing_data() {
        let mut value = sample_finding_json();
        value["evidence"] = json!({
            "kind": "structured",
            "summary": "contract compat",
        });
        match validate_finding_json(&value) {
            Err(FindingError::Schema(detail)) => {
                assert!(detail.contains("evidence"), "error must cite evidence: {detail}");
            }
            other => panic!("expected Schema error, got {other:?}"),
        }
    }

    /// (8d) Strict evidence `oneOf` — structured variant with extra
    /// `value` field (snippet leakage) violates the structured
    /// branch's `additionalProperties: false`.
    #[test]
    fn validate_finding_json_rejects_structured_with_extra_value() {
        let mut value = sample_finding_json();
        value["evidence"] = json!({
            "kind": "structured",
            "summary": "contract compat",
            "data": {"breaking": true},
            "value": "stray",
        });
        match validate_finding_json(&value) {
            Err(FindingError::Schema(detail)) => {
                assert!(detail.contains("evidence"), "error must cite evidence: {detail}");
            }
            other => panic!("expected Schema error, got {other:?}"),
        }
    }

    /// (9a) Well-formed `snippet` evidence passes the full validator.
    #[test]
    fn validate_accepts_snippet_variant() {
        let mut finding = sample_finding();
        finding.evidence = FindingEvidence::Snippet {
            value: "let x = 1;".into(),
        };
        finding.fingerprint = fingerprint(&finding);
        validate(&finding).expect("snippet variant must validate");
    }

    /// (9b) Well-formed `digest` evidence passes the full validator.
    #[test]
    fn validate_accepts_digest_variant() {
        let mut finding = sample_finding();
        finding.evidence = FindingEvidence::Digest {
            sha256: "a".repeat(64),
            summary: "binary blob".into(),
            locations: Some(vec![FindingLocation {
                path: "src/lib.rs".into(),
                line: Some(1),
                column: None,
                end_line: None,
                end_column: None,
            }]),
        };
        finding.fingerprint = fingerprint(&finding);
        validate(&finding).expect("digest variant must validate");
    }

    /// (9c) Well-formed `structured` evidence passes the full
    /// validator.
    #[test]
    fn validate_accepts_structured_variant() {
        let mut finding = sample_finding();
        finding.evidence = FindingEvidence::Structured {
            summary: "contract compat".into(),
            data: json!({"breaking": true, "removed": ["GET /v1/foo"]}),
            locations: None,
        };
        finding.fingerprint = fingerprint(&finding);
        validate(&finding).expect("structured variant must validate");
    }

    /// `validate` short-circuits on the first failure: a schema
    /// violation is reported even when the evidence cap and
    /// fingerprint are independently broken.
    #[test]
    fn validate_short_circuits_on_first_failure() {
        let mut finding = sample_finding();
        finding.title = String::new();
        finding.fingerprint =
            "sha256:1111111111111111111111111111111111111111111111111111111111111111".into();
        match validate(&finding) {
            Err(FindingError::Schema(_)) => {}
            other => panic!("expected Schema error to short-circuit, got {other:?}"),
        }
    }
}
