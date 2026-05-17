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
