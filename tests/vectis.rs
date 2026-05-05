//! Integration tests for the `specify vectis` subcommand tree.
//!
//! These lock in the v2 JSON contract for the four `vectis` verbs that
//! ship under the `specify` binary (chunk 5 of
//! `docs/plans/fold-vectis-into-specify.md`). The tests deliberately
//! exercise the whole `specify` binary end-to-end via `assert_cmd`
//! rather than calling into the `specify-vectis` library directly so
//! the global `--format` flag, `emit_json` envelope, and
//! `emit_vectis_error` mapping are all in scope.
//!
//! Where a test depends on workstation toolchain (`vectis init` needs
//! `rustup`/`cargo-deny`/`cargo-vet` to be on PATH), we soft-skip when
//! the binary reports `missing-prerequisites` so the suite stays green
//! on a stripped-down CI host. The dedicated
//! `init_missing_prereqs_json_shape` test goes the other way: it
//! *forces* the missing-prereqs path by clearing PATH so the JSON shape
//! is asserted unconditionally.

use std::path::PathBuf;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::tempdir;

fn specify() -> Command {
    Command::cargo_bin("specify").expect("cargo_bin(specify)")
}

fn parse_json(stdout: &[u8]) -> Value {
    let s = String::from_utf8(stdout.to_vec()).expect("utf8 stdout");
    serde_json::from_str(&s).unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {s}"))
}

#[test]
fn vectis_help_lists_subcommands() {
    let assert = specify().args(["vectis", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    for verb in ["init", "verify", "add-shell", "update-versions", "versions", "validate"] {
        assert!(
            stdout.contains(verb),
            "expected `vectis --help` to mention {verb}, got:\n{stdout}"
        );
    }
}

/// `init` happy path: assert the exact top-level kebab-case key set
/// plus the auto-injected `schema-version`. Soft-skips when the host
/// is missing the core toolchain (CI hosts without `rustup` etc.) so
/// the assertion below fires only when we actually got the success
/// payload.
#[test]
fn init_success_json_has_kebab_keys_and_schema_version() {
    let tmp = tempdir().unwrap();
    let assert = specify()
        .args(["--format", "json", "vectis", "init", "Foo", "--dir"])
        .arg(tmp.path())
        .assert();
    let output = assert.get_output();
    let stdout = output.stdout.clone();
    let value = parse_json(&stdout);

    if value.get("error").and_then(Value::as_str) == Some("missing-prerequisites") {
        eprintln!(
            "skipping init success test: workstation lacks core prereqs ({})",
            value.get("message").and_then(Value::as_str).unwrap_or("(no message)")
        );
        return;
    }

    assert!(
        output.status.success(),
        "expected success, got status {:?} and stdout:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&stdout)
    );

    assert_eq!(
        value.get("schema-version"),
        Some(&Value::from(2)),
        "missing schema-version: {value}"
    );

    let map = value.as_object().expect("top-level JSON is an object");
    let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "app-name",
            "app-struct",
            "assemblies",
            "capabilities",
            "project-dir",
            "schema-version",
            "shells",
        ],
        "init success payload key set drifted (chunk 4 invariant): {value}"
    );

    assert_eq!(value["app-name"], "Foo");
    assert_eq!(value["app-struct"], "Foo");
    let project_dir = value["project-dir"].as_str().expect("project-dir is a string");
    let canonical_tmp = std::fs::canonicalize(tmp.path()).expect("canonicalize tmp");
    let canonical_project =
        std::fs::canonicalize(PathBuf::from(project_dir)).expect("canonicalize project-dir");
    assert_eq!(canonical_project, canonical_tmp);

    let core = value["assemblies"].get("core").expect("`core` assembly present");
    assert_eq!(core["status"], "created");
    assert!(core["files"].is_array(), "core.files is an array");
}

/// `init` with a `--version-file` pointing at a missing path: the v2
/// error envelope must report `invalid-project` with `exit-code: 1`.
/// This path is independent of workstation toolchain, so it runs
/// unconditionally.
#[test]
fn init_invalid_project_json_shape() {
    let tmp = tempdir().unwrap();
    // Build the bogus version-file path *inside* `tempdir` so it's
    // guaranteed nonexistent on every platform (Windows, sandboxed
    // CI, etc.) without colliding with anything else under `/tmp`.
    let missing = tmp.path().join("definitely-not-there.toml");
    let assert = specify()
        .args(["--format", "json", "vectis", "init", "Foo", "--dir"])
        .arg(tmp.path())
        .arg("--version-file")
        .arg(&missing)
        .assert()
        .failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(value["error"], "invalid-project");
    assert_eq!(value["exit-code"], 1);
    assert_eq!(value["schema-version"], 2);
    assert!(
        value["message"].as_str().unwrap_or("").contains("version file not found"),
        "unexpected message: {value}"
    );
    assert_eq!(output.status.code(), Some(1));
}

