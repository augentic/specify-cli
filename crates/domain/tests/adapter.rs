//! Integration tests for the the axis-aware adapter loader
//! (`specify_domain::adapter`).
//!
//! Covers:
//! - axis routing — `(source, foo)` and `(target, foo)` resolve to
//!   distinct manifests even when the directory names collide.
//! - cache-vs-local probe order — the agent-populated manifest cache
//!   wins.
//! - cache placement — a load of `(source, …)` populates
//!   `.specify/.cache/manifests/sources/<name>/`; `(target, …)`
//!   mirrors under `manifests/targets/`. The extraction cache fingerprint contract extraction
//!   cache lives in a sibling tree under
//!   `.specify/.cache/extractions/<adapter>/` and is exercised by the
//!   `adapter::cache` unit tests.
//! - schema validation — both the shared shape and the axis-specific
//!   refinements (axis literal, closed `briefs.<operation>` keys) reject
//!   hand-rolled inputs.

use std::fs;
use std::path::{Path, PathBuf};

use specify_domain::adapter::{
    AdapterLocation, Axis, SourceAdapter, SourceOperation, TargetAdapter, TargetOperation,
    cache_dir, check_axis_unique_for_name,
};
use specify_error::Error;

fn fixtures_root() -> PathBuf {
    // `crates/domain/tests/` -> `tests/fixtures/plugins/`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins")
}

/// Build a temporary project layout by copying the in-tree fixture
/// directory into a fresh tempdir. The resulting `project_dir` carries
/// `sources/` and `targets/` (local axis) but no `.specify/.cache/`
/// entries — cache fixtures are populated by individual tests below.
fn local_project() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let project = tmp.path().to_path_buf();
    copy_dir_recursive(&fixtures_root(), &project);
    (tmp, project)
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create dst");
    for entry in fs::read_dir(src).expect("read fixtures") {
        let entry = entry.expect("entry");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_dir_recursive(&from, &to);
        } else {
            fs::copy(&from, &to).expect("copy fixture file");
        }
    }
}

#[test]
fn resolves_source_adapter_from_local_directory() {
    let (_tmp, project) = local_project();
    let resolved = SourceAdapter::resolve("code-typescript", &project)
        .expect("resolve source adapter from adapters/sources/<name>/adapter.yaml");
    assert_eq!(resolved.manifest.name, "code-typescript");
    assert_eq!(resolved.manifest.axis, Axis::Source);
    assert_eq!(
        resolved.manifest.operations().copied().collect::<Vec<_>>(),
        vec![SourceOperation::Extract, SourceOperation::Survey]
    );
    assert_eq!(
        resolved.manifest.briefs.get(&SourceOperation::Extract).map(String::as_str),
        Some("briefs/extract.md")
    );
    assert!(matches!(resolved.location, AdapterLocation::Local(_)));
    assert!(resolved.location.path().ends_with("adapters/sources/code-typescript"));
}

#[test]
fn resolves_target_adapter_from_local_directory() {
    let (_tmp, project) = local_project();
    let resolved = TargetAdapter::resolve("omnia", &project)
        .expect("resolve target adapter from adapters/targets/<name>/adapter.yaml");
    assert_eq!(resolved.manifest.name, "omnia");
    assert_eq!(resolved.manifest.axis, Axis::Target);
    // `briefs` is a BTreeMap, so `operations()` yields keys in
    // ascending kebab-name order: build < merge < shape.
    assert_eq!(
        resolved.manifest.operations().copied().collect::<Vec<_>>(),
        vec![TargetOperation::Build, TargetOperation::Merge, TargetOperation::Shape]
    );
    assert!(resolved.location.path().ends_with("adapters/targets/omnia"));
}

#[test]
fn axis_collision_rejected_at_resolve_time() {
    // Both `adapters/sources/foo/` and `adapters/targets/foo/` exist
    // in the fixture. Per DECISIONS.md §"Adapter name uniqueness"
    // the loader must reject this configuration on either axis with
    // the kebab-case `adapter-name-axis-collision` discriminant.
    let (_tmp, project) = local_project();
    for err in [
        SourceAdapter::resolve("foo", &project)
            .expect_err("source-axis resolve must reject the collision"),
        TargetAdapter::resolve("foo", &project)
            .expect_err("target-axis resolve must reject the collision"),
    ] {
        let Error::Validation { results } = err else {
            panic!("expected Error::Validation, got: {err:?}");
        };
        assert_eq!(results.len(), 1, "single-finding payload");
        assert_eq!(results[0].rule_id, "adapter-name-axis-collision");
        let detail = results[0].detail.as_deref().unwrap_or_default();
        assert!(
            detail.contains("adapters/sources/") && detail.contains("adapters/targets/"),
            "error body must name both axes, got: {detail}"
        );
    }
}

