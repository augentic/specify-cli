use super::*;
use crate::Platform;

#[test]
fn axis_dir_segment_round_trips() {
    assert_eq!(Axis::Source.dir_segment(), "sources");
    assert_eq!(Axis::Target.dir_segment(), "targets");
}

#[test]
fn cache_dir_routes_by_axis() {
    // The manifest mirror lives out-of-tree under the per-project OS
    // cache; `cache_dir` routes `manifests/<axis>/<name>` beneath it.
    let project = Path::new("/proj");
    let base = crate::config::Layout::new(project).cache_dir();
    assert_eq!(
        cache_dir(project, Axis::Source, "documentation"),
        base.join("manifests/sources/documentation")
    );
    assert_eq!(cache_dir(project, Axis::Target, "omnia"), base.join("manifests/targets/omnia"));
}

#[test]
fn source_briefs_typed_at_parse_boundary() {
    let yaml = r"name: documentation
version: 1.0.0
axis: source
briefs:
  survey: briefs/survey.md
  extract: briefs/extract.md
";
    let manifest: SourceAdapter = serde_saphyr::from_str(yaml).expect("parse");
    assert_eq!(
        manifest.operations().copied().collect::<Vec<_>>(),
        vec![SourceOperation::Extract, SourceOperation::Survey]
    );
    let rendered = serde_saphyr::to_string(&manifest).expect("serialise");
    let reparsed: SourceAdapter = serde_saphyr::from_str(&rendered).expect("reparse");
    assert_eq!(manifest, reparsed);
}

#[test]
fn target_briefs_typed_at_parse_boundary() {
    let yaml = r"name: omnia
version: 1.0.0
axis: target
briefs:
  shape: briefs/shape.md
  build: briefs/build.md
  merge: briefs/merge.md
";
    let manifest: TargetAdapter = serde_saphyr::from_str(yaml).expect("parse");
    assert_eq!(
        manifest.operations().copied().collect::<Vec<_>>(),
        // BTreeMap key order: build < merge < shape (kebab-case).
        vec![TargetOperation::Build, TargetOperation::Merge, TargetOperation::Shape]
    );
}

#[test]
fn execution_mode_round_trips() {
    let yaml = r"name: documentation
version: 1.0.0
axis: source
execution: agent
briefs:
  survey: briefs/survey.md
  extract: briefs/extract.md
";
    let manifest: SourceAdapter = serde_saphyr::from_str(yaml).expect("parse");
    assert_eq!(manifest.execution, Some(Execution::Agent));
    let rendered = serde_saphyr::to_string(&manifest).expect("serialise");
    assert!(
        rendered.contains("execution: agent"),
        "execution must round-trip as kebab-case, got:\n{rendered}"
    );
    let reparsed: SourceAdapter = serde_saphyr::from_str(&rendered).expect("reparse");
    assert_eq!(manifest, reparsed);
}

#[test]
fn execution_tool_parses() {
    let yaml = r"name: omnia
version: 1.0.0
axis: target
execution: tool
briefs:
  shape: briefs/shape.md
  build: briefs/build.md
  merge: briefs/merge.md
";
    let manifest: TargetAdapter = serde_saphyr::from_str(yaml).expect("parse");
    assert_eq!(manifest.execution, Some(Execution::Tool));
}

#[test]
fn check_execution_rejects_missing_mode() {
    // The typed gate refuses to default silently when `execution`
    // is absent — the kebab discriminant routes to exit 2.
    let err = check_execution(None, Path::new("adapter.yaml"))
        .expect_err("missing execution must be rejected");
    let Error::Validation { code, .. } = err else {
        panic!("expected Error::Validation, got: {err:?}");
    };
    assert_eq!(code, "adapter-execution-mode-required");
}

#[test]
fn check_execution_accepts_declared_mode() {
    check_execution(Some(Execution::Agent), Path::new("adapter.yaml"))
        .expect("agent execution passes");
    check_execution(Some(Execution::Tool), Path::new("adapter.yaml"))
        .expect("tool execution passes (target axis)");
}

#[test]
fn version_parses_as_semver() {
    // RFC-47 D1: `version` is a semver string on the wire and a typed
    // `semver::Version` in memory.
    let yaml = r"name: documentation
version: 2.3.4
axis: source
briefs:
  survey: briefs/survey.md
  extract: briefs/extract.md
";
    let manifest: SourceAdapter = serde_saphyr::from_str(yaml).expect("parse");
    assert_eq!(manifest.version, semver::Version::new(2, 3, 4));
}

#[test]
fn check_version_rejects_non_semver() {
    // RFC-47 D1 belt-and-suspenders: a non-semver `version` surfaces as
    // the specific `adapter-version-malformed` finding.
    let value: serde_json::Value = serde_json::json!({ "version": "1" });
    let err = check_version(&value, Path::new("adapter.yaml"))
        .expect_err("integer-shaped version must be rejected");
    let Error::Validation { code, .. } = err else {
        panic!("expected Error::Validation, got: {err:?}");
    };
    assert_eq!(code, "adapter-version-malformed");
}

