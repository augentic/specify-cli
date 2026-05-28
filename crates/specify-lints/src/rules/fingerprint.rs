//! structured lint finding fingerprint and canonical JSON helpers.
//!
//! The fingerprint algorithm pins the wire format at `v1` per the rules contract
//! §"Structured review finding schema" §"Fingerprint algorithm". The
//! algorithm, exclusion table, and inner / outer SHA-256 framing are
//! normative — any drift in canonicalization breaks dedup across CI
//! history. Touch this module only with a deliberate `v2` bump.
//!
//! # Algorithm
//!
//! ```text
//! fingerprint = "sha256:" + hex(sha256(
//!     "v1\n"
//!   + rule-id-or-empty + "\n"
//!   + canonical(location) + "\n"
//!   + hex(sha256(evidence-payload))
//! ))
//! ```
//!
//! Where:
//!
//! - `rule-id-or-empty` is the literal `rule-id` string when set,
//!   the empty string otherwise.
//! - `canonical(location)` is `"{path}:{line}:{column}"` using the
//!   raw [`FindingLocation::path`] verbatim (no normalisation, no
//!   platform munging) with `line.unwrap_or(0)` and
//!   `column.unwrap_or(0)`. `end-line` and `end-column` are
//!   deliberately excluded from the hash. When the finding's
//!   `location` is `None` this term is the empty string.
//! - `evidence-payload` is the UTF-8 bytes of `evidence.value` for
//!   `kind: snippet`, the UTF-8 bytes of `evidence.summary` for
//!   `kind: digest` (NOT `evidence.sha256` — the digest variant
//!   hashes the human summary, not the underlying blob digest), and
//!   `evidence.summary + "\n" + canonical_json(evidence.data)` for
//!   `kind: structured`.
//!
//! Producer-side fields — `id`, `title`, `severity`, `confidence`,
//!   `status`, `change`, `slice`, `target-adapter`, `source-adapter`,
//!   `related-rule-ids` — are deliberately excluded so that regrading
//!   severity, attaching slice/change context after the fact,
//!   rephrasing a title between scanner runs, or migrating between
//!   producers cannot duplicate findings for the same underlying
//!   issue.
//!
//! # Canonical JSON
//!
//! [`canonical_json`] is a small recursive serialiser that sorts
//! object keys lexicographically by Unicode code point (byte order
//! matches code-point order for valid UTF-8 strings, so plain
//! `&str` comparison suffices), emits no insignificant whitespace,
//! preserves array order, and renders leaves via
//! [`serde_json::to_string`]. **Do not** substitute
//! `serde_json::to_string` on a `serde_json::Value` at call sites:
//! `serde_json::Map` preserves insertion order rather than sorting,
//! so direct calls produce a different (non-canonical) byte stream
//! and therefore a different fingerprint.
//!
//! Numbers, booleans, null, and string-escape rules follow
//! `serde_json` defaults (`\\`, `\"`, `\b`, `\f`, `\n`, `\r`,
//! `\t`, control chars as `\u00XX`; `42`, `42.5`, no trailing zeros
//! or scientific notation).

use serde_json::Value;
use specify_tool::sha256_hex;

use crate::rules::{FindingEvidence, FindingLocation, LintFinding};

/// Wire-format version embedded into every fingerprint preimage.
const FINGERPRINT_VERSION: &str = "v1";

/// Compute the structured lint finding fingerprint for `finding`.
///
/// Returns `sha256:` followed by 64 lowercase hex chars. The
/// `finding.fingerprint` field is **not** consulted — callers
/// typically assign the returned value to that field before
/// serialisation, or pass it through [`verify_fingerprint`].
#[must_use]
pub fn fingerprint(finding: &LintFinding) -> String {
    let rule_id = finding.rule_id.as_deref().unwrap_or("");
    let location = canonical_location(finding.location.as_ref());
    let evidence_hex = sha256_hex(evidence_payload(&finding.evidence).as_bytes());

    let mut input = String::with_capacity(
        FINGERPRINT_VERSION
            .len()
            .saturating_add(rule_id.len())
            .saturating_add(location.len())
            .saturating_add(evidence_hex.len())
            .saturating_add(3),
    );
    input.push_str(FINGERPRINT_VERSION);
    input.push('\n');
    input.push_str(rule_id);
    input.push('\n');
    input.push_str(&location);
    input.push('\n');
    input.push_str(&evidence_hex);

    format!("sha256:{}", sha256_hex(input.as_bytes()))
}