#[test]
fn axis_unique_passes_distinct() {
    // The fixture declares `code-typescript` only on the source axis
    // and `omnia` only on the target axis. Installing each on its
    // declared axis (or any brand-new name on either axis) must not
    // collide.
    let (_tmp, project) = local_project();
    check_axis_unique_for_name(Axis::Source, "code-typescript", &project)
        .expect("source-only adapter name is unique on the source axis");
    check_axis_unique_for_name(Axis::Target, "omnia", &project)
        .expect("target-only adapter name is unique on the target axis");
    check_axis_unique_for_name(Axis::Source, "brand-new-name", &project)
        .expect("absent adapter name is unique on the source axis");
    check_axis_unique_for_name(Axis::Target, "brand-new-name", &project)
        .expect("absent adapter name is unique on the target axis");
}

#[test]
fn axis_unique_rejects_opposite_axis() {
    // The init-time helper for the cross-axis uniqueness invariant.
    // Asking to install `foo` on either axis must fail because the
    // fixture already declares `foo` on both.
    let (_tmp, project) = local_project();
    for axis in [Axis::Source, Axis::Target] {
        let err = check_axis_unique_for_name(axis, "foo", &project)
            .expect_err("colliding adapter name must fail");
        let Error::Validation { results } = err else {
            panic!("expected Error::Validation, got: {err:?}");
        };
        assert_eq!(results[0].rule_id, "adapter-name-axis-collision");
        let detail = results[0].detail.as_deref().unwrap_or_default();
        assert!(
            detail.contains("adapters/sources/") && detail.contains("adapters/targets/"),
            "error body must name both axes, got: {detail}"
        );
    }
}

#[test]
fn cache_dir_resolves_under_axis_segment() {
    let project = Path::new("/proj");
    assert_eq!(
        cache_dir(project, Axis::Source, "documentation"),
        project.join(".specify/.cache/manifests/sources/documentation"),
        "per-axis manifest cache root for source adapters lives under .specify/.cache/manifests/sources/",
    );
    assert_eq!(
        cache_dir(project, Axis::Target, "omnia"),
        project.join(".specify/.cache/manifests/targets/omnia"),
        "per-axis manifest cache root for target adapters lives under .specify/.cache/manifests/targets/",
    );
}

#[test]
fn cache_wins_over_local() {
    // Stage a manifest under `.specify/.cache/manifests/sources/code-typescript/`
    // alongside the in-tree `adapters/sources/code-typescript/`; assert the
    // cached copy wins per workflow §Resolver and cache.
    let (_tmp, project) = local_project();
    let cached_root = cache_dir(&project, Axis::Source, "code-typescript");
    fs::create_dir_all(&cached_root).expect("create cache dir");
    fs::write(
        cached_root.join("adapter.yaml"),
        r"name: code-typescript
version: 7
axis: source
briefs:
  survey: briefs/survey.md
  extract: briefs/extract.md
description: Cached source adapter fixture.
",
    )
    .expect("stage cache manifest");

    let resolved = SourceAdapter::resolve("code-typescript", &project).expect("resolve from cache");
    assert_eq!(resolved.manifest.version, 7, "cache wins over local");
    assert!(matches!(resolved.location, AdapterLocation::Cached(_)));
}

#[test]
fn missing_adapter_reports_adapter_not_found() {
    let (_tmp, project) = local_project();
    let err =
        SourceAdapter::resolve("nonexistent", &project).expect_err("missing adapter must fail");
    let detail = err.to_string();
    assert!(detail.contains("adapter-not-found"), "{detail}");
}

