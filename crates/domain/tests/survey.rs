//! Integration tests for the survey DTOs and validators.
//!
//! Covers JSON + saphyr-YAML round-trips, byte-stable golden output,
//! and each validation discriminant.
//!
//! Regenerate the golden with
//! `REGENERATE_GOLDENS=1 cargo nextest run -p specify-domain --test survey`.

use std::fs;
use std::path::PathBuf;

use specify_domain::survey::{
    MetadataDocument, Surface, SurfaceKind, SurfacesDocument, validate_metadata, validate_surfaces,
};

fn fixtures_dir() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.join("tests/fixtures/survey")
}

fn golden_path(name: &str) -> PathBuf {
    fixtures_dir().join(format!("{name}.golden.json"))
}

fn canonical_surfaces_doc() -> SurfacesDocument {
    SurfacesDocument {
        version: 1,
        source_key: "legacy-monolith".to_string(),
        language: "typescript".to_string(),
        surfaces: vec![
            Surface {
                id: "http-get-users".to_string(),
                kind: SurfaceKind::HttpRoute,
                identifier: "GET /users".to_string(),
                handler: "src/routes/users.ts:listUsers".to_string(),
                touches: vec![
                    "src/routes/users.ts".to_string(),
                    "src/users/repository.ts".to_string(),
                ],
                declared_at: vec!["src/server.ts:10".to_string()],
            },
            Surface {
                id: "http-post-users".to_string(),
                kind: SurfaceKind::HttpRoute,
                identifier: "POST /users".to_string(),
                handler: "src/auth/register.ts:registerUser".to_string(),
                touches: vec![
                    "src/auth/register.ts".to_string(),
                    "src/notifications/email.ts".to_string(),
                    "src/users/repository.ts".to_string(),
                ],
                declared_at: vec!["src/server.ts:42".to_string()],
            },
            Surface {
                id: "message-pub-user-created".to_string(),
                kind: SurfaceKind::MessagePub,
                identifier: "user.created".to_string(),
                handler: "src/users/events.ts:publishUserCreated".to_string(),
                touches: vec!["src/users/events.ts".to_string()],
                declared_at: vec!["src/users/events.ts:18".to_string()],
            },
            Surface {
                id: "scheduled-job-cleanup".to_string(),
                kind: SurfaceKind::ScheduledJob,
                identifier: "cleanup-expired-sessions".to_string(),
                handler: "src/jobs/cleanup.ts:run".to_string(),
                touches: vec![
                    "src/jobs/cleanup.ts".to_string(),
                    "src/sessions/repository.ts".to_string(),
                ],
                declared_at: vec!["src/jobs/scheduler.ts:5".to_string()],
            },
        ],
    }
}

fn canonical_metadata_doc() -> MetadataDocument {
    MetadataDocument {
        version: 1,
        source_key: "legacy-monolith".to_string(),
        language: "typescript".to_string(),
        loc: 42_000,
        module_count: 15,
        top_level_modules: vec![
            "auth".to_string(),
            "billing".to_string(),
            "jobs".to_string(),
            "notifications".to_string(),
            "sessions".to_string(),
            "users".to_string(),
        ],
    }
}

// ── JSON round-trip ─────────────────────────────────────────────────

#[test]
fn surfaces_json_round_trip() {
    let doc = canonical_surfaces_doc();
    let json = serde_json::to_string_pretty(&doc).unwrap();
    let parsed: SurfacesDocument = serde_json::from_str(&json).unwrap();
    assert_eq!(doc, parsed);
}

#[test]
fn metadata_json_round_trip() {
    let doc = canonical_metadata_doc();
    let json = serde_json::to_string_pretty(&doc).unwrap();
    let parsed: MetadataDocument = serde_json::from_str(&json).unwrap();
    assert_eq!(doc, parsed);
}

// ── saphyr-YAML round-trip ──────────────────────────────────────────

