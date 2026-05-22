//! Integration tests for the RFC-25 axis-aware adapter loader
//! (`specify_domain::adapter`).
//!
//! Covers:
//! - axis routing — `(source, foo)` and `(target, foo)` resolve to
//!   distinct manifests even when the directory names collide.
//! - cache-vs-local probe order — the agent-populated cache wins.
//! - cache placement — a load of `(source, …)` populates
//!   `.specify/.cache/adapters/sources/<name>/`; `(target, …)` mirrors under
//!   `adapters/targets/`.
//! - schema validation — both the shared shape and the axis-specific
//!   refinements (axis literal, closed `operations[]`) reject hand-rolled
//!   inputs.

use std::fs;
use std::path::{Path, PathBuf};

use specify_domain::adapter::{Adapter, AdapterLocation, Axis, cache_dir};

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
    let resolved = Adapter::resolve(Axis::Source, "code-typescript", &project)
        .expect("resolve source adapter from adapters/sources/<name>/adapter.yaml");
    assert_eq!(resolved.manifest.name, "code-typescript");
    assert_eq!(resolved.manifest.axis, Axis::Source);
    assert_eq!(resolved.manifest.operations, vec!["enumerate", "extract"]);
    assert_eq!(
        resolved.manifest.briefs.get("extract").map(String::as_str),
        Some("briefs/extract.md")
    );
    assert!(matches!(resolved.location, AdapterLocation::Local(_)));
    assert!(resolved.root_dir.ends_with("adapters/sources/code-typescript"));
}

#[test]
fn resolves_target_adapter_from_local_directory() {
    let (_tmp, project) = local_project();
    let resolved = Adapter::resolve(Axis::Target, "omnia", &project)
        .expect("resolve target adapter from adapters/targets/<name>/adapter.yaml");
    assert_eq!(resolved.manifest.name, "omnia");
    assert_eq!(resolved.manifest.axis, Axis::Target);
    assert_eq!(resolved.manifest.operations, vec!["shape", "build", "merge"]);
    assert!(resolved.root_dir.ends_with("adapters/targets/omnia"));
}

#[test]
fn axis_disambiguates_colliding_names() {
    // Both `adapters/sources/foo/` and `adapters/targets/foo/` exist in the fixture; the
    // axis argument is the only thing that distinguishes them.
    let (_tmp, project) = local_project();
    let source = Adapter::resolve(Axis::Source, "foo", &project).expect("source/foo resolves");
    let target = Adapter::resolve(Axis::Target, "foo", &project).expect("target/foo resolves");
    assert_eq!(source.manifest.axis, Axis::Source);
    assert_eq!(target.manifest.axis, Axis::Target);
    assert_ne!(source.root_dir, target.root_dir);
    assert!(source.root_dir.ends_with("adapters/sources/foo"));
    assert!(target.root_dir.ends_with("adapters/targets/foo"));
}

#[test]
fn cache_dir_resolves_under_axis_segment() {
    let project = Path::new("/proj");
    assert_eq!(
        cache_dir(project, Axis::Source, "documentation"),
        project.join(".specify/.cache/adapters/sources/documentation"),
        "per-axis cache root for source adapters lives under .specify/.cache/adapters/sources/",
    );
    assert_eq!(
        cache_dir(project, Axis::Target, "omnia"),
        project.join(".specify/.cache/adapters/targets/omnia"),
        "per-axis cache root for target adapters lives under .specify/.cache/adapters/targets/",
    );
}

#[test]
fn cache_directory_wins_over_local_when_both_exist() {
    // Stage a manifest under `.specify/.cache/adapters/sources/code-typescript/`
    // alongside the in-tree `adapters/sources/code-typescript/`; assert the
    // cached copy wins per RFC-25 §Resolver and cache.
    let (_tmp, project) = local_project();
    let cached_root = cache_dir(&project, Axis::Source, "code-typescript");
    fs::create_dir_all(&cached_root).expect("create cache dir");
    fs::write(
        cached_root.join("adapter.yaml"),
        r"name: code-typescript
version: 7
axis: source
operations: [enumerate, extract]
briefs:
  enumerate: briefs/enumerate.md
  extract: briefs/extract.md
",
    )
    .expect("stage cache manifest");

    let resolved =
        Adapter::resolve(Axis::Source, "code-typescript", &project).expect("resolve from cache");
    assert_eq!(resolved.manifest.version, 7, "cache wins over local");
    assert!(matches!(resolved.location, AdapterLocation::Cached(_)));
}

