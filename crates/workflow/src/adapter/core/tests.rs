use super::*;
use crate::Platform;

#[test]
fn axis_dir_segment_round_trips() {
    assert_eq!(Axis::Source.dir_segment(), "sources");
    assert_eq!(Axis::Target.dir_segment(), "targets");
}

#[test]
fn cache_dir_routes_by_axis() {
    let project = Path::new("/proj");
    assert_eq!(
        cache_dir(project, Axis::Source, "documentation"),
        project.join(".specify/.cache/manifests/sources/documentation")
    );
    assert_eq!(
        cache_dir(project, Axis::Target, "omnia"),
        project.join(".specify/.cache/manifests/targets/omnia")
    );
}

#[test]
fn source_cache_field_defaults_to_none() {
    let yaml = r"name: documentation
version: 1
axis: source
briefs:
  survey: briefs/survey.md
  extract: briefs/extract.md
";
    let manifest: SourceAdapter = serde_saphyr::from_str(yaml).expect("parse");
    assert_eq!(manifest.cache, None, "missing cache field must default to None");
    let rendered = serde_saphyr::to_string(&manifest).expect("serialise");
    assert!(
        !rendered.contains("cache:"),
        "absent cache field must elide on write, got:\n{rendered}"
    );
}

#[test]
fn source_cache_opt_out_round_trips() {
    let yaml = r"name: documentation
version: 1
axis: source
briefs:
  survey: briefs/survey.md
  extract: briefs/extract.md
cache: opt-out
";
    let manifest: SourceAdapter = serde_saphyr::from_str(yaml).expect("parse");
    assert_eq!(manifest.cache, Some(CacheMode::OptOut));
    assert_eq!(
        manifest.operations().copied().collect::<Vec<_>>(),
        vec![SourceOperation::Extract, SourceOperation::Survey]
    );
    let rendered = serde_saphyr::to_string(&manifest).expect("serialise");
    assert!(
        rendered.contains("cache: opt-out"),
        "cache: opt-out must round-trip as kebab-case, got:\n{rendered}"
    );
    let reparsed: SourceAdapter = serde_saphyr::from_str(&rendered).expect("reparse");
    assert_eq!(manifest, reparsed);
}

#[test]
fn target_briefs_typed_at_parse_boundary() {
    let yaml = r"name: omnia
version: 1
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
version: 1
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
version: 1
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
    let err = check_execution(None, None, Path::new("adapter.yaml"))
        .expect_err("missing execution must be rejected");
    let Error::Validation { code, .. } = err else {
        panic!("expected Error::Validation, got: {err:?}");
    };
    assert_eq!(code, "adapter-execution-mode-required");
}

#[test]
fn execution_agent_allows_forced_opt_out() {
    // `execution: agent` forces `cache: opt-out`; declaring the
    // matching opt-out (or no cache at all) is not a conflict.
    check_execution(Some(Execution::Agent), Some(CacheMode::OptOut), Path::new("adapter.yaml"))
        .expect("agent + opt-out is consistent");
    check_execution(Some(Execution::Agent), None, Path::new("adapter.yaml"))
        .expect("agent + absent cache is consistent");
}

#[test]
fn check_execution_tool_passes() {
    check_execution(Some(Execution::Tool), None, Path::new("adapter.yaml"))
        .expect("tool execution with no cache declaration passes");
    check_execution(Some(Execution::Tool), Some(CacheMode::OptOut), Path::new("adapter.yaml"))
        .expect("tool execution may still opt out of the cache");
}

#[test]
fn agent_execution_forces_effective_opt_out() {
    let yaml = r"name: documentation
version: 1
axis: source
execution: agent
briefs:
  survey: briefs/survey.md
  extract: briefs/extract.md
";
    let manifest: SourceAdapter = serde_saphyr::from_str(yaml).expect("parse");
    assert_eq!(manifest.cache, None, "no declared cache field");
    assert_eq!(
        manifest.effective_cache_mode(),
        Some(CacheMode::OptOut),
        "execution: agent forces cache: opt-out even with no declared cache"
    );
}

#[test]
fn unknown_brief_key_rejected() {
    // `shape` is a target operation; appearing on a source manifest
    // must fail at the typed `briefs: BTreeMap<SourceOperation, _>`
    // deserialisation boundary before any downstream code runs.
    let yaml = r"name: bogus
version: 1
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
version: 1
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
version: 1
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
version: 1
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