#[test]
fn surfaces_yaml_round_trip() {
    let doc = canonical_surfaces_doc();
    let yaml = serde_saphyr::to_string(&doc).unwrap();
    let parsed: SurfacesDocument = serde_saphyr::from_str(&yaml).unwrap();
    assert_eq!(doc, parsed);
}

#[test]
fn metadata_yaml_round_trip() {
    let doc = canonical_metadata_doc();
    let yaml = serde_saphyr::to_string(&doc).unwrap();
    let parsed: MetadataDocument = serde_saphyr::from_str(&yaml).unwrap();
    assert_eq!(doc, parsed);
}

// ── Fixture file round-trip ─────────────────────────────────────────

#[test]
fn surfaces_fixture_deserialises() {
    let raw = fs::read_to_string(fixtures_dir().join("surfaces-valid.json")).unwrap();
    let doc: SurfacesDocument = serde_json::from_str(&raw).unwrap();
    assert_eq!(doc, canonical_surfaces_doc());
}

#[test]
fn metadata_fixture_deserialises() {
    let raw = fs::read_to_string(fixtures_dir().join("metadata-valid.json")).unwrap();
    let doc: MetadataDocument = serde_json::from_str(&raw).unwrap();
    assert_eq!(doc, canonical_metadata_doc());
}

// ── Byte-stable golden ─────────────────────────────────────────────

#[test]
fn surfaces_golden_stable() {
    let doc = canonical_surfaces_doc();
    let mut serialised = serde_json::to_string_pretty(&doc).unwrap();
    serialised.push('\n');
    let path = golden_path("surfaces");

    if std::env::var_os("REGENERATE_GOLDENS").is_some() {
        fs::write(&path, &serialised).unwrap();
        return;
    }

    let expected = fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!("missing golden {}: {err}; regenerate with REGENERATE_GOLDENS=1", path.display())
    });
    assert_eq!(serialised, expected, "golden mismatch — regenerate with REGENERATE_GOLDENS=1");

    let mut second_run = serde_json::to_string_pretty(&doc).unwrap();
    second_run.push('\n');
    assert_eq!(serialised, second_run, "output is not byte-stable across two runs");
}

#[test]
fn metadata_golden_stable() {
    let doc = canonical_metadata_doc();
    let mut serialised = serde_json::to_string_pretty(&doc).unwrap();
    serialised.push('\n');
    let path = golden_path("metadata");

    if std::env::var_os("REGENERATE_GOLDENS").is_some() {
        fs::write(&path, &serialised).unwrap();
        return;
    }

    let expected = fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!("missing golden {}: {err}; regenerate with REGENERATE_GOLDENS=1", path.display())
    });
    assert_eq!(serialised, expected, "golden mismatch — regenerate with REGENERATE_GOLDENS=1");

    let mut second_run = serde_json::to_string_pretty(&doc).unwrap();
    second_run.push('\n');
    assert_eq!(serialised, second_run, "output is not byte-stable across two runs");
}

// ── Validation: valid docs pass ─────────────────────────────────────

#[test]
fn valid_surfaces_pass() {
    validate_surfaces(&canonical_surfaces_doc()).unwrap();
}

#[test]
fn valid_metadata_pass() {
    validate_metadata(&canonical_metadata_doc()).unwrap();
}

// ── Validation: version ─────────────────────────────────────────────

#[test]
fn surfaces_version_unsupported() {
    let mut doc = canonical_surfaces_doc();
    doc.version = 2;
    let err = validate_surfaces(&doc).unwrap_err();
    assert_has_finding(&err, "surfaces-version-unsupported");
}

#[test]
fn metadata_version_unsupported() {
    let mut doc = canonical_metadata_doc();
    doc.version = 0;
    let err = validate_metadata(&doc).unwrap_err();
    assert_has_finding(&err, "metadata-version-unsupported");
}

// ── Validation: sort order ──────────────────────────────────────────