#[test]
fn schema_violations_reject_at_load_time() {
    // Source-axis adapter with the wrong brief key set — `shape` is
    // not a source operation, and `extract` is required by
    // `source.schema.json#/properties/briefs`.
    let (_tmp, project) = local_project();
    let bad_root = project.join("adapters").join("sources").join("wrong-ops");
    fs::create_dir_all(&bad_root).expect("create bad source dir");
    fs::write(
        bad_root.join("adapter.yaml"),
        r"name: wrong-ops
version: 1
axis: source
briefs:
  survey: briefs/survey.md
  shape: briefs/shape.md
",
    )
    .expect("write bad manifest");

    let err = SourceAdapter::resolve("wrong-ops", &project)
        .expect_err("source-axis adapter with wrong brief keys must fail");
    let detail = err.to_string();
    assert!(
        detail.contains("adapter-schema-violation")
            || detail.contains("adapter-manifest-malformed"),
        "expected schema violation, got: {detail}"
    );
}

#[test]
fn resolves_captures_with_tools() {
    // workflow §Acceptance scenario #26-1 (release blocker, D1):
    // pin the loader against the live `adapters/sources/captures/`
    // adapter shape shipped by the `plg` repo. The manifest carries
    // a `tools: [{ name: replay-index }]` declaration and a free-
    // form `description:` field; both must round-trip through the
    // axis-aware loader without forcing the operator to bind the
    // declared WASI tool (the tool itself is a follow-up per authority and reconciliation contract
    // §Implementation plan).
    //
    // This test is the cli-side complement to the deno harness
    // assertions in `augentic/specify` at
    // `tests/cross_repo/sources_test.ts` — the harness pins the
    // golden-fixture data shape (Evidence + reconciliation.yaml +
    // discovery.md) while this test pins the loader behaviour.
    let (_tmp, project) = local_project();
    let manifest_dir = project.join("adapters").join("sources").join("captures");
    fs::create_dir_all(manifest_dir.join("briefs")).expect("create captures adapter dir");
    fs::write(
        manifest_dir.join("adapter.yaml"),
        r"name: captures
version: 1
axis: source
briefs:
  survey: briefs/survey.md
  extract: briefs/extract.md
tools:
  - name: replay-index
    version: 0.1.0
description: >-
  Runtime capture source adapter. Walks a read-only capture tree under
  `$SOURCE_DIR` and emits one lead per observed handler entry point.
",
    )
    .expect("write captures manifest");
    fs::write(manifest_dir.join("briefs/survey.md"), "# survey\n").expect("survey brief stub");
    fs::write(manifest_dir.join("briefs/extract.md"), "# extract\n").expect("extract brief stub");

    let resolved = SourceAdapter::resolve("captures", &project)
        .expect("captures adapter loads via SourceAdapter::resolve");
    assert_eq!(resolved.manifest.name, "captures");
    assert_eq!(resolved.manifest.axis, Axis::Source);
    assert_eq!(
        resolved.manifest.operations().copied().collect::<Vec<_>>(),
        vec![SourceOperation::Extract, SourceOperation::Survey],
        "captures declares survey + extract per workflow §Runtime source adapter"
    );
    assert_eq!(
        resolved.manifest.briefs.get(&SourceOperation::Extract).map(String::as_str),
        Some("briefs/extract.md")
    );
    assert!(
        matches!(resolved.location, AdapterLocation::Local(_)),
        "live plg manifest resolves under adapters/sources/<name>/ (local axis)"
    );
    assert!(
        resolved.location.path().ends_with("adapters/sources/captures"),
        "resolver root must land on the plg-tree adapter directory, got: {}",
        resolved.location.path().display()
    );
}

#[test]
fn axis_mismatch_reports_dedicated_diagnostic() {
    // Adapter file lives under `adapters/sources/<name>/` but declares
    // `axis: target` — should fall through to the source schema and
    // ultimately the axis-mismatch check.
    let (_tmp, project) = local_project();
    let bad_root = project.join("adapters").join("sources").join("mislabeled");
    fs::create_dir_all(&bad_root).expect("create dir");
    fs::write(
        bad_root.join("adapter.yaml"),
        r"name: mislabeled
version: 1
axis: target
briefs:
  shape: briefs/shape.md
  build: briefs/build.md
  merge: briefs/merge.md
",
    )
    .expect("write manifest");

    let err = SourceAdapter::resolve("mislabeled", &project)
        .expect_err("axis literal must match the requested axis");
    let detail = err.to_string();
    assert!(
        detail.contains("adapter-schema-violation") || detail.contains("adapter-axis-mismatch"),
        "expected axis diagnostic, got: {detail}"
    );
}