/// `vectis validate --help` MUST list every mode the RFC-11 §H verb
/// table promises (`layout | composition | tokens | assets | all`)
/// plus the optional `[PATH]` positional. Phase 1.5 acceptance bullet:
/// the surface lands now even though every mode is a stub.
#[test]
fn vectis_validate_help_lists_every_mode_and_path_positional() {
    let assert = specify().args(["vectis", "validate", "--help"]).assert().success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    for mode in ["layout", "composition", "tokens", "assets", "all"] {
        assert!(
            stdout.contains(mode),
            "expected `vectis validate --help` to mention `{mode}`, got:\n{stdout}"
        );
    }
    // Positional `[PATH]` (clap renders the value-name in upper-case).
    assert!(
        stdout.contains("[PATH]"),
        "expected `vectis validate --help` to advertise an optional `[PATH]` positional, got:\n{stdout}"
    );
}

/// Phase 1.5 wired the `not-implemented` envelope for every mode;
/// Phase 1.6 promoted `tokens`, Phase 1.7 promoted `assets`, and
/// Phase 1.8 promoted `layout`, so this test now pins the envelope
/// across the two still-stubbed modes only (`composition`, `all`).
/// The live modes get their own dedicated tests below.
#[test]
fn vectis_validate_stub_modes_emit_not_implemented_envelope() {
    for mode in ["composition", "all"] {
        let assert =
            specify().args(["--format", "json", "vectis", "validate", mode]).assert().failure();
        let output = assert.get_output();
        let value = parse_json(&output.stdout);
        assert_eq!(value["error"], "not-implemented", "[{mode}] error variant: {value}");
        assert_eq!(value["exit-code"], 1, "[{mode}] exit-code: {value}");
        assert_eq!(value["schema-version"], 2, "[{mode}] schema-version: {value}");
        assert_eq!(value["command"], format!("validate {mode}"), "[{mode}] command field: {value}");
        let message = value["message"].as_str().unwrap_or("");
        assert!(
            message.contains("not implemented"),
            "[{mode}] expected message to mention `not implemented`, got: {message}"
        );
        assert_eq!(output.status.code(), Some(1), "[{mode}] expected exit 1");
    }
}

/// Phase 1.7 update: the `tokens` and `assets` modes are now real;
/// only `layout`, `composition`, and `all` remain as stubs. Asserts
/// the v2 `not-implemented` envelope across the three still-stubbed
/// modes -- the live modes get their own dedicated tests below.
#[test]
fn vectis_validate_assets_clean_run_exits_zero_with_envelope() {
    // Minimal valid `assets.yaml` (the schema permits an
    // assets-only manifest with `version` + an empty `assets` map,
    // because `assets` is required at the document level but
    // `additionalProperties: { ... }` allows zero entries).
    let tmp = tempdir().unwrap();
    let assets_path = tmp.path().join("assets.yaml");
    std::fs::write(&assets_path, "version: 1\nassets: {}\n").expect("write assets.yaml");

    let assert = specify()
        .args(["--format", "json", "vectis", "validate", "assets"])
        .arg(&assets_path)
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);

    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["mode"], "assets");
    assert_eq!(
        value["path"].as_str().expect("path is a string"),
        assets_path.display().to_string()
    );
    assert_eq!(value["errors"].as_array().map(Vec::len), Some(0), "expected no errors: {value}");
    assert_eq!(
        value["warnings"].as_array().map(Vec::len),
        Some(0),
        "expected no warnings: {value}"
    );
}