#[test]
fn surfaces_out_of_order() {
    let mut doc = canonical_surfaces_doc();
    doc.surfaces.reverse();
    let err = validate_surfaces(&doc).unwrap_err();
    assert_has_finding(&err, "surfaces-out-of-order");
}

#[test]
fn surfaces_touches_out_of_order() {
    let mut doc = canonical_surfaces_doc();
    doc.surfaces[0].touches = vec!["src/z.ts".to_string(), "src/a.ts".to_string()];
    let err = validate_surfaces(&doc).unwrap_err();
    assert_has_finding(&err, "surfaces-touches-out-of-order");
}

#[test]
fn surfaces_declared_at_out_of_order() {
    let mut doc = canonical_surfaces_doc();
    doc.surfaces[0].declared_at = vec!["src/z.ts:99".to_string(), "src/a.ts:1".to_string()];
    let err = validate_surfaces(&doc).unwrap_err();
    assert_has_finding(&err, "surfaces-declared-at-out-of-order");
}

// ── Validation: declared-at non-empty ───────────────────────────────

#[test]
fn surfaces_declared_at_empty() {
    let mut doc = canonical_surfaces_doc();
    doc.surfaces[0].declared_at.clear();
    let err = validate_surfaces(&doc).unwrap_err();
    assert_has_finding(&err, "surfaces-declared-at-empty");
}

// ── Validation: paths under source root ─────────────────────────────

#[test]
fn surfaces_absolute_path_in_touches_out_of_tree() {
    let mut doc = canonical_surfaces_doc();
    doc.surfaces[0].touches = vec!["/absolute/path.ts".to_string()];
    let err = validate_surfaces(&doc).unwrap_err();
    assert_has_finding(&err, "surfaces-touches-out-of-tree");
}

#[test]
fn surfaces_absolute_path_in_declared_at_out_of_tree() {
    let mut doc = canonical_surfaces_doc();
    doc.surfaces[0].declared_at = vec!["C:\\Windows\\path.ts:1".to_string()];
    let err = validate_surfaces(&doc).unwrap_err();
    assert_has_finding(&err, "surfaces-touches-out-of-tree");
}

#[test]
fn surfaces_parent_segment_in_touches_out_of_tree() {
    let mut doc = canonical_surfaces_doc();
    doc.surfaces[0].touches = vec!["src/../escaped/path.ts".to_string()];
    let err = validate_surfaces(&doc).unwrap_err();
    assert_has_finding(&err, "surfaces-touches-out-of-tree");
}

#[test]
fn surfaces_parent_segment_in_declared_at_out_of_tree() {
    let mut doc = canonical_surfaces_doc();
    doc.surfaces[0].declared_at = vec!["../escaped/path.ts:1".to_string()];
    let err = validate_surfaces(&doc).unwrap_err();
    assert_has_finding(&err, "surfaces-touches-out-of-tree");
}

// ── Validation: duplicate ids ───────────────────────────────────────

#[test]
fn surface_id_duplicate() {
    let mut doc = canonical_surfaces_doc();
    let dupe = doc.surfaces[0].clone();
    doc.surfaces.push(dupe);
    doc.surfaces.sort_by(|a, b| a.id.cmp(&b.id));
    let err = validate_surfaces(&doc).unwrap_err();
    assert_has_finding(&err, "surface-id-duplicate");
}

// ── Validation: all surface kinds round-trip ────────────────────────

#[test]
fn all_surface_kinds_json_round_trip() {
    let kinds = [
        (SurfaceKind::HttpRoute, "http-route"),
        (SurfaceKind::MessagePub, "message-pub"),
        (SurfaceKind::MessageSub, "message-sub"),
        (SurfaceKind::WsHandler, "ws-handler"),
        (SurfaceKind::ScheduledJob, "scheduled-job"),
        (SurfaceKind::CliCommand, "cli-command"),
        (SurfaceKind::UiRoute, "ui-route"),
        (SurfaceKind::ExternalCallOut, "external-call-out"),
    ];
    for (kind, expected_str) in kinds {
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, format!("\"{expected_str}\""));
        let parsed: SurfaceKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kind);
    }
}