/// Recompute the fingerprint from `finding`'s other fields and
/// compare against the stored [`LintFinding::fingerprint`].
///
/// Returns `true` only when:
///
/// 1. `finding.fingerprint` begins with the `sha256:` prefix;
/// 2. the suffix is exactly 64 ASCII hex characters; and
/// 3. the recomputed fingerprint matches `finding.fingerprint` byte
///    for byte (the recomputed value is always lowercase hex, so
///    uppercase stored hex naturally fails this comparison).
#[must_use]
pub fn verify_fingerprint(finding: &LintFinding) -> bool {
    let Some(hex) = finding.fingerprint.strip_prefix("sha256:") else {
        return false;
    };
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return false;
    }
    fingerprint(finding) == finding.fingerprint
}

/// Canonical JSON serialisation: sorted object keys, no
/// insignificant whitespace, arrays preserve insertion order.
///
/// See the module-level docs for the full canonicalisation rules
/// and the rationale for hand-rolling the serialiser instead of
/// reusing [`serde_json::to_string`].
#[must_use]
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(&mut out, value);
    out
}

fn write_canonical(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => {
            out.push_str(&serde_json::to_string(s).expect("serde_json serialises &str"));
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).expect("serde_json serialises key"));
                out.push(':');
                write_canonical(out, &map[*key]);
            }
            out.push('}');
        }
    }
}

fn canonical_location(location: Option<&FindingLocation>) -> String {
    location.map_or_else(String::new, |loc| {
        format!(
            "{path}:{line}:{column}",
            path = loc.path,
            line = loc.line.unwrap_or(0),
            column = loc.column.unwrap_or(0),
        )
    })
}