/// Phase 1.7: a referenced raster file that is not on disk must
/// surface as an error pointing at the corresponding density slot
/// (`/assets/<id>/sources/ios/1x`) and at the missing-file path.
#[test]
fn vectis_validate_assets_missing_raster_file_exits_one() {
    let tmp = tempdir().unwrap();
    let design = tmp.path().join("design-system");
    std::fs::create_dir_all(&design).expect("mkdir design-system");
    let assets_path = design.join("assets.yaml");
    // Asset declares a 1x density file that we never create on
    // disk.
    std::fs::write(
        &assets_path,
        r"version: 1
assets:
  hero:
    kind: raster
    role: illustration
    sources:
      ios:
        1x: assets/hero.png
",
    )
    .expect("write assets.yaml");

    let assert = specify()
        .args(["--format", "json", "vectis", "validate", "assets"])
        .arg(&assets_path)
        .assert()
        .failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(output.status.code(), Some(1), "expected exit 1: {value}");
    assert_eq!(value["mode"], "assets");
    let errors = value["errors"].as_array().expect("errors array");
    let any_hits = errors.iter().any(|e| {
        e.get("path").and_then(Value::as_str) == Some("/assets/hero/sources/ios/1x")
            && e.get("message").and_then(Value::as_str).unwrap_or("").contains("file not found")
    });
    assert!(any_hits, "expected file-not-found error for 1x: {errors:?}");
}

/// Phase 1.7: when a sibling `composition.yaml` is present at the
/// canonical path `.specify/specs/composition.yaml`, missing optional
/// raster densities surface as warnings (not errors) for any asset
/// the composition references.
#[test]
fn vectis_validate_assets_missing_density_emits_warning_when_referenced() {
    let tmp = tempdir().unwrap();
    let design = tmp.path().join("design-system");
    let assets_dir = design.join("assets/android");
    std::fs::create_dir_all(&assets_dir).expect("mkdir assets/android");
    std::fs::write(design.join("assets/hero@2x.png"), b"PNGSTUB").expect("write 2x");
    std::fs::write(design.join("assets/hero@3x.png"), b"PNGSTUB").expect("write 3x");
    std::fs::write(assets_dir.join("hero-mdpi.png"), b"PNGSTUB").expect("write mdpi");
    std::fs::write(assets_dir.join("hero-hdpi.png"), b"PNGSTUB").expect("write hdpi");
    std::fs::write(assets_dir.join("hero-xhdpi.png"), b"PNGSTUB").expect("write xhdpi");
    std::fs::write(assets_dir.join("hero-xxhdpi.png"), b"PNGSTUB").expect("write xxhdpi");
    std::fs::write(assets_dir.join("hero-xxxhdpi.png"), b"PNGSTUB").expect("write xxxhdpi");

    let assets_path = design.join("assets.yaml");
    std::fs::write(
        &assets_path,
        r"version: 1
assets:
  hero:
    kind: raster
    role: illustration
    sources:
      ios:
        2x: assets/hero@2x.png
        3x: assets/hero@3x.png
      android:
        mdpi: assets/android/hero-mdpi.png
        hdpi: assets/android/hero-hdpi.png
        xhdpi: assets/android/hero-xhdpi.png
        xxhdpi: assets/android/hero-xxhdpi.png
        xxxhdpi: assets/android/hero-xxxhdpi.png
",
    )
    .expect("write assets.yaml");

    let specs = tmp.path().join(".specify/specs");
    std::fs::create_dir_all(&specs).expect("mkdir .specify/specs");
    std::fs::write(
        specs.join("composition.yaml"),
        r"version: 1
screens:
  s:
    name: S
    body:
      list:
        item:
          - image:
              name: hero
",
    )
    .expect("write composition.yaml");

    let assert = specify()
        .args(["--format", "json", "vectis", "validate", "assets"])
        .arg(&assets_path)
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);

    assert_eq!(value["mode"], "assets");
    assert_eq!(value["errors"].as_array().map(Vec::len), Some(0), "errors unexpected: {value}");
    let warnings = value["warnings"].as_array().expect("warnings array");
    assert!(
        warnings.iter().any(|w| w
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .contains("missing optional `1x`")),
        "expected a missing-1x warning: {warnings:?}"
    );
}

