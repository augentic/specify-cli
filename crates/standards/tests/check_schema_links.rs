//! Integration coverage for the framework brief schema-link checks.

use std::fs;
use std::path::Path;

use specify_standards::framework::check::schema_links::run_on_root;
use specify_standards::framework::{core_id_for, snippet};

fn scaffold(root: &Path) {
    fs::create_dir_all(root.join("plugins")).expect("plugins dir");
    fs::create_dir_all(root.join("adapters")).expect("adapters dir");
}

fn write_brief(root: &Path, rel_path: &str, content: &str) {
    let path = root.join(rel_path);
    fs::create_dir_all(path.parent().unwrap()).expect("create parent");
    fs::write(path, content).expect("write brief");
}

#[test]
fn valid_vectis_schema_url_passes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    scaffold(root);
    write_brief(
        root,
        "adapters/targets/vectis/references/guide.md",
        "Validates against [`tokens.schema.json`](https://schemas.specify.dev/vectis/tokens.schema.json).\n",
    );

    let findings = run_on_root(root);
    assert!(findings.is_empty(), "expected no findings, got: {findings:?}");
}

#[test]
fn all_three_vectis_schemas_pass() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    scaffold(root);
    write_brief(
        root,
        "adapters/targets/vectis/briefs/build.md",
        &[
            "See [`tokens`](https://schemas.specify.dev/vectis/tokens.schema.json),",
            "[`assets`](https://schemas.specify.dev/vectis/assets.schema.json),",
            "and [`composition`](https://schemas.specify.dev/vectis/composition.schema.json).",
        ]
        .join("\n"),
    );

    let findings = run_on_root(root);
    assert!(findings.is_empty(), "expected no findings, got: {findings:?}");
}

#[test]
fn unknown_tool_name_fails() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    scaffold(root);
    write_brief(
        root,
        "adapters/targets/vectis/references/guide.md",
        "See [`foo`](https://schemas.specify.dev/unknown-tool/foo.schema.json).\n",
    );

    let findings = run_on_root(root);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule_id.as_deref(), core_id_for("links.brief-schema-link-resolve"));
    assert!(snippet(&findings[0]).contains("unknown-tool/foo.schema.json"));
}

#[test]
fn unknown_schema_for_known_tool() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    scaffold(root);
    write_brief(
        root,
        "adapters/targets/vectis/briefs/shape.md",
        "See [`missing`](https://schemas.specify.dev/vectis/missing.schema.json).\n",
    );

    let findings = run_on_root(root);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].rule_id.as_deref(), core_id_for("links.brief-schema-link-resolve"));
    assert!(snippet(&findings[0]).contains("vectis/missing.schema.json"));
}

#[test]
fn urls_in_fences_skipped() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    scaffold(root);
    write_brief(
        root,
        "adapters/sources/screenshots/briefs/extract.md",
        "Example:\n\n```text\nhttps://schemas.specify.dev/unknown/bad.schema.json\n```\n",
    );

    let findings = run_on_root(root);
    assert!(findings.is_empty(), "expected no findings, got: {findings:?}");
}

#[test]
fn urls_inside_inline_code_are_skipped() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    scaffold(root);
    write_brief(
        root,
        "adapters/targets/vectis/references/notes.md",
        "The URL `https://schemas.specify.dev/unknown/bad.schema.json` is illustrative.\n",
    );

    let findings = run_on_root(root);
    assert!(findings.is_empty(), "expected no findings, got: {findings:?}");
}

#[test]
fn no_adapters_no_findings() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    fs::create_dir_all(root.join("plugins")).expect("plugins dir");

    let findings = run_on_root(root);
    assert!(findings.is_empty(), "expected no findings, got: {findings:?}");
}

#[test]
fn multiple_bad_urls_multiple_findings() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path();
    scaffold(root);
    write_brief(
        root,
        "adapters/targets/vectis/references/multi.md",
        &[
            "Bad: [`a`](https://schemas.specify.dev/vectis/nope.schema.json)",
            "Also bad: [`b`](https://schemas.specify.dev/fake/tokens.schema.json)",
        ]
        .join("\n"),
    );

    let findings = run_on_root(root);
    assert_eq!(findings.len(), 2);
    assert!(
        findings
            .iter()
            .all(|f| f.rule_id.as_deref() == core_id_for("links.brief-schema-link-resolve"))
    );
}
