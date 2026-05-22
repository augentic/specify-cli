//! Integration tests for the RFC-25 axis-aware adapter loader
//! (`specify_domain::adapter`).
//!
//! Covers:
//! - axis routing — `(source, foo)` and `(target, foo)` resolve to
//!   distinct manifests even when the directory names collide.
//! - cache-vs-local probe order — the agent-populated cache wins.
//! - cache placement — a load of `(source, …)` populates
//!   `.specify/.cache/sources/<name>/`; `(target, …)` mirrors under
//!   `targets/`.
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
        .expect("resolve source adapter from sources/<name>/adapter.yaml");
    assert_eq!(resolved.manifest.name, "code-typescript");
    assert_eq!(resolved.manifest.axis, Axis::Source);
    assert_eq!(resolved.manifest.operations, vec!["enumerate", "extract"]);
    assert_eq!(
        resolved.manifest.briefs.get("extract").map(String::as_str),
        Some("briefs/extract.md")
    );
    assert!(matches!(resolved.location, AdapterLocation::Local(_)));
    assert!(resolved.root_dir.ends_with("sources/code-typescript"));
}

#[test]
fn resolves_target_adapter_from_local_directory() {
    let (_tmp, project) = local_project();
    let resolved = Adapter::resolve(Axis::Target, "omnia", &project)
        .expect("resolve target adapter from targets/<name>/adapter.yaml");
    assert_eq!(resolved.manifest.name, "omnia");
    assert_eq!(resolved.manifest.axis, Axis::Target);
    assert_eq!(resolved.manifest.operations, vec!["shape", "build", "merge"]);
    assert!(resolved.root_dir.ends_with("targets/omnia"));
}

#[test]
fn axis_disambiguates_colliding_names() {
    // Both `sources/foo/` and `targets/foo/` exist in the fixture; the
    // axis argument is the only thing that distinguishes them.
    let (_tmp, project) = local_project();
    let source = Adapter::resolve(Axis::Source, "foo", &project).expect("source/foo resolves");
    let target = Adapter::resolve(Axis::Target, "foo", &project).expect("target/foo resolves");
    assert_eq!(source.manifest.axis, Axis::Source);
    assert_eq!(target.manifest.axis, Axis::Target);
    assert_ne!(source.root_dir, target.root_dir);
    assert!(source.root_dir.ends_with("sources/foo"));
    assert!(target.root_dir.ends_with("targets/foo"));
}

#[test]
fn cache_dir_resolves_under_axis_segment() {
    let project = Path::new("/proj");
    assert_eq!(
        cache_dir(project, Axis::Source, "documentation"),
        project.join(".specify/.cache/sources/documentation"),
        "per-axis cache root for source adapters lives under .specify/.cache/sources/",
    );
    assert_eq!(
        cache_dir(project, Axis::Target, "omnia"),
        project.join(".specify/.cache/targets/omnia"),
        "per-axis cache root for target adapters lives under .specify/.cache/targets/",
    );
}

#[test]
fn cache_directory_wins_over_local_when_both_exist() {
    // Stage a manifest under `.specify/.cache/sources/code-typescript/`
    // alongside the in-tree `sources/code-typescript/`; assert the
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
    let bad_root = project.join("sources/wrong-ops");
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
fn axis_mismatch_reports_dedicated_diagnostic() {
    // Adapter file lives under `sources/<name>/` but declares
    // `axis: target` — should fall through to the source schema and
    // ultimately the axis-mismatch check.
    let (_tmp, project) = local_project();
    let bad_root = project.join("sources/mislabeled");
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