/// Phase 1.7: when the sibling `composition.yaml` references an
/// asset id that is NOT in the manifest, the run errors with an
/// "unknown asset id" message.
#[test]
fn vectis_validate_assets_unresolved_composition_reference_exits_one() {
    let tmp = tempdir().unwrap();
    let design = tmp.path().join("design-system");
    std::fs::create_dir_all(&design).expect("mkdir design-system");
    let assets_path = design.join("assets.yaml");
    std::fs::write(
        &assets_path,
        // Manifest declares `gear` but composition references `mystery`.
        r"version: 1
assets:
  gear:
    kind: symbol
    role: icon
    symbols:
      ios: gearshape
      android: settings
",
    )
    .expect("write assets.yaml");

    let specs = tmp.path().join(".specify/specs");
    std::fs::create_dir_all(&specs).expect("mkdir .specify/specs");
    std::fs::write(
        specs.join("composition.yaml"),
        r"version: 1
screens:
  s:
    name: S
    header:
      title: T
      trailing:
        - icon-button:
            icon: mystery
            label: Mystery
    body:
      list:
        item: []
",
    )
    .expect("write composition.yaml");

    let assert = specify()
        .args(["--format", "json", "vectis", "validate", "assets"])
        .arg(&assets_path)
        .assert()
        .failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(output.status.code(), Some(1), "expected exit 1: {value}");
    let errors = value["errors"].as_array().expect("errors array");
    assert!(
        errors.iter().any(|e| e
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .contains("references unknown asset id `mystery`")),
        "expected unresolved-reference error: {errors:?}"
    );
}

/// Phase 1.7: a missing `assets.yaml` MUST surface as the v2
/// `invalid-project` error envelope (exit 1) -- distinct from the
/// validator-reported `errors` array, which is reserved for shape /
/// reference findings against an actually-loaded document.
#[test]
fn vectis_validate_assets_missing_file_surfaces_invalid_project() {
    let tmp = tempdir().unwrap();
    let missing = tmp.path().join("nope-assets.yaml");

    let assert = specify()
        .args(["--format", "json", "vectis", "validate", "assets"])
        .arg(&missing)
        .assert()
        .failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(value["error"], "invalid-project");
    assert_eq!(value["exit-code"], 1);
    assert_eq!(value["schema-version"], 2);
    assert!(
        value["message"].as_str().unwrap_or("").contains("assets.yaml not readable"),
        "unexpected message: {value}"
    );
    assert_eq!(output.status.code(), Some(1));
}

/// Phase 1.6: the `tokens` mode is now real. A valid Appendix-D-shaped
/// document MUST exit 0 with the per-mode envelope (`mode`, `path`,
/// empty `errors` and `warnings` arrays) and the auto-injected
/// `schema-version`.
#[test]
fn vectis_validate_tokens_clean_run_exits_zero_with_envelope() {
    let tmp = tempdir().unwrap();
    let tokens_path = tmp.path().join("tokens.yaml");
    std::fs::write(
        &tokens_path,
        // Tiny version-only document: structurally valid against
        // tokens.schema.json's `additionalProperties: false` because
        // every category is optional.
        "version: 1\n",
    )
    .expect("write tokens.yaml");

    let assert = specify()
        .args(["--format", "json", "vectis", "validate", "tokens"])
        .arg(&tokens_path)
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);

    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["mode"], "tokens");
    assert_eq!(
        value["path"].as_str().expect("path is a string"),
        tokens_path.display().to_string()
    );
    assert_eq!(value["errors"].as_array().map(Vec::len), Some(0), "expected no errors: {value}");
    assert_eq!(
        value["warnings"].as_array().map(Vec::len),
        Some(0),
        "expected no warnings: {value}"
    );
}

/// Phase 1.6: a deliberately broken `tokens.yaml` MUST exit 1 with at
/// least one error entry whose `path` points at the offending node
/// (`/colors/primary/light`). The `error` discriminator is **not**
/// present on the validate envelope (that's reserved for the v2 error
/// shape) -- the validator wrote a real report and exited non-zero.
#[test]
fn vectis_validate_tokens_broken_hex_exits_one_with_pathful_error() {
    let tmp = tempdir().unwrap();
    let tokens_path = tmp.path().join("tokens.yaml");
    std::fs::write(
        &tokens_path,
        "version: 1\ncolors:\n  primary:\n    light: \"#xyz\"\n    dark: \"#000000\"\n",
    )
    .expect("write tokens.yaml");

    let assert = specify()
        .args(["--format", "json", "vectis", "validate", "tokens"])
        .arg(&tokens_path)
        .assert()
        .failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(output.status.code(), Some(1), "expected exit 1: {value}");
    assert_eq!(value["mode"], "tokens");
    let errors = value["errors"].as_array().expect("errors array");
    assert!(!errors.is_empty(), "expected at least one error: {value}");
    let any_path_hits_primary_light = errors.iter().any(|e| {
        e.get("path").and_then(Value::as_str).is_some_and(|p| p.contains("/colors/primary/light"))
    });
    assert!(
        any_path_hits_primary_light,
        "expected an error pointing at /colors/primary/light, got: {errors:?}"
    );
}

