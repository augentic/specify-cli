//! Golden tests for `specify rules export`.
//!
//! Exercises the runtime rules export contract — `ResolvedRules` export
//! rules export" and §"Codex root resolution (v1)" — via the
//! [`specify_standards::build_resolved_rules`] library entrypoint
//! for the positive scenarios and `assert_cmd` for the negative
//! `rules-root-required` scenario (the latter end-to-end proof that
//! the CH-17 CLI plumbing wires through to `Exit::ValidationFailed`).
//!
//! ## Sibling-repo dependency
//!
//! Golden tests resolve their rules root against the
//! [`augentic/specify`](https://github.com/augentic/specify) plugin
//! checkout — the canonical source of `UNI-*`, target overlays, and
//! the CH-05 `SRC-001` fixture. The checkout location is configurable
//! via the `SPECIFY_PLUGIN_REPO` env var and defaults to `../specify`
//! relative to the CLI repo (the standard layout per `AGENTS.md`).
//!
//! When the checkout is absent (e.g. CI without the sibling clone),
//! every scenario prints a `SKIP` line and returns early. The negative
//! scenario does not depend on the sibling tree and always runs.
//!
//! ## Regenerating goldens
//!
//! Golden JSON fixtures live under
//! `tests/fixtures/rules-export/<scenario>.json`. They are
//! pretty-printed (`serde_json::to_string_pretty`, 2-space indent) with
//! a single trailing newline. To refresh after an intentional change to
//! the export shape or sibling-repo codex content:
//!
//! ```text
//! REGENERATE_GOLDENS=1 cargo nextest run --test rules_export
//! ```
//!
//! Regeneration only writes files for tests that ran (sibling repo
//! present); the negative test has no golden.

use std::path::{Path, PathBuf};
use std::{env, fs};

use assert_cmd::Command;
use serde_json::Value;
use specify_standards::{ResolveInputs, ResolvedRules, build_resolved_rules};
use tempfile::tempdir;

/// Locate the `augentic/specify` plugin-repo checkout. Returns
/// `None` (and the caller should `SKIP`) when the path does not exist.
fn plugin_repo_path() -> Option<PathBuf> {
    // CARGO_MANIFEST_DIR is the CLI repo root; `../specify` resolves
    // to the sibling clone per AGENTS.md.
    let path = env::var("SPECIFY_PLUGIN_REPO").map_or_else(
        |_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("specify"),
        PathBuf::from,
    );
    path.is_dir().then_some(path)
}

/// Resolve the directory where golden fixtures live.
fn goldens_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("rules-export")
}

/// Build the export envelope by calling the library entrypoint
/// directly. The project dir is a fresh tempdir so no project-local
/// adapter rungs interfere with the rules-root fallback path.
fn run_export(
    rules_root: &Path, target: &str, sources: &[String], include_deprecated: bool,
) -> ResolvedRules {
    let project = tempdir().expect("project tempdir");
    let inputs = ResolveInputs {
        project_dir: project.path(),
        rules_root: Some(rules_root),
        target_adapter: target,
        source_adapters: sources,
        artifact_paths: &[],
        languages: &[],
        include_deprecated,
        include_unmatched: false,
        include_core: false,
    };
    build_resolved_rules(&inputs).expect("build_resolved_rules succeeds")
}

/// Compare `actual` against `<goldens_dir>/<name>.json`, or write the
/// fixture when `REGENERATE_GOLDENS` is set.
///
/// Goldens are pretty-printed JSON with a single trailing newline so
/// the file diffs as one logical record per rule.
#[track_caller]
fn assert_golden(actual: &Value, name: &str) {
    let golden_path = goldens_dir().join(format!("{name}.json"));
    let mut rendered = serde_json::to_string_pretty(actual).expect("pretty json");
    rendered.push('\n');

    if env::var_os("REGENERATE_GOLDENS").is_some() {
        fs::create_dir_all(golden_path.parent().unwrap()).expect("mkdir golden parent");
        fs::write(&golden_path, &rendered).expect("write golden");
        return;
    }

    let expected = fs::read_to_string(&golden_path).unwrap_or_else(|err| {
        panic!(
            "golden {} missing ({err}); regenerate via \
             REGENERATE_GOLDENS=1 cargo nextest run --test rules_export",
            golden_path.display()
        )
    });

    assert_eq!(
        rendered,
        expected,
        "golden divergence at {}\n--- actual (truncated head) ---\n{}\n--- expected (truncated head) ---\n{}",
        golden_path.display(),
        rendered.chars().take(400).collect::<String>(),
        expected.chars().take(400).collect::<String>(),
    );
}

