//! Path-resolver and `validate all` integration tests.

mod engine_support;

use engine_support::{extract_envelope, write_specify_project};
use serde_json::Value;
use specify_vectis::validate::__test_internals::{
    discover_artifact, expand_path_template, find_project_root, paths_for_key,
    resolve_default_path_with_root,
};
use specify_vectis::validate::{ValidateArgs as Args, ValidateMode, run};

/// `find_project_root` walks up from a starting path until it finds a
/// `.specify/` ancestor. A starting path that is itself the project
/// root resolves cleanly; a starting path nested under the root walks
/// up to find it. A path with no Specify ancestor returns `None`.
#[test]
fn find_project_root_walks_up_to_specify_dir() {
    let tmp = write_specify_project();
    let nested = tmp.path().join("a/b/c");
    std::fs::create_dir_all(&nested).expect("mkdir nested");

    assert_eq!(find_project_root(tmp.path()).as_deref(), Some(tmp.path()));
    assert_eq!(find_project_root(&nested).as_deref(), Some(tmp.path()));
    let file = nested.join("file.yaml");
    std::fs::write(&file, b"version: 1\n").expect("write file");
    assert_eq!(find_project_root(&file).as_deref(), Some(tmp.path()));

    let bare = tempfile::tempdir().expect("tempdir");
    assert!(find_project_root(bare.path()).is_none());
}

/// `paths_for_key` returns the canonical resolution order for known
/// keys and an empty candidate list for unknown keys.
#[test]
fn paths_for_key_returns_embedded_canonical_order() {
    let tokens = paths_for_key("tokens");
    assert_eq!(
        tokens,
        vec![
            ".specify/slices/<name>/tokens.yaml".to_string(),
            "design-system/tokens.yaml".to_string(),
        ]
    );

    assert!(paths_for_key("components").is_empty());
}

/// `expand_path_template` substitutes `<name>` against every
/// directory under `.specify/slices/`, sorted alphabetically.
/// Templates without `<name>` resolve to a single absolute path
/// rooted at the project root. Templates with `<name>` against a
/// project that has no `.specify/slices/` directory resolve to an
/// empty list so the caller skips to the next template.
#[test]
fn expand_path_template_handles_name_substitution() {
    let tmp = write_specify_project();
    let slices_dir = tmp.path().join(".specify/slices");
    std::fs::create_dir_all(slices_dir.join("zeta")).expect("mkdir zeta");
    std::fs::create_dir_all(slices_dir.join("alpha")).expect("mkdir alpha");

    let with_name = expand_path_template(".specify/slices/<name>/layout.yaml", tmp.path());
    assert_eq!(with_name.len(), 2);
    assert!(with_name[0].ends_with(".specify/slices/alpha/layout.yaml"));
    assert!(with_name[1].ends_with(".specify/slices/zeta/layout.yaml"));

    let without_name = expand_path_template("design-system/layout.yaml", tmp.path());
    assert_eq!(without_name.len(), 1);
    assert!(without_name[0].ends_with("design-system/layout.yaml"));

    let empty = tempfile::tempdir().expect("tempdir");
    let no_changes = expand_path_template(".specify/slices/<name>/x.yaml", empty.path());
    assert!(no_changes.is_empty());
}

/// The default-path resolver's primary acceptance bullet: when no
/// `[path]` is supplied, `validate layout` discovers
/// `.specify/slices/<active>/layout.yaml` first (the `change_local`
/// template) before falling back to `design-system/layout.yaml` (the
/// `project` template).
#[test]
fn resolve_default_path_prefers_change_local_over_project() {
    let tmp = write_specify_project();
    let change_dir = tmp.path().join(".specify/slices/active");
    std::fs::create_dir_all(&change_dir).expect("mkdir change");
    std::fs::write(change_dir.join("layout.yaml"), "version: 1\nscreens: {}\n")
        .expect("write layout.yaml");
    let design = tmp.path().join("design-system");
    std::fs::create_dir_all(&design).expect("mkdir design-system");
    std::fs::write(design.join("layout.yaml"), "version: 1\nscreens: {}\n")
        .expect("write design-system/layout.yaml");

    let resolved = resolve_default_path_with_root(ValidateMode::Layout, tmp.path());
    assert!(
        resolved.ends_with(".specify/slices/active/layout.yaml"),
        "expected change-local resolution, got: {}",
        resolved.display(),
    );
}