// ── Schema validation ───────────────────────────────────────────────

#[test]
fn surfaces_schema_accepts_valid() {
    let schema_src = include_str!("../../../schemas/surfaces.schema.json");
    let schema: serde_json::Value = serde_json::from_str(schema_src).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();

    let fixture = fs::read_to_string(fixtures_dir().join("surfaces-valid.json")).unwrap();
    let instance: serde_json::Value = serde_json::from_str(&fixture).unwrap();
    let result = validator.validate(&instance);
    assert!(result.is_ok(), "schema rejected valid fixture: {result:?}");
}

#[test]
fn metadata_schema_accepts_valid() {
    let schema_src = include_str!("../../../schemas/survey-metadata.schema.json");
    let schema: serde_json::Value = serde_json::from_str(schema_src).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();

    let fixture = fs::read_to_string(fixtures_dir().join("metadata-valid.json")).unwrap();
    let instance: serde_json::Value = serde_json::from_str(&fixture).unwrap();
    let result = validator.validate(&instance);
    assert!(result.is_ok(), "schema rejected valid fixture: {result:?}");
}

#[test]
fn surfaces_schema_rejects_bad_kind() {
    let schema_src = include_str!("../../../schemas/surfaces.schema.json");
    let schema: serde_json::Value = serde_json::from_str(schema_src).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();

    let mut instance: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(fixtures_dir().join("surfaces-valid.json")).unwrap(),
    )
    .unwrap();
    instance["surfaces"][0]["kind"] = serde_json::Value::String("unknown-kind".to_string());
    assert!(validator.validate(&instance).is_err());
}

#[test]
fn surfaces_schema_rejects_bad_version() {
    let schema_src = include_str!("../../../schemas/surfaces.schema.json");
    let schema: serde_json::Value = serde_json::from_str(schema_src).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();

    let mut instance: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(fixtures_dir().join("surfaces-valid.json")).unwrap(),
    )
    .unwrap();
    instance["version"] = serde_json::json!(2);
    assert!(validator.validate(&instance).is_err());
}

#[test]
fn surfaces_schema_rejects_extra_fields() {
    let schema_src = include_str!("../../../schemas/surfaces.schema.json");
    let schema: serde_json::Value = serde_json::from_str(schema_src).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();

    let mut instance: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(fixtures_dir().join("surfaces-valid.json")).unwrap(),
    )
    .unwrap();
    instance["timestamp"] = serde_json::Value::String("2026-05-16T00:00:00Z".to_string());
    assert!(validator.validate(&instance).is_err());
}

#[test]
fn metadata_schema_rejects_bad_version() {
    let schema_src = include_str!("../../../schemas/survey-metadata.schema.json");
    let schema: serde_json::Value = serde_json::from_str(schema_src).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();

    let mut instance: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(fixtures_dir().join("metadata-valid.json")).unwrap(),
    )
    .unwrap();
    instance["version"] = serde_json::json!(99);
    assert!(validator.validate(&instance).is_err());
}

// ── Detector contract ───────────────────────────────────────────────

use specify_domain::survey::{
    Detector, DetectorError, DetectorInput, DetectorOutput, DetectorRegistry,
    merge_detector_outputs,
};

struct MockDetector {
    detector_name: &'static str,
    result: fn() -> Result<DetectorOutput, DetectorError>,
}

impl Detector for MockDetector {
    fn name(&self) -> &'static str {
        self.detector_name
    }

    fn detect(&self, _input: &DetectorInput<'_>) -> Result<DetectorOutput, DetectorError> {
        (self.result)()
    }
}

fn test_surface(id: &str, kind: SurfaceKind) -> Surface {
    Surface {
        id: id.to_string(),
        kind,
        identifier: format!("test-{id}"),
        handler: format!("src/{id}.ts:handler"),
        touches: vec![format!("src/{id}.ts")],
        declared_at: vec![format!("src/app.ts:1")],
    }
}