/// `ResolvedRules` export contract: exporting against the `omnia`
/// target carries shared `UNI-*` rules plus the omnia overlay.
#[test]
fn omnia_golden() {
    let Some(rules_root) = plugin_repo_path() else {
        eprintln!("SKIP omnia_golden: ../specify checkout not found");
        return;
    };
    let resolved = run_export(&rules_root, "omnia", &[], false);
    let value = serde_json::to_value(&resolved).expect("to_value");
    assert_golden(&value, "omnia");
}

/// `vectis` target overlay rolls up alongside the shared rules.
#[test]
fn vectis_golden() {
    let Some(rules_root) = plugin_repo_path() else {
        eprintln!("SKIP vectis_golden: ../specify checkout not found");
        return;
    };
    let resolved = run_export(&rules_root, "vectis", &[], false);
    let value = serde_json::to_value(&resolved).expect("to_value");
    assert_golden(&value, "vectis");
}

/// `contracts` target overlay rolls up alongside the shared rules.
#[test]
fn contracts_golden() {
    let Some(rules_root) = plugin_repo_path() else {
        eprintln!("SKIP contracts_golden: ../specify checkout not found");
        return;
    };
    let resolved = run_export(&rules_root, "contracts", &[], false);
    let value = serde_json::to_value(&resolved).expect("to_value");
    assert_golden(&value, "contracts");
}

/// CH-05 `SRC-001` source overlay flows in as `origin: source` when
/// the `documentation` source adapter is bound to the export context.
#[test]
fn omnia_with_documentation_source_overlay() {
    let Some(rules_root) = plugin_repo_path() else {
        eprintln!("SKIP omnia_with_documentation_source_overlay: ../specify checkout not found");
        return;
    };
    let sources = vec!["documentation".to_string()];
    let resolved = run_export(&rules_root, "omnia", &sources, false);

    let src_001 = resolved
        .rules
        .iter()
        .find(|r| r.rule_id == "SRC-001")
        .expect("SRC-001 must appear when documentation source is bound");
    assert_eq!(
        src_001.origin,
        specify_standards::Origin::Source,
        "SRC-001 must carry origin=source"
    );

    let value = serde_json::to_value(&resolved).expect("to_value");
    assert_golden(&value, "omnia-with-documentation");
}

/// `--include-deprecated` toggles the deprecation filter on. If no
/// first-party rules are currently deprecated the resulting envelope
/// matches the no-flag `omnia` golden exactly; pinning both goldens
/// makes a future deprecation visible as a focused diff.
#[test]
fn omnia_include_deprecated() {
    let Some(rules_root) = plugin_repo_path() else {
        eprintln!("SKIP omnia_include_deprecated: ../specify checkout not found");
        return;
    };
    let resolved = run_export(&rules_root, "omnia", &[], true);
    let value = serde_json::to_value(&resolved).expect("to_value");
    assert_golden(&value, "omnia-include-deprecated");
}

/// CLI-level byte-stability sanity check — two back-to-back calls with
/// the same inputs must emit byte-identical JSON. CH-14's library
/// tests already cover this against the typed envelope; this guard
/// pins the property at the `serde_json::to_string_pretty` boundary
/// the goldens themselves use.
#[test]
fn stable_ordering_byte_identical() {
    let Some(rules_root) = plugin_repo_path() else {
        eprintln!("SKIP stable_ordering_byte_identical: ../specify checkout not found");
        return;
    };
    let first = serde_json::to_string_pretty(&run_export(&rules_root, "omnia", &[], false))
        .expect("first pretty");
    let second = serde_json::to_string_pretty(&run_export(&rules_root, "omnia", &[], false))
        .expect("second pretty");
    assert_eq!(first, second, "two consecutive exports must be byte-identical");
}