/// When the change-local file is absent but the project-shape exists,
/// `validate layout` falls back to `design-system/`.
#[test]
fn resolve_default_path_falls_back_to_project_when_change_local_missing() {
    let tmp = write_specify_project();
    let design = tmp.path().join("design-system");
    std::fs::create_dir_all(&design).expect("mkdir design-system");
    std::fs::write(design.join("tokens.yaml"), "version: 1\n").expect("write tokens.yaml");

    let resolved = resolve_default_path_with_root(ValidateMode::Tokens, tmp.path());
    assert!(
        resolved.ends_with("design-system/tokens.yaml"),
        "expected project-shape resolution, got: {}",
        resolved.display(),
    );
}

/// When neither template resolves, the resolver returns the last
/// candidate (the project / baseline shape) so the caller's
/// "<file>.yaml not readable" error names the most operator-friendly
/// path.
#[test]
fn resolve_default_path_returns_last_candidate_when_nothing_exists() {
    let tmp = write_specify_project();
    let layout = resolve_default_path_with_root(ValidateMode::Layout, tmp.path());
    assert!(
        layout.ends_with("design-system/layout.yaml"),
        "expected design-system/layout.yaml fallback, got: {}",
        layout.display(),
    );
    let composition = resolve_default_path_with_root(ValidateMode::Composition, tmp.path());
    assert!(
        composition.ends_with(".specify/specs/composition.yaml"),
        "expected baseline composition fallback, got: {}",
        composition.display(),
    );
}

/// `discover_artifact` is the cross-artifact discovery helper. It
/// returns `Some(path)` only when the file is actually on disk --
/// never the "best guess" fallback path the per-mode resolver
/// returns. This pins that contract distinction: `Some` means "we
/// found it"; `None` means "no sibling was found, skip cross-artifact
/// resolution".
#[test]
fn discover_artifact_returns_some_only_for_existing_files() {
    let tmp = write_specify_project();
    let comp_dir = tmp.path().join(".specify/specs");
    std::fs::create_dir_all(&comp_dir).expect("mkdir specs");
    std::fs::write(comp_dir.join("composition.yaml"), "version: 1\nscreens: {}\n")
        .expect("write composition.yaml");
    let assets_path = tmp.path().join("design-system/assets.yaml");
    std::fs::create_dir_all(assets_path.parent().expect("parent")).expect("mkdir design-system");
    std::fs::write(&assets_path, "version: 1\nassets: {}\n").expect("write assets.yaml");

    let found = discover_artifact(&assets_path, ValidateMode::Composition);
    assert!(
        found.as_deref().is_some_and(|p| p.ends_with(".specify/specs/composition.yaml")),
        "expected composition discovery to succeed, got: {found:?}",
    );

    let missing = discover_artifact(&assets_path, ValidateMode::Tokens);
    assert!(missing.is_none(), "expected tokens discovery to return None, got: {missing:?}");

    let bare = tempfile::tempdir().expect("tempdir");
    assert!(
        discover_artifact(bare.path(), ValidateMode::Composition).is_none(),
        "expected None for non-Specify starting paths"
    );
}