#[test]
fn check_version_accepts_semver() {
    let value: serde_json::Value = serde_json::json!({ "version": "1.2.3" });
    check_version(&value, Path::new("adapter.yaml")).expect("exact semver passes");
}

#[test]
fn check_requested_version_matches_single_identity() {
    // RFC-47 D2: a `None` pin always picks the installed identity; a
    // matching `Some(_)` pin passes; a mismatched pin cannot resolve a
    // single installed identity (`adapter-version-required`).
    let installed = semver::Version::new(1, 0, 0);
    check_requested_version(None, "omnia", &installed, Path::new("adapter.yaml"))
        .expect("bare ref resolves the single identity");
    check_requested_version(Some(&installed), "omnia", &installed, Path::new("adapter.yaml"))
        .expect("matching pin resolves");

    let other = semver::Version::new(2, 0, 0);
    let err = check_requested_version(Some(&other), "omnia", &installed, Path::new("adapter.yaml"))
        .expect_err("mismatched pin must be rejected");
    let Error::Validation { code, .. } = err else {
        panic!("expected Error::Validation, got: {err:?}");
    };
    assert_eq!(code, "adapter-version-required");
}

#[test]
fn unknown_brief_key_rejected() {
    // `shape` is a target operation; appearing on a source manifest
    // must fail at the typed `briefs: BTreeMap<SourceOperation, _>`
    // deserialisation boundary before any downstream code runs.
    let yaml = r"name: bogus
version: 1.0.0
axis: source
briefs:
  survey: briefs/survey.md
  shape: briefs/shape.md
";
    let err = serde_saphyr::from_str::<SourceAdapter>(yaml)
        .expect_err("unknown source operation must be rejected");
    let detail = err.to_string();
    assert!(
        detail.contains("shape") || detail.contains("survey"),
        "expected closed-enum diagnostic, got: {detail}"
    );
}

#[test]
fn target_without_platforms_round_trips() {
    let yaml = r"name: omnia
version: 1.0.0
axis: target
briefs:
  shape: briefs/shape.md
  build: briefs/build.md
  merge: briefs/merge.md
";
    let manifest: TargetAdapter = serde_saphyr::from_str(yaml).expect("parse");
    assert_eq!(manifest.platforms, None, "absent platforms must default to None");
    let rendered = serde_saphyr::to_string(&manifest).expect("serialise");
    assert!(
        !rendered.contains("platforms"),
        "absent platforms field must elide on write, got:\n{rendered}"
    );
    let reparsed: TargetAdapter = serde_saphyr::from_str(&rendered).expect("reparse");
    assert_eq!(manifest, reparsed);
}

#[test]
fn target_with_platforms_round_trips() {
    let yaml = r"name: vectis
version: 1.0.0
axis: target
briefs:
  shape: briefs/shape.md
  build: briefs/build.md
  merge: briefs/merge.md
platforms:
  required: true
  allowed:
    - core
    - ios
    - android
    - web
    - desktop
  default:
    - core
    - ios
    - android
";
    let manifest: TargetAdapter = serde_saphyr::from_str(yaml).expect("parse");
    let cap = manifest.platforms.as_ref().expect("platforms must be Some");
    assert!(cap.required);
    assert_eq!(
        cap.allowed,
        vec![Platform::Core, Platform::Ios, Platform::Android, Platform::Web, Platform::Desktop]
    );
    assert_eq!(cap.default, vec![Platform::Core, Platform::Ios, Platform::Android]);

    let rendered = serde_saphyr::to_string(&manifest).expect("serialise");
    assert!(rendered.contains("platforms:"), "platforms must appear in serialised output");
    assert!(rendered.contains("required: true"), "required must round-trip");

    let reparsed: TargetAdapter = serde_saphyr::from_str(&rendered).expect("reparse");
    assert_eq!(manifest, reparsed);
}

#[test]
fn platforms_optional_round_trip() {
    let yaml = r"name: contracts
version: 1.0.0
axis: target
briefs:
  shape: briefs/shape.md
  build: briefs/build.md
  merge: briefs/merge.md
platforms:
  required: false
  allowed:
    - core
  default:
    - core
";
    let manifest: TargetAdapter = serde_saphyr::from_str(yaml).expect("parse");
    let cap = manifest.platforms.as_ref().expect("platforms must be Some");
    assert!(!cap.required);
    assert_eq!(cap.allowed, vec![Platform::Core]);
    assert_eq!(cap.default, vec![Platform::Core]);

    let reparsed: TargetAdapter =
        serde_saphyr::from_str(&serde_saphyr::to_string(&manifest).unwrap()).expect("reparse");
    assert_eq!(manifest, reparsed);
}