/// Agent-consumable invariants on the `omnia` envelope.
///
/// - At least one rule body contains the `## Rule` heading verbatim
///   (the codex file shape requires reviewing agents to see the
///   policy text intact).
/// - At least one rule carries a non-empty `references` list when a
///   source overlay that ships references is bound (CH-05's
///   `documentation/SRC-001` is the canonical fixture). This pins the
///   downstream review skills' "follow the citation" contract.
/// - Every `path` is anchored (no leading `/`, no Windows drive
///   prefix, no backslash separators) — durable proof that no
///   absolute machine path leaks into the wire envelope.
/// - When `--include-deprecated` is set and any deprecated rule
///   exists, its `deprecated.replaced-by` field (when populated) is
///   spelled with the kebab-case wire key.
#[test]
fn omnia_agent_consumable_assertions() {
    let Some(rules_root) = plugin_repo_path() else {
        eprintln!("SKIP omnia_agent_consumable_assertions: ../specify checkout not found");
        return;
    };
    let sources = vec!["documentation".to_string()];
    let resolved = run_export(&rules_root, "omnia", &sources, true);

    assert!(
        resolved.rules.iter().any(|r| r.body.contains("## Rule")),
        "at least one rule body must carry the verbatim `## Rule` heading",
    );
    assert!(
        resolved.rules.iter().any(|r| r.references.as_ref().is_some_and(|refs| !refs.is_empty())),
        "at least one rule (e.g. SRC-001 from the documentation overlay) must carry \
         non-empty `references` for agent citation follow",
    );

    for rule in &resolved.rules {
        assert!(
            !rule.path.starts_with('/'),
            "rule {} path leaked an absolute prefix: {}",
            rule.rule_id,
            rule.path,
        );
        let bytes = rule.path.as_bytes();
        let drive_letter = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
        assert!(
            !drive_letter,
            "rule {} path leaked a Windows drive prefix: {}",
            rule.rule_id, rule.path,
        );
        assert!(
            !rule.path.contains('\\'),
            "rule {} path leaked a backslash separator: {}",
            rule.rule_id,
            rule.path,
        );
    }

    // Wire-key check: serialise the envelope and verify the
    // kebab-case `replaced-by` form is the only spelling present, per
    // `ResolvedRules` export contract. This holds whether or not any
    // deprecated rule actually appears today (no `replaced_by` token
    // can exist either way).
    let body = serde_json::to_string(&resolved).expect("serialise");
    assert!(
        !body.contains("\"replaced_by\""),
        "snake_case wire key `replaced_by` must not appear in the export envelope",
    );
}

/// CLI smoke test: a hand-built rules-root tree with
/// a `CORE-*` rule under `adapters/shared/rules/core/` excludes that
/// rule from `specify rules export` by default and includes it under
/// `--include-core`. Uses `assert_cmd` so the closed CLI plumbing
/// (clap struct → handler → resolver) is exercised end-to-end.
#[test]
fn include_core_flag_toggles_core_rules() {
    let rules_root = tempdir().expect("rules root tempdir");
    let project = tempdir().expect("project tempdir");

    write_rule_fixture(
        &rules_root.path().join("adapters/shared/rules/universal/uni-001.md"),
        "UNI-001",
        "Universal anchor",
    );
    // C7 lands the canonical `CORE-001`; this fixture uses a high
    // out-of-the-way id (`CORE-999`) so the test never collides with
    // a future first-party core rule.
    write_rule_fixture(
        &rules_root.path().join("adapters/shared/rules/core/CORE-fixture.md"),
        "CORE-999",
        "Core fixture",
    );

    let off = export_via_cli(rules_root.path(), project.path(), false);
    let off_rules =
        off.pointer("/rules").and_then(Value::as_array).expect("rules array on default export");
    assert!(
        !off_rules.iter().any(|r| rule_id(r) == "CORE-999"),
        "CORE-999 must NOT appear without --include-core; got: {off_rules:#?}",
    );
    assert!(
        off_rules.iter().any(|r| rule_id(r) == "UNI-001"),
        "UNI-001 must still appear without --include-core; got: {off_rules:#?}",
    );

    let on = export_via_cli(rules_root.path(), project.path(), true);
    let on_rules =
        on.pointer("/rules").and_then(Value::as_array).expect("rules array with --include-core");
    let core_rule = on_rules
        .iter()
        .find(|r| rule_id(r) == "CORE-999")
        .expect("CORE-999 must appear with --include-core");
    assert_eq!(
        core_rule.pointer("/origin").and_then(Value::as_str),
        Some("core"),
        "core fixture must carry origin=core in the envelope",
    );
}