// ── DetectorRegistry ────────────────────────────────────────────────

#[test]
fn registry_with_builtins_is_empty() {
    let registry = DetectorRegistry::with_builtins();
    assert_eq!(
        registry.iter().count(),
        0,
        "registry is empty in v1; reserved for deferred extension points"
    );
}

// ── Merge: empty registry produces empty surfaces ───────────────────

#[test]
fn merge_empty_produces_empty() {
    let result = merge_detector_outputs(std::iter::empty()).unwrap();
    assert!(result.is_empty());
}

// ── Merge: synthetic detector round-trips through validate ──────────

#[test]
fn merge_two_surfaces_validates() {
    let outputs = vec![(
        "mock-framework",
        Ok(DetectorOutput {
            surfaces: vec![
                test_surface("http-get-users", SurfaceKind::HttpRoute),
                test_surface("message-pub-created", SurfaceKind::MessagePub),
            ],
        }),
    )];

    let merged = merge_detector_outputs(outputs).unwrap();
    assert_eq!(merged.len(), 2);
    assert!(merged[0].id < merged[1].id, "surfaces must be sorted by id");

    let doc = SurfacesDocument {
        version: 1,
        source_key: "test-source".to_string(),
        language: "typescript".to_string(),
        surfaces: merged,
    };
    validate_surfaces(&doc).unwrap();
}

// ── Merge: multiple detectors with distinct ids ─────────────────────

#[test]
fn merge_multiple_detectors_distinct_ids() {
    let outputs = vec![
        (
            "detector-a",
            Ok(DetectorOutput {
                surfaces: vec![test_surface("alpha", SurfaceKind::HttpRoute)],
            }),
        ),
        (
            "detector-b",
            Ok(DetectorOutput {
                surfaces: vec![test_surface("beta", SurfaceKind::MessageSub)],
            }),
        ),
    ];

    let merged = merge_detector_outputs(outputs).unwrap();
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[0].id, "alpha");
    assert_eq!(merged[1].id, "beta");
}

// ── Merge: detector-id-collision ────────────────────────────────────

#[test]
fn merge_detector_id_collision() {
    let outputs = vec![
        (
            "express",
            Ok(DetectorOutput {
                surfaces: vec![test_surface("http-get-users", SurfaceKind::HttpRoute)],
            }),
        ),
        (
            "nestjs",
            Ok(DetectorOutput {
                surfaces: vec![test_surface("http-get-users", SurfaceKind::HttpRoute)],
            }),
        ),
    ];

    let err = merge_detector_outputs(outputs).unwrap_err();
    assert_eq!(err.variant_str(), "detector-id-collision");
    let detail = err.to_string();
    assert!(detail.contains("http-get-users"), "payload must include the colliding id");
    assert!(detail.contains("express"), "payload must include first detector name");
    assert!(detail.contains("nestjs"), "payload must include second detector name");
}

// ── Merge: same detector emitting duplicate ids (not a collision) ───

#[test]
fn merge_same_detector_duplicate_ids_not_cross_detector_collision() {
    let outputs = vec![(
        "express",
        Ok(DetectorOutput {
            surfaces: vec![
                test_surface("http-get-users", SurfaceKind::HttpRoute),
                test_surface("http-get-users", SurfaceKind::HttpRoute),
            ],
        }),
    )];

    // Same detector emitting duplicate ids is not a cross-detector
    // collision; the downstream validate_surfaces catches it as
    // `surface-id-duplicate`.
    let merged = merge_detector_outputs(outputs).unwrap();
    assert_eq!(merged.len(), 2);
}

// ── Merge: detector-failure on Malformed ────────────────────────────