fn evidence_payload(evidence: &FindingEvidence) -> String {
    match evidence {
        FindingEvidence::Snippet { value } => value.clone(),
        FindingEvidence::Digest { summary, .. } => summary.clone(),
        FindingEvidence::Structured { summary, data, .. } => {
            let canonical = canonical_json(data);
            let mut payload = String::with_capacity(summary.len() + 1 + canonical.len());
            payload.push_str(summary);
            payload.push('\n');
            payload.push_str(&canonical);
            payload
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{canonical_json, fingerprint, verify_fingerprint};
    use crate::rules::{
        Artifact, Confidence, FindingEvidence, FindingLocation, FindingSource, FindingStatus,
        LintFinding, Severity,
    };

    /// Minimal valid finding used as a shared template for the
    /// fingerprint mutation tests below. Callers mutate the returned
    /// value and recompute the fingerprint to assert which
    /// dimensions enter / do not enter the hash.
    fn sample_finding() -> LintFinding {
        LintFinding {
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
        }
    }

    /// (1) Identical inputs produce identical fingerprints across
    /// two independent invocations.
    #[test]
    fn fingerprint_is_deterministic() {
        let finding = sample_finding();
        let a = fingerprint(&finding);
        let b = fingerprint(&finding);
        assert_eq!(a, b);
        assert!(a.starts_with("sha256:"));
        assert_eq!(a.len(), "sha256:".len() + 64);
    }

    /// (2) Mutating any producer-only field
    /// (`id`, `title`, `severity`, `confidence`, `status`, `change`,
    /// `slice`, `target-adapter`, `source-adapter`) MUST leave the
    /// fingerprint unchanged.
    #[test]
    fn excluded_producer_fields_do_not_change_fingerprint() {
        type Mutation = fn(&mut LintFinding);
        let baseline = fingerprint(&sample_finding());

        let cases: &[Mutation] = &[
            |f| f.id = "FIND-9999".into(),
            |f| f.title = "totally different title".into(),
            |f| f.severity = Severity::Critical,
            |f| f.confidence = Some(Confidence::Low),
            |f| f.status = Some(FindingStatus::Accepted),
            |f| f.change = Some("other-change".into()),
            |f| f.slice = Some("other-slice".into()),
            |f| f.target_adapter = Some("vectis".into()),
            |f| f.source_adapter = Some("captures".into()),
            |f| f.related_rule_ids = Some(vec!["SEC-001".into()]),
        ];

        for mutate in cases {
            let mut finding = sample_finding();
            mutate(&mut finding);
            assert_eq!(
                fingerprint(&finding),
                baseline,
                "mutating an excluded field must not change the fingerprint"
            );
        }
    }

    /// (3) Changing `rule-id` MUST change the fingerprint.
    #[test]
    fn rule_id_change_changes_fingerprint() {
        let baseline = fingerprint(&sample_finding());

        let mut other = sample_finding();
        other.rule_id = Some("SEC-001".into());
        assert_ne!(fingerprint(&other), baseline);

        let mut absent = sample_finding();
        absent.rule_id = None;
        assert_ne!(fingerprint(&absent), baseline);
    }

    /// (4a) Changing `location.path` MUST change the fingerprint.
    #[test]
    fn location_path_change_changes_fingerprint() {
        let baseline = fingerprint(&sample_finding());
        let mut mutated = sample_finding();
        mutated.location.as_mut().expect("location present").path = "src/other.rs".into();
        assert_ne!(fingerprint(&mutated), baseline);
    }

    /// (4b) Changing `location.line` MUST change the fingerprint.
    #[test]
    fn location_line_change_changes_fingerprint() {
        let baseline = fingerprint(&sample_finding());
        let mut mutated = sample_finding();
        mutated.location.as_mut().expect("location present").line = Some(42);
        assert_ne!(fingerprint(&mutated), baseline);
    }

    /// (4c) Changing `location.column` MUST change the fingerprint.
    #[test]
    fn location_column_change_changes_fingerprint() {
        let baseline = fingerprint(&sample_finding());
        let mut mutated = sample_finding();
        mutated.location.as_mut().expect("location present").column = Some(42);
        assert_ne!(fingerprint(&mutated), baseline);
    }

    /// Per the fingerprint contract, `end-line` and `end-column` are NOT part of the
    /// canonical location. Mutating either MUST NOT change the
    /// fingerprint.
    #[test]
    fn location_end_fields_do_not_change_fingerprint() {
        let baseline = fingerprint(&sample_finding());
        let mut mutated = sample_finding();
        let loc = mutated.location.as_mut().expect("location present");
        loc.end_line = Some(99);
        loc.end_column = Some(99);
        assert_eq!(fingerprint(&mutated), baseline);
    }

    /// (5) Changing `evidence.value` (snippet variant) MUST change
    /// the fingerprint.
    #[test]
    fn snippet_value_change_changes_fingerprint() {
        let baseline = fingerprint(&sample_finding());
        let mut mutated = sample_finding();
        mutated.evidence = FindingEvidence::Snippet {
            value: "different snippet".into(),
        };
        assert_ne!(fingerprint(&mutated), baseline);
    }

    /// (6) For the `digest` variant: changing `evidence.summary`
    /// MUST change the fingerprint; changing `evidence.sha256` MUST
    /// NOT (the fingerprint hashes `summary`, not `sha256`).
    #[test]
    fn digest_summary_and_sha256_have_distinct_effects() {
        let mut original = sample_finding();
        original.evidence = FindingEvidence::Digest {
            sha256: "a".repeat(64),
            summary: "original summary".into(),
            locations: None,
        };
        let baseline = fingerprint(&original);

        let mut summary_changed = original.clone();
        summary_changed.evidence = FindingEvidence::Digest {
            sha256: "a".repeat(64),
            summary: "different summary".into(),
            locations: None,
        };
        assert_ne!(fingerprint(&summary_changed), baseline);

        let mut sha_changed = original.clone();
        sha_changed.evidence = FindingEvidence::Digest {
            sha256: "b".repeat(64),
            summary: "original summary".into(),
            locations: None,
        };
        assert_eq!(
            fingerprint(&sha_changed),
            baseline,
            "evidence.sha256 must be excluded from the digest-variant payload"
        );

        let mut locations_changed = original;
        locations_changed.evidence = FindingEvidence::Digest {
            sha256: "a".repeat(64),
            summary: "original summary".into(),
            locations: Some(vec![FindingLocation {
                path: "irrelevant.rs".into(),
                line: Some(1),
                column: None,
                end_line: None,
                end_column: None,
            }]),
        };
        assert_eq!(
            fingerprint(&locations_changed),
            baseline,
            "evidence.locations must be excluded from the digest-variant payload"
        );
    }

    /// (7) For the `structured` variant: changing either
    /// `evidence.summary` or `evidence.data` MUST change the
    /// fingerprint.
    #[test]
    fn structured_summary_and_data_change_fingerprint() {
        let mut original = sample_finding();
        original.evidence = FindingEvidence::Structured {
            summary: "contract compat".into(),
            data: json!({"breaking": true, "removed": ["GET /v1/foo"]}),
            locations: None,
        };
        let baseline = fingerprint(&original);

        let mut summary_changed = original.clone();
        summary_changed.evidence = FindingEvidence::Structured {
            summary: "different summary".into(),
            data: json!({"breaking": true, "removed": ["GET /v1/foo"]}),
            locations: None,
        };
        assert_ne!(fingerprint(&summary_changed), baseline);

        let mut data_changed = original.clone();
        data_changed.evidence = FindingEvidence::Structured {
            summary: "contract compat".into(),
            data: json!({"breaking": false, "removed": []}),
            locations: None,
        };
        assert_ne!(fingerprint(&data_changed), baseline);

        let mut reordered = original;
        reordered.evidence = FindingEvidence::Structured {
            summary: "contract compat".into(),
            data: json!({"removed": ["GET /v1/foo"], "breaking": true}),
            locations: None,
        };
        assert_eq!(
            fingerprint(&reordered),
            baseline,
            "canonical_json must sort object keys before hashing"
        );
    }

    /// (8) `canonical_json` correctness: sorted object keys, arrays
    /// preserve order, recursive sort, JSON string escapes.
    #[test]
    fn canonical_json_correctness() {
        assert_eq!(canonical_json(&json!({"b": 2, "a": 1})), r#"{"a":1,"b":2}"#);
        assert_eq!(canonical_json(&json!([3, 2, 1])), "[3,2,1]");
        assert_eq!(
            canonical_json(&json!({"z": {"y": 1, "x": 2}, "a": [3, 2, 1]})),
            r#"{"a":[3,2,1],"z":{"x":2,"y":1}}"#
        );
        assert_eq!(canonical_json(&json!(null)), "null");
        assert_eq!(canonical_json(&json!(true)), "true");
        assert_eq!(canonical_json(&json!(false)), "false");
        assert_eq!(canonical_json(&json!(42)), "42");
        assert_eq!(
            canonical_json(&json!("hello\nworld\t\"quoted\"")),
            r#""hello\nworld\t\"quoted\"""#
        );
        assert_eq!(canonical_json(&json!({})), "{}");
        assert_eq!(canonical_json(&json!([])), "[]");
    }

    /// (9) Findings with absent `rule-id` and absent `location`
    /// still produce a well-formed `sha256:<64 hex>` fingerprint.
    #[test]
    fn empty_rule_id_and_absent_location_produce_well_formed_fingerprint() {
        let mut finding = sample_finding();
        finding.rule_id = None;
        finding.location = None;
        let fp = fingerprint(&finding);
        assert!(fp.starts_with("sha256:"));
        let hex = fp.strip_prefix("sha256:").expect("prefix already checked");
        assert_eq!(hex.len(), 64);
        assert!(hex.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    }

    /// `verify_fingerprint` accepts a stored fingerprint that
    /// matches the recomputed one and rejects every malformed or
    /// stale variant.
    #[test]
    fn verify_fingerprint_round_trip_and_rejection() {
        let mut finding = sample_finding();
        finding.fingerprint = fingerprint(&finding);
        assert!(verify_fingerprint(&finding));

        let mut tampered = finding.clone();
        tampered.rule_id = Some("SEC-001".into());
        assert!(
            !verify_fingerprint(&tampered),
            "changing a covered field must invalidate the stored fingerprint"
        );

        let mut missing_prefix = finding.clone();
        missing_prefix.fingerprint = finding.fingerprint.trim_start_matches("sha256:").to_owned();
        assert!(!verify_fingerprint(&missing_prefix));

        let mut wrong_length = finding.clone();
        wrong_length.fingerprint = "sha256:deadbeef".into();
        assert!(!verify_fingerprint(&wrong_length));

        let mut non_hex = finding.clone();
        non_hex.fingerprint = format!("sha256:{}", "z".repeat(64));
        assert!(!verify_fingerprint(&non_hex));

        let mut uppercase = finding.clone();
        let stored_hex =
            finding.fingerprint.strip_prefix("sha256:").expect("baseline fingerprint has prefix");
        uppercase.fingerprint = format!("sha256:{}", stored_hex.to_ascii_uppercase());
        assert!(
            !verify_fingerprint(&uppercase),
            "uppercase hex is non-canonical and must fail verification"
        );
    }

    /// Golden fingerprint canary — pins the exact wire bytes for a
    /// hand-crafted minimal finding. If this assertion goes red the
    /// algorithm has drifted (likely a canonicalisation change) and
    /// the wire format must move to `v2`; do not "fix" the constant
    /// without a deliberate version bump.
    ///
    /// Fixture:
    /// - `rule-id = "UNI-014"`
    /// - `location = { path: "src/lib.rs", line: 18, column: 5 }`
    /// - `evidence = { kind: snippet, value: "let x = 1;" }`
    #[test]
    fn golden_fingerprint_pins_algorithm() {
        let finding = LintFinding {
            id: "FIND-0001".into(),
            rule_id: Some("UNI-014".into()),
            related_rule_ids: None,
            title: "golden".into(),
            severity: Severity::Important,
            source: FindingSource::Deterministic,
            target_adapter: None,
            source_adapter: None,
            slice: None,
            change: None,
            artifact: Artifact::Code,
            location: Some(FindingLocation {
                path: "src/lib.rs".into(),
                line: Some(18),
                column: Some(5),
                end_line: None,
                end_column: None,
            }),
            evidence: FindingEvidence::Snippet {
                value: "let x = 1;".into(),
            },
            impact: "n/a".into(),
            remediation: "n/a".into(),
            confidence: None,
            fingerprint: String::new(),
            status: None,
        };
        assert_eq!(
            fingerprint(&finding),
            "sha256:f3fee654d173694494b18a4b73a5b7d4be0460896457d2f41ad0c7d752beff72",
            "if this fails, recompute and pin the constant only after confirming the algorithm has not drifted"
        );
    }
}