/// Write a minimal rule fixture that satisfies the CH-11 parser and
/// the codex-rule schema. Mirrors the helper used by the resolver's
/// unit tests in `crates/standards/src/rules/resolve.rs`.
fn write_rule_fixture(path: &Path, id: &str, title: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    let body = format!(
        "---\nid: {id}\ntitle: {title}\nseverity: important\ntrigger: Synthetic fixture trigger sentence long enough for schema.\n---\n\n## Rule\n\nBody for {id}.\n",
    );
    fs::write(path, body).expect("write rule fixture");
}

/// Invoke `specify rules export` against an explicit rules root and
/// parse the JSON envelope on stdout. `include_core` toggles the
/// closed `--include-core` flag.
fn export_via_cli(rules_root: &Path, project: &Path, include_core: bool) -> Value {
    let mut cmd = Command::cargo_bin("specify").expect("cargo_bin(specify)");
    cmd.args(["--format", "json", "rules", "export", "--target", "omnia"])
        .args(["--rules-root".as_ref(), rules_root.as_os_str()])
        .args(["--project-dir".as_ref(), project.as_os_str()]);
    if include_core {
        cmd.arg("--include-core");
    }
    let output = cmd.output().expect("specify invocation");
    assert!(
        output.status.success(),
        "specify rules export failed (status: {:?}); stderr:\n{}\nstdout:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout),
    );
    let stdout = std::str::from_utf8(&output.stdout).expect("utf8 stdout");
    serde_json::from_str(stdout)
        .unwrap_or_else(|err| panic!("stdout is not JSON ({err}); raw:\n{stdout}"))
}

fn rule_id(rule: &Value) -> &str {
    rule.pointer("/rule-id").and_then(Value::as_str).unwrap_or("")
}

/// Negative scenario: the global `--format text` default is rejected
/// before any resolution work — v1 export emits JSON only, so the
/// handler returns `Error::Argument` (exit 2) with a hint to rerun
/// with `--format json`. Exercises the CLI plumbing end-to-end so the
/// JSON-only contract stays pinned at the wire boundary.
#[test]
fn negative_text_format_rejected() {
    let project = tempdir().expect("project tempdir");

    let output = Command::cargo_bin("specify")
        .expect("cargo_bin(specify)")
        .args(["--format", "text", "rules", "export", "--target", "omnia"])
        .args(["--project-dir".as_ref(), project.path().as_os_str()])
        .output()
        .expect("specify invocation");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit 2; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--format json"),
        "stderr must hint the JSON-only contract; got:\n{stderr}"
    );
}

/// Negative scenario: a project dir with no shared rules tree, no
/// `--rules-root`, must exit `2` (validation) with `rules-root-required`
/// surfaced through the error envelope. Exercises the CH-17 CLI
/// plumbing end-to-end so the wire contract for the closed
/// `ResolveError::RulesRootRequired` mapping stays pinned.
#[test]
fn negative_rules_root_required() {
    let project = tempdir().expect("project tempdir");

    let output = Command::cargo_bin("specify")
        .expect("cargo_bin(specify)")
        .args(["--format", "json", "rules", "export", "--target", "omnia"])
        .args(["--project-dir".as_ref(), project.path().as_os_str()])
        .output()
        .expect("specify invocation");

    assert_eq!(
        output.status.code(),
        Some(2),
        "expected exit 2; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = std::str::from_utf8(&output.stderr).expect("utf8 stderr");
    let envelope: Value = serde_json::from_str(stderr)
        .unwrap_or_else(|err| panic!("stderr is not JSON ({err}); raw:\n{stderr}"));

    let code = envelope
        .get("error")
        .and_then(Value::as_str)
        .expect("envelope must carry the top-level error code");
    assert_eq!(code, "rules-root-required", "envelope:\n{envelope:#}");
}