/// The combined-run envelope MUST carry `mode: "all"`, the project
/// root in `path`, and a `results` array with exactly four sub-reports
/// in the canonical order layout → composition → tokens → assets.
/// Each sub-report has its own per-mode envelope under `report`.
#[test]
fn all_envelope_runs_every_mode_in_canonical_order() {
    let tmp = write_specify_project();

    let design = tmp.path().join("design-system");
    std::fs::create_dir_all(&design).expect("mkdir design-system");
    std::fs::write(design.join("layout.yaml"), "version: 1\nscreens: {}\n")
        .expect("write layout.yaml");
    std::fs::write(design.join("tokens.yaml"), "version: 1\n").expect("write tokens.yaml");
    std::fs::write(design.join("assets.yaml"), "version: 1\nassets: {}\n")
        .expect("write assets.yaml");
    let specs = tmp.path().join(".specify/specs");
    std::fs::create_dir_all(&specs).expect("mkdir specs");
    std::fs::write(specs.join("composition.yaml"), "version: 1\nscreens: {}\n")
        .expect("write composition.yaml");

    let envelope = extract_envelope(
        run(&Args {
            mode: ValidateMode::All,
            path: Some(tmp.path().to_path_buf()),
        })
        .expect("run all succeeds"),
    );

    assert_eq!(envelope["mode"], "all");
    assert_eq!(envelope["path"].as_str().expect("path string"), tmp.path().display().to_string());
    let results = envelope["results"].as_array().expect("results array");
    assert_eq!(results.len(), 4, "expected four sub-reports: {envelope}");
    assert_eq!(results[0]["mode"], "layout");
    assert_eq!(results[1]["mode"], "composition");
    assert_eq!(results[2]["mode"], "tokens");
    assert_eq!(results[3]["mode"], "assets");

    for entry in results {
        let report = &entry["report"];
        assert!(report.get("skipped").is_none(), "unexpected skipped: {entry}");
        assert_eq!(
            report["errors"].as_array().map(Vec::len),
            Some(0),
            "{}: unexpected errors: {entry}",
            entry["mode"]
        );
    }
}

/// Sub-modes whose default-resolved input does not exist on disk MUST
/// surface as a synthetic `{ skipped: true }` sub-report rather than a
/// hard `InvalidProject` failure -- so `validate all` keeps running
/// through the rest of the modes.
#[test]
fn all_envelope_skips_missing_inputs_without_failing() {
    let tmp = write_specify_project();
    let design = tmp.path().join("design-system");
    std::fs::create_dir_all(&design).expect("mkdir design-system");
    std::fs::write(design.join("tokens.yaml"), "version: 1\n").expect("write tokens.yaml");

    let envelope = extract_envelope(
        run(&Args {
            mode: ValidateMode::All,
            path: Some(tmp.path().to_path_buf()),
        })
        .expect("run all does not fail on missing inputs"),
    );

    let results = envelope["results"].as_array().expect("results array");
    let by_mode: std::collections::BTreeMap<&str, &Value> =
        results.iter().map(|e| (e["mode"].as_str().expect("mode str"), e)).collect();

    for skipped_mode in ["layout", "composition", "assets"] {
        let report = &by_mode[skipped_mode]["report"];
        assert_eq!(
            report["skipped"],
            Value::Bool(true),
            "[{skipped_mode}] expected skipped: {report}",
        );
        assert_eq!(
            report["errors"].as_array().map(Vec::len),
            Some(0),
            "[{skipped_mode}] errors must stay empty: {report}"
        );
    }
    let tokens_report = &by_mode["tokens"]["report"];
    assert!(
        tokens_report.get("skipped").is_none(),
        "tokens.yaml IS on disk; skipped MUST be absent: {tokens_report}",
    );
}

/// A sub-mode's findings MUST surface inside `results[*].report` so
/// the dispatcher's recursion-aware `validate_exit_code` helper picks
/// them up. This test feeds a deliberately-broken tokens.yaml and
/// asserts the broken-hex error rides the nested sub-report.
#[test]
fn all_envelope_propagates_sub_mode_errors_into_nested_report() {
    let tmp = write_specify_project();
    let design = tmp.path().join("design-system");
    std::fs::create_dir_all(&design).expect("mkdir design-system");
    std::fs::write(
        design.join("tokens.yaml"),
        "version: 1\ncolors:\n  primary:\n    light: \"#xyz\"\n    dark: \"#000000\"\n",
    )
    .expect("write tokens.yaml");

    let envelope = extract_envelope(
        run(&Args {
            mode: ValidateMode::All,
            path: Some(tmp.path().to_path_buf()),
        })
        .expect("run all succeeds"),
    );
    let results = envelope["results"].as_array().expect("results array");
    let tokens_entry =
        results.iter().find(|e| e["mode"] == "tokens").expect("tokens sub-report present");
    let tokens_errors = tokens_entry["report"]["errors"].as_array().expect("tokens errors array");
    assert!(
        !tokens_errors.is_empty(),
        "broken hex MUST surface in nested tokens report: {envelope}"
    );
}