#[test]
fn missing_adapter_reports_adapter_not_found() {
    let (_tmp, project) = local_project();
    let err = Adapter::resolve(Axis::Source, "nonexistent", &project)
        .expect_err("missing adapter must fail");
    let detail = err.to_string();
    assert!(detail.contains("adapter-not-found"), "{detail}");
}

#[test]
fn schema_violations_reject_at_load_time() {
    // Source-axis adapter with the wrong operation set — `shape` is not
    // a source operation.
    let (_tmp, project) = local_project();
    let bad_root = project.join("adapters").join("sources").join("wrong-ops");
    fs::create_dir_all(&bad_root).expect("create bad source dir");
    fs::write(
        bad_root.join("adapter.yaml"),
        r"name: wrong-ops
version: 1
axis: source
operations: [enumerate, shape]
briefs:
  enumerate: briefs/enumerate.md
  shape: briefs/shape.md
",
    )
    .expect("write bad manifest");

    let err = Adapter::resolve(Axis::Source, "wrong-ops", &project)
        .expect_err("source-axis adapter with wrong operations must fail");
    let detail = err.to_string();
    assert!(
        detail.contains("adapter-schema-violation")
            || detail.contains("adapter-manifest-malformed"),
        "expected schema violation, got: {detail}"
    );
}

#[test]
fn resolves_code_runtime_source_adapter_with_tools_array() {
    // RFC-27 §Acceptance scenario #26-1 (release blocker, D1):
    // pin the loader against the live `adapters/sources/code-runtime/`
    // adapter shape shipped by the `plg` repo. The manifest carries
    // a `tools: [{ name: fixture-index }]` declaration and a free-
    // form `description:` field; both must round-trip through the
    // axis-aware loader without forcing the operator to bind the
    // declared WASI tool (the tool itself is a follow-up per RFC-27
    // §Implementation plan).
    //
    // This test is the cli-side complement to the deno harness
    // assertions in `augentic/specify` at
    // `tests/cross_repo/sources_test.ts` — the harness pins the
    // golden-fixture data shape (Evidence + fusion.yaml +
    // discovery.md) while this test pins the loader behaviour.
    let (_tmp, project) = local_project();
    let manifest_dir = project.join("adapters").join("sources").join("code-runtime");
    fs::create_dir_all(manifest_dir.join("briefs")).expect("create code-runtime adapter dir");
    fs::write(
        manifest_dir.join("adapter.yaml"),
        r"name: code-runtime
version: 1
axis: source
operations: [enumerate, extract]
briefs:
  enumerate: briefs/enumerate.md
  extract: briefs/extract.md
tools:
  - name: fixture-index
description: >-
  Runtime-fixture source adapter. Walks a read-only fixture tree under
  `$SOURCE_DIR` and emits one candidate per observed handler entry point.
",
    )
    .expect("write code-runtime manifest");
    fs::write(manifest_dir.join("briefs/enumerate.md"), "# enumerate\n")
        .expect("enumerate brief stub");
    fs::write(manifest_dir.join("briefs/extract.md"), "# extract\n").expect("extract brief stub");

    let resolved = Adapter::resolve(Axis::Source, "code-runtime", &project)
        .expect("code-runtime adapter loads via Axis::Source resolver");
    assert_eq!(resolved.manifest.name, "code-runtime");
    assert_eq!(resolved.manifest.axis, Axis::Source);
    assert_eq!(
        resolved.manifest.operations,
        vec!["enumerate", "extract"],
        "code-runtime declares enumerate + extract per RFC-27 §Runtime source adapter"
    );
    assert_eq!(
        resolved.manifest.briefs.get("extract").map(String::as_str),
        Some("briefs/extract.md")
    );
    assert!(
        matches!(resolved.location, AdapterLocation::Local(_)),
        "live plg manifest resolves under adapters/sources/<name>/ (local axis)"
    );
    assert!(
        resolved.root_dir.ends_with("adapters/sources/code-runtime"),
        "resolver root must land on the plg-tree adapter directory, got: {}",
        resolved.root_dir.display()
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
operations: [shape, build, merge]
briefs:
  shape: briefs/shape.md
  build: briefs/build.md
  merge: briefs/merge.md
",
    )
    .expect("write manifest");

    let err = Adapter::resolve(Axis::Source, "mislabeled", &project)
        .expect_err("axis literal must match the requested axis");
    let detail = err.to_string();
    assert!(
        detail.contains("adapter-schema-violation") || detail.contains("adapter-axis-mismatch"),
        "expected axis diagnostic, got: {detail}"
    );
}