#[test]
fn merge_detector_failure_malformed() {
    let outputs: Vec<(&'static str, Result<DetectorOutput, DetectorError>)> = vec![(
        "broken-detector",
        Err(DetectorError::Malformed {
            reason: "unexpected AST shape".to_string(),
        }),
    )];

    let err = merge_detector_outputs(outputs).unwrap_err();
    assert_eq!(err.variant_str(), "detector-failure");
    let detail = err.to_string();
    assert!(detail.contains("broken-detector"), "payload must include detector name");
    assert!(detail.contains("unexpected AST shape"), "payload must include reason");
}

// ── Merge: detector-failure on Io ───────────────────────────────────

#[test]
fn merge_detector_failure_io() {
    let outputs: Vec<(&'static str, Result<DetectorOutput, DetectorError>)> = vec![(
        "io-detector",
        Err(DetectorError::Io {
            reason: "permission denied".to_string(),
        }),
    )];

    let err = merge_detector_outputs(outputs).unwrap_err();
    assert_eq!(err.variant_str(), "detector-failure");
    let detail = err.to_string();
    assert!(detail.contains("io-detector"));
    assert!(detail.contains("permission denied"));
}

// ── Merge: failure stops before collision check ─────────────────────

#[test]
fn merge_failure_is_immediate() {
    let outputs: Vec<(&'static str, Result<DetectorOutput, DetectorError>)> = vec![
        (
            "failing",
            Err(DetectorError::Malformed {
                reason: "bad".to_string(),
            }),
        ),
        (
            "good",
            Ok(DetectorOutput {
                surfaces: vec![test_surface("alpha", SurfaceKind::HttpRoute)],
            }),
        ),
    ];

    let err = merge_detector_outputs(outputs).unwrap_err();
    assert_eq!(err.variant_str(), "detector-failure");
}

// ── Merge: output is sorted by id ───────────────────────────────────

#[test]
fn merge_output_sorted() {
    let outputs = vec![(
        "sorter",
        Ok(DetectorOutput {
            surfaces: vec![
                test_surface("zulu", SurfaceKind::HttpRoute),
                test_surface("alpha", SurfaceKind::MessagePub),
                test_surface("mike", SurfaceKind::ScheduledJob),
            ],
        }),
    )];

    let merged = merge_detector_outputs(outputs).unwrap();
    let ids: Vec<&str> = merged.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, vec!["alpha", "mike", "zulu"]);
}

// ── Detector trait: MockDetector exercises the contract ──────────────

#[test]
fn mock_detector_exercises_trait() {
    let detector = MockDetector {
        detector_name: "test-mock",
        result: || {
            Ok(DetectorOutput {
                surfaces: vec![test_surface("mock-surface", SurfaceKind::CliCommand)],
            })
        },
    };

    let input = DetectorInput {
        source_root: std::path::Path::new("/tmp/fake"),
        language_hint: None,
    };

    let output = detector.detect(&input).unwrap();
    assert_eq!(output.surfaces.len(), 1);
    assert_eq!(output.surfaces[0].id, "mock-surface");
    assert_eq!(detector.name(), "test-mock");
}

// ── Language enum ───────────────────────────────────────────────────

use specify_domain::survey::Language;

#[test]
fn language_serde_round_trip() {
    let langs = [
        (Language::TypeScript, "\"typescript\""),
        (Language::JavaScript, "\"javascript\""),
        (Language::Rust, "\"rust\""),
        (Language::Python, "\"python\""),
        (Language::Go, "\"go\""),
    ];
    for (lang, expected_json) in langs {
        let json = serde_json::to_string(&lang).unwrap();
        assert_eq!(json, expected_json);
        let parsed: Language = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, lang);
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn assert_has_finding(err: &specify_error::Error, expected_rule_id: &str) {
    let specify_error::Error::Validation { results } = err else {
        panic!("expected Error::Validation, got: {err}");
    };
    assert!(
        results.iter().any(|r| r.rule_id == expected_rule_id),
        "expected finding `{expected_rule_id}` in {results:?}"
    );
}