/// Phase 1.6: a missing `tokens.yaml` MUST surface as the v2
/// `invalid-project` error envelope (exit 1) -- distinct from the
/// validator-reported `errors` array, which is reserved for shape /
/// reference findings against an actually-loaded document.
#[test]
fn vectis_validate_tokens_missing_file_surfaces_invalid_project() {
    let tmp = tempdir().unwrap();
    let missing = tmp.path().join("nope-tokens.yaml");

    let assert = specify()
        .args(["--format", "json", "vectis", "validate", "tokens"])
        .arg(&missing)
        .assert()
        .failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(value["error"], "invalid-project");
    assert_eq!(value["exit-code"], 1);
    assert_eq!(value["schema-version"], 2);
    assert!(
        value["message"].as_str().unwrap_or("").contains("tokens.yaml not readable"),
        "unexpected message: {value}"
    );
    assert_eq!(output.status.code(), Some(1));
}

/// Phase 1.8: a minimal but realistic `layout.yaml` (single screen,
/// no `component:` directive, no forbidden wiring keys) MUST exit 0
/// silently with the per-mode envelope. Asserts the `mode` and
/// `path` fields plus empty `errors` / `warnings` arrays so any
/// future drift in the envelope shape surfaces here.
#[test]
fn vectis_validate_layout_clean_run_exits_zero_with_envelope() {
    let tmp = tempdir().unwrap();
    let layout_path = tmp.path().join("layout.yaml");
    std::fs::write(
        &layout_path,
        r"version: 1
screens:
  s:
    name: S
    body:
      list:
        each: tasks
        item:
          - text:
              content: hello
",
    )
    .expect("write layout.yaml");

    let assert = specify()
        .args(["--format", "json", "vectis", "validate", "layout"])
        .arg(&layout_path)
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);

    assert_eq!(value["schema-version"], 2);
    assert_eq!(value["mode"], "layout");
    assert_eq!(
        value["path"].as_str().expect("path is a string"),
        layout_path.display().to_string()
    );
    assert_eq!(value["errors"].as_array().map(Vec::len), Some(0), "expected no errors: {value}");
    assert_eq!(
        value["warnings"].as_array().map(Vec::len),
        Some(0),
        "expected no warnings: {value}"
    );
}

/// Phase 1.8: a `bind:` key anywhere in `layout.yaml` MUST exit 1
/// with an error pointing at the offending JSON Pointer. Use a
/// nested `bind:` so the path-precision claim is observable.
#[test]
fn vectis_validate_layout_bind_key_exits_one_with_pathful_error() {
    let tmp = tempdir().unwrap();
    let layout_path = tmp.path().join("layout.yaml");
    std::fs::write(
        &layout_path,
        r"version: 1
screens:
  s:
    name: S
    body:
      list:
        each: tasks
        item:
          - checkbox:
              bind: tasks.completed
",
    )
    .expect("write layout.yaml");

    let assert = specify()
        .args(["--format", "json", "vectis", "validate", "layout"])
        .arg(&layout_path)
        .assert()
        .failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(output.status.code(), Some(1), "expected exit 1: {value}");
    assert_eq!(value["mode"], "layout");
    let errors = value["errors"].as_array().expect("errors array");
    let any_hits = errors.iter().any(|e| {
        e["path"].as_str().unwrap_or("").ends_with("/checkbox/bind")
            && e["message"].as_str().unwrap_or("").contains("`bind` is define-owned")
    });
    assert!(any_hits, "expected pathful `bind` rejection: {errors:?}");
}

/// Phase 1.8: a `delta:`-shaped document MUST be rejected even
/// though the underlying composition schema's `oneOf` permits it.
/// Layout is restricted to the `screens` half of the schema (RFC-11
/// §A unwired-subset rule).
#[test]
fn vectis_validate_layout_delta_document_exits_one() {
    let tmp = tempdir().unwrap();
    let layout_path = tmp.path().join("layout.yaml");
    std::fs::write(
        &layout_path,
        r"version: 1
delta:
  added:
    new-screen:
      name: New
      body:
        list:
          each: things
          item:
            - text:
                content: hello
",
    )
    .expect("write layout.yaml");

    let assert = specify()
        .args(["--format", "json", "vectis", "validate", "layout"])
        .arg(&layout_path)
        .assert()
        .failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(output.status.code(), Some(1), "expected exit 1: {value}");
    let errors = value["errors"].as_array().expect("errors array");
    assert!(
        errors.iter().any(|e| e["path"].as_str().unwrap_or("") == "/delta"
            && e["message"].as_str().unwrap_or("").contains("MUST NOT use the `delta` shape")),
        "expected `/delta` rejection: {errors:?}"
    );
}

