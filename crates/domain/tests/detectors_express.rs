//! Express detector integration tests.
//!
//! Regenerate goldens:
//! `REGENERATE_GOLDENS=1 cargo nextest run -p specify-domain --test detectors_express`

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use specify_domain::survey::detectors::ExpressDetector;
use specify_domain::survey::{Detector, DetectorInput, SurfacesDocument, validate_surfaces};

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/detectors/express")
}

fn golden_path(name: &str) -> PathBuf {
    fixtures_dir().join(format!("{name}.expected.json"))
}

fn run_detector(source_dir: &str) -> specify_domain::survey::DetectorOutput {
    let source = fixtures_dir().join(source_dir);
    let input = DetectorInput {
        source_root: &source,
        language_hint: None,
    };
    ExpressDetector.detect(&input).unwrap()
}

fn surfaces_doc(output: specify_domain::survey::DetectorOutput) -> SurfacesDocument {
    SurfacesDocument {
        version: 1,
        source_key: "express-fixture".to_string(),
        language: "typescript".to_string(),
        surfaces: output.surfaces,
    }
}

// ── Golden / byte-stable ────────────────────────────────────────────

#[test]
fn detects_express_routes_golden() {
    let doc = surfaces_doc(run_detector("synthetic-app"));
    validate_surfaces(&doc).unwrap();

    let mut serialised = serde_json::to_string_pretty(&doc).unwrap();
    serialised.push('\n');
    let path = golden_path("synthetic-app");

    if std::env::var_os("REGENERATE_GOLDENS").is_some() {
        fs::write(&path, &serialised).unwrap();
        return;
    }

    let expected = fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("missing golden {}: {e}; regen with REGENERATE_GOLDENS=1", path.display())
    });
    assert_eq!(serialised, expected, "golden mismatch — regen with REGENERATE_GOLDENS=1");
}

#[test]
fn express_byte_stable() {
    let a = surfaces_doc(run_detector("synthetic-app"));
    let b = surfaces_doc(run_detector("synthetic-app"));
    let ja = serde_json::to_string_pretty(&a).unwrap();
    let jb = serde_json::to_string_pretty(&b).unwrap();
    assert_eq!(ja, jb, "output not byte-stable across two runs");
}

// ── Structural assertions ───────────────────────────────────────────

#[test]
fn no_duplicate_ids() {
    let output = run_detector("synthetic-app");
    let mut ids = HashSet::new();
    for s in &output.surfaces {
        assert!(ids.insert(&s.id), "duplicate id: {}", s.id);
    }
}

#[test]
fn validates_against_schema() {
    let doc = surfaces_doc(run_detector("synthetic-app"));
    validate_surfaces(&doc).unwrap();
}

// ── Applicability gating ────────────────────────────────────────────

#[test]
fn skips_when_no_package_json() {
    let dir = tempfile::TempDir::new().unwrap();
    let input = DetectorInput {
        source_root: dir.path(),
        language_hint: None,
    };
    let output = ExpressDetector.detect(&input).unwrap();
    assert!(output.surfaces.is_empty());
}

#[test]
fn skips_when_express_not_dependency() {
    let dir = tempfile::TempDir::new().unwrap();
    fs::write(dir.path().join("package.json"), r#"{"dependencies":{"fastify":"^4.0.0"}}"#).unwrap();
    let input = DetectorInput {
        source_root: dir.path(),
        language_hint: None,
    };
    let output = ExpressDetector.detect(&input).unwrap();
    assert!(output.surfaces.is_empty());
}