/// Phase 1.8: two groups in different screens carrying the same
/// `component:` slug with materially different skeletons MUST
/// produce a structural-identity error (RFC-11 §G).
#[test]
fn vectis_validate_layout_structural_identity_violation_exits_one() {
    let tmp = tempdir().unwrap();
    let layout_path = tmp.path().join("layout.yaml");
    std::fs::write(
        &layout_path,
        r"version: 1
screens:
  one:
    name: One
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: heading
            - text:
                content: body
  two:
    name: Two
    body:
      - group:
          component: card
          direction: column
          items:
            - text:
                content: heading
            - icon:
                name: chevron-right
            - text:
                content: body
",
    )
    .expect("write layout.yaml");

    let assert = specify()
        .args(["--format", "json", "vectis", "validate", "layout"])
        .arg(&layout_path)
        .assert()
        .failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(output.status.code(), Some(1), "expected exit 1: {value}");
    let errors = value["errors"].as_array().expect("errors array");
    assert!(
        errors.iter().any(|e| e["message"]
            .as_str()
            .unwrap_or("")
            .contains("component slug `card` has a different skeleton")),
        "expected structural-identity rejection for `card`: {errors:?}"
    );
}

/// Phase 1.8: a missing `layout.yaml` MUST surface as the v2
/// `invalid-project` error envelope (exit 1) -- distinct from the
/// validator-reported `errors` array, which is reserved for shape /
/// reference / unwired-subset findings against an actually-loaded
/// document.
#[test]
fn vectis_validate_layout_missing_file_surfaces_invalid_project() {
    let tmp = tempdir().unwrap();
    let missing = tmp.path().join("nope-layout.yaml");

    let assert = specify()
        .args(["--format", "json", "vectis", "validate", "layout"])
        .arg(&missing)
        .assert()
        .failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(value["error"], "invalid-project");
    assert_eq!(value["exit-code"], 1);
    assert_eq!(value["schema-version"], 2);
    assert!(
        value["message"].as_str().unwrap_or("").contains("layout.yaml not readable"),
        "unexpected message: {value}"
    );
    assert_eq!(output.status.code(), Some(1));
}

/// Phase 1.5 acceptance kept: an explicit `[PATH]` positional MUST be
/// accepted by clap and threaded through. Phase 1.6 makes it
/// observable -- the resolved path appears in the validate envelope's
/// `path` field on a successful run.
#[test]
fn vectis_validate_accepts_explicit_path_positional() {
    let tmp = tempdir().unwrap();
    let tokens_path = tmp.path().join("custom-tokens.yaml");
    std::fs::write(&tokens_path, "version: 1\n").expect("write tokens.yaml");

    let assert = specify()
        .args(["--format", "json", "vectis", "validate", "tokens"])
        .arg(&tokens_path)
        .assert()
        .success();
    let value = parse_json(&assert.get_output().stdout);
    assert_eq!(value["mode"], "tokens");
    assert_eq!(
        value["path"].as_str().expect("path is a string"),
        tokens_path.display().to_string()
    );
}

/// Force the `missing-prerequisites` path by clearing PATH so every
/// `Command::new("rustup")` etc. lookup fails with ENOENT. The binary
/// itself is launched via an absolute path by `assert_cmd` so the
/// process still starts.
#[test]
fn init_missing_prereqs_json_shape() {
    let tmp = tempdir().unwrap();
    let assert = specify()
        .env("PATH", "")
        .env_remove("CARGO_HOME")
        .env_remove("RUSTUP_HOME")
        .args(["--format", "json", "vectis", "init", "Foo", "--dir"])
        .arg(tmp.path())
        .assert()
        .failure();
    let output = assert.get_output();
    let value = parse_json(&output.stdout);

    assert_eq!(value["error"], "missing-prerequisites");
    assert_eq!(value["exit-code"], 2);
    assert_eq!(value["schema-version"], 2);
    let missing = value["missing"].as_array().expect("missing is an array");
    assert!(!missing.is_empty(), "expected at least one missing tool with PATH cleared: {value}");
    let first = &missing[0];
    for field in ["tool", "assembly", "check", "install"] {
        assert!(
            first.get(field).and_then(Value::as_str).is_some(),
            "missing[0].{field} should be a string: {value}"
        );
    }
    assert_eq!(output.status.code(), Some(2));
}
